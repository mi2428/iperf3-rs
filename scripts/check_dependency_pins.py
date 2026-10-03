"""Coordinated pin guard: python3 scripts/check_dependency_pins.py."""

from pathlib import Path
import json
import re
import shutil
import tempfile
import tomllib


def check(root):
    channel = tomllib.loads((root / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    minimum = tomllib.loads((root / "Cargo.toml").read_text())["package"]["rust-version"]
    assert tuple(map(int, minimum.split("."))) <= tuple(map(int, channel.split("."))), "toolchain is below MSRV"
    short = ".".join(channel.split(".")[:2])
    make = (root / "Makefile").read_text()
    assert re.search(r"^RUSTUP_TOOLCHAIN\s*\?=\s*(\S+)", make, re.M).group(1) == channel
    assert f"rust:{short}-bullseye" in make
    docker = (root / "Dockerfile").read_text()
    images = re.findall(r"^ARG (?:RELEASE_)?BUILD_IMAGE_TAG=([0-9.]+)-", docker, re.M)
    assert images and all(image == short for image in images), "Docker Rust pin drift"
    checks = (root / ".github/workflows/checks.yml").read_text()
    installs = re.findall(r"rustup toolchain install ([0-9.]+)", checks)
    assert installs and all(version == channel for version in installs), "CI toolchain pin drift"
    assert f"rust:{short}-bullseye" in checks and f"rust-{short}-bullseye" in checks
    release = (root / ".github/workflows/release.yml").read_text()
    assert f"rust:{short}-bullseye" in release, "release Rust/ABI pin drift"
    dist = tomllib.loads((root / "dist-workspace.toml").read_text())["dist"]["cargo-dist-version"]
    installers = re.findall(r"cargo-dist/releases/download/v([^/]+)/", release)
    assert installers and all(version == dist for version in installers), "cargo-dist installer pin drift"


if __name__ == "__main__":
    root = Path(__file__).resolve().parents[1]
    check(root)
    renovate = json.loads((root / ".github/renovate.json").read_text())
    assert renovate["lockFileMaintenance"]["enabled"] is True
    assert renovate["lockFileMaintenance"]["schedule"] == renovate["schedule"]
    assert renovate["git-submodules"]["enabled"] is False and renovate["automerge"] is False
    patterns = renovate["github-actions"]["managerFilePatterns"]
    assert any(re.search(pattern.strip("/"), ".github/dist-build-setup.yml") for pattern in patterns)
    (root / "target").mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="pin-check-", dir=root / "target") as temporary:
        fixture = Path(temporary)
        for name in ("Cargo.toml", "rust-toolchain.toml", "Makefile", "Dockerfile", "dist-workspace.toml",
                     ".github/workflows/checks.yml", ".github/workflows/release.yml"):
            (fixture / name).parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(root / name, fixture / name)
        docker = fixture / "Dockerfile"
        docker.write_text(docker.read_text().replace("BUILD_IMAGE_TAG=", "BUILD_IMAGE_TAG=0.0-"))
        try:
            check(fixture)
        except AssertionError:
            pass
        else:
            raise AssertionError("image pin drift was not rejected")
    print("PASS coordinated Rust/MSRV/ABI/dist pins; isolated image drift is rejected")
