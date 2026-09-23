#!/usr/bin/env bash
# End-to-end check through Kakoune and kak-lsp, driven in a pseudo-terminal:
# the same delete-and-undo scenario as scripts/e2e-helix.sh, verified from
# kak-lsp's own log. Needs kak and kak-lsp (16 or later) on PATH.
#
#   cargo build --release && scripts/e2e-kak.sh
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/env.sh
command -v kak >/dev/null || { echo "kak not found" >&2; exit 2; }
command -v kak-lsp >/dev/null || { echo "kak-lsp not found" >&2; exit 2; }
command -v script >/dev/null || { echo "script(1) from util-linux is required" >&2; exit 2; }

work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
cp fixtures/guix/system.scm "$work/system.scm"
log=/tmp/guile-lsp-kak-lsp.log
rm -f "$log"
export TERM=xterm-256color

keys() {
  sleep 4                 # editor start, kak-lsp start, LSP initialize
  printf '/fixture\r'     # select "fixture" inside (host-name "fixture")
  sleep 0.5
  printf 'f);d'           # extend to the closing paren, collapse onto it, delete
  sleep 3                 # diagnostics round trip
  printf 'u'              # undo
  sleep 3
  printf ':q!\r'
}
keys | SHELL=$(command -v sh) script -qfc "stty cols 120 rows 40; kak '$work/system.scm'" /dev/null >/dev/null 2>&1 || true

[ -s "$log" ] || { echo "FAIL: no kak-lsp log at $log" >&2; exit 1; }
# At -vvvv kak-lsp logs every message from the server.
grep -o '{"jsonrpc".*"method":"textDocument/publishDiagnostics".*}' "$log" \
  | jq -c '.params | {version, count: (.diagnostics | length),
                      first: (.diagnostics[0] // null | if . then {message, start: .range.start} else null end)}' \
  > "$work/diagnostics" || true
echo "diagnostics received by kak-lsp:"; cat "$work/diagnostics"

expected_error='{"message":"unclosed form; expected a closing delimiter","start":{"character":0,"line":16}}'
after_delete=$(jq -c 'select(.count == 1) | .first' "$work/diagnostics" | head -1)
last=$(tail -1 "$work/diagnostics" | jq -c '.count' 2>/dev/null || echo none)
if [ "$after_delete" = "$expected_error" ] && [ "$last" = "0" ]; then
  echo "PASS: one error on the unclosed opener after the deletion, none after undo"
else
  echo "FAIL: expected an entry with count 1 and first=$expected_error, then a final count of 0" >&2
  grep -i "error\|initialize" "$log" | head -20 >&2 || true
  exit 1
fi
