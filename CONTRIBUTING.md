# Contributing

This document collects development, verification, and release details for
`iperf3-rs`. The README is intentionally focused on user-facing behavior.

## Local setup

Clone the repository with the upstream iperf3 submodule:

```sh
git clone --recursive https://github.com/mi2428/iperf3-rs.git
cd iperf3-rs
```

If the checkout already exists:

```sh
git submodule update --init --recursive
```

## Useful commands

```sh
make help
make build
make install COMPLETION=1
make fmt CHECK_ONLY=1
make lint
make doc
make test
make check
make integration
make integration EXAMPLES=bwcheck
make kani
make check integration kani
make dist OS=darwin,linux ARCH=amd64,arm64
make multipass
```

`make check` runs formatting, clippy, rustdoc, unit tests, and shell completion
syntax checks.

`make check integration kani` is the broad local quality gate. It requires Kani
and a running Docker daemon.

## Build internals

The upstream `esnet/iperf3` source is vendored as the `./iperf3` git submodule.
The Rust build script compiles `libiperf` from that submodule instead of linking
to a system iperf3 package.

At build time, `build.rs`:

1. checks that the submodule exists;
2. cleans stale in-source Autotools configuration artifacts when needed;
3. configures iperf3 in Cargo's `OUT_DIR`;
4. requests a static `libiperf` and disables the shared library;
5. builds only `src/libiperf.la`;
6. compiles a small C shim from `native/`;
7. links the Rust binary against the static `libiperf.a`;
8. mirrors linker flags discovered by upstream configure;
9. exports version metadata such as git describe, commit, commit date, build
   date, host, target, and profile.

The shim is intentionally small. It exposes the few C operations that Rust needs
but that are not ergonomic through the public libiperf headers alone, such as:

- attaching interval metrics to libiperf's reporter path without changing
  stdout mode;
- rendering upstream help text;
- preserving upstream server-loop behavior;
- reading the current iperf error;
- ignoring `SIGPIPE`.

Release builds pass:

```text
IPERF3_RS_CONFIGURE_ARGS=--without-openssl
```

This avoids a runtime OpenSSL dependency in the bundled libiperf build. The
Pushgateway HTTP client uses Rustls with webpki roots, so HTTPS Pushgateway
requests still work from the scratch release image.

## Project layout

```text
.
|-- src/
|   |-- args.rs          # iperf3-rs option extraction and validation
|   |-- cli.rs           # CLI orchestration over the library modules
|   |-- command.rs       # public Rust command API over libiperf
|   |-- help.rs          # wrapper help inserted into upstream help text
|   |-- iperf.rs         # Rust wrapper around libiperf FFI
|   |-- lib.rs           # public crate entry point and re-exports
|   |-- main.rs          # CLI entry point
|   |-- metrics.rs       # interval callback, event streams, and window aggregation
|   |-- pushgateway.rs   # Pushgateway URL construction, rendering, and HTTP writes
|   `-- version.rs       # one-line version rendering
|-- native/              # small C shim over libiperf
|-- iperf3/              # esnet/iperf3 git submodule
|-- completions/         # bash, zsh, and fish completions
|-- docker/              # Prometheus and Grafana provisioning
|-- examples/            # library-crate usage examples with their own tests
|-- tests/               # Docker Compose integration tests
|-- Dockerfile           # build, integration-test, and release image stages
|-- docker-compose.yml   # local observability stack
`-- docker-compose.test.yml
```

## Tests

Unit tests cover the argument splitter, label validation, duration parsing,
version rendering, Pushgateway URL construction, Prometheus rendering, window
metric aggregation, and selected libiperf argument parsing behavior.

The Docker Compose integration test is ignored by default because it requires
Docker:

```sh
make integration
```

It verifies:

- the shared integration image builds;
- an iperf3-rs server, upstream iperf3 reference server, and Pushgateway start;
- Pushgateway readiness;
- upstream `iperf3` client to `iperf3-rs` server interoperability;
- `iperf3-rs` client to upstream `iperf3` server interoperability;
- `iperf3-rs` client to `iperf3-rs` server metrics;
- aggregated window metrics from `--push.interval`;
- server-mode metrics from the long-running `iperf3-rs` server;
- interval metrics are visible while a longer client run is still active;
- server callbacks continue to work across multiple accepted tests;
- UDP metrics, including packet counts;
- reverse TCP metrics;
- bidirectional TCP metrics.

The test uses environment-based Pushgateway defaults for both `client-rs` and
`server-rs` services so the commands under test stay close to normal iperf3
usage.

Example applications can carry their own Docker Compose integration tests. Run
one from the repository root with:

```sh
make integration EXAMPLES=bwcheck
```

Use `EXAMPLES=all` to run every example directory that has both a
`Cargo.toml` and `integration_test.rs`. The bwcheck example protects
library-crate usage by importing `iperf3-rs`, running UDP clients through
`IperfCommand`, consuming live interval metrics, and checking bandwidth/loss
threshold behavior.

## Kani

Run:

```sh
make kani
```

Kani currently checks selected pure logic:

- Pushgateway path segment encoding escapes reserved bytes;
- retryable status classification matches the retry policy;
- retry delay is bounded for configured retry counts;
- Prometheus label-name validation matches the intended ASCII shape;
- reserved label-name detection matches the configured reserved keys;
- duration arithmetic handles minute overflow.
- command metrics-window validation rejects zero-duration windows;
- metrics callback mode selection matches the requested stream mode;
- window aggregation keeps counter summaries nonnegative and gauge summaries
  ordered for bounded symbolic samples.

Kani is not a replacement for integration tests here; it is used to lock down
small, security-sensitive or correctness-sensitive parsing and encoding rules.

## Shell completions

Completion scripts are checked in under `completions/`:

- `completions/iperf3-rs.bash`
- `completions/_iperf3-rs`
- `completions/iperf3-rs.fish`

Syntax checks run as part of `make check`.

Install locally:

```sh
make install COMPLETION=1
```

The install directories are configurable:

- `BASH_COMPLETION_DIR`
- `ZSH_COMPLETION_DIR`
- `FISH_COMPLETION_DIR`

For zsh, the Makefile prefers a writable `site-functions` directory that is
already in `$fpath`, falling back to `$(INSTALL_PREFIX)/share/zsh/site-functions`
and printing a note if the fallback is not in `$fpath`.

## Release process

Releases are driven by cargo-dist.

Publishing a tag such as `v0.1.0` runs `.github/workflows/release.yml`, which:

1. plans the release with cargo-dist;
2. builds archives for the configured targets;
3. generates checksums;
4. creates the GitHub Release;
5. uploads release artifacts;
6. generates a Homebrew formula;
7. pushes the formula to `mi2428/homebrew-iperf3-rs`.

Release archives are built for:

- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`

The manual `make dist` target is still available for local release-style
binaries under `dist/`.

Linux dist binaries are built in a Debian bullseye-based Rust image and then
smoke-tested in `debian:bullseye-slim` with `-h` and `--version`. This keeps the
glibc baseline low enough for older Raspberry Pi OS / Debian bullseye systems
and catches binaries that build successfully but cannot start.

## Container release

GHCR publishing is handled by `.github/workflows/ghcr.yml` when a GitHub Release
is published.

The workflow:

1. builds `linux/amd64` on an x86 runner;
2. builds `linux/arm64` on an arm runner;
3. pushes each image by digest;
4. creates and pushes a multi-arch manifest list;
5. tags prereleases with their version tag;
6. tags non-prereleases with both the version tag and `latest`.

It uses the automatically provided `GITHUB_TOKEN` with `packages: write`. No
separate GHCR token is required.

## Homebrew tap

The tap repository is:

```text
mi2428/homebrew-iperf3-rs
```

The user-facing commands are:

```sh
brew tap mi2428/iperf3-rs
brew install iperf3-rs
```

The release workflow pushes generated formula updates to the tap. The
Homebrew publishing step requires this repository secret on `mi2428/iperf3-rs`:

```text
HOMEBREW_TAP_TOKEN
```

Use a fine-grained personal access token when possible:

- Repository access: `mi2428/homebrew-iperf3-rs` only
- Permissions:
  - `Contents: Read and write`
  - `Metadata: Read-only`

A classic PAT also works, but it needs broader scope: `public_repo` for a public
tap or `repo` for a private tap.

## GitHub Actions

Workflows:

- `.github/workflows/checks.yml`: pull-request checks for workflow linting,
  Rust linting, unit tests, Kani, integration tests, and Linux dist startup
  smoke tests.
- `.github/workflows/release.yml`: cargo-dist release workflow for archives,
  GitHub Releases, and Homebrew formula publishing.
- `.github/workflows/ghcr.yml`: multi-arch GHCR image publishing after a
  GitHub Release is published.

`checks.yml` intentionally spells out cargo, Docker, and Kani commands instead
of invoking Makefile targets so CI behavior does not silently change when the
Makefile is refactored.

## Maintainer checklist

Before publishing a release:

1. Confirm `Cargo.toml` has the intended version.
2. Confirm `dist-workspace.toml` targets and cargo-dist version are correct.
3. Run `make check integration kani`.
4. Run `make dist OS=linux ARCH=arm64` if you want a local Raspberry Pi-style
   glibc compatibility check before tagging.
5. Confirm `HOMEBREW_TAP_TOKEN` exists and can push to the tap repository.
6. Push a version tag such as `v0.1.0`.
7. Confirm the GitHub Release contains the expected archives and checksums.
8. Confirm GHCR has the version tag and, for stable releases, `latest`.
9. Confirm the Homebrew formula was updated in `mi2428/homebrew-iperf3-rs`.
