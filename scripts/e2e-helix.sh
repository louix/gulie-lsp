#!/usr/bin/env bash
# End-to-end check through a real editor: Helix driven in a pseudo-terminal.
#
#   cargo build --release && nix develop --command scripts/e2e-helix.sh
#
# On a copy of the Guix fixture: delete the closing paren of
# (host-name "fixture"), expect exactly one diagnostic, on the opener of
# (operating-system ...); undo, expect none. The result is read from Helix's
# own LSP log, so it verifies what the editor received. Exit 0 on pass.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/env.sh
command -v hx >/dev/null || { echo "hx not found; run inside nix develop" >&2; exit 2; }
command -v script >/dev/null || { echo "script(1) from util-linux is required" >&2; exit 2; }

XDG_CACHE_HOME=$(mktemp -d); export XDG_CACHE_HOME
# Helix 25 logs under the state directory; older releases used the cache.
export XDG_STATE_HOME=$XDG_CACHE_HOME
work=$(mktemp -d); trap '[ -n "${KEEP:-}" ] || rm -rf "$work" "$XDG_CACHE_HOME"' EXIT
cp fixtures/guix/system.scm "$work/system.scm"
export TERM=xterm-256color

keys() {
  sleep 3                 # editor start and LSP initialize
  printf '/fixture\r'     # select "fixture" inside (host-name "fixture")
  sleep 0.5
  printf 'f);d'           # extend to the closing paren, collapse onto it, delete
  sleep 2                 # diagnostics round trip
  printf 'u'              # undo
  sleep 2
  printf ':q!\r'
}
# script(1) runs the command through $SHELL, whose rc files may reset the
# XDG variables; use a plain sh so the repo-local configuration is seen.
keys | SHELL=$(command -v sh) script -qfc "stty cols 120 rows 40; hx -vv --log '$work/helix.log' '$work/system.scm'" "$work/typescript" >/dev/null 2>&1 || true

log="$work/helix.log"
[ -s "$log" ] || { echo "FAIL: no Helix log at $log" >&2; echo "terminal output:" >&2; strings "$work/typescript" | grep -v "^\\[" | tail -15 >&2; exit 1; }
# At -vv Helix logs every incoming message as one JSON line.
grep publishDiagnostics "$log" | grep -o '{"jsonrpc".*}$' \
  | jq -c '.params | {version, count: (.diagnostics | length),
                      first: (.diagnostics[0] // null | if . then {message, start: .range.start} else null end)}' \
  > "$work/diagnostics" || true
echo "diagnostics received by Helix:"; cat "$work/diagnostics"

expected_error='{"message":"unclosed form; expected a closing delimiter","start":{"character":0,"line":16}}'
after_delete=$(jq -c 'select(.count == 1) | .first' "$work/diagnostics" | head -1)
last=$(tail -1 "$work/diagnostics" | jq -c '.count')
if [ "$after_delete" = "$expected_error" ] && [ "$last" = "0" ] && [ "$(wc -l < "$work/diagnostics")" -ge 3 ]; then
  echo "PASS: one error on the unclosed opener after the deletion, none after undo"
else
  echo "FAIL: expected an entry with count 1 and first=$expected_error, then a final count of 0" >&2
  echo "server traffic (didChange bodies truncated):" >&2
  grep -o '"method":"textDocument/didChange".\{0,120\}' "$log" >&2 || true
  grep -o '"method":"window/logMessage".\{0,200\}' "$log" >&2 || true
  exit 1
fi
