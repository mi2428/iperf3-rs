"""Notice regeneration/archive fixture: python3 scripts/check_notices.py."""

from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

root = Path(__file__).resolve().parents[1]
assert "LICENSE-IPERF3" in tomllib.loads((root / "dist-workspace.toml").read_text())["dist"]["include"]
cargo = tomllib.loads((root / "Cargo.toml").read_text())["package"]["include"]
assert "/iperf3/LICENSE" in cargo and "!/iperf3/src/private.pem" in cargo
assert "/tests/e2e_test.rs" not in cargo and "/tests/e2e/**" not in cargo
docker = (root / "Dockerfile").read_text()
assert "COPY LICENSE LICENSE-SORACOM /licenses/" in docker
assert "COPY iperf3/LICENSE /licenses/LICENSE-IPERF3" in docker
workflow = (root / ".github/workflows/release.yml").read_text()
assert workflow.count("run: cp iperf3/LICENSE LICENSE-IPERF3") == 4
assert "release.Dockerfile" in (root / ".github/workflows/ghcr.yml").read_text()
(root / "target").mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix="notices-", dir=root / "target") as temporary:
    fixture = Path(temporary)
    (fixture / "scripts").mkdir()
    (fixture / "iperf3").mkdir()
    shutil.copy2(root / "scripts/prepare_notices.sh", fixture / "scripts/prepare_notices.sh")
    for name in ("LICENSE", "LICENSE-SORACOM", "iperf3/LICENSE"):
        shutil.copy2(root / name, fixture / name)
    destination = fixture / "dist"
    subprocess.run(["sh", "scripts/prepare_notices.sh", str(destination)], cwd=fixture, check=True)
    for source, name in (("LICENSE", "LICENSE"), ("LICENSE-SORACOM", "LICENSE-SORACOM"), ("iperf3/LICENSE", "LICENSE-IPERF3")):
        assert (destination / name).read_bytes() == (root / source).read_bytes()
    archive = fixture / "notices.tar.xz"
    with tarfile.open(archive, "w:xz") as output:
        for path in destination.iterdir():
            output.add(path, arcname=path.name)
    with tarfile.open(archive) as bundled:
        assert len(bundled.getnames()) == 3
        assert bundled.extractfile("LICENSE-IPERF3").read() == (root / "iperf3/LICENSE").read_bytes()
    (fixture / "iperf3/LICENSE").write_text("fixture new upstream notice\n")
    subprocess.run(["sh", "scripts/prepare_notices.sh", str(destination)], cwd=fixture, check=True)
    assert (destination / "LICENSE-IPERF3").read_text() == "fixture new upstream notice\n"
    assert (destination / "LICENSE").read_bytes() == (root / "LICENSE").read_bytes()
print("PASS distinct complete notices, regenerated upstream update, archive/source/image policy")
