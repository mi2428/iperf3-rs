#!/usr/bin/env bash
set -euo pipefail

formula_path="${FORMULA_PATH:-dist/iperf3-rs.rb}"
release_tag="${RELEASE_TAG:?RELEASE_TAG is required}"
tap_token="${HOMEBREW_TAP_TOKEN:?HOMEBREW_TAP_TOKEN is required}"

if [ ! -f "${formula_path}" ]; then
  echo "Homebrew formula not found: ${formula_path}" >&2
  exit 1
fi

if [ -n "${HOMEBREW_TAP_REPOSITORY:-}" ]; then
  tap_repository="${HOMEBREW_TAP_REPOSITORY}"
elif [ -n "${GITHUB_REPOSITORY_OWNER:-}" ]; then
  tap_repository="${GITHUB_REPOSITORY_OWNER}/homebrew-iperf3-rs"
elif [ -n "${GITHUB_REPOSITORY:-}" ]; then
  owner="${GITHUB_REPOSITORY%%/*}"
  tap_repository="${owner}/homebrew-iperf3-rs"
else
  echo "HOMEBREW_TAP_REPOSITORY or GitHub repository metadata is required" >&2
  exit 1
fi

workdir="$(mktemp -d)"
trap 'rm -rf "${workdir}"' EXIT

git clone "https://github.com/${tap_repository}.git" "${workdir}"
mkdir -p "${workdir}/Formula"
cp "${formula_path}" "${workdir}/Formula/iperf3-rs.rb"

git -C "${workdir}" config user.name "github-actions[bot]"
git -C "${workdir}" config user.email \
  "41898282+github-actions[bot]@users.noreply.github.com"
git -C "${workdir}" add Formula/iperf3-rs.rb

if git -C "${workdir}" diff --cached --quiet; then
  echo "Homebrew formula is already up to date"
  exit 0
fi

git -C "${workdir}" commit -m "iperf3-rs ${release_tag}"
git -C "${workdir}" remote set-url origin \
  "https://x-access-token:${tap_token}@github.com/${tap_repository}.git"
git -C "${workdir}" push
