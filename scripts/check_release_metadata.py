"""Offline tag fixtures: python3 scripts/check_release_metadata.py."""

import os
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
helper = root / ".github/scripts/release_metadata.sh"
workflow = (root / ".github/workflows/ghcr.yml").read_text()
assert workflow.count("ref: ${{ github.workflow_sha }}") == 2
assert workflow.count('run: bash "${RUNNER_TEMP}/release_metadata.sh"') == 2
assert workflow.count("ref: refs/tags/${{") == 2
(root / "target").mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix="release-tags-", dir=root / "target") as temporary:
    fixture = Path(temporary)
    env = dict(os.environ, GITHUB_REPOSITORY="Example/Fixture", GIT_AUTHOR_NAME="Fixture",
               GIT_AUTHOR_EMAIL="fixture@example.invalid", GIT_COMMITTER_NAME="Fixture",
               GIT_COMMITTER_EMAIL="fixture@example.invalid")

    def git(*args):
        return subprocess.check_output(["git", *args], cwd=fixture, env=env, text=True).strip()

    def commit(version):
        (fixture / "Cargo.toml").write_text(f'[package]\nname="fixture"\nversion="{version}"\n')
        git("add", "Cargo.toml")
        git("commit", "-qm", version)
        return git("rev-parse", "HEAD")

    def metadata(tag, flag="false", success=True, latest=False):
        output = fixture / "output"
        output.write_text("")
        result = subprocess.run(["bash", str(helper)], cwd=fixture,
                                env=dict(env, RELEASE_TAG=tag, RELEASE_PRERELEASE=flag,
                                         GITHUB_OUTPUT=str(output)), capture_output=True, text=True, timeout=30)
        assert (result.returncode == 0) == success, result.stderr
        text = output.read_text()
        if success:
            assert f"ghcr.io/example/fixture:{tag}\n" in text
            assert ("ghcr.io/example/fixture:latest\n" in text) == latest
            assert f"git_commit={git('rev-parse', 'HEAD')}\n" in text
        else:
            assert not text, "failed validation must emit no publication outputs"

    git("init", "-q")
    stable = commit("1.2.3")
    git("tag", "v1.2.3")
    git("tag", "1.2.3")
    metadata("v1.2.3", latest=True)
    metadata("v1.2.3", latest=True)  # idempotent retry
    metadata("1.2.3", "true")  # manual stable without latest
    for invalid in ("main", stable, "v1.2.4", "v01.2.3", "v1.2.3+build", "v1.2.3-01", "v1.2.3-"):
        metadata(invalid, success=False)
    metadata("v1.2.3", "invalid", success=False)
    (fixture / "Cargo.toml").write_text('[package]\nname="fixture"\nversion="9.9.9"\n')
    metadata("v1.2.3", success=False)
    git("restore", "Cargo.toml")
    commit("1.2.4-rc.1")
    git("tag", "v1.2.4-rc.1")
    metadata("v1.2.3", success=False)  # wrong HEAD
    metadata("v1.2.4-rc.1", success=False)
    metadata("v1.2.4-rc.1", "true")
    git("checkout", "-q", "--detach", stable)
    metadata("v1.2.3", latest=True)  # existing rollback path
print("PASS stable/prerelease/retry/rollback; unsupported ref, HEAD/version/flag mismatches reject")
