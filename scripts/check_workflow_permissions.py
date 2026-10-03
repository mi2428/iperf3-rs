"""Static permissions plus mocked plan/host execution; requires existing Ruby/jq."""

import copy
import json
import os
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]


def load(name):
    return json.loads(subprocess.check_output(["ruby", "-ryaml", "-rjson", "-e",
                                              "puts JSON.generate(YAML.load_file(ARGV[0]))",
                                              str(root / ".github/workflows" / name)], text=True))


def check(release, ghcr):
    assert release["permissions"] == {"contents": "read"}
    writers = {name for name, job in release["jobs"].items()
               if job.get("permissions", release["permissions"]).get("contents") == "write"}
    assert writers == {"host", "announce"}, writers
    for name in ("build-local-artifacts", "build-global-artifacts"):
        job = release["jobs"][name]
        assert "GH_TOKEN" not in job.get("env", {})
        assert all("GH_TOKEN" not in step.get("env", {}) for step in job["steps"])
    homebrew = release["jobs"]["publish-homebrew-formula"]
    assert any(step.get("with", {}).get("token") == "${{ secrets.HOMEBREW_TAP_TOKEN }}" for step in homebrew["steps"])
    assert "--clobber" in str(release["jobs"]["host"]) and 'if ! git diff --cached --quiet' in str(homebrew)
    assert ghcr["permissions"] == {"contents": "read"}
    for name in ("build", "publish"):
        assert ghcr["jobs"][name]["permissions"] == {"contents": "read", "packages": "write"}


release, ghcr = load("release.yml"), load("ghcr.yml")
check(release, ghcr)
bad = copy.deepcopy(release)
bad["jobs"]["build-local-artifacts"]["permissions"] = {"contents": "write"}
try:
    check(bad, ghcr)
except AssertionError:
    pass
else:
    raise AssertionError("write-enabled native build was not rejected")

plan = next(step["run"] for step in release["jobs"]["plan"]["steps"] if step.get("id") == "plan")
host = next(step["run"] for step in release["jobs"]["host"]["steps"] if step.get("id") == "host")
(root / "target").mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix="permissions-", dir=root / "target") as temporary:
    fixture = Path(temporary)
    mock = fixture / "dist"
    mock.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$DIST_LOG"\nprintf \'{"fixture":true}\\n\'\n')
    mock.chmod(0o755)
    log = fixture / "calls"
    env = dict(os.environ, PATH=f"{fixture}:{os.environ['PATH']}", DIST_LOG=str(log),
               GITHUB_OUTPUT=str(fixture / "output"), RELEASE_TAG="v1.2.3")
    for event in ("pull_request", "push", "workflow_dispatch"):
        log.write_text("")
        subprocess.run(["bash", "-euo", "pipefail", "-c", plan], cwd=fixture,
                       env=dict(env, GITHUB_EVENT_NAME=event), capture_output=True, check=True)
        calls = log.read_text()
        assert calls.startswith("plan ") and "--steps=create" not in calls, calls
    log.write_text("")
    host = host.replace("${{ needs.plan.outputs.tag-flag }}", "--tag=v1.2.3")
    subprocess.run(["bash", "-euo", "pipefail", "-c", host], cwd=fixture, env=env,
                   capture_output=True, check=True)
    assert all(step in log.read_text() for step in ("host ", "--steps=create", "--steps=upload", "--steps=release"))
print("PASS least-privilege jobs; read-only PR/tag/retry plans and host lifecycle; tap/artifact retry paths")
