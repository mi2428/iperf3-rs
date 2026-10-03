"""Daemon-free regression: python3 scripts/check_compose_config.py."""

import json
import os
from pathlib import Path
import shutil
import subprocess

root = Path(__file__).resolve().parents[1]
compose = (root / "docker-compose.yml").read_text()
assert compose.count("${OBSERVABILITY_BIND_IP:-127.0.0.1}") == 3
assert "${GRAFANA_ADMIN_PASSWORD:?" in compose and ":-changeme" not in compose
assert "prometheus:9090" in (root / "docker/grafana/provisioning/datasources/prometheus.yml").read_text()
if not shutil.which("docker"):
    print("SKIP resolved Compose config: Docker CLI unavailable; static assertions passed")
    raise SystemExit(0)

env = os.environ.copy()
for key in ("GRAFANA_ADMIN_PASSWORD", "OBSERVABILITY_BIND_IP", "PUSHGATEWAY_PORT", "PROMETHEUS_PORT", "GRAFANA_PORT"):
    env.pop(key, None)
args = ["docker", "compose", "-f", str(root / "docker-compose.yml"), "config", "--format", "json"]
missing = subprocess.run(args, cwd=root, env=env, text=True, capture_output=True, timeout=30)
assert missing.returncode != 0 and "Grafana admin password" in missing.stderr, missing.stderr
env["GRAFANA_ADMIN_PASSWORD"] = "fixture-only-not-a-real-secret"
for bind in ("127.0.0.1", "0.0.0.0"):
    if bind == "0.0.0.0":
        env.update(OBSERVABILITY_BIND_IP=bind, PUSHGATEWAY_PORT="19091", PROMETHEUS_PORT="19090", GRAFANA_PORT="13000")
    output = subprocess.check_output(args, cwd=root, env=env, text=True, timeout=30)
    services = json.loads(output)["services"]
    for name, published in (("pushgateway", "9091"), ("prometheus", "9090"), ("grafana", "3000")):
        ports = services[name]["ports"]
        assert len(ports) == 1 and ports[0]["host_ip"] == bind, ports
        assert str(ports[0]["published"]) == (published if bind == "127.0.0.1" else "1" + published), ports
    assert services["grafana"]["environment"]["GF_SECURITY_ADMIN_PASSWORD"] == env["GRAFANA_ADMIN_PASSWORD"]
print("PASS required credentials, loopback default, explicit remote/custom ports, internal datasource")
