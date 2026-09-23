{
  description = "Guile LSP development environment";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";

  outputs = { nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in {
      packages = forAllSystems (system:
        let pkgs = import nixpkgs { inherit system; };
        in rec {
          # The server binary; the Guile worker sources are embedded in it and
          # materialised at run time, so this is the whole program. Guile (or
          # Guix's `guix repl`) is found on PATH at run time.
          guile-lsp = pkgs.rustPlatform.buildRustPackage {
            pname = "guile-lsp";
            version = "0.1.0";
            src = pkgs.lib.cleanSource ./.;
            cargoLock.lockFile = ./Cargo.lock;
            # Integration tests need Guile and Guix processes; unit tests run.
            cargoTestFlags = [ "--lib" ];
            meta = {
              description = "Guile language server with optional Guix integration";
              mainProgram = "guile-lsp";
            };
          };
          default = guile-lsp;
        });

      devShells = forAllSystems (system:
        let pkgs = import nixpkgs { inherit system; };
        in {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              rustc
              rustfmt
              clippy
              rust-analyzer
              pkg-config
              guile
              # Reference editor client for manual and scripted testing.
              helix
              jq
            ];
            RUST_BACKTRACE = "1";
          };
        });
    };
}
