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
  - [File output](#file-output)
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
and `--metrics.*` options, then passes the remaining argv to upstream
`iperf_parse_arguments()`.

That means upstream iperf3 options such as `-s`, `-c`, `-u`, `-R`, `--bidir`,
`-b`, `-t`, `-i`, `-P`, `-J`, authentication options, bind options, and the rest
of the esnet/iperf3 CLI are parsed by libiperf itself. The Rust layer does not
maintain a separate clone of the iperf3 option grammar.
Authentication options require building the vendored libiperf with OpenSSL; the
default crate build disables OpenSSL to keep native dependencies deterministic.

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

Each metrics sample is an aggregate for one stream direction selected from the
libiperf report. In bidirectional mode, iperf3-rs currently emits the
client-side sending aggregate and the server-side receiving aggregate; it does
not emit both bidirectional halves from one process. The `direction` field in
JSONL and library metrics identifies the represented aggregate.

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

The help output includes the iperf3-rs wrapper metrics options first, then the
upstream iperf3 option list rendered by libiperf.

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
`port`, `duration`, `report_interval`, `logfile`, `connect_timeout`, `omit`,
`bind`, `udp`, `sctp`, `bitrate_bits_per_second`, `reverse`, `bidirectional`,
`no_delay`, `zerocopy`, `congestion_control`, `json`, `quiet`, and
`inherit_output`. Use `arg()` or `args()` for any upstream iperf3 option that
does not need a dedicated Rust helper.

`IperfCommand` suppresses libiperf's ordinary stdout output by default so
library use does not unexpectedly write to the embedding application's
terminal. Use `inherit_output()` for upstream-style human output, or
`logfile()` to send libiperf output to a file. Retained JSON remains available
through `IperfResult` when `json()` is enabled.

When `json()` is enabled, the completed `IperfResult` retains upstream iperf3
JSON. `json_output()` returns the raw string, and `json_value()` parses it as a
`serde_json::Value` when the `serde` feature is enabled.

Use `MetricsMode::Window(duration)` to receive the same representative window
summaries used by `--push.interval`. `PushGateway` and `PushGatewayConfig` are
also exported for applications that want to push collected metrics themselves.
Protocol-specific fields such as TCP RTT or UDP jitter are exposed as
`Option<f64>`, so application code can distinguish an observed zero from a
metric that libiperf did not report for that protocol or traffic direction.
SCTP runs are identified as `TransportProtocol::Sctp` when libiperf reports
that protocol. Interval samples include `role`, `direction`, `stream_count`,
and `timestamp_unix_seconds` context; window summaries carry the same role,
direction, protocol, stream count, and newest-sample timestamp context.
Bidirectional runs expose one aggregate direction per process: the client-side
sample is `Sender`, and the server-side sample is `Receiver`.
Use `MetricsStream::recv()` for simple iterator-style loops. For automation that
needs to distinguish "no sample yet" from "the run has ended",
`try_recv()` and `recv_timeout()` return `MetricsRecvError::Empty`,
`MetricsRecvError::Timeout`, or `MetricsRecvError::Closed`.

Applications that want to use their own delivery path can reuse the same
encoding and file output as the CLI. `PrometheusEncoder` renders interval and
window snapshots without requiring HTTP dependencies. With the `serde` feature,
`MetricsFileSink` writes JSONL or Prometheus text files from `MetricEvent`,
`Metrics`, or `WindowMetrics` values. Use `with_labels` or
`with_prefix_and_labels` when every Prometheus sample should carry the same
label set:

```rust
use iperf3_rs::{
    MetricEvent, MetricsFileFormat, MetricsFileSink, PrometheusEncoder, Result,
};

fn write_metrics(event: &MetricEvent) -> Result<()> {
    let encoder = PrometheusEncoder::with_labels("nettest", [("site", "ci")])?;
    if let MetricEvent::Interval(metrics) = event {
        let body = encoder.encode_interval(metrics);
        println!("{body}");
    }

    let sink = MetricsFileSink::with_prefix_and_labels(
        "iperf3.prom",
        MetricsFileFormat::Prometheus,
        "nettest",
        [("site", "ci")],
    )?;
    if let MetricEvent::Interval(metrics) = event {
        sink.write_interval(metrics)?;
    }
    Ok(())
}
```

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
Direct Pushgateway delivery is best-effort, matching the CLI contract:
push/delete failures are logged to stderr and do not make the iperf run fail.
Applications that need strict delivery should consume `spawn_with_metrics()` and
handle `PushGateway::push()` or `PushGateway::push_window()` results directly.

The default feature set enables both `pushgateway` and `serde`. The
`pushgateway` feature provides `PushGateway`, `PushGatewayConfig`, and HTTP
delivery dependencies without pulling in JSON serialization by itself. The
`serde` feature derives serialization for metric types and provides
`MetricsFileSink` JSONL output. `PrometheusEncoder` is always available.
Library-only consumers that only need `IperfCommand` and `MetricsStream` can
disable default features:

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

Wrapper metrics options:

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

Pushgateway push and delete requests are best-effort. Failures are reported on
stderr, but they do not make the CLI fail when the iperf run itself succeeds.
Use `--metrics.file` for required artifacts that must affect the exit status.

--push.user-agent VALUE
    HTTP User-Agent for Pushgateway requests. Defaults to iperf3-rs/<version>.

--metrics.prefix PREFIX
    Prometheus metric name prefix for Pushgateway and Prometheus file output.
    Defaults to iperf3.

--push.interval DURATION
    Aggregate libiperf interval samples for this duration before pushing window
    metrics. Accepts values like 500ms, 10s, 1m, or bare seconds. When omitted,
    iperf3-rs pushes immediate interval metrics.

--push.delete-on-exit
    Best-effort deletion of this Pushgateway grouping key after the iperf run
    exits.

--metrics.file PATH
    Write live interval metrics to a file without changing iperf stdout.

--metrics.format FORMAT
    Metrics file format. Accepts jsonl or prometheus. Defaults to jsonl.

--metrics.label KEY=VALUE
    Add a Prometheus sample label to metrics file output. Repeatable.
    Requires --metrics.format prometheus.
```

Every wrapper environment default uses the `IPERF3_RS_` namespace:

```text
IPERF3_RS_PUSH_URL=URL
IPERF3_RS_PUSH_JOB=JOB
IPERF3_RS_PUSH_LABELS=KEY=VALUE,...
IPERF3_RS_PUSH_TIMEOUT=DURATION
IPERF3_RS_PUSH_RETRIES=N
IPERF3_RS_PUSH_USER_AGENT=VALUE
IPERF3_RS_METRICS_PREFIX=PREFIX
IPERF3_RS_PUSH_INTERVAL=DURATION
IPERF3_RS_PUSH_DELETE_ON_EXIT=BOOL
IPERF3_RS_METRICS_FILE=PATH
IPERF3_RS_METRICS_FORMAT=FORMAT
IPERF3_RS_METRICS_LABELS=KEY=VALUE,...
```

CLI values override environment defaults. `IPERF3_RS_PUSH_LABELS` and `IPERF3_RS_METRICS_LABELS`
are applied before their matching CLI label values. Duplicate label names are
rejected within each label set.
Unprefixed names such as `PUSH_URL` or `METRICS_FILE` are intentionally not
read, so generic CI or shell variables cannot enable outputs accidentally.
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

### File output

Use `--metrics.file` when you want live metrics without a Pushgateway:

```sh
iperf3-rs \
  -c 127.0.0.1 \
  -t 10 \
  -i 1 \
  --metrics.file iperf3-metrics.jsonl
```

The default `jsonl` format writes one JSON object per libiperf interval. Each
record includes `schema_version: 1`, an `event` kind, and the metric fields. The
`prometheus` format writes the latest interval as Prometheus text exposition and
atomically replaces the file on each interval, which is useful for textfile
collectors or for keeping the final interval snapshot as a CI artifact:

```sh
iperf3-rs \
  -c 127.0.0.1 \
  -t 10 \
  -i 1 \
  --metrics.file iperf3.prom \
  --metrics.format prometheus \
  --metrics.prefix nettest \
  --metrics.label site=ci
```

File output can be used by itself or together with Pushgateway export. It never
changes stdout, so normal human output and `-J` JSON behavior remain owned by
upstream libiperf. Unlike Pushgateway delivery, file output is required: failing
to create or write the requested file makes the CLI exit with an error.
For Prometheus file output, `--metrics.label` labels are rendered on every
sample, for example `nettest_transferred_bytes{site="ci"} 1234`.

### Metric names

With the default `--metrics.prefix iperf3`, iperf3-rs emits immediate
interval gauges when `--push.interval` is not set:

```text
iperf3_transferred_bytes
iperf3_bandwidth_bits_per_second
iperf3_stream_count
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
iperf3_omitted_intervals
```

Metric mapping:

| Metric | Source field | Notes |
| --- | --- | --- |
| `iperf3_transferred_bytes` | `bytes_transferred` | Interval bytes from the aggregate report side. |
| `iperf3_bandwidth_bits_per_second` | `bytes_transferred`, `interval_duration` | Bits per second for the interval. |
| `iperf3_stream_count` | matched streams | Number of libiperf streams represented by the interval sample. |
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
| `iperf3_omitted_intervals` | `omitted` | `1` for omitted warm-up intervals, otherwise `0`. |

Not every metric is meaningful for every iperf mode. Protocol-specific metrics
that libiperf did not report are omitted from Prometheus output and represented
as `null` in JSONL. TCP_INFO-derived fields depend on libiperf and
operating-system support for TCP information.

When `--push.interval` or `IPERF3_RS_PUSH_INTERVAL` is set, iperf3-rs emits window
summary gauges instead of the immediate interval metric names:

```text
iperf3_window_duration_seconds
iperf3_window_transferred_bytes
iperf3_window_stream_count
iperf3_window_bandwidth_mean_bits_per_second
iperf3_window_bandwidth_min_bits_per_second
iperf3_window_bandwidth_max_bits_per_second
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
| `*_mean_*`, `*_min_*`, `*_max_*` | Arithmetic mean, minimum, and maximum of gauge-like interval values in the pushed window. Bandwidth mean is derived from total transferred bits divided by total interval duration. |
| `iperf3_window_transferred_bytes` | Total transferred bytes across the pushed window. |
| `iperf3_window_stream_count` | Number of libiperf streams represented by the window. |
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

Use a custom prefix when multiple tools share a Pushgateway or textfile
collector directory:

```sh
iperf3-rs \
  -c 127.0.0.1 \
  -t 10 \
  -i 1 \
  --push.url http://127.0.0.1:9091 \
  --metrics.prefix nettest
```

This emits `nettest_transferred_bytes`, `nettest_bandwidth_bits_per_second`, and so on.

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
`native/`. The bundled libiperf build uses `--without-openssl` by default so a
host OpenSSL installation does not change whether `iperf3-rs` links libssl or
libcrypto. Pushgateway HTTPS support comes from Rustls with webpki roots.
Enable the `openssl` Cargo feature when you need upstream iperf authentication,
or pass upstream configure options such as
`IPERF3_RS_CONFIGURE_ARGS=--with-openssl=/opt/openssl`.

Current caveats:

- Metrics export is attached to libiperf's reporting path through the local C
  shim. The CLI keeps stdout behavior aligned with upstream iperf3 while still
  allowing live interval export without patching the submodule; `IperfCommand`
  library runs are quiet by default unless `inherit_output()` or `logfile()` is
  selected.
- `RunningIperf` observes worker completion; dropping it detaches the worker and
  does not stop libiperf. Long-lived `server_unbounded()` runs should live in a
  process dedicated to serving tests, or in a helper process the application can
  terminate externally.
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
