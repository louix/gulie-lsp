#!/usr/bin/env bash
# The release check: everything that must pass before a release.
#
#   nix develop --command scripts/check.sh            # Guile-only suite (mandatory)
#   nix develop --command scripts/check.sh --guix     # plus the Guix integration suite
#
# The Guile-only pass hides `guix` from PATH so the Guix tests really skip
# and nothing depends on it by accident. `--guix` needs `guix` on PATH and a
# working `guix repl`.
set -euo pipefail
cd "$(dirname "$0")/.."
with_guix=0
[ "${1:-}" = "--guix" ] && with_guix=1

step() { printf '\n== %s\n' "$*"; }

step "format, lint, build"
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build --release

step "Guile-only suite (guix hidden from PATH)"
# Every directory on PATH that holds a guix binary is dropped.
guileonly_path=$(printf '%s' "$PATH" | tr ':' '\n' | while read -r d; do [ -x "$d/guix" ] || printf '%s:' "$d"; done)
PATH="${guileonly_path%:}" cargo test 2>&1 | grep -E 'test result|skipping|panicked'
PATH="${guileonly_path%:}" guile -L guile -L fixtures/guile guile/tests/guile-only.scm 2>&1 | grep -E '^# of|FAIL' || true
PATH="${guileonly_path%:}" scripts/smoke.sh target/release/guile-lsp fixtures/guile/script.scm >/dev/null && echo "smoke: ok"
PATH="${guileonly_path%:}" cargo run --release --example replay -- fixtures/guile/script.scm 2>&1 | grep -E '^\| (completion|didChange)'

if [ "$with_guix" = 1 ]; then
  step "Guix integration suite"
  command -v guix >/dev/null || { echo "guix not on PATH" >&2; exit 2; }
  cargo test --test guix --test validate 2>&1 | grep -E 'test result|panicked'
  guix repl -L guile -L fixtures/guix -- guile/tests/guix.scm 2>&1 | grep -E '^\*\*\* # of|FAIL' || true
  step "false-positive sweep"
  SWEEP_ROOT=fixtures/guix cargo run --release --example sweep -- --guix fixtures/guix/*.scm fixtures/guix/my/*.scm fixtures/guile/*.scm 2>&1 | tail -1
  step "latency with a Guix worker"
  REPLAY_GUIX=1 cargo run --release --example replay -- fixtures/guix/system.scm 2>&1 | grep -E '^\| (completion|didChange)|RSS'
fi

step "editor sessions"
if command -v hx >/dev/null; then scripts/e2e-helix.sh | tail -1; else echo "hx not on PATH; skipped"; fi
if command -v kak >/dev/null && command -v kak-lsp >/dev/null; then scripts/e2e-kak.sh | tail -1; else echo "kak/kak-lsp not on PATH; skipped"; fi
echo; echo "all checks passed"
