#!/usr/bin/env bash
set -Eeuo pipefail

readonly SEMVER_TAG_RE='^v[0-9]+[.][0-9]+[.][0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?([+][0-9A-Za-z][0-9A-Za-z.-]*)?$'

release_created_tag=0
release_pushed_created_tag=0
release_tag=
release_worktree=

fail() {
  echo "release: $*" >&2
  exit 1
}

run() {
  printf '+'
  printf ' %q' "$@"
  printf '\n'
  "$@"
}

manifest_value_at_ref() {
  local ref="$1"
  local key="$2"

  git show "${ref}:Cargo.toml" | sed -n "s/^${key} = \"\\(.*\\)\"/\\1/p" | head -n 1
}

require_clean_worktree() {
  local status

  status="$(git status --porcelain)"
  if [[ -n "${status}" ]]; then
    git status --short >&2
    fail "working tree must be clean before release"
  fi
}

require_crates_io_token() {
  local cargo_home="${CARGO_HOME:-${HOME}/.cargo}"

  if [[ -n "${CARGO_REGISTRY_TOKEN:-}" ]]; then
    return
  fi
  if [[ -s "${cargo_home}/credentials.toml" || -s "${cargo_home}/credentials" ]]; then
    return
  fi

  fail "crates.io token is required; run 'cargo login' or set CARGO_REGISTRY_TOKEN"
}

cleanup() {
  local status=$?

  if [[ -n "${release_worktree}" ]]; then
    git worktree remove --force "${release_worktree}" >/dev/null 2>&1 || rm -rf -- "${release_worktree}"
  fi
  if [[ "${release_created_tag}" == "1" && "${release_pushed_created_tag}" == "0" && -n "${release_tag}" ]]; then
    git tag -d "${release_tag}" >/dev/null 2>&1 || true
  fi

  return "${status}"
}

crates_io_version_exists() {
  local package_name="$1"
  local package_version="$2"
  local status

  command -v curl >/dev/null 2>&1 || return 1
  status="$(curl -sS -o /dev/null -w '%{http_code}' "https://crates.io/api/v1/crates/${package_name}/${package_version}" || true)"
  case "${status}" in
    200) return 0 ;;
    404) return 1 ;;
    *) fail "failed to query crates.io for ${package_name} ${package_version}: HTTP ${status}" ;;
  esac
}

main() {
  local tag="${TAG:-}"
  local remote="${GIT_REMOTE:-origin}"
  local cargo="${CARGO:-cargo}"
  local repo_root tag_version local_oid remote_line remote_oid release_ref
  local package_name package_version
  local version_published=0

  [[ -n "${tag}" ]] || fail "TAG is required, for example: make release TAG=v1.0.1"
  [[ "${tag}" =~ ${SEMVER_TAG_RE} ]] || fail "TAG must look like vMAJOR.MINOR.PATCH"

  repo_root="$(git rev-parse --show-toplevel)"
  cd "${repo_root}"
  release_tag="${tag}"
  trap cleanup EXIT

  require_clean_worktree

  remote_line="$(git ls-remote --tags "${remote}" "refs/tags/${tag}" | sed -n '1p')"
  remote_oid="${remote_line%%[[:space:]]*}"

  if git rev-parse -q --verify "refs/tags/${tag}" >/dev/null; then
    local_oid="$(git rev-parse "refs/tags/${tag}")"
    if [[ -n "${remote_oid}" && "${remote_oid}" != "${local_oid}" ]]; then
      fail "local tag ${tag} does not match ${remote}/tags/${tag}"
    fi
    printf 'Using existing tag %s at %s\n' "${tag}" "$(git rev-list -n 1 "${tag}")"
  elif [[ -n "${remote_oid}" ]]; then
    run git fetch "${remote}" "refs/tags/${tag}:refs/tags/${tag}"
    printf 'Using fetched tag %s at %s\n' "${tag}" "$(git rev-list -n 1 "${tag}")"
  else
    run git tag "${tag}"
    release_created_tag=1
    printf 'Created tag %s at %s\n' "${tag}" "$(git rev-parse HEAD)"
  fi

  release_ref="refs/tags/${tag}"
  tag_version="${tag#v}"
  package_name="$(manifest_value_at_ref "${release_ref}" "name")"
  package_version="$(manifest_value_at_ref "${release_ref}" "version")"

  [[ "${package_name}" == "iperf3-rs" ]] || fail "Cargo.toml package name is ${package_name}, expected iperf3-rs"
  [[ "${package_version}" == "${tag_version}" ]] || fail "Cargo.toml version ${package_version} does not match ${tag}"

  if crates_io_version_exists "${package_name}" "${package_version}"; then
    version_published=1
    printf '%s %s already exists on crates.io; cargo publish will be skipped\n' "${package_name}" "${package_version}"
  else
    require_crates_io_token
  fi

  release_worktree="$(mktemp -d "${TMPDIR:-/tmp}/iperf3-rs-release-${tag}.XXXXXX")"

  run git worktree add --detach "${release_worktree}" "${tag}"
  run git -C "${release_worktree}" submodule update --init --recursive

  (
    cd "${release_worktree}"
    run "${cargo}" publish --dry-run --locked
  )

  run git push "${remote}" "refs/tags/${tag}"
  release_pushed_created_tag=1

  if [[ "${version_published}" == "0" ]]; then
    (
      cd "${release_worktree}"
      run "${cargo}" publish --locked
    )
  fi

  if [[ "${version_published}" == "0" ]]; then
    printf 'Published %s %s and pushed tag %s to %s\n' "${package_name}" "${package_version}" "${tag}" "${remote}"
  else
    printf 'Verified existing %s %s and pushed tag %s to %s\n' "${package_name}" "${package_version}" "${tag}" "${remote}"
  fi
}

main "$@"
