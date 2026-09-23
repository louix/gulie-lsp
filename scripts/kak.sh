#!/usr/bin/env bash
# Open FILE... in Kakoune with the repo-local kak-lsp config and this server.
#   nix develop --command scripts/kak.sh fixtures/guix/system.scm
# kak-lsp logs to /tmp/guile-lsp-kak-lsp.log at maximum verbosity.
set -euo pipefail
source "$(dirname "$0")/env.sh"
echo "server: $GUILE_LSP_BIN" >&2
echo "kak-lsp log: /tmp/guile-lsp-kak-lsp.log" >&2
exec kak "$@"
