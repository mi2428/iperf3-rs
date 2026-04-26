# syntax=docker/dockerfile:1
ARG BUILD_IMAGE=rust
ARG BUILD_IMAGE_TAG=1.93-bookworm

FROM ${BUILD_IMAGE}:${BUILD_IMAGE_TAG} AS test

# hadolint ignore=DL3008
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      build-essential \
      ca-certificates \
      make \
      pkg-config \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace
COPY . .

RUN if [ -f iperf3/config.status ]; then make -C iperf3 distclean; fi \
 && mkdir -p /tmp/iperf3-build

WORKDIR /tmp/iperf3-build
RUN /workspace/iperf3/configure --prefix=/opt/iperf3 --without-openssl \
 && make -j"$(nproc)" \
 && make install \
 && rm -rf /tmp/iperf3-build

WORKDIR /workspace
RUN --mount=type=cache,id=iperf3rs-example-bwcheck-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=iperf3rs-example-bwcheck-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=iperf3rs-example-bwcheck-target,target=/workspace/target/example-bwcheck,sharing=locked \
    CARGO_TARGET_DIR=/workspace/target/example-bwcheck \
    cargo build --release --locked --manifest-path examples/bwcheck/Cargo.toml \
 && install -m 0755 \
      target/example-bwcheck/release/iperf3-rs-bwcheck \
      /usr/local/bin/iperf3-rs-bwcheck

ENV PATH="/usr/local/bin:/opt/iperf3/bin:${PATH}"
ENV LD_LIBRARY_PATH="/opt/iperf3/lib"
