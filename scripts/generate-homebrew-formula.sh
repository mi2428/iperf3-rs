#!/usr/bin/env bash
set -euo pipefail

tag="${1:?usage: generate-homebrew-formula.sh <tag> <checksums.txt> <output.rb>}"
checksums="${2:?usage: generate-homebrew-formula.sh <tag> <checksums.txt> <output.rb>}"
output="${3:?usage: generate-homebrew-formula.sh <tag> <checksums.txt> <output.rb>}"
repository="${GITHUB_REPOSITORY:-mi2428/iperf3-rs}"
version="${tag#v}"

checksum_for() {
  local asset="$1"
  awk -v asset="${asset}" '$2 == asset { print $1 }' "${checksums}"
}

darwin_amd64_sha="$(checksum_for iperf3-rs-darwin-amd64)"
darwin_arm64_sha="$(checksum_for iperf3-rs-darwin-arm64)"
linux_amd64_sha="$(checksum_for iperf3-rs-linux-amd64)"
linux_arm64_sha="$(checksum_for iperf3-rs-linux-arm64)"

for value in \
  "${darwin_amd64_sha}" \
  "${darwin_arm64_sha}" \
  "${linux_amd64_sha}" \
  "${linux_arm64_sha}"; do
  if [ -z "${value}" ]; then
    echo "missing checksum in ${checksums}" >&2
    exit 1
  fi
done

mkdir -p "$(dirname "${output}")"

cat > "${output}" <<FORMULA
class Iperf3Rs < Formula
  desc "Rust frontend for libiperf with live Pushgateway export"
  homepage "https://github.com/${repository}"
  version "${version}"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/${repository}/releases/download/${tag}/iperf3-rs-darwin-arm64",
          using: :nounzip
      sha256 "${darwin_arm64_sha}"
    else
      url "https://github.com/${repository}/releases/download/${tag}/iperf3-rs-darwin-amd64",
          using: :nounzip
      sha256 "${darwin_amd64_sha}"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/${repository}/releases/download/${tag}/iperf3-rs-linux-arm64",
          using: :nounzip
      sha256 "${linux_arm64_sha}"
    else
      url "https://github.com/${repository}/releases/download/${tag}/iperf3-rs-linux-amd64",
          using: :nounzip
      sha256 "${linux_amd64_sha}"
    end
  end

  def install
    binary = Dir["iperf3-rs-*"].first
    chmod 0755, binary
    bin.install binary => "iperf3-rs"
  end

  test do
    assert_match "iperf3-rs #{version}", shell_output("#{bin}/iperf3-rs --version")
  end
end
FORMULA
