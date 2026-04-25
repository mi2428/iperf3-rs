#!/usr/bin/env bash
set -Eeuo pipefail

# Publish the generated formula into the Homebrew tap repository.
readonly formula_path="${FORMULA_PATH:-dist/iperf3-rs.rb}"
readonly release_tag="${RELEASE_TAG:?RELEASE_TAG is required}"
readonly tap_token="${HOMEBREW_TAP_TOKEN:?HOMEBREW_TAP_TOKEN is required}"
readonly bot_name="github-actions[bot]"
readonly bot_email="41898282+github-actions[bot]@users.noreply.github.com"

workdir=""

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  [[ -z "${workdir}" ]] || rm -rf "${workdir}"
}

# Resolve the tap repository while keeping the default tied to the release owner.
resolve_tap_repository() {
  local owner

  if [[ -n "${HOMEBREW_TAP_REPOSITORY:-}" ]]; then
    printf '%s\n' "${HOMEBREW_TAP_REPOSITORY}"
    return
  fi

  if [[ -n "${GITHUB_REPOSITORY_OWNER:-}" ]]; then
    printf '%s/homebrew-iperf3-rs\n' "${GITHUB_REPOSITORY_OWNER}"
    return
  fi

  if [[ -n "${GITHUB_REPOSITORY:-}" && "${GITHUB_REPOSITORY}" == */* ]]; then
    owner="${GITHUB_REPOSITORY%%/*}"
    printf '%s/homebrew-iperf3-rs\n' "${owner}"
    return
  fi

  die "HOMEBREW_TAP_REPOSITORY or GitHub repository metadata is required"
}

# Keep tap commits clearly attributable to GitHub Actions.
configure_git_author() {
  git -C "${workdir}" config user.name "${bot_name}"
  git -C "${workdir}" config user.email "${bot_email}"
}

# Copy the generated formula, commit it only when it changed, and push it.
publish_formula() {
  local tap_repository="$1"

  git clone "https://github.com/${tap_repository}.git" "${workdir}"
  mkdir -p "${workdir}/Formula"
  cp "${formula_path}" "${workdir}/Formula/iperf3-rs.rb"

  configure_git_author
  git -C "${workdir}" add Formula/iperf3-rs.rb

  if git -C "${workdir}" diff --cached --quiet; then
    printf 'Homebrew formula is already up to date\n'
    return
  fi

  git -C "${workdir}" commit -m "iperf3-rs ${release_tag}"
  git -C "${workdir}" remote set-url origin \
    "https://x-access-token:${tap_token}@github.com/${tap_repository}.git"
  git -C "${workdir}" push
}

main() {
  local tap_repository

  [[ -f "${formula_path}" ]] || die "Homebrew formula not found: ${formula_path}"

  tap_repository="$(resolve_tap_repository)"
  workdir="$(mktemp -d)"
  trap cleanup EXIT

  publish_formula "${tap_repository}"
}

main "$@"
