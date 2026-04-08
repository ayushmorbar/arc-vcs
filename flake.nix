{
  description = "arc-vcs reproducible development and CI flake";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            rustc
            cargo
            clippy
            rust-analyzer
            cargo-nextest
            blake3
          ];

          shellHook = ''
            export RUST_BACKTRACE=1
            export CARGO_TERM_COLOR=always
          '';
        };

        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "arc-vcs";
          version = "0.1.0";
          src = self;
          cargoLock = {
            lockFile = ./Cargo.lock;
          };
          doCheck = false;
        };

        checks.default = self.packages.${system}.default;
      }
    );
}
