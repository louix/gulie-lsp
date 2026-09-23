# guile-lsp

A language server for Guile Scheme, with optional Guix integration.

- Completion of lexical names, imported names (honouring `#:select`, `#:hide`, `#:prefix`), module paths, special forms, and Guix record fields.
- Hover and go-to-definition for lexical bindings, open buffers, imported bindings, Guix service types, record fields, and module names in imports. Go-to-type-definition takes a service type to its configuration record.
- Syntax diagnostics on every change; arity, keyword, and Guix record-field diagnostics after a short pause. Quoted and staged code is never checked.
- Document symbols, selection ranges, and code actions that add a missing import.
- Formatting that reindents the way Emacs `scheme-mode` does, also available as `guile-lsp format` ([docs/FORMATTING.md](docs/FORMATTING.md)).
- `guile.validate`: load a saved Guix system or home configuration without building anything and report errors as diagnostics ([docs/VALIDATION.md](docs/VALIDATION.md)).

Semantic features come from a supervised Guile worker process ([docs/PROTOCOL.md](docs/PROTOCOL.md)); syntax features never wait for it.

## Install

`cargo install --path .`, `nix build .`, or `guix build -f guix.scm`. A Guile (or `guix repl`) must be on `PATH` at run time. See [docs/USAGE.md](docs/USAGE.md) for environments, trust, editor setup, limitations, and troubleshooting.

## Configuration

Settings come from the client's `initializationOptions`; all are optional:

```json
{ "guile": { "command": ["guix", "repl", "--"],
             "loadPath": ["."],
             "trust": "trusted",
             "deadlineMs": 15000,
             "indent": { "with-my-thing": 1 },
             "validateTimeoutMs": 120000 } }
```

- `command`: the Guile to run the worker under. Defaults to `guile` on `PATH`, then `guix repl --`.
- `loadPath`: extra load-path roots. Workspace folders and roots implied by open files are added automatically.
- `trust`: `restricted` (default) or `trusted`. Restricted workspaces never load modules, so imported names get no hover or definition.
- `deadlineMs`: per-request worker deadline.
- `indent`: extra formatting rules, a head symbol to its number of distinguished arguments.
- `validateTimeoutMs`: time limit for one `guile.validate` run.

## Develop

```sh
nix develop
cargo test
cargo clippy --all-targets -- -D warnings
scripts/check.sh [--guix]     # the full release check
```

Guile-side tests:

```sh
guile -L guile -L fixtures/guile guile/tests/guile-only.scm
guix repl -L guile -L fixtures/guix -- guile/tests/guix.scm
```

`scripts/hx.sh` and `scripts/kak.sh` open an editor with a repo-local configuration and this server; `scripts/e2e-helix.sh` and `scripts/e2e-kak.sh` drive them headlessly. `examples/` has a latency replay harness (`replay`), a false-positive sweep (`sweep`), and a formatter fidelity check against Emacs output (`indent`).
