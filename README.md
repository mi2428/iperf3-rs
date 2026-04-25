# iperf3-rs

Rust frontend for `libiperf` with live Pushgateway export.

The CLI keeps iperf3 compatibility by passing iperf options to upstream
`iperf_parse_arguments()`. Only iperf3-rs specific options are stripped before
that call:

- `--push-gateway URL`
- `--job NAME`
- `--test NAME`
- `--scenario NAME`

When `--push-gateway` is set, iperf3-rs enables libiperf JSON stream output
internally, receives interval events through `iperf_set_test_json_callback()`,
converts each interval into Prometheus text format, and sends it to Pushgateway.

## Build

This repository vendors esnet/iperf as the `./iperf3` git submodule. For a fresh
checkout:

```sh
git submodule update --init --recursive
make build
```

`build.rs` runs `./configure --enable-static --disable-shared` and builds
`src/.libs/libiperf.a` when needed.

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
  --push-gateway http://127.0.0.1:9091 \
  --job iperf3 \
  --test testrun \
  --scenario sample1
```

The grouping path is:

```text
/metrics/job/{job}/test/{test}/scenario/{scenario}/iperf_mode/{client|server}
```

Exported gauges currently match the iperf3-go names:

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
