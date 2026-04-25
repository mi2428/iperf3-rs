#!/usr/bin/env bash
set -Eeuo pipefail

# Emit GitHub Actions output values shared by the release image build.
readonly release_tag="${RELEASE_TAG:?RELEASE_TAG is required}"
readonly release_prerelease="${RELEASE_PRERELEASE:-false}"
readonly output="${GITHUB_OUTPUT:-/dev/stdout}"
readonly repository="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"

# Compute each Git-derived value once so the output block is easy to audit.
main() {
  local build_date git_commit git_commit_date git_describe image

  image="ghcr.io/${repository,,}"
  git_commit="$(git rev-parse HEAD)"
  git_commit_date="$(git show -s --format=%cI HEAD)"
  git_describe="$(git describe --tags --always --dirty=-dirty)"
  build_date="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

  {
    printf 'tags<<EOF\n'
    printf '%s:%s\n' "${image}" "${release_tag}"
    if [[ "${release_prerelease}" != "true" ]]; then
      printf '%s:latest\n' "${image}"
    fi
    printf 'EOF\n'
    printf 'source=https://github.com/%s\n' "${repository}"
    printf 'revision=%s\n' "${git_commit}"
    printf 'version=%s\n' "${release_tag}"
    printf 'git_describe=%s\n' "${git_describe}"
    printf 'git_commit=%s\n' "${git_commit}"
    printf 'git_commit_date=%s\n' "${git_commit_date}"
    printf 'build_date=%s\n' "${build_date}"
  } >> "${output}"
}

main "$@"
