# iperf3-rs bwcheck example

This example is a small application that uses `iperf3-rs` as a library crate.
It accepts `HOST:PORT` endpoints, runs fixed UDP iperf parameters against each
endpoint, consumes live interval metrics, and exits successfully only when every
endpoint reaches the configured minimum bandwidth and stays below the configured
loss threshold.

```sh
cargo run --manifest-path examples/bwcheck/Cargo.toml -- \
  --min-bandwidth-bps 100000 \
  --max-loss-percent 10 \
  127.0.0.1:5201
```

Fixed iperf parameters:

```text
-u -b 1M -t 3 -i 1
```

The Docker Compose integration test in this directory is intended to protect the
library API usage path from regressions independently of the main CLI
integration test.

Run it from the repository root with:

```sh
make integration EXAMPLES=bwcheck
```
