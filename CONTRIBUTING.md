# Contributing

This document is the maintainer and developer reference for `iperf3-rs`.

### Table of Contents

- [Common Commands](#common-commands)
- [Rustdoc](#rustdoc)
- [Design Contracts](#design-contracts)
  - [CLI Compatibility](#cli-compatibility)
  - [Metrics Delivery](#metrics-delivery)
  - [libiperf Process State](#libiperf-process-state)
  - [Native Build](#native-build)
- [Verification](#verification)
  - [When to Run What](#when-to-run-what)
  - [Integration and E2E Tests](#integration-and-e2e-tests)
  - [Kani](#kani)
- [Release Operations](#release-operations)
  - [cargo-dist](#cargo-dist)
- [GitHub Actions](#github-actions)
- [Maintainer Checklist](#maintainer-checklist)

## Common Commands

The Makefile help is the source of truth for local commands:

```console
$ make help
```

Common development targets:

| Target | Purpose |
| --- | --- |
| `make build` | Build the host binary into `bin/`. |
| `make install` | Build and install the host binary into `INSTALL_BINDIR`. Use `COMPLETION=1` to install shell completions too. |
| `make fmt CHECK_ONLY=1` | Check Rust formatting without writing changes. |
| `make lint` | Run clippy with warnings treated as errors. |
| `make doc` | Build rustdoc with warnings treated as errors. |
| `make test` | Run the default Cargo test suite; ignored Docker E2E tests stay ignored. |
| `make test NO_DEFAULT=1` | Run the same Cargo test target with default features disabled. |
| `make integration` | Run the local integration suite in `tests/integration`. |
| `make integration EXAMPLES=bwcheck` | Run a specific example integration test. |
| `make integration EXAMPLES=all` | Run every example integration test with `integration_test.rs`. |
| `make e2e` | Run ignored Docker E2E tests in `tests/e2e`. |
| `make kani` | Run Kani model checking harnesses. |
| `make check` | Run formatting, clippy, rustdoc, default tests, no-default tests, and completion checks. |
| `make multipass` | Launch a Multipass VM and copy the source tree for manual Linux testing. |
| `make clean` | Remove local build artifacts. |

Distribution targets:

| Target | Purpose |
| --- | --- |
| `make release TAG=v1.0.0` | Tag, push, and publish the crate to crates.io. |
| `make dist OS=darwin,linux ARCH=amd64,arm64` | Build release binaries into `dist/` and write checksums. |
| `make dist-smoke` | Smoke-test Linux dist binaries in an old-glibc Debian container. |
| `make checksums` | Write SHA-256 checksums for dist artifacts. |

## Rustdoc

Build API documentation with:

```console
$ make doc
$ cargo doc --no-deps --open
```

Start at the crate root, then follow the public types relevant to the change: `IperfCommand`, `MetricsMode`, `MetricEvent`, `Metrics`, `WindowMetrics`, `PrometheusEncoder`, `MetricsFileSink`, `PushGateway`, and `PushGatewayConfig`.
When changing public API behavior, update rustdoc and examples instead of expanding this file with API walkthroughs.

## Design Contracts

### CLI Compatibility

`iperf3-rs` strips only its wrapper options (`--push.*` and `--metrics.*`) and passes the remaining argv to upstream `iperf_parse_arguments()`.

Keep this boundary intact:

- do not clone upstream iperf3 option parsing in Rust;
- do not change upstream stdout or `-J` JSON behavior when metrics are enabled;
- keep wire behavior delegated to the vendored `esnet/iperf3` `libiperf`.

Authentication support depends on building the vendored libiperf with OpenSSL. The default build uses `--without-openssl` for deterministic native builds.

### Metrics Delivery

Metrics are emitted from libiperf's reporting path. The callback must stay nonblocking.

Important contracts:

- Pushgateway delivery is best-effort. Push/delete failures are reported on stderr and do not fail a successful iperf run.
- `--metrics.file` is required output. File creation or write failures make the CLI fail.
- Immediate Pushgateway mode keeps the newest queued interval when HTTP writes fall behind. Fresh gauges are preferred over replaying stale samples.
- `--push.interval` aggregates libiperf intervals locally and emits `*_window_*` metric families instead of immediate interval names.
- Metric names, label validation, and Pushgateway path encoding are public compatibility surfaces. Update tests when changing them.

Use bounded-cardinality Pushgateway labels. The grouping key always includes `job`; extra labels are user supplied.

### libiperf Process State

libiperf still has process-global error, signal, and output state. High-level `IperfCommand` runs are serialized inside one process.

Keep these library constraints visible in rustdoc:

- long-lived server mode requires explicit opt-in;
- `RunningIperf` observes completion but is not a cancellation handle;
- `wait_timeout()` stops waiting, not the underlying iperf run;
- `MetricsStream` preserves every emitted sample in library modes, so long runs must drain or drop the stream.

### Native Build

The repository vendors upstream iperf3 as the `iperf3/` git submodule. `build.rs` builds a static `libiperf` from that submodule instead of linking to a system iperf3 package.

At a high level the native build:

1. verifies the submodule;
2. configures iperf3 in Cargo's `OUT_DIR`;
3. builds `src/libiperf.la`;
4. compiles the small C shim in `native/`;
5. links Rust against the static `libiperf.a`;
6. exports build/version metadata.

The C shim should stay small. It exists for operations that are awkward through public libiperf headers alone: interval callback attachment, upstream help rendering, server-loop preservation, current error access, and `SIGPIPE` handling.

Pushgateway HTTPS uses Rustls with webpki roots, so HTTPS Pushgateway requests do not depend on OpenSSL.

## Verification

### When to Run What

| Change area | Minimum local check |
| --- | --- |
| Rust implementation | `make check` |
| CLI args, help, env vars, labels, durations | `make check integration kani` |
| Prometheus, Pushgateway, metrics files, window aggregation | `make check integration e2e kani` |
| Public Rust API or examples | `make check integration`, `make integration EXAMPLES=all`, and `make doc` |
| libiperf, native shim, build script, Dockerfile | `make check integration e2e` plus a relevant `make dist ...` |
| Release metadata, cargo-dist, workflows | `make check e2e` and review generated/CI behavior carefully |

### Integration and E2E Tests

`make integration` runs the local integration suite under `tests/integration`. It does not require Docker.
It covers the cases that should stay fast enough for ordinary local development:

- public API metadata and upstream usage exposure;
- zero-duration metrics window rejection;
- public Prometheus encoding and JSONL metrics file writing;
- CLI metrics-file behavior without replacing normal iperf stdout;
- CLI Prometheus file output with custom prefixes and labels;
- fatal CLI handling for metrics-file creation failures;
- library interval and window metrics through `IperfCommand::spawn()`;
- library interval and window metrics retained by blocking `IperfCommand::run()`;
- direct Pushgateway request path and payload construction against a local HTTP sink.

`make e2e` runs the ignored Docker E2E suite under `tests/e2e` with a single test thread.
It covers the cases most likely to regress across containers, the Rust/libiperf boundary, or published image shape:

- upstream `iperf3` to `iperf3-rs` interoperability in both directions;
- `iperf3-rs` to `iperf3-rs` metrics export;
- Pushgateway readiness, delete-on-exit, and window metrics;
- Prometheus file output with custom prefix and labels inside the Compose topology;
- server-side metrics and repeated accepted tests;
- UDP, reverse TCP, and bidirectional TCP;
- live interval visibility before a long-running client exits;
- release Docker image startup and release-image-to-release-image traffic.

Example integration tests run through the `integration` target by setting `EXAMPLES`:

```console
$ make integration EXAMPLES=bwcheck
$ make integration EXAMPLES=all
```

### Kani

`make kani` protects pure logic that is easy to get subtly wrong:

- Pushgateway path encoding;
- retryable status classification and retry delay bounds;
- metrics file format parsing;
- Prometheus metric prefix and label validation;
- reserved label detection;
- boolean and duration parsing;
- zero-duration window rejection;
- callback mode selection;
- C value normalization;
- window aggregation invariants.

Kani is not a replacement for integration tests. Use it to lock down parsing, encoding, and aggregation rules.

## Release Operations

### cargo-dist

Run `make release TAG=v1.0.0` to publish the crate to crates.io and push the release tag that drives cargo-dist.
The target requires a clean worktree and `cargo login` or `CARGO_REGISTRY_TOKEN` when the crates.io version does not already exist; it creates the tag when missing, uses an existing matching tag when present, and publishes from a temporary worktree checked out at that tag.

Publishing a tag such as `v1.0.0` runs `.github/workflows/release.yml`.
`release.yml` is intentionally hand-edited for the Linux release container and smoke test, so `dist-workspace.toml` sets `allow-dirty = ["ci"]`.

Linux artifacts are built in a Debian bullseye-based Rust image and smoke-tested in `debian:bullseye-slim` with `-h` and `--version`.
This keeps the glibc baseline suitable for older Debian/Raspberry Pi OS systems.
Use `make dist OS=... ARCH=...` for local release-style builds under `dist/`.

## GitHub Actions

Workflows:

- `.github/workflows/checks.yml`: pull-request checks for workflow linting, Rust linting, default/no-default Cargo tests, Kani, Docker E2E tests, example integration tests, and Linux dist startup smoke tests.
- `.github/workflows/release.yml`: cargo-dist archives, GitHub Releases, and Homebrew formula publishing.
- `.github/workflows/ghcr.yml`: multi-arch GHCR image publishing after a GitHub Release is published.

`checks.yml` intentionally spells out cargo, Docker, and Kani commands instead of invoking Makefile targets so CI behavior does not silently change when the Makefile is refactored.

## Maintainer Checklist

Before publishing a release:

1. Confirm `Cargo.toml` has the intended version.
2. Confirm `dist-workspace.toml` targets and cargo-dist version are correct.
3. Run `make check integration e2e kani`.
4. Run `make integration EXAMPLES=all`.
5. Run `make dist OS=linux ARCH=arm64` if you want a local Linux arm64/glibc compatibility check before tagging.
6. Confirm `HOMEBREW_TAP_TOKEN` can push to the tap repository.
7. Run `make release TAG=v1.0.0`.
8. Confirm the crates.io version exists.
9. Confirm the GitHub Release contains the expected archives and checksums.
10. Confirm GHCR has the version tag and, for stable releases, `latest`.
11. Confirm the Homebrew formula was updated in `mi2428/homebrew-iperf3-rs`.
