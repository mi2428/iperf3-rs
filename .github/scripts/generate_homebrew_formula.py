#!/usr/bin/env python3
"""Generate the Homebrew formula published by the release workflow."""

from __future__ import annotations

import argparse
import os
import re
import sys
import textwrap
from pathlib import Path

ASSETS = {
    "darwin_amd64": "iperf3-rs-darwin-amd64",
    "darwin_arm64": "iperf3-rs-darwin-arm64",
    "linux_amd64": "iperf3-rs-linux-amd64",
    "linux_arm64": "iperf3-rs-linux-arm64",
}

SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", required=True)
    parser.add_argument("--checksums", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument(
        "--repository",
        default=os.environ.get("GITHUB_REPOSITORY", "mi2428/iperf3-rs"),
        help="GitHub repository that owns the release assets",
    )
    return parser.parse_args()


def read_checksums(path: Path) -> dict[str, str]:
    checksums: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        parts = line.split()
        if len(parts) < 2:
            continue
        digest, asset = parts[0], parts[-1].lstrip("*")
        checksums[asset] = digest
    return checksums


def require_checksum(checksums: dict[str, str], asset: str) -> str:
    digest = checksums.get(asset)
    if digest is None:
        raise ValueError(f"missing checksum for {asset}")
    if not SHA256_RE.match(digest):
        raise ValueError(f"invalid SHA-256 checksum for {asset}: {digest}")
    return digest


def formula(args: argparse.Namespace, checksums: dict[str, str]) -> str:
    tag = args.tag
    version = tag.removeprefix("v")
    repository = args.repository

    values = {
        key: require_checksum(checksums, asset) for key, asset in ASSETS.items()
    }

    return textwrap.dedent(
        f"""\
        class Iperf3Rs < Formula
          desc "Rust frontend for libiperf with live Pushgateway export"
          homepage "https://github.com/{repository}"
          version "{version}"
          license "MIT"
          head "https://github.com/{repository}.git", branch: "main"

          depends_on "rust" => :build if build.head?

          on_macos do
            if Hardware::CPU.arm?
              url "https://github.com/{repository}/releases/download/{tag}/iperf3-rs-darwin-arm64",
                  using: :nounzip
              sha256 "{values["darwin_arm64"]}"
            else
              url "https://github.com/{repository}/releases/download/{tag}/iperf3-rs-darwin-amd64",
                  using: :nounzip
              sha256 "{values["darwin_amd64"]}"
            end
          end

          on_linux do
            if Hardware::CPU.arm?
              url "https://github.com/{repository}/releases/download/{tag}/iperf3-rs-linux-arm64",
                  using: :nounzip
              sha256 "{values["linux_arm64"]}"
            else
              url "https://github.com/{repository}/releases/download/{tag}/iperf3-rs-linux-amd64",
                  using: :nounzip
              sha256 "{values["linux_amd64"]}"
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
    args = parse_args()
    try:
        checksums = read_checksums(args.checksums)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(formula(args, checksums), encoding="utf-8")
    except OSError as exc:
        print(exc, file=sys.stderr)
        return 1
    except ValueError as exc:
        print(exc, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
