#!/usr/bin/env bash
set -euo pipefail

release_tag="${RELEASE_TAG:?RELEASE_TAG is required}"
release_prerelease="${RELEASE_PRERELEASE:-false}"
output="${GITHUB_OUTPUT:-/dev/stdout}"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
image="ghcr.io/${GITHUB_REPOSITORY,,}"

{
  printf 'tags<<EOF\n'
  printf '%s:%s\n' "${image}" "${release_tag}"
  if [ "${release_prerelease}" != "true" ]; then
    printf '%s:latest\n' "${image}"
  fi
  printf 'EOF\n'
  printf 'source=https://github.com/%s\n' "${GITHUB_REPOSITORY}"
  printf 'revision=%s\n' "$(git rev-parse HEAD)"
  printf 'version=%s\n' "${release_tag}"
  printf 'git_describe=%s\n' "$(git describe --tags --always --dirty=-dirty)"
  printf 'git_commit=%s\n' "$(git rev-parse HEAD)"
  printf 'git_commit_date=%s\n' "$(git show -s --format=%cI HEAD)"
  printf 'build_date=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
} >> "${output}"
