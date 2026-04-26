# iperf3-rs

[![Release](https://github.com/mi2428/iperf3-rs/actions/workflows/release.yml/badge.svg)](https://github.com/mi2428/iperf3-rs/actions/workflows/release.yml) [![GHCR](https://github.com/mi2428/iperf3-rs/actions/workflows/ghcr.yml/badge.svg)](https://github.com/mi2428/iperf3-rs/actions/workflows/ghcr.yml)

Rust frontend for upstream `libiperf` with live Prometheus Pushgateway export.

`iperf3-rs` is intentionally not a shell wrapper around the `iperf3` executable.
It links the `esnet/iperf3` `libiperf` library directly through FFI, lets
upstream parse and execute normal iperf3 tests, and observes libiperf interval
results while the test is still running.

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
a Rust-managed callback on libiperf's reporting path. By default, each interval
is converted to Prometheus text format and pushed to Pushgateway immediately;
with `--push.interval`, iperf3-rs pushes window summary metrics instead. In
practice, this means Prometheus-backed views can update during the test, not
only after it.

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
iperf3-rs metrics export, window metric aggregation, UDP, reverse TCP,
bidirectional TCP, server-side metrics, and live interval visibility before the
client exits.

## How live metrics work

When `--push.url` or `PUSH_URL` is set, iperf3-rs:

1. lets libiperf parse the iperf3 arguments;
2. installs an interval metrics callback without changing the user-requested
   output mode;
3. receives the latest libiperf interval summary from the reporting path;
4. maps each interval summary to Prometheus gauges;
5. sends either the newest interval sample or an aggregated window summary to
   Pushgateway.

The callback path is deliberately nonblocking. In the default immediate mode,
it sends interval metrics into a size-one channel and a worker thread performs
HTTP writes. If Pushgateway is slow and interval events arrive faster than they
can be pushed, the queued sample is replaced with the newest interval. For
gauges, freshness is more useful than replaying stale samples.

Metrics are emitted once per libiperf reporting interval. Use normal iperf3
interval controls such as `-i 1` when you want one-second pushes.

When `--push.interval` is set, iperf3-rs buffers libiperf interval samples in
the worker thread and pushes representative `*_window_*` gauges once per window.
The final partial window is flushed when the iperf test exits. This is not a
historical datapoint replay mechanism; Pushgateway still stores the latest
sample for the grouping key, and iperf3-rs performs the aggregation before the
push. If the same grouping key previously received immediate metrics,
Pushgateway can show that older body until the first window summary is pushed.

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

## Rust library API

The same libiperf frontend is available as a Rust crate. This is intended for
programs that want to start iperf tests directly from Rust instead of spawning
the `iperf3-rs` CLI and scraping its output.

`IperfCommand` accepts normal iperf arguments, excluding `argv[0]`, and passes
them to upstream `iperf_parse_arguments()`:

```rust
use iperf3_rs::{IperfCommand, MetricEvent, MetricsMode};

fn run_client() -> anyhow::Result<()> {
    let mut command = IperfCommand::new();
    command
        .args(["-c", "127.0.0.1", "-t", "10", "-i", "1"])
        .metrics(MetricsMode::Interval);

    let mut running = command.spawn()?;
    let mut metrics = running.take_metrics().expect("metrics enabled");

    while let Some(event) = metrics.recv() {
        match event {
            MetricEvent::Interval(sample) => {
                println!("{} bit/s", sample.bandwidth_bits_per_second);
            }
            MetricEvent::Window(window) => {
                println!("{} bytes", window.transferred_bytes);
            }
            _ => {}
        }
    }

    running.wait()?;
    Ok(())
}
```

Use `MetricsMode::Window(duration)` to receive the same representative window
summaries used by `--push.interval`. `PushGateway` and `PushGatewayConfig` are
also exported for applications that want to push the collected metrics
themselves.

The first public API keeps high-level `IperfCommand` runs serialized inside one
process because libiperf still has process-global error, signal, and output
state. For local client/server interop tests, run the peer as a separate
process, container, or VM.

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

--push.interval DURATION
    Aggregate libiperf interval samples for this duration before pushing window
    metrics. Accepts values like 500ms, 10s, 1m, or bare seconds. When omitted,
    iperf3-rs pushes immediate interval metrics.
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
PUSH_INTERVAL=DURATION
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
/metrics/job/{job}/{label_name}/{label_value}/...
```

`iperf3-rs` does not add role or mode labels automatically. Add explicit labels
such as `mode=client` or `mode=server` with `--push.label` when you want them.
User labels may use any Prometheus-style label name:

```text
[a-zA-Z_][a-zA-Z0-9_]*
```

The reserved label names are:

- `job`

Label values must be non-empty. Path segments are percent-encoded, so values can
contain characters such as `/` or spaces.

## Exported metrics

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

## Build notes

The upstream `esnet/iperf3` source is vendored as the `./iperf3` git submodule.
The Rust build script compiles `libiperf` from that submodule instead of linking
to a system iperf3 package.

The final Rust binary links a static `libiperf.a` plus a small C shim from
`native/`. Release builds use `IPERF3_RS_CONFIGURE_ARGS=--without-openssl` for
the bundled libiperf build, while Pushgateway HTTPS support comes from Rustls
with webpki roots.

See [CONTRIBUTING.md](CONTRIBUTING.md) for local development commands,
integration tests, Kani checks, release workflow details, and maintainer setup.

## Current caveats

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

## License

`iperf3-rs` is dual-licensed:

- General use is licensed under the MIT License. See [LICENSE](LICENSE).
- SORACOM, Inc. is additionally granted a separate permissive license with
  Unlicense-style terms. See [LICENSE-SORACOM](LICENSE-SORACOM).

The vendored `esnet/iperf3` submodule keeps its upstream license.
