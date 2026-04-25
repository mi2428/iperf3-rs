ARG BUILD_IMAGE=rust
ARG BUILD_IMAGE_TAG=1.95-bookworm
ARG RELEASE_BUILD_IMAGE=rust
ARG RELEASE_BUILD_IMAGE_TAG=1.95-alpine

FROM ${BUILD_IMAGE}:${BUILD_IMAGE_TAG} AS integration-build

# hadolint ignore=DL3008
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

FROM integration-build AS integration-test

RUN if [ -f iperf3/config.status ]; then make -C iperf3 distclean; fi \
 && mkdir -p /tmp/iperf3-build

WORKDIR /tmp/iperf3-build
RUN /workspace/iperf3/configure --prefix=/opt/iperf3 --without-openssl \
 && make -j"$(nproc)" \
 && make install \
 && rm -rf /tmp/iperf3-build

WORKDIR /workspace
RUN IPERF3_RS_CONFIGURE_ARGS=--without-openssl cargo build --release --locked

ENV PATH="/workspace/target/release:/opt/iperf3/bin:${PATH}"

FROM ${RELEASE_BUILD_IMAGE}:${RELEASE_BUILD_IMAGE_TAG} AS release-build

# hadolint ignore=DL3018
RUN apk add --no-cache build-base linux-headers make pkgconfig

WORKDIR /workspace
COPY . .

RUN IPERF3_RS_CONFIGURE_ARGS=--without-openssl cargo build --release --locked \
 && mkdir -p /out \
 && cp target/release/iperf3-rs /out/iperf3-rs \
 && chmod +x /out/iperf3-rs \
 && mkdir -p /out/rootfs/tmp \
 && chmod 1777 /out/rootfs/tmp

FROM scratch AS release

COPY --from=release-build /out/rootfs/ /
COPY --from=release-build /out/iperf3-rs /iperf3-rs
ENTRYPOINT ["/iperf3-rs"]
