"""Offline pin check; --verify-upstream resolves the official refs read-only."""

import json
from pathlib import Path
import re
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
files = [*sorted((root / ".github/workflows").glob("*.y*ml")), root / ".github/dist-build-setup.yml"]
pins = {}
count = 0
for path in files:
    for line in path.read_text().splitlines():
        match = re.search(r"\buses:\s*([^@\s]+)@([^\s]+)\s+#\s+(v[^\s]+)\s*$", line)
        if "uses:" not in line:
            continue
        assert match, f"missing immutable pin/version comment: {path.name}: {line}"
        action, digest, version = match.groups()
        assert re.fullmatch(r"[0-9a-f]{40}", digest), f"mutable Action ref: {line}"
        key = (action, version)
        assert key not in pins or pins[key] == digest, f"conflicting pins: {key}"
        pins[key] = digest
        count += 1
assert count and json.loads((root / ".github/renovate.json").read_text())["extends"].count("helpers:pinGitHubActionDigests") == 1
if "--verify-upstream" in sys.argv:
    for (action, version), digest in sorted(pins.items()):
        # Kani's existing v1 reference is an official branch, not a tag.
        kind = "heads" if action == "model-checking/kani-github-action" and version == "v1" else "tags"
        value = json.loads(subprocess.check_output(["gh", "api", f"repos/{action}/git/ref/{kind}/{version}"]))["object"]
        while value["type"] == "tag":
            value = json.loads(subprocess.check_output(["gh", "api", f"repos/{action}/git/tags/{value['sha']}"]))["object"]
        assert value["type"] == "commit" and value["sha"] == digest, f"official {action}@{version} moved; review before updating"
        print(f"VERIFIED {action}@{version} -> {digest}")
print(f"PASS {count} immutable Action references, {len(pins)} consistent official versions")
