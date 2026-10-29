#!/usr/bin/env bash
# Shared cloud setup. Products pass their checkout as the first argument.
set -euo pipefail
shared=$(dirname "$(realpath "$0")")
repo=$(realpath "${1:-$shared/../..}")
backend=${CLOUD_BACKEND:-nix}
prefix=$(realpath -m "${CLOUD_PREFIX:-$(dirname "$repo")/.cloud}")
export LINKING_COMMON=1

case "$backend" in
    native)
        python3 "$shared/native.py" --repo "$repo" --prefix "$prefix"
        environment="$prefix/env.sh"
        ;;
    nix)
        if ! command -v nix >/dev/null 2>&1; then
            if [ "$(id -u)" != 0 ]; then
                echo 'cloud setup: Nix installation requires root; select CLOUD_BACKEND=native for a restricted cloud runtime' >&2
                exit 1
            fi
            apt-get update -qq
            apt-get install -y -qq nix-bin
            mkdir -p /etc/nix
            printf '%s\n' 'build-users-group =' 'sandbox = false' >>/etc/nix/nix.conf
        fi
        mkdir -p "$prefix"
        env -C "$repo" nix --extra-experimental-features 'nix-command flakes' print-dev-env .#default >"$prefix/env.sh.new"
        mv "$prefix/env.sh.new" "$prefix/env.sh"
        environment="$prefix/env.sh"
        # Existing privileged cloud profiles use this BASH_ENV path.
        if [ "$(id -u)" = 0 ]; then
            cp "$environment" /etc/devenv.sh
            entry='[ -f /etc/devenv.sh ] && . /etc/devenv.sh'
            if ! grep -qxF "$entry" /root/.bashrc 2>/dev/null; then
                { echo "$entry"; cat /root/.bashrc 2>/dev/null || true; } >/root/.bashrc.new
                mv /root/.bashrc.new /root/.bashrc
            fi
        fi
        ;;
    *)
        echo "cloud setup: unknown CLOUD_BACKEND: $backend (expected nix or native)" >&2
        exit 2
        ;;
esac
set +u
# shellcheck source=/dev/null
. "$environment"
set -u
# Setup resolves the committed dependency graph. Editing a sibling common
# checkout is an explicit link-common operation, separate from preparation.
env -C "$repo" cargo fetch --locked
if [ -f "$repo/fuzz/Cargo.toml" ]; then
    env -C "$repo" cargo fetch --locked --manifest-path fuzz/Cargo.toml
fi
if [ "${CLOUD_PREBUILD:-0}" = 1 ]; then
    env -C "$repo" cargo xtask test --no-run
fi
printf 'cloud setup: initialize each later shell with . %q\n' "$environment"
