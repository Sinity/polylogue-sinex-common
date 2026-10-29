#!/usr/bin/env python3
"""Exercise the prepared PostgreSQL/TimescaleDB and NATS with synthetic data."""
import argparse
import json
import os
import shlex
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.request


def run(args):
    result = subprocess.run(list(map(str, args)), capture_output=True, text=True)
    if result.returncode:
        print(result.stdout, end="")
        print(result.stderr, end="")
        result.check_returncode()
    return result.stdout.strip()


def port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scratch", type=Path, required=True)
    args = parser.parse_args()
    if os.getuid() == 0:
        parser.error("run the cloud service smoke check as an ordinary user")
    with tempfile.TemporaryDirectory(prefix="smoke-", dir=args.scratch) as temporary:
        root = Path(temporary)
        data = root / "pg"
        sock = root / "sock"
        sock.mkdir()
        run(["initdb", "-D", data, "--no-locale", "--auth=trust", "--data-checksums"])
        started = False
        nats = None
        with (root / "nats.log").open("w") as nats_log:
            try:
                run(["pg_ctl", "-D", data, "-l", root / "pg.log", "-o", f"-k {shlex.quote(str(sock))} -c listen_addresses='' -c shared_preload_libraries=timescaledb -c timescaledb.telemetry_level=off", "-w", "start"])
                started = True
                answer = run(["psql", "-X", "-h", sock, "-d", "postgres", "-v", "ON_ERROR_STOP=1", "-Atc", "CREATE EXTENSION timescaledb; CREATE EXTENSION pg_trgm; CREATE TABLE synthetic(ts timestamptz NOT NULL, value int); SELECT create_hypertable('synthetic','ts'); INSERT INTO synthetic VALUES ('2020-01-01T00:00:00Z',7); SELECT sum(value) FROM synthetic;"])
                if answer.splitlines()[-1] != "7":
                    raise RuntimeError("synthetic TimescaleDB query returned " + answer)
                monitoring = port()
                nats = subprocess.Popen(["nats-server", "-js", "-sd", str(root / "nats"), "-a", "127.0.0.1", "-p", str(port()), "-m", str(monitoring)], stdout=nats_log, stderr=subprocess.STDOUT)
                deadline = time.monotonic() + 10
                opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
                while True:
                    if nats.poll() is not None:
                        raise RuntimeError("NATS exited before readiness")
                    try:
                        with opener.open(f"http://127.0.0.1:{monitoring}/healthz", timeout=1) as response:
                            health = json.load(response)
                        if health.get("status") != "ok":
                            raise RuntimeError("NATS health response: " + repr(health))
                        break
                    except OSError:
                        if time.monotonic() >= deadline:
                            raise
                        time.sleep(0.1)
                print(json.dumps({"postgres": run(["postgres", "--version"]), "timescaledb": "synthetic hypertable insert/query passed", "nats": "JetStream healthz passed"}))
            except BaseException:
                for log in (root / "pg.log", root / "nats.log"):
                    if log.exists():
                        print(log.name + ":\n" + log.read_text())
                raise
            finally:
                if nats is not None:
                    nats.terminate()
                    try:
                        nats.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        nats.kill()
                        nats.wait()
                if started or (data / "postmaster.pid").exists():
                    run(["pg_ctl", "-D", data, "-m", "immediate", "-w", "stop"])


if __name__ == "__main__":
    main()
