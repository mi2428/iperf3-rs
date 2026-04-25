#!/usr/bin/env python3
"""Generate the Homebrew formula published by the release workflow.

The release workflow builds platform-specific binaries first, then writes a
checksums.txt file. This script turns those checksums into a Homebrew formula
that downloads the prebuilt GitHub Release asset for the user's OS/CPU pair.
"""

from __future__ import annotations

import argparse
import os
import re
import sys
import textwrap
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Final

ASSETS: Final[Mapping[str, str]] = MappingProxyType(
    {
        "darwin_amd64": "iperf3-rs-darwin-amd64",
        "darwin_arm64": "iperf3-rs-darwin-arm64",
        "linux_amd64": "iperf3-rs-linux-amd64",
        "linux_arm64": "iperf3-rs-linux-arm64",
    }
)

SHA256_RE: Final[re.Pattern[str]] = re.compile(r"^[0-9a-f]{64}$")


@dataclass(frozen=True, slots=True)
class Config:
    """Validated command-line configuration for formula generation."""

    tag: str
    checksums: Path
    output: Path
    repository: str

    @property
    def version(self) -> str:
        """Return the Homebrew version derived from a GitHub Release tag."""
        return self.tag.removeprefix("v")


def parse_args(argv: Sequence[str] | None = None) -> Config:
    """Parse CLI arguments into a typed immutable config object."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--checksums", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument(
        "--repository",
        default=os.environ.get("GITHUB_REPOSITORY", "mi2428/iperf3-rs"),
        help="GitHub repository that owns the release assets",
    )
    namespace: argparse.Namespace = parser.parse_args(argv)
    return Config(
        tag=namespace.tag,
        checksums=namespace.checksums,
        output=namespace.output,
        repository=namespace.repository,
    )


def read_checksums(path: Path) -> dict[str, str]:
    """Read a sha256sum-compatible checksum file keyed by asset name."""
    checksums: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        parts: list[str] = line.split()
        if len(parts) < 2:
            continue
        digest: str = parts[0]
        asset: str = parts[-1].lstrip("*")
        checksums[asset] = digest
    return checksums


def require_checksum(checksums: Mapping[str, str], asset: str) -> str:
    """Return a valid SHA-256 digest for an expected release asset."""
    digest: str | None = checksums.get(asset)
    if digest is None:
        raise ValueError(f"missing checksum for {asset}")
    if not SHA256_RE.match(digest):
        raise ValueError(f"invalid SHA-256 checksum for {asset}: {digest}")
    return digest


def render_formula(config: Config, checksums: Mapping[str, str]) -> str:
    """Render the Homebrew formula with release URLs and checked digests."""
    digests: dict[str, str] = {
        key: require_checksum(checksums, asset) for key, asset in ASSETS.items()
    }
    base_url: str = (
        f"https://github.com/{config.repository}/releases/download/{config.tag}"
    )
    head_url: str = f"https://github.com/{config.repository}.git"

    return textwrap.dedent(
        f"""\
        class Iperf3Rs < Formula
          desc "Rust frontend for libiperf with live Pushgateway export"
          homepage "https://github.com/{config.repository}"
          version "{config.version}"
          license "MIT"
          head "{head_url}", branch: "main"

          depends_on "rust" => :build if build.head?

          on_macos do
            if Hardware::CPU.arm?
              url "{base_url}/iperf3-rs-darwin-arm64",
                  using: :nounzip
              sha256 "{digests["darwin_arm64"]}"
            else
              url "{base_url}/iperf3-rs-darwin-amd64",
                  using: :nounzip
              sha256 "{digests["darwin_amd64"]}"
            end
          end

          on_linux do
            if Hardware::CPU.arm?
              url "{base_url}/iperf3-rs-linux-arm64",
                  using: :nounzip
              sha256 "{digests["linux_arm64"]}"
            else
              url "{base_url}/iperf3-rs-linux-amd64",
                  using: :nounzip
              sha256 "{digests["linux_amd64"]}"
            end
          end

          def install
            if build.head?
              system "git", "submodule", "update", "--init", "--recursive"
              ENV["IPERF3_RS_CONFIGURE_ARGS"] = "--without-openssl"
              system "cargo", "install", *std_cargo_args

              bash_completion.install "completions/iperf3-rs.bash" => "iperf3-rs"
              zsh_completion.install "completions/_iperf3-rs"
              fish_completion.install "completions/iperf3-rs.fish"
            else
              binary = Dir["iperf3-rs-*"].first
              chmod 0755, binary
              bin.install binary => "iperf3-rs"
            end
          end

          test do
            assert_match "iperf3-rs", shell_output("#{{bin}}/iperf3-rs --version")
          end
        end
        """
    )


def main() -> int:
    """Run formula generation and return a process exit code."""
    config: Config = parse_args()
    try:
        checksums: dict[str, str] = read_checksums(config.checksums)
        config.output.parent.mkdir(parents=True, exist_ok=True)
        config.output.write_text(render_formula(config, checksums), encoding="utf-8")
    except OSError as exc:
        print(exc, file=sys.stderr)
        return 1
    except ValueError as exc:
        print(exc, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
