#!/usr/bin/env python3
"""Exercise real native binaries, persistent storage, and the live browser UI."""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent


def run(args, cwd=ROOT, env=None):
    return subprocess.run(args, cwd=cwd, env=env, check=True, text=True, capture_output=True)


def main():
    # Backend smoke also proves collector/API restart persistence and blob writes.
    print(run(["bash", "scripts/smoke-test.sh"]).stdout, end="")
    target = Path(json.loads(run(["cargo", "metadata", "--format-version", "1", "--no-deps"]).stdout)["target_directory"])
    binary = target / "debug/replaykit"
    with tempfile.TemporaryDirectory(prefix="replaykit-native-gate-") as directory:
        cli = [str(binary), "--storage", "sqlite", "--data-root", directory, "--json"]
        branch = json.loads(run(cli + ["demo-branch"]).stdout)
        diff = json.loads(run(cli + ["runs", "diff", branch["source_run_id"], branch["target_run_id"]]).stdout)
        assert diff["changed_span_count"] == 1
        assert diff["changed_artifact_count"] == 1
        assert diff["first_divergent_span_id"] == "tool-search"
        # This fixture intentionally retains a failed recorded LLM span. Do not
        # turn that into invented successful model execution for a demo.
        assert branch["target_status_label"] == "failed"
        print("PASS: persisted CLI fork and cached diff preserve recorded failure")

    web = ROOT / "apps/web"
    for command in (["npm", "run", "lint"], ["npm", "test"], ["npm", "run", "build"]):
        print(run(command, web).stdout, end="")

    # The API deliberately admits only the documented local frontend origins.
    # Keep that policy and fail clearly if another app owns the required port.
    web_port = 5173
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", web_port))
    with socket.socket() as collector_socket, socket.socket() as api_socket:
        collector_socket.bind(("127.0.0.1", 0))
        api_socket.bind(("127.0.0.1", 0))
        collector_port = collector_socket.getsockname()[1]
        api_port = api_socket.getsockname()[1]

    processes = []
    logs = []
    with tempfile.TemporaryDirectory(prefix="replaykit-browser-gate-") as directory:
        def start(args, env, cwd=ROOT):
            log = tempfile.TemporaryFile()
            logs.append(log)
            process = subprocess.Popen(args, cwd=cwd, env=env, stdout=log, stderr=subprocess.STDOUT)
            processes.append(process)
            return process

        try:
            environment = os.environ.copy()
            start([str(target / "debug/replaykit-collector")], dict(environment, REPLAYKIT_DATA_ROOT=directory, REPLAYKIT_PORT=str(collector_port)))
            start([str(binary), "--storage", "sqlite", "--data-root", directory, "serve", "--port", str(api_port)], environment)
            start(["npm", "run", "dev", "--", "--host", "127.0.0.1", "--port", str(web_port), "--strictPort"], dict(environment, VITE_REPLAYKIT_API_BASE_URL=f"http://127.0.0.1:{api_port}"), web)
            for port in (collector_port, api_port, web_port):
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    if any(process.poll() is not None for process in processes):
                        raise RuntimeError("native stack process exited before readiness")
                    try:
                        with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                            break
                    except OSError:
                        time.sleep(0.1)
                else:
                    raise RuntimeError(f"port {port} was not ready")
            run(["bash", "scripts/seed-stack-run.sh"], env=dict(environment, REPLAYKIT_COLLECTOR_URL=f"http://127.0.0.1:{collector_port}"))
            print(run(["npm", "run", "test:e2e"], web, dict(environment, PLAYWRIGHT_BASE_URL=f"http://127.0.0.1:{web_port}")).stdout, end="")
            print("PASS: collector -> SQLite -> API -> real browser run/tree/artifact")
        finally:
            for process in reversed(processes):
                process.terminate()
            for process in processes:
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            for log in logs:
                log.close()


if __name__ == "__main__":
    main()
