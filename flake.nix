{
  description = "Tiny static-site hosting for the tailnet";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.rust-overlay = {
    url = "github:oxalica/rust-overlay";
    inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    { self, nixpkgs, rust-overlay }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forEachSystem =
        f:
        lib.genAttrs systems (
          system:
          f (
            import nixpkgs {
              inherit system;
              overlays = [ rust-overlay.overlays.default ];
            }
          )
        );
      src = lib.fileset.toSource {
        root = ./.;
        fileset = lib.fileset.unions [
          ./Cargo.toml
          ./Cargo.lock
          ./build.rs
          ./src
          ./ops
          ./static
        ];
      };
    in
    {
      packages = forEachSystem (
        pkgs:
        let
          rust = pkgs.rust-bin.stable."1.98.0".default;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rust;
            rustc = rust;
          };
          symbol = rustPlatform.buildRustPackage {
            pname = "symbol";
            version = "0.1.0";
            inherit src;
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = [ pkgs.pkg-config ];
            meta = {
              description = "Tiny static-site hosting for the tailnet";
              mainProgram = "symbol";
            };
          };
        in
        {
          inherit symbol;
          default = symbol;
        }
      );

      devShells = forEachSystem (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.symbol ];
          packages = [
            (pkgs.rust-bin.stable."1.98.0".default.override {
              extensions = [
                "clippy"
                "rust-analyzer"
                "rust-src"
                "rustfmt"
              ];
            })
          ];
        };
      });

      overlays.default = final: _prev: {
        symbol = self.packages.${final.stdenv.hostPlatform.system}.symbol;
      };
    };
}
