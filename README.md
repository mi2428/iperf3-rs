# iperf3-rs

Rust frontend for upstream `libiperf` with live Prometheus Pushgateway export.

`iperf3-rs` is intentionally not a shell wrapper around the `iperf3` executable.
It links the `esnet/iperf3` `libiperf` library directly through FFI, lets
upstream parse and execute normal iperf3 tests, and uses libiperf's JSON stream
callback to observe interval results while the test is still running.

That gives the project two goals:

- Preserve iperf3 wire compatibility by using upstream `libiperf` and upstream
  option parsing.
- Provide a flexible Rust frontend for live metrics, packaging, and future
  operational behavior that would be awkward to bolt onto the stock CLI.

## Why this exists

There are many iperf helper tools that run `iperf3` as a child process and parse
the final JSON output. That works for post-run summaries, but it is a weak fit
for live observability:

- final JSON arrives after the test has already finished;
- parsing human output is brittle;
- long-running server mode is hard to enrich cleanly from outside the process;
- retry, timeout, User-Agent, labeling, and packaging behavior become wrapper
  logic around an opaque child process.

`iperf3-rs` takes a different path. It embeds the upstream library and registers
a Rust-managed callback for libiperf JSON stream events. Each `interval` event is
converted to Prometheus text format and pushed to Pushgateway immediately. In
practice, this means dashboards can update during the test, not only after it.

Because the frontend is Rust, the wrapper-specific pieces are normal Rust code:
argument extraction, URL validation, Pushgateway HTTP behavior, metric
rendering, shell completions, Docker images, tests, and model-checking harnesses.
The network test itself still belongs to upstream iperf3.

## Compatibility model

The compatibility rule is simple: `iperf3-rs` strips only its own `--push.*`
options, then passes the remaining argv to upstream `iperf_parse_arguments()`.

This means upstream iperf3 options such as `-s`, `-c`, `-u`, `-R`, `--bidir`,
`-b`, `-t`, `-i`, `-P`, `-J`, authentication options, bind options, and the rest
of the esnet/iperf3 CLI are parsed by libiperf itself. The Rust layer does not
try to maintain a separate clone of the iperf3 option grammar.

The wire protocol is upstream iperf3 as well. You can mix `iperf3-rs` and the
reference `iperf3` binary in either direction:

```sh
# Rust server, upstream client
iperf3-rs -s
iperf3 -c <server>

# Upstream server, Rust client
iperf3 -s
iperf3-rs -c <server>
```

The integration test suite exercises both directions, plus iperf3-rs to
iperf3-rs metrics export, UDP, reverse TCP, bidirectional TCP, server-side
metrics, and live interval visibility before the client exits.

## How live metrics work

When `--push.url` or `PUSH_URL` is set, iperf3-rs:

1. lets libiperf parse the iperf3 arguments;
2. determines the parsed role, such as client or server;
3. enables libiperf JSON stream output internally;
4. registers a JSON callback with libiperf;
5. receives line-delimited JSON stream events;
6. keeps `interval` events and ignores non-interval events;
7. maps each interval summary to Prometheus gauges;
8. sends the newest interval sample to Pushgateway.

The callback path is deliberately nonblocking. It sends JSON lines into a
size-one channel and a worker thread performs HTTP writes. If Pushgateway is
slow and interval events arrive faster than they can be pushed, the queued
sample is replaced with the newest interval. For gauges, freshness is more
useful than replaying stale samples.

Metrics are emitted once per libiperf reporting interval. Use normal iperf3
interval controls such as `-i 1` when you want one-second pushes.

## Install

### Homebrew

Released versions are intended to be installable from the project tap:

```sh
brew tap mi2428/iperf3-rs
brew install iperf3-rs
```

The tap repository is `mi2428/homebrew-iperf3-rs`. The release workflow updates
the generated formula when a version tag is published.

### GitHub Releases

Release archives are produced by cargo-dist for:

- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`

### Container Image

The release workflow also publishes a multi-arch Linux image to GHCR:

```text
ghcr.io/<owner>/iperf3-rs:<tag>
ghcr.io/<owner>/iperf3-rs:latest
```

The release image is a small `scratch` image containing the `iperf3-rs` binary
and the minimal runtime filesystem it needs. HTTPS Pushgateway endpoints are
supported by Rustls webpki roots, so the image does not rely on an OS CA bundle.

### From source

```sh
git clone --recursive https://github.com/mi2428/iperf3-rs.git
cd iperf3-rs
make build
```

If the repository was cloned without submodules:

```sh
git submodule update --init --recursive
```

Install the host binary under `~/.local/bin` by default:

```sh
make install
```

Install shell completions as well:

```sh
make install COMPLETION=1
```

Completion install directories are configurable:

- `BASH_COMPLETION_DIR`
- `ZSH_COMPLETION_DIR`
- `FISH_COMPLETION_DIR`

For zsh, the Makefile prefers a writable `site-functions` directory already in
`$fpath`, because zsh only loads completion functions from directories in that
path.

## Basic usage

Run iperf3-rs the same way you would run iperf3:

```sh
# Server
iperf3-rs -s

# One-off server
iperf3-rs -s -1

# Client
iperf3-rs -c 127.0.0.1 -t 10 -i 1

# UDP
iperf3-rs -c 127.0.0.1 -u -b 10M -t 10 -i 1

# Reverse mode
iperf3-rs -c 127.0.0.1 -R -t 10 -i 1

# Bidirectional mode
iperf3-rs -c 127.0.0.1 --bidir -t 10 -i 1
```

Upstream help is available through the same flags:

```sh
iperf3-rs --help
iperf3-rs --version
```

The help output includes the iperf3-rs push options first, then the upstream
iperf3 option list rendered by libiperf.

## Pushgateway export

Start an iperf3-rs server:

```sh
iperf3-rs -s
```

Run a client that pushes interval metrics:

```sh
iperf3-rs \
  -c 127.0.0.1 \
  -t 10 \
  -i 1 \
  --push.url http://127.0.0.1:9091 \
  --push.job iperf3 \
  --push.label test=testrun \
  --push.label scenario=baseline
```

Bare host:port Pushgateway values default to HTTP, so these are equivalent:

```sh
--push.url 127.0.0.1:9091
--push.url http://127.0.0.1:9091
```

Server mode can export metrics too:

```sh
iperf3-rs \
  -s \
  --push.url http://127.0.0.1:9091 \
  --push.job iperf3 \
  --push.label test=testrun \
  --push.label scenario=server
```

### Push options

```text
--push.url URL
    Pushgateway URL. Required to enable metrics export.

--push.job JOB
    Pushgateway job name. Defaults to iperf3.

--push.label KEY=VALUE
    Add a Pushgateway grouping label. Repeatable.

--push.timeout DURATION
    Per-request HTTP timeout. Accepts values like 500ms, 5s, 1m, or bare
    seconds. Defaults to 5s.

--push.retries N
    Retry failed Pushgateway requests. Defaults to 0. The maximum is 10.
    HTTP 429 and 5xx responses are retryable.

--push.user-agent VALUE
    HTTP User-Agent for Pushgateway requests. Defaults to iperf3-rs/<version>.

--push.metric-prefix PREFIX
    Prometheus metric name prefix. Defaults to iperf3.
```

### Environment variables

Every push option has an environment default:

```text
PUSH_URL=URL
PUSH_JOB=JOB
PUSH_LABELS=KEY=VALUE,...
PUSH_TIMEOUT=DURATION
PUSH_RETRIES=N
PUSH_USER_AGENT=VALUE
PUSH_METRIC_PREFIX=PREFIX
```

CLI values override environment defaults. `PUSH_LABELS` are applied before
`--push.label` values. Duplicate label names are rejected.

This is convenient for containerized test runners:

```sh
PUSH_URL=http://pushgateway:9091 \
PUSH_JOB=integration \
PUSH_LABELS=test=self,scenario=tcp \
iperf3-rs -c server-rs -t 10 -i 1
```

### Grouping labels

Pushgateway grouping labels are encoded into the request path:

```text
/metrics/job/{job}/{label_name}/{label_value}/.../iperf_mode/{client|server|unknown}
```

`iperf3-rs` automatically appends `iperf_mode` after libiperf parses the role.
User labels may use any Prometheus-style label name:

```text
[a-zA-Z_][a-zA-Z0-9_]*
```

The reserved label names are:

- `job`
- `iperf_mode`

Label values must be non-empty. Path segments are percent-encoded, so values can
contain characters such as `/` or spaces.

## Exported metrics

With the default `--push.metric-prefix iperf3`, iperf3-rs emits these gauges:

```text
iperf3_bytes
iperf3_bandwidth
iperf3_packets
iperf3_error_packets
iperf3_jitter
iperf3_tcp_retransmits
```

Metric mapping:

| Metric | Source field | Notes |
| --- | --- | --- |
| `iperf3_bytes` | `bytes` | Interval bytes from the aggregate sum. |
| `iperf3_bandwidth` | `bits_per_second` | Bits per second for the interval. |
| `iperf3_packets` | `packets` | UDP packet count when available. |
| `iperf3_error_packets` | `lost_packets` | UDP lost/error packet count when available. |
| `iperf3_jitter` | `jitter_ms` | Converted from milliseconds to seconds. |
| `iperf3_tcp_retransmits` | `retransmits` | TCP sender retransmits when reported by libiperf and the OS. |

Not every metric is meaningful for every iperf mode. For example, UDP-specific
packet and jitter fields are zero for normal TCP runs, and retransmits depend on
libiperf and operating-system support for TCP information.

Use a custom prefix when multiple tools share a Pushgateway:

```sh
iperf3-rs \
  -c 127.0.0.1 \
  -t 10 \
  -i 1 \
  --push.url http://127.0.0.1:9091 \
  --push.metric-prefix nettest
```

This emits `nettest_bytes`, `nettest_bandwidth`, and so on.

## Observability stack

`docker-compose.yml` starts only the observability services:

- Pushgateway
- Prometheus
- Grafana

It does not start iperf3-rs clients or servers. This keeps the observability
stack reusable for local experiments.

```sh
docker compose up -d
```

Default ports:

```text
Pushgateway: http://localhost:9091
Prometheus:  http://localhost:9090
Grafana:     http://localhost:3000
```

Grafana defaults to `admin` / `admin`. The compose file provisions:

- a Prometheus datasource named `Prometheus` with UID `prometheus`;
- an `iperf3-rs Metrics` dashboard under the `iperf3-rs` folder;
- a Prometheus scrape config for `pushgateway:9091` with one-second scrape
  intervals.

Ports and Grafana credentials can be overridden:

```sh
PUSHGATEWAY_PORT=19091 \
PROMETHEUS_PORT=19090 \
GRAFANA_PORT=13000 \
GRAFANA_ADMIN_USER=admin \
GRAFANA_ADMIN_PASSWORD=admin \
docker compose up -d
```

After the stack is running, point iperf3-rs at the Pushgateway:

```sh
iperf3-rs \
  -c 127.0.0.1 \
  -t 30 \
  -i 1 \
  --push.url http://127.0.0.1:9091 \
  --push.label test=local \
  --push.label scenario=tcp
```

Shut the observability stack down:

```sh
docker compose down
```

Remove persisted Prometheus and Grafana volumes as well:

```sh
docker compose down -v
```

## Build details

The upstream `esnet/iperf3` source is vendored as the `./iperf3` git submodule.
The Rust build script compiles `libiperf` from that submodule instead of linking
to a system iperf3 package.

At build time, `build.rs`:

1. checks that the submodule exists;
2. configures iperf3 in Cargo's `OUT_DIR`;
3. requests a static `libiperf` and disables the shared library;
4. builds only `src/libiperf.la`;
5. compiles a small C shim from `native/`;
6. links the Rust binary against the static `libiperf.a`;
7. exports version metadata such as git describe, commit, commit date, build
   date, host, target, and profile.

The shim is intentionally small. It exposes the few C operations that Rust needs
but that are not ergonomic through the public libiperf headers alone, such as
enabling JSON stream callbacks, rendering upstream help text, preserving server
loop behavior, reading the current iperf error, and ignoring `SIGPIPE`.

Release builds pass:

```text
IPERF3_RS_CONFIGURE_ARGS=--without-openssl
```

This avoids a runtime OpenSSL dependency in the bundled libiperf build. The
Pushgateway HTTP client uses Rustls with webpki roots.

## Development

Useful commands:

```sh
make help
make build
make install COMPLETION=1
make fmt CHECK_ONLY=1
make lint
make test
make check
make integration
make kani
make check integration kani
make dist OS=darwin,linux ARCH=amd64,arm64
```

`make check` runs formatting, clippy, unit tests, and shell completion syntax
checks.

`make integration` runs the Docker Compose integration test:

- builds the shared test image unless `SKIP_INTEGRATION_IMAGE_BUILD=1`;
- starts an iperf3-rs server, an upstream iperf3 reference server, and
  Pushgateway;
- verifies upstream client to iperf3-rs server compatibility;
- verifies iperf3-rs client to upstream server compatibility;
- verifies iperf3-rs client to iperf3-rs server metrics;
- verifies server-mode metrics;
- verifies metrics are visible while a long-running client is still active;
- verifies UDP, reverse TCP, and bidirectional TCP scenarios.

`make kani` runs Kani harnesses for selected pure logic:

- Pushgateway path segment encoding;
- retryable status classification;
- retry delay bounds;
- Prometheus label-name validation;
- reserved label rejection;
- duration arithmetic.

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

The Homebrew publishing step requires this repository secret:

```text
HOMEBREW_TAP_TOKEN
```

The token needs push access to `mi2428/homebrew-iperf3-rs`. A fine-grained PAT
with `Contents: Read and write` for that repository is sufficient.

GHCR publishing is handled separately by `.github/workflows/ghcr.yml` when a
GitHub Release is published. It uses the automatically provided `GITHUB_TOKEN`
with `packages: write` and does not require a separate GHCR token.

## Project layout

```text
.
|-- src/
|   |-- args.rs          # iperf3-rs option extraction and validation
|   |-- help.rs          # wrapper help inserted into upstream help text
|   |-- iperf.rs         # Rust wrapper around libiperf FFI
|   |-- main.rs          # CLI entry point
|   |-- metrics.rs       # JSON stream callback and metric extraction
|   |-- pushgateway.rs   # Pushgateway URL construction and HTTP writes
|   `-- version.rs       # one-line version rendering
|-- native/              # small C shim over libiperf
|-- iperf3/              # esnet/iperf3 git submodule
|-- completions/         # bash, zsh, and fish completions
|-- docker/              # Prometheus and Grafana provisioning
|-- tests/               # Docker Compose integration tests
|-- Dockerfile           # build, integration-test, and release image stages
|-- docker-compose.yml   # local observability stack
`-- docker-compose.test.yml
```

## Current caveats

- Metrics export currently relies on libiperf JSON stream mode. This is the
  mechanism that makes live interval export possible without patching upstream.
- The Pushgateway path is based on grouping labels. Use labels with bounded
  cardinality, such as `test`, `scenario`, `site`, or `host_role`; avoid
  high-cardinality values that would create unbounded Pushgateway groups.
- Pushgateway stores the last pushed sample for each grouping key. It is useful
  for service-level or batch-style metrics, but it is not a long-term time
  series database. Prometheus is expected to scrape it.
- TCP retransmit metrics depend on what libiperf and the runtime OS can report.

## License

`iperf3-rs` is licensed under the MIT License. The vendored `esnet/iperf3`
submodule keeps its upstream license.
