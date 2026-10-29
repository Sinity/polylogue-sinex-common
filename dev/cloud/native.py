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


def unpack(archive, directory, *, single_root=False):
    directory.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive) as source:
        roots = {Path(member.name).parts[0] for member in source.getmembers() if Path(member.name).parts}
        if single_root and len(roots) != 1:
            raise RuntimeError(f"expected one source root in {archive.name}")
        source.extractall(directory, filter="data")
    return directory / next(iter(roots)) if single_root else directory


def install_frontend(pins, cache, prefix):
    """Install compiler artifacts without npm scripts or workspace manifests."""
    for name, relative in (("typescript", "lib/tsc"), ("esbuild", "bin/esbuild")):
        source = unpack(artifact(name + ".tgz", pins[name], cache), prefix / name, single_root=True)
        binary = source / relative
        if not binary.is_file():
            raise RuntimeError(f"missing {relative} in {name} artifact")
        link = prefix / "bin" / ("tsc" if name == "typescript" else "esbuild")
        link.unlink(missing_ok=True)
        link.symlink_to(binary)


def install_debian_bootstrap(pins, cache, bootstrap, env):
    """Extract signed-index-pinned Debian packages without package scripts."""
    for name in ("m4", "bison", "flex"):
        package = artifact("debian13-" + name + ".deb", pins["debian13"][name], cache)
        run(["dpkg-deb", "--extract", package, bootstrap], env=env)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prefix", required=True, type=Path)
    parser.add_argument("--repo", required=True, type=Path)
    args = parser.parse_args()
    if os.getuid() == 0:
        parser.error("the native backend expects an ordinary user; privileged environments can use Nix")
    if not (args.repo / "Cargo.toml").is_file():
        parser.error("the target checkout has no Cargo.toml")
    bootstrap_kind = os.environ.get("CLOUD_NATIVE_BOOTSTRAP", "source")
    if bootstrap_kind not in ("source", "debian13"):
        parser.error("CLOUD_NATIVE_BOOTSTRAP must be source or debian13")
    if bootstrap_kind == "debian13":
        release = platform.freedesktop_os_release()
        if release.get("ID") != "debian" or release.get("VERSION_ID") != "13":
            parser.error("debian13 bootstrap requires a Debian 13 base image")
        if not shutil.which("dpkg-deb"):
            parser.error("debian13 bootstrap requires dpkg-deb")
    jobs = int(os.environ.get("CLOUD_BUILD_JOBS", "2"))
    if jobs < 1:
        parser.error("CLOUD_BUILD_JOBS must be positive")
    jobs = str(jobs)
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        parser.error("the pinned native toolchain currently targets x86_64 Linux")
    for tool in ("cc", "c++", "make", "pkg-config", "perl", "node", "dbus-daemon"):
        if not shutil.which(tool):
            parser.error(f"native setup requires {tool} from the cloud base image")
    run(["perl", "-MGetopt::Long", "-MFindBin", "-MFile::Compare", "-e", "exit 0"])
    run(["node", "-e", "if (Number(process.versions.node.split('.')[0]) < 24) process.exit(1)"])
    run(["dbus-daemon", "--version"])
    pins_path = Path(__file__).with_name("pins.json")
    pins = json.loads(pins_path.read_text())
    toolchain = args.repo / "rust-toolchain.toml"
    if toolchain.exists():
        import tomllib
        if tomllib.loads(toolchain.read_text())["toolchain"]["channel"] != pins["rust"]:
            parser.error("repository Rust toolchain differs from the shared cloud pin")
    prefix = args.prefix.resolve()
    prefix.mkdir(parents=True, exist_ok=True)
    scratch = prefix / "tmp"
    scratch.mkdir(mode=0o700, exist_ok=True)
    scratch.chmod(0o700)
    cache = prefix / "downloads"
    cache.mkdir(exist_ok=True)
    build = prefix / "build"
    build.mkdir(exist_ok=True)
    tools = prefix / "bin"
    tools.mkdir(exist_ok=True)
    cargo = prefix / "cargo"
    rustup = prefix / "rustup"
    bootstrap = prefix / ("bootstrap-debian13" if bootstrap_kind == "debian13" else "bootstrap")
    bootstrap_bin = bootstrap / ("usr/bin" if bootstrap_kind == "debian13" else "bin")
    pg = prefix / "postgres"
    cmake = prefix / "cmake" / "cmake-3.31.8-linux-x86_64" / "bin"
    env = os.environ | {"CARGO_HOME": str(cargo), "RUSTUP_HOME": str(rustup), "RUSTUP_INIT_SKIP_PATH_CHECK": "yes", "TMPDIR": str(scratch)}
    paths = list(map(str, (cargo / "bin", tools, pg / "bin", bootstrap_bin, cmake))) + os.environ["PATH"].split(os.pathsep)
    env["PATH"] = os.pathsep.join(dict.fromkeys(paths))
    if bootstrap_kind == "debian13":
        env |= {"M4": str(bootstrap_bin / "m4"), "BISON_PKGDATADIR": str(bootstrap / "usr/share/bison")}
    stamp = prefix / "installed.json"
    # Extension changes reuse the independently qualified core toolchain.
    core_pins = {key: value for key, value in pins.items() if key != "pgvector"}
    identity = hashlib.sha256((json.dumps(core_pins, indent=2) + "\n").encode()).hexdigest()
    postgres_pin = pins["postgres_github"] if bootstrap_kind == "debian13" else pins["postgres"]
    saved = json.loads(stamp.read_text()) if stamp.exists() else {}
    installed = saved.get("pins_sha256") == identity and saved.get("bootstrap") == bootstrap_kind
    if not installed:
        install_frontend(pins, cache, prefix)
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
        # PostgreSQL 18 requires its parser generators even from release
        # archives; these are part of the owned bootstrap, not system packages.
        if bootstrap_kind == "debian13":
            install_debian_bootstrap(pins, cache, bootstrap, env)
        else:
            # These release portability headers predate C23 libc macros.
            bootstrap_env = env | {"CFLAGS": env.get("CFLAGS", "-O2") + " -std=gnu17"}
            for name in ("m4", "bison", "flex"):
                sources = unpack(artifact(name + ".tar", pins[name], cache), build / name, single_root=True)
                if (sources / "Makefile").exists():
                    run(["make", "distclean"], cwd=sources, env=bootstrap_env)
                run([sources / "configure", "--prefix=" + str(bootstrap), "--disable-nls"], cwd=sources, env=bootstrap_env)
                run(["make", "-j" + jobs], cwd=sources, env=bootstrap_env)
                run(["make", "install"], cwd=sources, env=bootstrap_env)
        pg_sources = unpack(artifact("postgres-" + bootstrap_kind + ".tar", postgres_pin, cache), build / ("postgres-" + bootstrap_kind), single_root=True)
        # The test clusters use Unix sockets. These optional client features
        # would otherwise require system development packages in a rootless VM.
        run([pg_sources / "configure", "--prefix=" + str(pg), "--without-readline", "--without-icu", "--without-zlib"], cwd=pg_sources, env=env)
        run(["make", "-j" + jobs], cwd=pg_sources, env=env)
        run(["make", "install"], cwd=pg_sources, env=env)
        run(["make", "-C", "contrib/pg_trgm", "install"], cwd=pg_sources, env=env)
        ts_sources = unpack(artifact("timescale.tar.gz", pins["timescale"], cache), build / "timescale", single_root=True)
        ts_build = build / "timescale-build"
        run(["cmake", "-S", ts_sources, "-B", ts_build, "-DPG_CONFIG=" + str(pg / "bin/pg_config"), "-DREGRESS_CHECKS=OFF", "-DWARNINGS_AS_ERRORS=OFF", "-DUSE_OPENSSL=OFF"], env=env)
        run(["cmake", "--build", ts_build, "--parallel", jobs], env=env)
        run(["cmake", "--install", ts_build], env=env)
    if not installed or saved.get("pgvector_sha256") != pins["pgvector"]["sha256"]:
        vector_sources = unpack(artifact("pgvector.tar.gz", pins["pgvector"], cache), build / "pgvector", single_root=True)
        vector_make = ["make", "PG_CONFIG=" + str(pg / "bin/pg_config"), "OPTFLAGS="]
        run(vector_make + ["clean"], cwd=vector_sources, env=env)
        run(vector_make + ["-j" + jobs], cwd=vector_sources, env=env)
        run(vector_make + ["install"], cwd=vector_sources, env=env)
    # Check the tools again when reusing a completed prefix; a deleted binary
    # must fail setup instead of being silently accepted by a saved stamp.
    for command in (["m4", "--version"], ["bison", "--version"], ["flex", "--version"], ["node", "--version"], ["tsc", "--version"], ["esbuild", "--version"], ["dbus-daemon", "--version"], ["rustc", "--version"], ["cargo", "--version"], ["cargo", "nextest", "--version"], ["postgres", "--version"], ["nats-server", "--version"]):
        run(command, env=env)
    pkglib = Path(subprocess.check_output([str(pg / "bin/pg_config"), "--pkglibdir"], text=True, env=env).strip())
    if not (pkglib / "timescaledb.so").is_file():
        raise RuntimeError("TimescaleDB loader is missing from the prepared PostgreSQL")
    sharedir = Path(subprocess.check_output([str(pg / "bin/pg_config"), "--sharedir"], text=True, env=env).strip())
    if not (pkglib / "vector.so").is_file() or not (sharedir / "extension/vector.control").is_file():
        raise RuntimeError("pgvector library or extension control file is missing")
    run([sys.executable, Path(__file__).with_name("smoke.py"), "--scratch", prefix], env=env)
    entries = {"CARGO_HOME": str(cargo), "RUSTUP_HOME": str(rustup), "PATH": env["PATH"], "LINKING_COMMON": "1", "TMPDIR": str(scratch)}
    if bootstrap_kind == "debian13":
        entries |= {key: env[key] for key in ("M4", "BISON_PKGDATADIR")}
    environment = prefix / "env.sh"
    temporary = environment.with_suffix(".new")
    temporary.write_text("\n".join("export " + key + "=" + shlex.quote(value) for key, value in entries.items()) + "\n")
    temporary.replace(environment)
    stamp.write_text(json.dumps({"pins_sha256": identity, "backend": "native", "bootstrap": bootstrap_kind, "pgvector_sha256": pins["pgvector"]["sha256"]}, indent=2) + "\n")
    print("cloud setup: source " + shlex.quote(str(environment)), flush=True)


if __name__ == "__main__":
    main()
