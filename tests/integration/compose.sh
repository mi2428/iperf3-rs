#!/usr/bin/env bash
set -euo pipefail

run_json_with_retry() {
  local label="$1"
  local output="$2"
  shift 2

  local error="/tmp/${label//[^a-zA-Z0-9]/_}.err"
  for attempt in $(seq 1 30); do
    if "$@" >"${output}" 2>"${error}"; then
      jq -e 'has("start") and has("end")' "${output}" >/dev/null
      jq -e '((.end.sum_received.bytes // .end.sum.bytes // .end.sum_sent.bytes // 0) > 0)' "${output}" >/dev/null
      printf 'ok: %s\n' "${label}"
      return 0
    fi

    if [ "${attempt}" -eq 30 ]; then
      printf 'failed: %s\n' "${label}" >&2
      cat "${error}" >&2
      return 1
    fi
    sleep 1
  done
}

wait_for_pushgateway() {
  for attempt in $(seq 1 30); do
    if curl -fsS http://pushgateway:9091/-/ready >/dev/null; then
      return 0
    fi

    if [ "${attempt}" -eq 30 ]; then
      printf 'failed: pushgateway did not become ready\n' >&2
      return 1
    fi
    sleep 1
  done
}

metric_value_gt_zero() {
  local name="$1"
  local line
  line="$(
    grep -E "^${name}\\{" /tmp/pushgateway.metrics \
      | grep 'job="integration"' \
      | grep 'test="self"' \
      | grep 'scenario="tcp"' \
      | grep 'iperf_mode="client"' \
      | head -n 1
  )"

  if [ -z "${line}" ]; then
    return 1
  fi

  awk -v value="$(printf '%s\n' "${line}" | awk '{print $NF}')" 'BEGIN { exit !(value > 0) }'
}

assert_pushgateway_metrics() {
  for attempt in $(seq 1 30); do
    curl -fsS http://pushgateway:9091/metrics >/tmp/pushgateway.metrics
    if metric_value_gt_zero iperf3_bytes && metric_value_gt_zero iperf3_bandwidth; then
      printf 'ok: pushgateway received iperf3-rs metrics\n'
      return 0
    fi

    if [ "${attempt}" -eq 30 ]; then
      printf 'failed: expected iperf3 metrics were not found in pushgateway\n' >&2
      cat /tmp/pushgateway.metrics >&2
      return 1
    fi
    sleep 1
  done
}

wait_for_pushgateway

run_json_with_retry \
  "upstream client to iperf3-rs server" \
  /tmp/upstream-to-iperf3rs.json \
  iperf3 -c iperf3rs-server -t 1 -J

run_json_with_retry \
  "iperf3-rs client to upstream server" \
  /tmp/iperf3rs-to-upstream.json \
  iperf3-rs -c upstream-server -t 1 -J

iperf3-rs \
  --push-gateway http://pushgateway:9091 \
  --job integration \
  --test self \
  --scenario tcp \
  -c iperf3rs-server \
  -t 3 \
  -i 1 \
  --json-stream \
  >/tmp/iperf3rs-self-stream.json

assert_pushgateway_metrics
