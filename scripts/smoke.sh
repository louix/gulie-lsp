#!/usr/bin/env bash
# Smoke-test the server from a shell without an editor.
#
#   cargo build && scripts/smoke.sh [path/to/guile-lsp] [file.scm]
#
# Runs a canned session (initialize, open FILE, symbols, completion,
# selection, a broken edit, close, shutdown, exit) as a real client would:
# each message waits for the reply it expects. Prints every server message
# as one condensed JSON object per line. Exits 0 when the server exits
# cleanly, 1 otherwise.
set -euo pipefail
cd "$(dirname "$0")/.."
server=${1:-target/debug/guile-lsp}
file=${2:-fixtures/guix/my/services.scm}
[ -x "$server" ] || { echo "no server binary at $server; run cargo build" >&2; exit 2; }

uri="file://$(realpath "$file")"
text=$(jq -Rs . < "$file")           # JSON string literal of the file
broken=$(head -c 200 "$file" | jq -Rs .)

coproc SERVER { exec "$server"; }
out=$(mktemp); trap 'rm -f "$out"' EXIT

send() {                             # Content-Length framing; lengths in bytes
  local body=$1
  printf 'Content-Length: %d\r\n\r\n%s' "$(LC_ALL=C; printf '%s' "$body" | wc -c)" "$body" >&"${SERVER[1]}"
}

recv() {                             # read N framed messages into $out
  local n=$1 length line body
  for ((i = 0; i < n; i++)); do
    length=
    while IFS= read -r line <&"${SERVER[0]}"; do
      line=${line%$'\r'}
      [ -z "$line" ] && break
      case $line in Content-Length:*) length=${line#Content-Length: } ;; esac
    done
    [ -n "$length" ] || { echo "server closed the connection early" >&2; exit 1; }
    LC_ALL=C IFS= read -r -N "$length" body <&"${SERVER[0]}"
    printf '%s\n' "$body" >> "$out"
  done
}

# Message counts are deterministic: the server handles one message at a time.
send '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":null,"capabilities":{}}}'; recv 1
send '{"jsonrpc":"2.0","method":"initialized","params":{}}'; recv 1   # window/logMessage
send "{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/didOpen\",\"params\":{\"textDocument\":{\"uri\":\"$uri\",\"languageId\":\"scheme\",\"version\":1,\"text\":$text}}}"; recv 1
send "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"textDocument/documentSymbol\",\"params\":{\"textDocument\":{\"uri\":\"$uri\"}}}"; recv 1
send "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"textDocument/completion\",\"params\":{\"textDocument\":{\"uri\":\"$uri\"},\"position\":{\"line\":3,\"character\":6}}}"; recv 1
send "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"textDocument/selectionRange\",\"params\":{\"textDocument\":{\"uri\":\"$uri\"},\"positions\":[{\"line\":3,\"character\":6}]}}"; recv 1
send "{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/didChange\",\"params\":{\"textDocument\":{\"uri\":\"$uri\",\"version\":2},\"contentChanges\":[{\"text\":$broken}]}}"; recv 1
send "{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/didClose\",\"params\":{\"textDocument\":{\"uri\":\"$uri\"}}}"; recv 1
send '{"jsonrpc":"2.0","id":5,"method":"shutdown"}'; recv 1
send '{"jsonrpc":"2.0","method":"exit"}'
exec {SERVER[1]}>&-                  # close stdin: the server exits on EOF after `exit`
wait "$SERVER_PID"; status=$?

jq -c '
  if .method == "textDocument/publishDiagnostics" then
    {notification: .method, version: .params.version, diagnostics: [.params.diagnostics[] | {message, start: .range.start}]}
  elif .method then {notification: .method, params}
  elif .id == 2 then {id, symbols: [.result[] | {name, kind, children: ((.children // []) | map(.name))}]}
  elif .id == 3 then {id, completions: [.result[].label] | .[0:12], total: (.result | length)}
  elif .id == 4 then {id, selection: [.result[0] | recurse(.parent; . != null) | .range | "\(.start.line):\(.start.character)-\(.end.line):\(.end.character)"]}
  else . end' "$out"
echo "server exit status: $status"
exit "$status"
