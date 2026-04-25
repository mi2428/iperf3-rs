# iperf3-rs

Rust frontend for `libiperf` with live Pushgateway export.

The CLI keeps iperf3 compatibility by passing iperf options to upstream
`iperf_parse_arguments()`. Only iperf3-rs specific options are stripped before
that call:

- `--push.url URL`
- `--push.job NAME`
- `--push.label KEY=VALUE`
- `--push.timeout DURATION`
- `--push.retries N`
- `--push.user-agent VALUE`
- `--push.metric-prefix PREFIX`

When `--push.url` is set, iperf3-rs enables libiperf JSON stream output
internally, receives interval events through `iperf_set_test_json_callback()`,
converts each interval into Prometheus text format, and sends it to Pushgateway.

## Build

This repository vendors esnet/iperf as the `./iperf3` git submodule. For a fresh
checkout:

```sh
git submodule update --init --recursive
make build
```

`build.rs` runs `./configure --enable-static --disable-shared` in Cargo's target
build directory and links the resulting static `libiperf.a`.

Useful development targets:

```sh
make help
make check
make check integration kani
make kani
make dist OS=darwin ARCH=arm64
```

`make check integration kani` runs the release-blocking quality gates, so it
requires Kani and a running Docker daemon.

Shell completions for bash, zsh, and fish are checked in under
`completions/` and can be installed under `~/.local/share`:

```sh
make install COMPLETION=1
```

## Install

Released versions are installable with Homebrew:

```sh
brew install mi2428/iperf3-rs/iperf3-rs
```

## Release

Publishing a GitHub Release triggers `.github/workflows/release.yml`. The
workflow checks out the release tag, builds and pushes the multi-arch GHCR image,
uploads release binaries plus `checksums.txt` and the generated Homebrew formula
to the GitHub Release, and updates the Homebrew tap formula.

Release assets are built for `darwin-amd64`, `darwin-arm64`, `linux-amd64`, and
`linux-arm64`. The container image is published as
`ghcr.io/<owner>/iperf3-rs:<tag>`; non-prerelease releases also update
`ghcr.io/<owner>/iperf3-rs:latest`.

Release builds set `IPERF3_RS_CONFIGURE_ARGS=--without-openssl` so the bundled
libiperf does not depend on external OpenSSL libraries.

Homebrew publishing expects a tap repository named `<owner>/homebrew-iperf3-rs`;
for this repository that is `mi2428/homebrew-iperf3-rs`. Configure a
`HOMEBREW_TAP_TOKEN` repository secret with push access to that tap. Set the
`HOMEBREW_TAP_REPOSITORY` repository variable to override the default tap
repository.

## Usage

Run a normal one-off server and client:

```sh
target/debug/iperf3-rs -s -1
target/debug/iperf3-rs -c 127.0.0.1 -t 1
```

Push interval metrics:

```sh
target/debug/iperf3-rs \
  -c 127.0.0.1 -t 10 \
  --push.url http://127.0.0.1:9091 \
  --push.job iperf3 \
  --push.label test=testrun \
  --push.label scenario=sample1
```

The grouping path is:

```text
/metrics/job/{job}/test/{test}/scenario/{scenario}/iperf_mode/{client|server}
```

Pushgateway behavior can be tuned per run:

```sh
target/debug/iperf3-rs \
  -c 127.0.0.1 -t 10 \
  --push.url http://127.0.0.1:9091 \
  --push.timeout 500ms \
  --push.retries 2 \
  --push.user-agent iperf3-rs/lab \
  --push.metric-prefix nettest
```

`--push.timeout` accepts `500ms`, `5s`, `1m`, or a bare seconds value. Retries
apply to failed requests after the initial Pushgateway attempt.

With the default `--push.metric-prefix iperf3`, exported gauges are:

- `iperf3_bytes`
- `iperf3_bandwidth`
- `iperf3_packets`
- `iperf3_error_packets`
- `iperf3_jitter`
- `iperf3_tcp_retransmits`

## Notes

With Pushgateway enabled, libiperf is switched to JSON stream mode so interval
data can be consumed without patching upstream. This can change normal stdout
behavior compared with plain iperf3. The next refinement is to preserve the
original human-readable output while still exporting metrics.
