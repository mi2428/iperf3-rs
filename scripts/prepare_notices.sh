#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
mkdir -p "${1:-.}"
destination=$(CDPATH='' cd -- "${1:-.}" && pwd)
if [ "$destination" != "$root" ]; then
  cp "$root/LICENSE" "$root/LICENSE-SORACOM" "$destination/"
fi
cp "$root/iperf3/LICENSE" "$destination/LICENSE-IPERF3"
