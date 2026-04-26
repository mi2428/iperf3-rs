# syntax=docker/dockerfile:1
ARG BUILD_IMAGE=rust
ARG BUILD_IMAGE_TAG=1.93-bookworm
ARG RELEASE_BUILD_IMAGE=rust
ARG RELEASE_BUILD_IMAGE_TAG=1.93-alpine

FROM ${BUILD_IMAGE}:${BUILD_IMAGE_TAG} AS integration-build
ARG IPERF3_RS_BUILD_DATE
ARG IPERF3_RS_GIT_COMMIT
ARG IPERF3_RS_GIT_COMMIT_DATE
ARG IPERF3_RS_GIT_DESCRIBE
ENV IPERF3_RS_BUILD_DATE=${IPERF3_RS_BUILD_DATE}
ENV IPERF3_RS_GIT_COMMIT=${IPERF3_RS_GIT_COMMIT}
ENV IPERF3_RS_GIT_COMMIT_DATE=${IPERF3_RS_GIT_COMMIT_DATE}
ENV IPERF3_RS_GIT_DESCRIBE=${IPERF3_RS_GIT_DESCRIBE}

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
RUN --mount=type=cache,id=iperf3rs-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=iperf3rs-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=iperf3rs-integration-target,target=/workspace/target,sharing=locked \
    cargo build --release --locked \
 && install -m 0755 target/release/iperf3-rs /usr/local/bin/iperf3-rs

ENV PATH="/usr/local/bin:/opt/iperf3/bin:${PATH}"

FROM ${RELEASE_BUILD_IMAGE}:${RELEASE_BUILD_IMAGE_TAG} AS release-build
ARG IPERF3_RS_BUILD_DATE
ARG IPERF3_RS_GIT_COMMIT
ARG IPERF3_RS_GIT_COMMIT_DATE
ARG IPERF3_RS_GIT_DESCRIBE
ENV IPERF3_RS_BUILD_DATE=${IPERF3_RS_BUILD_DATE}
ENV IPERF3_RS_GIT_COMMIT=${IPERF3_RS_GIT_COMMIT}
ENV IPERF3_RS_GIT_COMMIT_DATE=${IPERF3_RS_GIT_COMMIT_DATE}
ENV IPERF3_RS_GIT_DESCRIBE=${IPERF3_RS_GIT_DESCRIBE}

# hadolint ignore=DL3018
RUN apk add --no-cache build-base linux-headers make pkgconfig

WORKDIR /workspace
COPY . .

RUN --mount=type=cache,id=iperf3rs-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=iperf3rs-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=iperf3rs-release-target,target=/workspace/target,sharing=locked \
    cargo build --release --locked \
 && mkdir -p /out \
 && cp target/release/iperf3-rs /out/iperf3-rs \
 && chmod +x /out/iperf3-rs \
 && mkdir -p /out/rootfs/tmp \
 && chmod 1777 /out/rootfs/tmp

FROM scratch AS release

COPY --from=release-build /out/rootfs/ /
COPY --from=release-build /out/iperf3-rs /iperf3-rs
ENTRYPOINT ["/iperf3-rs"]
