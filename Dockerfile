FROM rust:1.95-bookworm AS build

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        jq \
        make \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace
COPY . .

FROM build AS test

RUN if [ -f iperf3/config.status ]; then make -C iperf3 distclean; fi \
    && mkdir -p /tmp/iperf3-build \
    && cd /tmp/iperf3-build \
    && /workspace/iperf3/configure --prefix=/opt/iperf3 --without-openssl \
    && make -j"$(nproc)" \
    && make install \
    && rm -rf /tmp/iperf3-build

RUN IPERF3_RS_CONFIGURE_ARGS=--without-openssl cargo build --release --locked

ENV PATH="/workspace/target/release:/opt/iperf3/bin:${PATH}"

FROM rust:1.95-alpine AS release-build

RUN apk add --no-cache build-base linux-headers make pkgconfig

WORKDIR /workspace
COPY . .

RUN IPERF3_RS_CONFIGURE_ARGS=--without-openssl cargo build --release --locked \
    && mkdir -p /out \
    && cp target/release/iperf3-rs /out/iperf3-rs \
    && chmod +x /out/iperf3-rs

FROM scratch AS release

COPY --from=release-build /out/iperf3-rs /iperf3-rs
ENTRYPOINT ["/iperf3-rs"]
