#!/usr/bin/env bash
# Reindent Scheme files with Emacs' scheme-mode, honouring .dir-locals.el
# rules found above each file, and write the results to an output directory.
# The formatter's fidelity check (examples/indent.rs) compares against these.
# Usage: scripts/emacs-indent.sh OUT-DIR FILE...
set -euo pipefail
out=${1:?usage: $0 OUT-DIR FILE...}; shift
mkdir -p "$out"
for file in "$@"; do
  emacs --batch -Q \
    --eval '(setq enable-local-eval t enable-local-variables :all)' \
    --eval "(find-file \"$file\")" \
    --eval '(indent-region (point-min) (point-max))' \
    --eval "(write-region (point-min) (point-max) \"$out/$(basename "$file")\")" \
    2>/dev/null
done
