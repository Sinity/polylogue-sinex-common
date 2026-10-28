#!/usr/bin/env python3
"""Install the pinned cloud toolchain into a caller-owned directory."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import subprocess
import sys
import tarfile
import urllib.request


def run(args, *, cwd=None, env=None):
    print("cloud setup: " + shlex.join(map(str, args)), flush=True)
    subprocess.run(list(map(str, args)), cwd=cwd, env=env, check=True)


def artifact(name, pin, cache):
    target = cache / name
    if target.exists():
        with target.open("rb") as stream:
            if hashlib.file_digest(stream, "sha256").hexdigest() == pin["sha256"]:
                return target
    partial = target.with_suffix(".part")
    print("cloud setup: downloading " + pin["url"], flush=True)
    with urllib.request.urlopen(pin["url"], timeout=120) as response, partial.open("wb") as output:
        shutil.copyfileobj(response, output)
    with partial.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    if digest != pin["sha256"]:
        partial.unlink()
        raise RuntimeError(f"SHA256 mismatch for {name}: {digest}")
    partial.replace(target)
    return target


def unpack(archive, directory):
    directory.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive) as source:
        source.extractall(directory, filter="data")
    return directory


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prefix", required=True, type=Path)
    parser.add_argument("--repo", required=True, type=Path)
    args = parser.parse_args()
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        parser.error("the pinned native toolchain currently targets x86_64 Linux")
    for tool in ("cc", "make", "pkg-config"):
        if not shutil.which(tool):
            parser.error(f"native setup requires {tool} from the cloud base image")
    pins_path = Path(__file__).with_name("pins.json")
    pins = json.loads(pins_path.read_text())
    toolchain = args.repo / "rust-toolchain.toml"
    if toolchain.exists():
        import tomllib
        if tomllib.loads(toolchain.read_text())["toolchain"]["channel"] != pins["rust"]:
            parser.error("repository Rust toolchain differs from the shared cloud pin")
    prefix = args.prefix.resolve()
    prefix.mkdir(parents=True, exist_ok=True)
    cache = prefix / "downloads"
    cache.mkdir(exist_ok=True)
    build = prefix / "build"
    build.mkdir(exist_ok=True)
    tools = prefix / "bin"
    tools.mkdir(exist_ok=True)
    cargo = prefix / "cargo"
    rustup = prefix / "rustup"
    pg = prefix / "postgres"
    cmake = prefix / "cmake" / "cmake-3.31.8-linux-x86_64" / "bin"
    env = os.environ | {"CARGO_HOME": str(cargo), "RUSTUP_HOME": str(rustup), "RUSTUP_INIT_SKIP_PATH_CHECK": "yes"}
    env["PATH"] = os.pathsep.join(map(str, (cargo / "bin", tools, pg / "bin", cmake))) + os.pathsep + os.environ["PATH"]
    stamp = prefix / "installed.json"
    identity = hashlib.sha256(pins_path.read_bytes()).hexdigest()
    installed = stamp.exists() and json.loads(stamp.read_text()).get("pins_sha256") == identity
    if not installed:
        installer = artifact("rustup-init", pins["rustup"], cache)
        installer.chmod(0o755)
        run([installer, "-y", "--no-modify-path", "--profile", "minimal", "--default-toolchain", pins["rust"], "--component", "rustfmt,clippy"], env=env)
        for name, binary in (("nextest", "cargo-nextest"), ("nats", "nats-server")):
            archive = artifact(name + ".tar.gz", pins[name], cache)
            extracted = unpack(archive, build / name)
            candidates = list(extracted.rglob(binary))
            if len(candidates) != 1:
                raise RuntimeError(f"expected one {binary} in {name} archive")
            shutil.copy2(candidates[0], tools / binary)
        unpack(artifact("cmake.tar.gz", pins["cmake"], cache), prefix / "cmake")
        pg_sources = unpack(artifact("postgres.tar.bz2", pins["postgres"], cache), build / "postgres") / ("postgresql-" + pins["postgres"]["version"])
        # The test clusters use Unix sockets. These optional client features
        # would otherwise require system development packages in a rootless VM.
        run([pg_sources / "configure", "--prefix=" + str(pg), "--without-readline", "--without-icu", "--without-zlib"], cwd=pg_sources, env=env)
        jobs = str(int(os.environ.get("CLOUD_BUILD_JOBS", "2")))
        run(["make", "-j" + jobs], cwd=pg_sources, env=env)
        run(["make", "install"], cwd=pg_sources, env=env)
        run(["make", "-C", "contrib/pg_trgm", "install"], cwd=pg_sources, env=env)
        ts_sources = unpack(artifact("timescale.tar.gz", pins["timescale"], cache), build / "timescale")
        ts_sources = next(path for path in ts_sources.iterdir() if path.is_dir())
        ts_build = build / "timescale-build"
        run(["cmake", "-S", ts_sources, "-B", ts_build, "-DPG_CONFIG=" + str(pg / "bin/pg_config"), "-DREGRESS_CHECKS=OFF", "-DWARNINGS_AS_ERRORS=OFF", "-DUSE_OPENSSL=OFF"], env=env)
        run(["cmake", "--build", ts_build, "--parallel", jobs], env=env)
        run(["cmake", "--install", ts_build], env=env)
    # Check the tools again when reusing a completed prefix; a deleted binary
    # must fail setup instead of being silently accepted by a saved stamp.
    for command in (["rustc", "--version"], ["cargo", "--version"], ["cargo", "nextest", "--version"], ["postgres", "--version"], ["nats-server", "--version"]):
        run(command, env=env)
    pkglib = Path(subprocess.check_output([str(pg / "bin/pg_config"), "--pkglibdir"], text=True, env=env).strip())
    if not (pkglib / "timescaledb.so").is_file():
        raise RuntimeError("TimescaleDB loader is missing from the prepared PostgreSQL")
    run([sys.executable, Path(__file__).with_name("smoke.py"), "--scratch", prefix], env=env)
    entries = {"CARGO_HOME": str(cargo), "RUSTUP_HOME": str(rustup), "PATH": env["PATH"], "LINKING_COMMON": "1"}
    environment = prefix / "env.sh"
    temporary = environment.with_suffix(".new")
    temporary.write_text("\n".join("export " + key + "=" + shlex.quote(value) for key, value in entries.items()) + "\n")
    temporary.replace(environment)
    stamp.write_text(json.dumps({"pins_sha256": identity, "backend": "native"}, indent=2) + "\n")
    print("cloud setup: source " + shlex.quote(str(environment)), flush=True)


if __name__ == "__main__":
    main()
