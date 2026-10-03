"""Mock-only Make regression: python3 scripts/check_dist_smoke.py."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
(root / "target").mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix="dist-smoke-", dir=root / "target") as temporary:
    fixture = Path(temporary)
    shutil.copy2(root / "Makefile", fixture / "Makefile")
    (fixture / "scripts").mkdir()
    (fixture / "iperf3").mkdir()
    shutil.copy2(root / "scripts/prepare_notices.sh", fixture / "scripts/prepare_notices.sh")
    for name in ("LICENSE", "LICENSE-SORACOM", "iperf3/LICENSE"):
        shutil.copy2(root / name, fixture / name)
    docker = fixture / "docker-mock"
    docker.write_text('''#!/bin/bash
printf '%s\n' "$*" >> "$DOCKER_LOG"
if [[ "$1" == info && "${FAIL_INFO:-0}" == 1 ]]; then exit 1; fi
if [[ "$1" == run && "${FAIL_RUN:-0}" == 1 ]]; then exit 1; fi
''')
    docker.chmod(0o755)
    log = fixture / "docker.log"
    env = dict(os.environ, DOCKER_LOG=str(log))
    args = ["make", "--no-print-directory", f"DOCKER={docker}", "RUSTUP=true", "CARGO=true",
            "RUSTC=true", "RUSTDOC=true"]

    def run(target, success=True, extra=(), failure=None):
        log.write_text("")
        output = subprocess.run([*args, target, *extra], cwd=fixture,
                                env=dict(env, **(failure or {})), text=True,
                                capture_output=True, timeout=30)
        assert (output.returncode == 0) == success, output.stdout + output.stderr
        return log.read_text().splitlines()

    assert run("dist-smoke") == []
    (fixture / "dist").mkdir()
    (fixture / "dist/iperf3-rs-darwin-arm64").write_text("mock Darwin binary\n")
    assert run("dist-smoke") == []
    release = fixture / "target/aarch64-apple-darwin/release"
    release.mkdir(parents=True)
    (release / "iperf3-rs").write_text("mock Darwin binary\n")
    assert run("dist", extra=("OS=darwin", "ARCH=arm64", "HOST_OS=Darwin")) == []
    assert "iperf3-rs-darwin-arm64" in (fixture / "dist/checksums.txt").read_text()
    for arch in ("amd64", "arm64"):
        (fixture / f"dist/iperf3-rs-linux-{arch}").write_text("mock Linux binary\n")
    calls = run("dist-smoke")
    assert calls[0] == "info" and len(calls) == 5, calls
    for arch in ("amd64", "arm64"):
        selected = [call for call in calls if f"iperf3-rs-linux-{arch}" in call]
        assert len(selected) == 2 and any(call.endswith(" -h") for call in selected)
        assert any(call.endswith(" --version") for call in selected)
    assert run("dist-smoke", success=False, failure={"FAIL_INFO": "1"}) == ["info"]
    assert len(run("dist-smoke", success=False, failure={"FAIL_RUN": "1"})) == 2
print("PASS Darwin-only/no-artifact skip; Linux help/version; daemon/startup failures")
