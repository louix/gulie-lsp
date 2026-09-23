#!/usr/bin/env bash
# Open FILE... in Helix with the repo-local config and this server.
#   nix develop --command scripts/hx.sh fixtures/guix/system.scm
# Helix writes its log (including LSP traffic at -vv) under $XDG_CACHE_HOME.
set -euo pipefail
source "$(dirname "$0")/env.sh"
echo "server: $GUILE_LSP_BIN" >&2
log="$XDG_CACHE_HOME/helix.log"
echo "helix log: $log" >&2
mkdir -p "$XDG_CACHE_HOME"
exec hx -vv --log "$log" "$@"
