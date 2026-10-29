#!/usr/bin/env python3
"""Exercise the prepared PostgreSQL extensions and NATS with synthetic data."""
import argparse
import json
import os
import signal
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


def stop(process, shutdown_signal=signal.SIGTERM):
    if process is None:
        return
    if process.poll() is None:
        process.send_signal(shutdown_signal)
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def postgres_ready(process, sock):
    deadline = time.monotonic() + 60
    while True:
        if process.poll() is not None:
            raise RuntimeError("PostgreSQL exited before readiness")
        ready = subprocess.run(
            ["pg_isready", "-h", str(sock), "-d", "postgres"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.STDOUT,
            timeout=2,
        )
        if ready.returncode == 0:
            return
        if time.monotonic() >= deadline:
            raise RuntimeError("PostgreSQL did not become ready")
        time.sleep(0.1)


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
        postgres = None
        nats = None
        with (root / "pg.log").open("w") as pg_log, (root / "nats.log").open("w") as nats_log:
            try:
                postgres = subprocess.Popen(
                    ["postgres", "-D", str(data), "-k", str(sock),
                     "-c", "listen_addresses=", "-c", "shared_preload_libraries=timescaledb",
                     "-c", "timescaledb.telemetry_level=off"],
                    stdout=pg_log, stderr=subprocess.STDOUT,
                )
                postgres_ready(postgres, sock)
                answer = run(["psql", "-X", "-h", sock, "-d", "postgres", "-v", "ON_ERROR_STOP=1", "-Atc", "CREATE EXTENSION timescaledb; CREATE EXTENSION pg_trgm; CREATE TABLE synthetic(ts timestamptz NOT NULL, value int); SELECT create_hypertable('synthetic','ts'); INSERT INTO synthetic VALUES ('2020-01-01T00:00:00Z',7); SELECT sum(value) FROM synthetic;"])
                if answer.splitlines()[-1] != "7":
                    raise RuntimeError("synthetic TimescaleDB query returned " + answer)
                vector_connection = ["psql", "-X", "-h", sock, "-d", "postgres", "-v", "ON_ERROR_STOP=1", "-Atc"]
                vector_answer = run(vector_connection + ["CREATE EXTENSION vector; CREATE TABLE synthetic_vectors(id int PRIMARY KEY, embedding vector(3)); INSERT INTO synthetic_vectors VALUES (1,'[1,0,0]'),(2,'[0,1,0]'),(3,'[-1,0,0]'); CREATE INDEX synthetic_vectors_hnsw ON synthetic_vectors USING hnsw(embedding vector_cosine_ops); SET enable_seqscan=off; SELECT vector_dims('[1,0,0]'::vector); SELECT abs(1-('[1,0,0]'::vector <=> '[0,1,0]'::vector))<1e-12; SELECT string_agg(id::text,',' ORDER BY distance,id) FROM (SELECT id,embedding <=> '[1,0,0]'::vector AS distance FROM synthetic_vectors ORDER BY embedding <=> '[1,0,0]'::vector LIMIT 3) ranked;"])
                if vector_answer.splitlines()[-3:] != ["3", "t", "1,2,3"]:
                    raise RuntimeError("synthetic pgvector distance/order query returned " + vector_answer)
                plan = run(vector_connection + ["SET enable_seqscan=off; EXPLAIN (COSTS OFF) SELECT id FROM synthetic_vectors ORDER BY embedding <=> '[1,0,0]'::vector LIMIT 3;"])
                if "Index Scan using synthetic_vectors_hnsw" not in plan:
                    raise RuntimeError("synthetic pgvector HNSW index was not selected: " + plan)
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
                print(json.dumps({"postgres": run(["postgres", "--version"]), "timescaledb": "synthetic hypertable insert/query passed", "pgvector": "synthetic dimensions, cosine distance and HNSW ordering passed", "nats": "JetStream healthz passed"}))
            except BaseException:
                for log in (root / "pg.log", root / "nats.log"):
                    if log.exists():
                        print(log.name + ":\n" + log.read_text())
                raise
            finally:
                try:
                    stop(nats)
                finally:
                    if postgres is not None:
                        try:
                            if postgres.poll() is None:
                                run(["pg_ctl", "-D", data, "-m", "immediate", "-w", "stop"])
                        finally:
                            # Direct ownership lets this process reap the postmaster,
                            # including startup failures before pg_ctl can stop it.
                            stop(postgres, signal.SIGQUIT)


if __name__ == "__main__":
    main()
