# Sourced by the editor launchers: puts the server on PATH and points the
# editor at the repo-local configuration. Usage: source scripts/env.sh
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
if [ -z "${GUILE_LSP_BIN:-}" ]; then
  for candidate in "$repo/target/release/guile-lsp" "$repo/target/debug/guile-lsp"; do
    [ -x "$candidate" ] && GUILE_LSP_BIN=$candidate && break
  done
fi
[ -x "${GUILE_LSP_BIN:-}" ] || { echo "no server binary; run cargo build --release (or set GUILE_LSP_BIN)" >&2; return 1 2>/dev/null || exit 1; }
export GUILE_LSP_BIN
export PATH="$(dirname "$GUILE_LSP_BIN"):$PATH"
export XDG_CONFIG_HOME="$repo/editor"
# Kakoune ignores XDG_CONFIG_HOME and reads ~/.config/kak unless told otherwise.
export KAKOUNE_CONFIG_DIR="$repo/editor/kak"
export XDG_CACHE_HOME="${TMPDIR:-/tmp}/guile-lsp-editor-cache"
export XDG_STATE_HOME="$XDG_CACHE_HOME"
