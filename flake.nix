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
          ./API.md
          ./api-version
          ./api-version.toml
          ./check
          ./crates
          ./flake.nix
          ./nix
          ./public-api-freeze.json
          ./schema.sql
          ./ops
          ./static
          ./tests
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
            cargoTestFlags = [
              "--workspace"
              "--all-targets"
              "--all-features"
            ];
            SYMBOL_GENERATION_MODE = "readonly";
            # Flake rev/dirtyRev follows tracked Git state and ignores untracked files.
            SYMBOL_BUILD_COMMIT =
              if self ? rev then
                self.rev
              else if self ? dirtyRev then
                lib.removeSuffix "-dirty" self.dirtyRev
              else
                "unknown";
            SYMBOL_BUILD_DIRTY = if self ? rev then "false" else "true";
            nativeBuildInputs = [
              pkgs.git
              pkgs.pkg-config
            ];
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

      checks = forEachSystem (
        pkgs:
        let
          posix = import ./nix/posix-checks.nix {
            inherit pkgs lib;
            root = ./.;
          };
          package = self.packages.${pkgs.stdenv.hostPlatform.system}.symbol;
          rust = pkgs.rust-bin.stable."1.98.0".default;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rust;
            rustc = rust;
          };
          generatedSources = rustPlatform.buildRustPackage {
            pname = "symbol-generated-sources";
            version = "0.1.0";
            inherit src;
            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [
              "-p"
              "symbol"
              "--example"
              "symbol-generate"
            ];
            cargoTestFlags = [
              "-p"
              "symbol"
              "--example"
              "symbol-generate"
            ];
            SYMBOL_GENERATION_MODE = "readonly";
            # Keep generated-source provenance identical to the package derivation.
            SYMBOL_BUILD_COMMIT =
              if self ? rev then
                self.rev
              else if self ? dirtyRev then
                lib.removeSuffix "-dirty" self.dirtyRev
              else
                "unknown";
            SYMBOL_BUILD_DIRTY = if self ? rev then "false" else "true";
            nativeBuildInputs = [
              pkgs.git
              pkgs.pkg-config
            ];
            installPhase = ''
              runHook preInstall
              generated_api=$(find target -type f -path '*/build/symbol-*/out/api.ts' -print -quit)
              test -n "$generated_api"
              generated_dir=$(dirname "$generated_api")
              mkdir -p "$out"
              cp \
                "$generated_dir/api.ts" \
                "$generated_dir/api.js" \
                "$generated_dir/api.global.js" \
                "$generated_dir/api.d.ts" \
                "$generated_dir/api.py" \
                "$generated_dir/schema.sql" \
                "$generated_dir/symbol-contract.json" \
                "$out/"
              runHook postInstall
            '';
          };
          freezeSrc = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./public-api-freeze.json
              ./tests/public_api_freeze.py
            ];
          };
          publicApiFreeze = pkgs.runCommand "symbol-public-api-freeze" {
            src = freezeSrc;
            nativeBuildInputs = [ pkgs.python3 ];
          } ''
            cp -R "$src" source
            chmod -R u+w source
            cd source
            SYMBOL_BIN="${package}/bin/symbol" python3 tests/public_api_freeze.py
            touch "$out"
          '';
          named = posix // {
            inherit package;
            generated-sources = generatedSources;
            public-api-freeze = publicApiFreeze;
          };
        in
        named
        // {
          all = pkgs.linkFarm "symbol-all-checks" (
            lib.mapAttrsToList (name: path: {
              inherit name path;
            }) named
          );
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
