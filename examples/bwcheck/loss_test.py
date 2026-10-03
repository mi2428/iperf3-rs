"""Bounded loopback loss regression; pass iperf3-rs and bwcheck binary paths.

Uses no Docker, external network, privileged packet filters, or third-party modules.
"""
import json
import select
import socket
import subprocess
import sys
import threading
import time


def probe(server_binary, checker_binary, drop_every_other, extra_endpoint=None):
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        backend_port = reservation.getsockname()[1]
    server = subprocess.Popen(
        [server_binary, "-s", "-1", "-p", str(backend_port), "-J"],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    stop = threading.Event()
    counts = {"data": 0, "dropped": 0}
    errors = []
    with socket.socket() as tcp, socket.socket(type=socket.SOCK_DGRAM) as udp, socket.socket(type=socket.SOCK_DGRAM) as upstream_udp:
        tcp.bind(("127.0.0.1", 0))
        proxy_port = tcp.getsockname()[1]
        tcp.listen()
        tcp.settimeout(0.1)
        udp.bind(("127.0.0.1", proxy_port))
        upstream_udp.bind(("127.0.0.1", 0))
        for stream in [udp, upstream_udp]:
            stream.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 262144)
            stream.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 262144)

        def relay_tcp():
            try:
                while not stop.is_set():
                    try:
                        client, _ = tcp.accept()
                        break
                    except TimeoutError:
                        continue
                else:
                    return
                with client, socket.create_connection(("127.0.0.1", backend_port), timeout=2) as upstream:
                    client.settimeout(2)
                    while not stop.is_set():
                        readable, _, _ = select.select([client, upstream], [], [], 0.1)
                        for source in readable:
                            data = source.recv(65536)
                            if not data:
                                return
                            (upstream if source is client else client).sendall(data)
            except Exception as exc:
                errors.append(f"{type(exc).__name__}: {exc}")

        def relay_udp():
            try:
                client_address = None
                while not stop.is_set():
                    readable, _, _ = select.select([udp, upstream_udp], [], [], 0.1)
                    for source in readable:
                        data, address = source.recvfrom(65536)
                        if source is udp:
                            client_address = address
                            if len(data) > 16:
                                counts["data"] += 1
                                if drop_every_other and counts["data"] % 2 == 0:
                                    counts["dropped"] += 1
                                    continue
                            upstream_udp.sendto(data, ("127.0.0.1", backend_port))
                        elif client_address is not None:
                            udp.sendto(data, client_address)
            except Exception as exc:
                errors.append(f"{type(exc).__name__}: {exc}")

        threads = [threading.Thread(target=relay_tcp), threading.Thread(target=relay_udp)]
        try:
            time.sleep(0.2)
            assert server.poll() is None, "local server did not start"
            for thread in threads:
                thread.start()
            endpoints = [f"127.0.0.1:{proxy_port}"]
            if extra_endpoint is not None:
                endpoints.append(extra_endpoint)
            checker = subprocess.run(
                [checker_binary, "--min-bandwidth-bps", "100000", "--max-loss-percent", "1",
                 *endpoints],
                capture_output=True, text=True, timeout=15,
            )
            server_stdout, _ = server.communicate(timeout=5)
            receiver = json.loads(server_stdout)["end"]["sum_received"]
            reported_loss = float(checker.stdout.split("loss_percent=")[1].split()[0])
            assert receiver["packets"] > 0, "proxy did not deliver traffic"
            expected_exit = 1 if drop_every_other else 0
            assert checker.returncode == expected_exit, (checker.stdout, checker.stderr)
            assert checker.stdout.startswith("FAIL " if drop_every_other else "PASS ")
            if extra_endpoint is not None:
                lines = checker.stdout.splitlines()
                assert len(lines) == 3, checker.stdout
                assert lines[1].startswith(f"PASS endpoint={extra_endpoint} "), checker.stdout
                assert lines[2] == f"summary checked=2 failed={expected_exit}", checker.stdout
            assert abs(reported_loss - receiver["lost_percent"]) < 0.001
            assert receiver["lost_percent"] > 40 if drop_every_other else receiver["lost_percent"] == 0
            assert counts["dropped"] > 0 if drop_every_other else counts["dropped"] == 0
            print(json.dumps({"loss_enabled": drop_every_other, "checked": len(endpoints),
                              "exit": checker.returncode,
                              "receiver_loss_percent": receiver["lost_percent"],
                              "checker_loss_percent": reported_loss}))
        finally:
            stop.set()
            if server.poll() is None:
                server.kill()
            server.wait(timeout=5)
            for thread in threads:
                if thread.ident is not None:
                    thread.join(timeout=3)
            assert not errors, errors
            assert all(not thread.is_alive() for thread in threads)


if __name__ == "__main__":
    server_binary, checker_binary = sys.argv[1:]
    probe(server_binary, checker_binary, False)
    probe(server_binary, checker_binary, True)
