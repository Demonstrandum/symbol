{
  description = "Tiny static-site hosting for the tailnet";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forEachSystem = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      src = lib.fileset.toSource {
        root = ./.;
        fileset = lib.fileset.unions [
          ./Cargo.toml
          ./Cargo.lock
          ./src
          ./ops
        ];
      };
    in
    {
      packages = forEachSystem (
        pkgs:
        let
          symbol = pkgs.rustPlatform.buildRustPackage {
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
            pkgs.cargo
            pkgs.clippy
            pkgs.rust-analyzer
            pkgs.rustc
            pkgs.rustfmt
          ];
        };
      });

      overlays.default = final: _prev: {
        symbol = self.packages.${final.stdenv.hostPlatform.system}.symbol;
      };
    };
}
