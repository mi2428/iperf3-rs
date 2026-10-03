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

  # Validate before emitting outputs or starting registry operations. Stable
  # tags may explicitly suppress latest; prerelease tags must suppress it.
  python3 - <<'PY'
import os
from pathlib import Path
import re
import subprocess
import tomllib

tag = os.environ["RELEASE_TAG"]
prerelease = os.environ.get("RELEASE_PRERELEASE", "false")
number = r"(?:0|[1-9][0-9]*)"
if len(tag) > 128 or not re.fullmatch(rf"v?{number}\.{number}\.{number}(?:-[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*)?", tag):
    raise SystemExit("RELEASE_TAG must be a Docker-compatible SemVer tag (build metadata is unsupported)")
if prerelease not in ("true", "false"):
    raise SystemExit("RELEASE_PRERELEASE must be true or false")
version = tag.removeprefix("v")
if "-" in version:
    identifiers = version.split("-", 1)[1].split(".")
    if any(part.isdigit() and len(part) > 1 and part.startswith("0") for part in identifiers):
        raise SystemExit("numeric prerelease identifiers must not have leading zeroes")
    if prerelease != "true":
        raise SystemExit("prerelease tags must suppress latest")
try:
    tagged = subprocess.check_output(["git", "rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}"], text=True, stderr=subprocess.DEVNULL).strip()
except subprocess.CalledProcessError:
    raise SystemExit("RELEASE_TAG must resolve to an existing Git tag")
head = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
if tagged != head:
    raise SystemExit("checked-out HEAD does not match RELEASE_TAG")
if tomllib.loads(Path("Cargo.toml").read_text())["package"]["version"] != version:
    raise SystemExit("Cargo package version does not match RELEASE_TAG")
PY

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
