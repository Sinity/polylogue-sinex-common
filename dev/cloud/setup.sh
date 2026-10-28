#!/usr/bin/env bash
# Prepares a cloud VM (Claude Code on the web, Codex cloud) to build and test
# this repository: the Nix development shell for every later command and
# Cargo's sources. It requires root permissions during environment setup while
# the network is open, and is safe to run again. Claude Code clones the
# repository to /home/user/polylogue-sinex-common; Codex starts in the checkout:
#
#     bash /home/user/polylogue-sinex-common/dev/cloud/setup.sh    # Claude Code
#     bash dev/cloud/setup.sh                   # Codex
#
# A product session runs the product's setup instead. Sinex's development
# shell covers this repository's tests; Polylogue's has no PostgreSQL, so
# devkit's `pg` tests do not run in a Polylogue session.
set -euo pipefail
cd "$(dirname "$0")/../.."

if ! command -v nix >/dev/null 2>&1; then
    # Ubuntu's package comes from mirrors every network preset allows.
    apt-get update -qq
    apt-get install -y -qq nix-bin
    mkdir -p /etc/nix
    printf '%s\n' 'build-users-group =' 'sandbox = false' >>/etc/nix/nix.conf
fi
nix=(nix --extra-experimental-features 'nix-command flakes')

# Every later shell enters the development shell: non-interactive ones through
# BASH_ENV=/etc/devenv.sh in the environment's variables, interactive ones
# through the first line of root's .bashrc, before its non-interactive return.
"${nix[@]}" print-dev-env .#default >/etc/devenv.sh.new
mv /etc/devenv.sh.new /etc/devenv.sh
entry='[ -f /etc/devenv.sh ] && . /etc/devenv.sh'
if ! grep -qxF "$entry" /root/.bashrc 2>/dev/null; then
    { echo "$entry"; cat /root/.bashrc 2>/dev/null || true; } >/root/.bashrc.new
    mv /root/.bashrc.new /root/.bashrc
fi

set +u
. /etc/devenv.sh
set -u
cargo fetch --locked
