# iperf3-rs

[![Release](https://github.com/mi2428/iperf3-rs/actions/workflows/release.yml/badge.svg)](https://github.com/mi2428/iperf3-rs/actions/workflows/release.yml) [![GHCR](https://github.com/mi2428/iperf3-rs/actions/workflows/ghcr.yml/badge.svg)](https://github.com/mi2428/iperf3-rs/actions/workflows/ghcr.yml)

A Rust frontend for `libiperf` that adds live observability while keeping iperf3 behavior intact.

- **Live Metrics:** Exports interval results to a Prometheus Pushgateway directly from `libiperf` while a test is running, without waiting for final JSON output or scraping CLI text.
- **Upstream Compatibility:** Uses upstream `esnet/iperf3` under the hood instead of a Rust reimplementation, so iperf3 options and wire behavior stay aligned with upstream.

[![](https://github.com/mi2428/iperf3-rs/blob/main/screenshot.png?raw=true)](https://github.com/mi2428/iperf3-rs/blob/main/screenshot.png)

## Installation

### Cargo (crates.io)

Install the CLI from crates.io with Cargo:

```console
$ cargo install iperf3-rs
```

To install a specific version:

```console
$ cargo install iperf3-rs --version 1.0.0
```

Cargo builds the vendored `libiperf` source, so the host needs a C compiler, `make`, and `pkg-config`.

### macOS (Homebrew)

Prebuilt macOS binaries are available from the Homebrew tap:

```console
$ brew tap mi2428/iperf3-rs
$ brew install iperf3-rs
```

### Build from source

Source builds compile the vendored upstream iperf3 submodule. Clone recursively, or initialize the submodule in an existing checkout, then install the release binary and shell completions:

```console
$ git clone --recursive https://github.com/mi2428/iperf3-rs
$ git submodule update --init --recursive  # if the submodule is missing
```

```console
$ make install               # install the binary only
$ make install COMPLETION=1  # install the binary and shell completions
```

## CLI Usage

Use `iperf3-rs` like `iperf3`, then add `--push.*` or `--metrics.*` options when you need live interval metrics.
The help output keeps the upstream iperf3 options and adds an `iperf3-rs options` section near the top:

```console
$ iperf3-rs -h | head -40

Usage: iperf3 [-s|-c host] [options]
       iperf3 [-h|--help] [-v|--version]

iperf3-rs options:
  --push.url URL            push interval metrics to a Pushgateway URL
                            bare host:port values default to http://
  --push.delete-on-exit     delete this Pushgateway grouping key after the run exits
  --push.interval DURATION  aggregate interval samples before pushing window metrics
  --push.job JOB            Pushgateway job name (default: iperf3)
  --push.label KEY=VALUE    add a Pushgateway grouping label; repeatable
  --push.retries N          retry failed Pushgateway requests N times (default: 0)
  --push.timeout DURATION   per-request timeout: 500ms, 5s, 1m, or seconds (default: 5s)
  --push.user-agent VALUE   HTTP User-Agent for Pushgateway requests
  --metrics.file PATH       write live interval metrics to a file
                            does not change iperf stdout
  --metrics.format FORMAT   metrics file format: jsonl or prometheus (default: jsonl)
  --metrics.label KEY=VALUE add a Prometheus file sample label; repeatable
                            requires --metrics.format prometheus
  --metrics.prefix P        Prometheus metric name prefix (default: iperf3)

iperf3-rs environment:
  IPERF3_PUSH_URL=URL                 default value for --push.url
  IPERF3_PUSH_DELETE_ON_EXIT=BOOL     default value for --push.delete-on-exit
  IPERF3_PUSH_INTERVAL=DURATION       default value for --push.interval
  IPERF3_PUSH_JOB=JOB                 default value for --push.job
  IPERF3_PUSH_LABELS=KEY=VALUE,...    default labels added before --push.label values
  IPERF3_PUSH_RETRIES=N               default value for --push.retries
  IPERF3_PUSH_TIMEOUT=DURATION        default value for --push.timeout
  IPERF3_PUSH_USER_AGENT=VALUE        default value for --push.user-agent
  IPERF3_METRICS_FILE=PATH            default value for --metrics.file
  IPERF3_METRICS_FORMAT=FORMAT        default value for --metrics.format
  IPERF3_METRICS_LABELS=KEY=VALUE,... default labels for Prometheus file output
  IPERF3_METRICS_PREFIX=P             default value for --metrics.prefix

Server or Client:
  -p, --port      #         server port to listen on/connect to
  -f, --format   [kmgtKMGT] format to report: Kbits, Mbits, Gbits, Tbits
  -i, --interval  #         seconds between periodic throughput reports
  -I, --pidfile file        write PID file
  -F, --file name           xmit/recv the specified file
```

CLI values override environment defaults. Duplicate label names are rejected within each label set.
Pushgateway writes and delete-on-exit cleanup are best-effort.
Failures are reported on stderr, but they do not make the CLI fail when the iperf run itself succeeds.

### Pushgateway

Start the local observability stack (Prometheus, Pushgateway, and Grafana) with Docker Compose:

```console
$ docker compose up
```

Bare `host:port` Pushgateway values default to HTTP. Use an explicit `https://` URL when your Pushgateway requires TLS.
Add grouping labels with `--push.label KEY=VALUE`, repeating the flag for multiple labels.
The grouping key always includes `job` (default: `iperf3`); other labels are included only when you set them.
When a client and server, or multiple concurrent sessions, push to the same gateway, add labels that distinguish the role or test run.

```text
$ iperf3-rs -c 198.18.0.1 \
    --push.url 127.0.0.1:9091 \
    --push.label role=client --push.label scenario=sample 
$ iperf3-rs -s \
    --push.url 127.0.0.1:9091 \
    --push.label role=server --push.label scenario=sample 
```

If the Pushgateway cannot keep up with every iperf interval, use `--push.interval` to aggregate samples locally and push one window summary per interval.
Window pushes use `iperf3_window_*` metric families such as `iperf3_window_duration_seconds`, `iperf3_window_transferred_bytes`, and `iperf3_window_bandwidth_mean_bits_per_second`.

```text
$ iperf3-rs -s \
    --push.url 127.0.0.1:9091 --push.interval 10s \
    --push.label role=server --push.label scenario=sample 
```

### File Output

Use `--metrics.file` when the metrics artifact should affect the exit status:

```console
$ iperf3-rs --metrics.file iperf3-metrics.jsonl -c 198.18.0.1
```

The default `jsonl` format writes one JSON object per `libiperf` interval.
The `prometheus` format writes the latest interval as Prometheus text exposition and atomically replaces the file on each interval:

```text
$ iperf3-rs -c 198.18.0.1 \
    --metrics.file iperf3.prom --metrics.format prometheus --metrics.prefix nettest \
    --metrics.label role=client --metrics.label scenario=sample
```

File output can be used by itself or together with Pushgateway export.
It does not change normal iperf stdout or `-J` JSON behavior.

## Rust API Usage

Use `iperf3-rs` as a Rust crate when bots, controllers, or test harnesses need to run iperf directly and consume live metrics programmatically.
See the [examples](examples) directory for complete library examples.

```toml
[dependencies]
iperf3-rs = "1"
```

```rust
use std::time::Duration;

use iperf3_rs::{IperfCommand, MetricEvent, MetricsMode, Result};

fn main() -> Result<()> {
    let mut command = IperfCommand::client("127.0.0.1");
    command
        .duration(Duration::from_secs(10))
        .report_interval(Duration::from_secs(1));

    let (running, metrics) = command.spawn_with_metrics(MetricsMode::Interval)?;

    while let Some(MetricEvent::Interval(sample)) = metrics.recv() {
        println!("{:.0} bit/s", sample.bandwidth_bits_per_second);
    }

    running.wait()?;
    Ok(())
}
```

Developer setup, verification, release operations, and detailed behavior contracts live in [CONTRIBUTING.md](CONTRIBUTING.md).

## License

- `iperf3-rs` Rust code is licensed under MIT.
- The vendored [`esnet/iperf3`](https://github.com/esnet/iperf) source keeps its upstream BSD-style license and third-party notices.
- SORACOM, Inc. also has a separate permissive license grant. See [LICENSE-SORACOM](LICENSE-SORACOM).
