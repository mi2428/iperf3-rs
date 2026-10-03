# iperf3-rs-bwcheck

`iperf3-rs-bwcheck` is a small UDP bandwidth and packet-loss checker built on the `iperf3-rs` Rust API.
It accepts one or more `HOST:PORT` endpoints, runs a fixed iperf3 UDP test against each endpoint, reads live interval metrics from `libiperf`, and exits successfully only when every endpoint satisfies the configured thresholds.

## Requirements

Each target endpoint must be running an iperf3-compatible server on the requested port:

```console
$ iperf3 -s -p 5201
```

The checker runs as an iperf3 UDP client. It does not start servers for you outside the Docker integration test.

## Usage

```console
$ cargo run -- --help
usage: iperf3-rs-bwcheck [--min-bandwidth-bps N] [--max-loss-percent N] HOST:PORT...
fixed iperf parameters: -u -b 1000000 -t 3 -i 1
```

| Option | Default | Meaning |
| --- | ---: | --- |
| `--min-bandwidth-bps N` | `500000` | Minimum aggregate bandwidth required for each endpoint. |
| `--max-loss-percent N` | `1` | UDP packet loss percentage must be lower than this value for each endpoint. |
| `-h`, `--help` | | Print usage. |

Run it from the repository root:

```text
$ cargo run --manifest-path examples/bwcheck/Cargo.toml -- \
    --min-bandwidth-bps 100000 --max-loss-percent 10 198.18.0.1:5201
```

Or from this directory:

```console
$ cargo run -- --min-bandwidth-bps 100000 --max-loss-percent 10 198.18.0.1:5201
```

Multiple endpoints are checked sequentially:

```console
$ cargo run -- server-a:5201 server-b:5201
```

### Output

Each endpoint prints one line:

```text
PASS endpoint=127.0.0.1:5201 bandwidth_bps=998000 loss_percent=0.000 packets=357 lost_packets=0
```

After all endpoints finish, the checker prints a summary:

```text
summary checked=2 failed=0
```

`bandwidth_bps` is computed from non-omitted live sender intervals. `loss_percent`,
`packets`, and `lost_packets` come from the final structured UDP receiver summary;
sender intervals cannot establish remote packet loss. The packet total already
includes lost packets. Missing or inconsistent receiver counters fail the check.
The probe remains forward UDP, and the tool does not scrape terminal output.

### Exit Status

- `0`: all endpoints passed, or `--help` was requested.
- `1`: at least one endpoint failed its threshold, an endpoint could not be tested, or arguments were invalid.

## Test

The Docker Compose integration test starts two iperf servers, runs the checker against both, and verifies both passing and failing thresholds:

```console
$ cargo test --test integration_test -- --ignored --nocapture --test-threads=1
```

From the repository root, the same test can be run through the top-level Makefile:

```console
$ make integration EXAMPLES=bwcheck
```

The receiver-loss regression also runs locally without Docker (Python 3 required):

```console
$ cargo build --bin iperf3-rs
$ cargo build --manifest-path examples/bwcheck/Cargo.toml
$ python3 examples/bwcheck/loss_test.py target/debug/iperf3-rs examples/bwcheck/target/debug/iperf3-rs-bwcheck
```

It compares a loss-free loopback transfer with a relay that drops every other UDP
data packet, requires a failing checker exit for the latter, and cleans up its
own server and relay threads within bounded waits.

## Limitations

- The traffic profile is intentionally fixed to UDP at 1 Mbit/s for 3 seconds.
- Endpoints are checked sequentially because high-level `iperf3-rs` runs serialize access to process-local libiperf state.
- Thresholds are application policy. Change the constants or extend the argument parser if you need a different probe shape.
