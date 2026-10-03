"""Bounded endpoint-reporting regression; pass iperf3-rs and bwcheck paths."""
import collections
import select
import socket
import subprocess
import sys
import threading

sys.dont_write_bytecode = True
from loss_test import probe


def check_results(checker_binary, endpoints, statuses):
    result = subprocess.run(
        [checker_binary, "--min-bandwidth-bps", "100000", "--max-loss-percent", "1", *endpoints],
        capture_output=True, text=True, timeout=20,
    )
    lines = result.stdout.splitlines()
    failed = statuses.count("FAIL")
    assert result.returncode == (1 if failed else 0), (result.stdout, result.stderr)
    assert len(lines) == len(endpoints) + 1, result.stdout
    for line, endpoint, status in zip(lines, endpoints, statuses):
        assert line.startswith(f"{status} endpoint={endpoint} "), result.stdout
        if status == "FAIL":
            assert "error=" in line and "bandwidth_bps=" not in line, line
        else:
            assert "bandwidth_bps=" in line and "error=" not in line, line
    assert lines[-1] == f"summary checked={len(endpoints)} failed={failed}", result.stdout
    print(f"{' -> '.join(statuses)}: checked={len(endpoints)} failed={failed} exit={result.returncode}")


def invalid_args_start_no_traffic(checker_binary):
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        endpoint = f"127.0.0.1:{listener.getsockname()[1]}"
        for args in [
            [endpoint, "--unknown-option"],
            [endpoint, "--min-bandwidth-bps", "NaN"],
            [endpoint, "--max-loss-percent", "-1"],
            [endpoint, "--max-loss-percent"],
            [endpoint, "missing-port"],
            [endpoint, "127.0.0.1:0"],
        ]:
            result = subprocess.run([checker_binary, *args], capture_output=True, text=True, timeout=5)
            assert result.returncode == 1 and not result.stdout, (args, result.stdout, result.stderr)
            assert "usage:" in result.stderr, result.stderr
            assert not select.select([listener], [], [], 0.1)[0], "invalid global input started traffic"
    print("invalid global arguments: 6 cases rejected before any control connection")


def main(server_binary, checker_binary):
    # A bound but non-listening TCP socket guarantees refusal and reserves the
    # negative endpoint's port throughout the test, without touching other stacks.
    with socket.socket() as refused, socket.socket() as reservation:
        refused.bind(("127.0.0.1", 0))
        unreachable = f"127.0.0.1:{refused.getsockname()[1]}"
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
        reservation.close()
        healthy = f"127.0.0.1:{port}"
        server = subprocess.Popen(
            [server_binary, "-s", "-B", "127.0.0.1", "-p", str(port), "--forceflush"],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        )
        ready = threading.Event()
        tail = collections.deque(maxlen=8)

        def drain():
            for line in server.stdout:
                tail.append(line.decode(errors="replace").rstrip())
                if b"Server listening on" in line:
                    ready.set()

        reader = threading.Thread(target=drain, daemon=True)
        try:
            reader.start()
            assert ready.wait(5) and server.poll() is None, list(tail)
            check_results(checker_binary, [unreachable, healthy], ["FAIL", "PASS"])
            check_results(checker_binary, [healthy, unreachable, healthy], ["PASS", "FAIL", "PASS"])
            check_results(checker_binary, [healthy, healthy], ["PASS", "PASS"])
            # Reuse the loss relay: a threshold failure, unlike an operational
            # failure, still has real metrics. The subsequent endpoint must pass.
            probe(server_binary, checker_binary, True, extra_endpoint=healthy)
            invalid_args_start_no_traffic(checker_binary)
        finally:
            if server.poll() is None:
                server.kill()
            server.wait(timeout=5)
            if reader.ident is not None:
                reader.join(timeout=5)
            assert not reader.is_alive(), "server output reader did not stop"
            server.stdout.close()


if __name__ == "__main__":
    main(*sys.argv[1:])
