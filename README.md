# iperf3-rs

[![Release](https://github.com/mi2428/iperf3-rs/actions/workflows/release.yml/badge.svg)](https://github.com/mi2428/iperf3-rs/actions/workflows/release.yml) [![GHCR](https://github.com/mi2428/iperf3-rs/actions/workflows/ghcr.yml/badge.svg)](https://github.com/mi2428/iperf3-rs/actions/workflows/ghcr.yml)

Rust API for `libiperf` with iperf3 compatibility and live Prometheus
Pushgateway export.

> [!TIP]
> `iperf3-rs` is not a shell wrapper around `iperf3`. It links upstream
> `esnet/iperf3` `libiperf` directly, lets libiperf parse and run normal iperf3
> tests, and exposes live interval metrics to Rust code and Pushgateway while
> tests are still running.

## Table of Contents

- [Project Overview](#project-overview)
  - [Why libiperf](#why-libiperf)
  - [Compatibility](#compatibility)
  - [Live metrics model](#live-metrics-model)
- [Install](#install)
- [Use as a CLI](#use-as-a-cli)
- [Use as a Rust Library](#use-as-a-rust-library)
- [Export Metrics](#export-metrics)
  - [Pushgateway configuration](#pushgateway-configuration)
  - [Metric names](#metric-names)
- [Local Observability Stack](#local-observability-stack)
- [Build Notes and Caveats](#build-notes-and-caveats)
- [License](#license)

## Project Overview

`iperf3-rs` has two goals:

- Preserve iperf3 wire compatibility by using upstream `libiperf` and upstream
  option parsing.
- Make libiperf useful from Rust applications, including live metrics export,
  labeling, packaging, and automation logic that would be awkward to bolt onto
  the stock CLI from the outside.

### Why libiperf

Many iperf helper tools run `iperf3` as a child process and parse the final JSON
output. That is fine for post-run summaries, but it is a weak fit for live
observability:

- final JSON arrives after the test has finished;
- parsing human output is brittle;
- long-running server mode is hard to enrich cleanly from outside the process;
- retry, timeout, User-Agent, labeling, and packaging behavior become wrapper
  logic around an opaque child process.

`iperf3-rs` embeds upstream `libiperf` and registers a Rust-managed callback on
libiperf's reporting path. The network test itself remains upstream iperf3; the
Rust layer owns orchestration, metrics, Pushgateway writes, shell completions,
Docker images, tests, and release packaging.

### Compatibility

The compatibility rule is simple: `iperf3-rs` strips only its own `--push.*`
options, then passes the remaining argv to upstream `iperf_parse_arguments()`.

That means upstream iperf3 options such as `-s`, `-c`, `-u`, `-R`, `--bidir`,
`-b`, `-t`, `-i`, `-P`, `-J`, authentication options, bind options, and the rest
of the esnet/iperf3 CLI are parsed by libiperf itself. The Rust layer does not
maintain a separate clone of the iperf3 option grammar.

The wire protocol is upstream iperf3 as well. You can mix `iperf3-rs` and the
reference `iperf3` binary in either direction:

This is not a from-scratch Rust reimplementation of iperf3. Compatibility comes
from calling the same upstream libiperf implementation that the reference CLI
uses, so updating the vendored upstream revision keeps iperf3-rs aligned with
esnet/iperf3 behavior instead of maintaining a parallel protocol stack.

```sh
# Rust server, upstream client
iperf3-rs -s
iperf3 -c <server>

# Upstream server, Rust client
iperf3 -s
iperf3-rs -c <server>
```

The integration test suite exercises both directions, plus iperf3-rs to
iperf3-rs metrics export, window metric aggregation, UDP, reverse TCP,
bidirectional TCP, server-side metrics, and live interval visibility before the
client exits.

### Live metrics model

When metrics are enabled, iperf3-rs:

1. lets libiperf parse the iperf3 arguments;
2. installs an interval metrics callback without changing the requested stdout
   mode;
3. receives libiperf interval summaries from the reporting path;
4. maps interval summaries to Rust `Metrics` values and Prometheus gauges;
5. sends either immediate interval samples or aggregated window summaries to
   Pushgateway.

The callback path is deliberately nonblocking. In default immediate mode, it
sends interval metrics into a size-one channel and a worker thread performs HTTP
writes. If Pushgateway is slow and interval events arrive faster than they can
be pushed, the queued sample is replaced with the newest interval. For gauges,
freshness is more useful than replaying stale samples.

Metrics are emitted once per libiperf reporting interval. Use normal iperf3
interval controls such as `-i 1` when you want one-second pushes.

When `--push.interval` is set, iperf3-rs buffers libiperf interval samples in
the worker thread and pushes representative `*_window_*` gauges once per window.
The final partial window is flushed when the iperf test exits. This is not a
historical datapoint replay mechanism; Pushgateway still stores the latest
sample for the grouping key, and iperf3-rs performs the aggregation before the
push.

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

Completion install directories are configurable with `BASH_COMPLETION_DIR`,
`ZSH_COMPLETION_DIR`, and `FISH_COMPLETION_DIR`.

## Use as a CLI

Run iperf3-rs the same way you would run iperf3:

```sh
# Server
iperf3-rs -s

# One-off server
iperf3-rs -s -1

# TCP client
iperf3-rs -c 127.0.0.1 -t 10 -i 1

# UDP client
iperf3-rs -c 127.0.0.1 -u -b 10M -t 10 -i 1

# Reverse mode
iperf3-rs -c 127.0.0.1 -R -t 10 -i 1

# Bidirectional mode
iperf3-rs -c 127.0.0.1 --bidir -t 10 -i 1
```

For a quick local smoke test, start a server in one terminal:

```sh
iperf3-rs -s
```

Then run a TCP measurement from another terminal:

```sh
iperf3-rs -c 127.0.0.1 -t 10 -i 1
```

Run a UDP measurement the same way, adding `-u` and an explicit target bitrate:

```sh
iperf3-rs -c 127.0.0.1 -u -b 10M -t 10 -i 1
```

For remote hosts, make sure UDP traffic to the server port is allowed as well
as TCP. The default iperf3 port is `5201`, and UDP tests need `5201/udp`; a
successful TCP control connection alone does not prove that UDP datagrams can
reach the server.

Upstream help is available through the same flags:

```sh
iperf3-rs --help
iperf3-rs --version
```

The help output includes the iperf3-rs push options first, then the upstream
iperf3 option list rendered by libiperf.

## Use as a Rust Library

The same libiperf frontend is available as a Rust crate. This is intended for
programs that want to start iperf tests directly from Rust instead of spawning
the `iperf3-rs` CLI and scraping its output.

`IperfCommand` provides typed helpers for common options, while still accepting
normal iperf arguments through `arg()` and `args()`. Both paths ultimately pass
ordinary argv-shaped iperf options to upstream `iperf_parse_arguments()`:

```rust
use std::time::Duration;

use iperf3_rs::{IperfCommand, MetricEvent, MetricsMode, Result};

fn main() -> Result<()> {
    let mut command = IperfCommand::client("127.0.0.1");
    command
        .duration(Duration::from_secs(10))
        .report_interval(Duration::from_secs(1));

    let (running, metrics) = command.spawn_with_metrics(MetricsMode::Interval)?;

    while let Some(event) = metrics.recv() {
        if let MetricEvent::Interval(sample) = event {
            println!("{} bit/s", sample.bandwidth_bits_per_second);
        }
    }

    running.wait()?;
    Ok(())
}
```

Typed helpers cover common roles and options such as `client`, `server_once`,
`port`, `duration`, `report_interval`, `udp`, `bitrate_bits_per_second`,
`reverse`, `bidirectional`, and `json`. Use `arg()` or `args()` for any upstream
iperf3 option that does not need a dedicated Rust helper.

Use `MetricsMode::Window(duration)` to receive the same representative window
summaries used by `--push.interval`. `PushGateway` and `PushGatewayConfig` are
also exported for applications that want to push collected metrics themselves.
Protocol-specific fields such as TCP RTT or UDP jitter are exposed as
`Option<f64>`, so application code can distinguish an observed zero from a
metric that libiperf did not report for that protocol or traffic direction.

Library code can also enable the same direct Pushgateway delivery used by the
CLI without manually draining a metrics stream:

```rust
use std::time::Duration;

use iperf3_rs::{IperfCommand, MetricsMode, PushGatewayConfig, Result};

fn main() -> Result<()> {
    let endpoint = PushGatewayConfig::parse_endpoint("127.0.0.1:9091")?;
    let config = PushGatewayConfig::new(endpoint).label("scenario", "library");

    let mut command = IperfCommand::client("127.0.0.1");
    command
        .duration(Duration::from_secs(10))
        .report_interval(Duration::from_secs(1))
        .run_with_pushgateway(config, MetricsMode::Interval)?;

    Ok(())
}
```

Direct Pushgateway delivery and `spawn_with_metrics()` are intentionally
separate run shapes. If an application needs to both inspect live events and
customize delivery, use `spawn_with_metrics()` and call `PushGateway::push()` or
`PushGateway::push_window()` from application code.

The `pushgateway` feature is enabled by default and provides the CLI,
`PushGateway`, `PushGatewayConfig`, and HTTP delivery dependencies. Library-only
consumers that only need `IperfCommand` and `MetricsStream` can disable default
features:

```toml
iperf3-rs = { version = "1", default-features = false }
```

See
[examples/bwcheck](https://github.com/mi2428/iperf3-rs/tree/main/examples/bwcheck)
for a complete example application. It accepts `HOST:PORT` endpoints, runs
fixed UDP iperf tests, consumes live interval metrics, and fails when bandwidth
or loss thresholds are not met.

The first public API keeps high-level `IperfCommand` runs serialized inside one
process because libiperf still has process-global error, signal, and output
state. Server mode must use iperf's one-off option (`-s -1`) by default, so a
library call cannot accidentally hold the process-wide libiperf lock forever.
Use `IperfCommand::allow_unbounded_server(true)` only when the Rust process is
dedicated to that long-lived server. For local client/server interop tests, run
the peer as a separate process, container, or VM. For long-running automation,
`RunningIperf` supports `try_wait()`, `wait_timeout(duration)`, and
`is_finished()` in addition to blocking `wait()`.

## Export Metrics

### Pushgateway configuration

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

Push options:

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

--push.interval DURATION
    Aggregate libiperf interval samples for this duration before pushing window
    metrics. Accepts values like 500ms, 10s, 1m, or bare seconds. When omitted,
    iperf3-rs pushes immediate interval metrics.

--push.delete-on-exit
    Delete this Pushgateway grouping key after the iperf run exits.
```

Every push option has an environment default:

```text
PUSH_URL=URL
PUSH_JOB=JOB
PUSH_LABELS=KEY=VALUE,...
PUSH_TIMEOUT=DURATION
PUSH_RETRIES=N
PUSH_USER_AGENT=VALUE
PUSH_METRIC_PREFIX=PREFIX
PUSH_INTERVAL=DURATION
PUSH_DELETE_ON_EXIT=BOOL
```

CLI values override environment defaults. `PUSH_LABELS` are applied before
`--push.label` values. Duplicate label names are rejected.
Boolean environment values accept `true`, `false`, `1`, `0`, `yes`, `no`, `on`,
and `off`.

Pushgateway grouping labels are encoded into the request path:

```text
/metrics/job/{job}/{label_name}/{label_value}/...
```

`iperf3-rs` does not add role or mode labels automatically. Add explicit labels
such as `mode=client` or `mode=server` with `--push.label` when you want them.
User labels may use any Prometheus-style label name:

```text
[a-zA-Z_][a-zA-Z0-9_]*
```

The reserved label name is `job`. Label values must be non-empty. Path segments
are percent-encoded, so values can contain characters such as `/` or spaces.

### Metric names

With the default `--push.metric-prefix iperf3`, iperf3-rs emits immediate
interval gauges when `--push.interval` is not set:

```text
iperf3_bytes
iperf3_bandwidth
iperf3_tcp_retransmits
iperf3_tcp_rtt_seconds
iperf3_tcp_rttvar_seconds
iperf3_tcp_snd_cwnd_bytes
iperf3_tcp_snd_wnd_bytes
iperf3_tcp_pmtu_bytes
iperf3_tcp_reorder_events
iperf3_udp_packets
iperf3_udp_lost_packets
iperf3_udp_jitter_seconds
iperf3_udp_out_of_order_packets
iperf3_omitted
```

Metric mapping:

| Metric | Source field | Notes |
| --- | --- | --- |
| `iperf3_bytes` | `bytes_transferred` | Interval bytes from the aggregate report side. |
| `iperf3_bandwidth` | `bytes_transferred`, `interval_duration` | Bits per second for the interval. |
| `iperf3_tcp_retransmits` | `interval_retrans` | TCP sender retransmits when reported by libiperf and the OS. |
| `iperf3_tcp_rtt_seconds` | `rtt` | TCP sender smoothed RTT from TCP_INFO, converted from microseconds to seconds. |
| `iperf3_tcp_rttvar_seconds` | `rttvar` | TCP sender RTT variance from TCP_INFO, converted from microseconds to seconds. |
| `iperf3_tcp_snd_cwnd_bytes` | `snd_cwnd` | TCP sender congestion window in bytes. |
| `iperf3_tcp_snd_wnd_bytes` | `snd_wnd` | TCP sender send window in bytes when the platform reports it. |
| `iperf3_tcp_pmtu_bytes` | `pmtu` | TCP sender path MTU in bytes when the platform reports it. |
| `iperf3_tcp_reorder_events` | `reorder` | TCP sender reordering events when the platform reports them. |
| `iperf3_udp_packets` | `interval_packet_count` | UDP packet count when available. |
| `iperf3_udp_lost_packets` | `interval_cnt_error` | UDP packets inferred lost from sequence gaps. |
| `iperf3_udp_jitter_seconds` | `jitter` | UDP receiver jitter in seconds. |
| `iperf3_udp_out_of_order_packets` | `out_of_order` | UDP out-of-order packets observed in the interval. |
| `iperf3_omitted` | `omitted` | `1` for omitted warm-up intervals, otherwise `0`. |

Not every metric is meaningful for every iperf mode. UDP-prefixed fields are
zero for normal TCP runs, and TCP_INFO-derived fields depend on libiperf and
operating-system support for TCP information.

When `--push.interval` or `PUSH_INTERVAL` is set, iperf3-rs emits window
summary gauges instead of the immediate interval metric names:

```text
iperf3_window_duration_seconds
iperf3_window_transferred_bytes
iperf3_window_bandwidth_mean_bytes_per_second
iperf3_window_bandwidth_min_bytes_per_second
iperf3_window_bandwidth_max_bytes_per_second
iperf3_window_tcp_rtt_mean_seconds
iperf3_window_tcp_rtt_min_seconds
iperf3_window_tcp_rtt_max_seconds
iperf3_window_tcp_rttvar_mean_seconds
iperf3_window_tcp_rttvar_min_seconds
iperf3_window_tcp_rttvar_max_seconds
iperf3_window_tcp_snd_cwnd_mean_bytes
iperf3_window_tcp_snd_cwnd_min_bytes
iperf3_window_tcp_snd_cwnd_max_bytes
iperf3_window_tcp_snd_wnd_mean_bytes
iperf3_window_tcp_snd_wnd_min_bytes
iperf3_window_tcp_snd_wnd_max_bytes
iperf3_window_tcp_pmtu_mean_bytes
iperf3_window_tcp_pmtu_min_bytes
iperf3_window_tcp_pmtu_max_bytes
iperf3_window_udp_jitter_mean_seconds
iperf3_window_udp_jitter_min_seconds
iperf3_window_udp_jitter_max_seconds
iperf3_window_tcp_retransmits
iperf3_window_tcp_reorder_events
iperf3_window_udp_packets
iperf3_window_udp_lost_packets
iperf3_window_udp_out_of_order_packets
iperf3_window_omitted_intervals
```

Window metric semantics:

| Metric shape | Meaning |
| --- | --- |
| `*_mean_*`, `*_min_*`, `*_max_*` | Arithmetic mean, minimum, and maximum of gauge-like interval values in the pushed window. Bandwidth mean is derived from total bytes divided by total interval duration. |
| `iperf3_window_transferred_bytes` | Total transferred bytes across the pushed window. |
| `iperf3_window_tcp_retransmits` | TCP retransmits accumulated across the pushed window. |
| `iperf3_window_tcp_reorder_events` | TCP reordering events accumulated across the pushed window. |
| `iperf3_window_udp_*_packets` | UDP packet counters accumulated across the pushed window. |
| `iperf3_window_omitted_intervals` | Count of omitted libiperf intervals in the pushed window. |

Example 10-second window export:

```sh
iperf3-rs \
  -c 127.0.0.1 \
  -t 60 \
  -i 1 \
  --push.url http://127.0.0.1:9091 \
  --push.label test=testrun \
  --push.label scenario=windowed \
  --push.interval 10s
```

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

## Local Observability Stack

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

## Build Notes and Caveats

The upstream `esnet/iperf3` source is vendored as the `./iperf3` git submodule.
The Rust build script compiles `libiperf` from that submodule instead of linking
to a system iperf3 package.

The final Rust binary links a static `libiperf.a` plus a small C shim from
`native/`. Release builds use `IPERF3_RS_CONFIGURE_ARGS=--without-openssl` for
the bundled libiperf build, while Pushgateway HTTPS support comes from Rustls
with webpki roots.

Current caveats:

- Metrics export is attached to libiperf's reporting path through the local C
  shim. This keeps stdout behavior aligned with upstream iperf3 while still
  allowing live interval export without patching the submodule.
- The Pushgateway path is based on grouping labels. Use labels with bounded
  cardinality, such as `test`, `scenario`, `site`, or `host_role`; avoid
  high-cardinality values that would create unbounded Pushgateway groups.
- Pushgateway stores the last pushed sample for each grouping key. It is useful
  for service-level or batch-style metrics, but it is not a long-term time
  series database. Prometheus is expected to scrape it.
- TCP retransmit metrics depend on what libiperf and the runtime OS can report.

See the repository's
[CONTRIBUTING.md](https://github.com/mi2428/iperf3-rs/blob/main/CONTRIBUTING.md)
for local development commands, integration tests, Kani checks, release workflow
details, and maintainer setup.

## License

The Rust code written for `iperf3-rs` is licensed under the MIT License. See
[LICENSE](LICENSE).

This repository vendors `esnet/iperf3` under `iperf3/`. `esnet/iperf3` is
distributed under its upstream BSD-style license and includes additional
third-party notices; see `iperf3/LICENSE`.

SORACOM, Inc. is additionally granted a separate permissive license with
Unlicense-style terms. See [LICENSE-SORACOM](LICENSE-SORACOM).
