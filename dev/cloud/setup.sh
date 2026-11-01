#!/usr/bin/env bash
# Shared cloud setup. Products pass their checkout as the first argument.
set -euo pipefail
shared=$(dirname "$(realpath "$0")")
repo=$(realpath "${1:-$shared/../..}")
backend=${CLOUD_BACKEND:-nix}
prefix=$(realpath -m "${CLOUD_PREFIX:-$(dirname "$repo")/.cloud}")
selector="$prefix/env.sh"
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
        # One prefix serves every checkout of a multi-repository session, and
        # each product has its own development shell: env.<checkout>.sh holds
        # it, and env.sh sources the one for the checkout holding $PWD.
        environment="$prefix/env.$(basename "$repo").sh"
        env -C "$repo" nix --extra-experimental-features 'nix-command flakes' print-dev-env .#default >"$environment.new"
        mv "$environment.new" "$environment"
        quoted=$(printf %q "$prefix")
        cat >"$selector.new" <<SELECT
# Sources the prepared development shell of the checkout holding \$PWD.
__cloud_root=\$(git rev-parse --show-toplevel 2>/dev/null)
if [ -n "\$__cloud_root" ] && [ -f $quoted/env."\${__cloud_root##*/}".sh ]; then
    export CARGO_TARGET_DIR="\${CARGO_TARGET_DIR:-\$__cloud_root/target}"
    # Builds use the committed dependency graph, as the native backend does;
    # a shell hook does not link a sibling common checkout.
    export LINKING_COMMON=1
    . $quoted/env."\${__cloud_root##*/}".sh
else
    echo "cloud environment: no prepared shell for \$PWD; run dev/cloud/setup.sh in that checkout" >&2
fi
unset __cloud_root
SELECT
        mv "$selector.new" "$selector"
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
printf 'cloud setup: initialize each later shell with . %q\n' "$selector"
