"""Capture a plugin result snapshot and run the opt-in hidden native layout benchmark.

Close the launcher first. This script opens no visible windows and changes no settings.
Reports and snapshots stay in the ignored artifacts directory.
"""
import argparse
import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import time


def main():
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plugin", type=Path, default=root / "target/release/process-killer.exe")
    args = parser.parse_args()
    output = root / "artifacts/text-panel-benchmark"
    output.mkdir(parents=True, exist_ok=True)
    child = subprocess.Popen([str(args.plugin.resolve())], stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, text=True, encoding="utf-8", cwd=root)
    replies = queue.Queue()

    def read():
        for line in child.stdout:
            replies.put(json.loads(line))

    threading.Thread(target=read, daemon=True).start()

    def send(method, params, request_id=None):
        request = {"jsonrpc": "2.0", "method": method, "params": params}
        if request_id is not None:
            request["id"] = request_id
        child.stdin.write(json.dumps(request) + "\n")
        child.stdin.flush()

    query_ms = []
    try:
        send("initialize", {"protocol_version": 2}, 0)
        assert replies.get(timeout=10)["result"]["protocol_version"] == 2
        for request_id in range(1, 6):
            start = time.perf_counter()
            send("query", {"query": "", "max_results": 100}, request_id)
            response = replies.get(timeout=10)
            query_ms.append((time.perf_counter() - start) * 1000)
            assert response["id"] == request_id, response
            snapshot = response["result"]
    finally:
        try:
            send("shutdown", {})
            child.wait(timeout=5)
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()
    snapshot_path = output / "input.json"
    snapshot_path.write_text(json.dumps(snapshot, ensure_ascii=False), encoding="utf-8")
    report_path = output / "report.json"
    env = os.environ.copy()
    env["CARGO_HOME"] = str(root / ".tools/cargo")
    env["RUSTUP_HOME"] = str(root / ".tools/rustup")
    env["MINILAUNCHER_BENCH_INPUT"] = str(snapshot_path)
    env["MINILAUNCHER_BENCH_OUTPUT"] = str(report_path)
    cargo = root / ".tools/cargo/bin/cargo.exe"
    subprocess.run([str(cargo), "test", "--offline", "--locked", "--release",
                    "--target-dir", str(root / "target"),
                    "--bin", "mini-launcher", "windows::text_panel::benchmark::hidden_native_list",
                    "--", "--ignored", "--exact", "--nocapture", "--test-threads=1"],
                   cwd=root, env=env, check=True)
    report = json.loads(report_path.read_text(encoding="utf-8"))
    report["plugin_query_round_trip_ms"] = query_ms
    report["measured_at"] = time.strftime("%Y-%m-%dT%H:%M:%S%z")
    report_path.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"Report: {report_path}")


if __name__ == "__main__":
    main()
