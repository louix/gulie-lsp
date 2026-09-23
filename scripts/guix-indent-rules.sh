#!/usr/bin/env bash
# Regenerate src/adapters/guix_indent.rs from a Guix checkout's .dir-locals.el.
# Usage: scripts/guix-indent-rules.sh /path/to/guix
set -euo pipefail
checkout=${1:?usage: $0 GUIX-CHECKOUT}
file=$checkout/.dir-locals.el
commit=$(git -C "$checkout" rev-parse --short HEAD 2>/dev/null || echo unknown)
out=$(dirname "$0")/../src/adapters/guix_indent.rs
{
  cat <<HEADER
//! Indentation rules for Guix forms, transcribed from the \`scheme-indent-function\`
//! entries in Guix's \`.dir-locals.el\` (commit $commit). Each entry names a
//! macro or procedure and the number of distinguished arguments: 0 for record
//! constructors whose fields are all body forms, 1 or more for binding forms.
//! Regenerate with \`scripts/guix-indent-rules.sh\`.

pub const RULES: &[(&str, u8)] = &[
HEADER
  grep -o "put '[^ ]* 'scheme-indent-function [0-9]*" "$file" \
    | sed "s/put '//; s/ 'scheme-indent-function / /" \
    | grep -v '^scheme-indent-function' | sort -u \
    | awk '{printf "    (\"%s\", %s),\n", $1, $2}'
  echo "];"
} > "$out"
echo "wrote $out"
