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
      injectedCommit = builtins.getEnv "SYMBOL_NIX_BUILD_COMMIT";
      injectedDirty = builtins.getEnv "SYMBOL_NIX_BUILD_DIRTY";
      provenanceCommit =
        if self ? rev then
          self.rev
        else if self ? dirtyRev then
          lib.removeSuffix "-dirty" self.dirtyRev
        else if injectedCommit != "" then
          injectedCommit
        else
          "unknown";
      provenanceDirty =
        if self ? rev then
          "false"
        else if self ? dirtyRev then
          "true"
        else if injectedDirty != "" then
          injectedDirty
        else
          "true";
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
          ./release-check
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
            SYMBOL_BUILD_COMMIT = provenanceCommit;
            SYMBOL_BUILD_DIRTY = provenanceDirty;
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
            SYMBOL_BUILD_COMMIT = provenanceCommit;
            SYMBOL_BUILD_DIRTY = provenanceDirty;
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
          provenance = pkgs.runCommand "symbol-generated-provenance" {
            nativeBuildInputs = [ pkgs.python3 ];
          } ''
            python3 - "${generatedSources}/symbol-contract.json" ${lib.escapeShellArg provenanceCommit} ${lib.escapeShellArg provenanceDirty} <<'PY'
            import json
            import sys

            fixture = json.load(open(sys.argv[1], encoding="utf-8"))
            assert fixture["build"]["commit"] == sys.argv[2]
            assert fixture["build"]["dirty"] is (sys.argv[3] == "true")
            PY
            touch "$out"
          '';
          sdkCheck =
            name: nativeBuildInputs: command:
            pkgs.runCommand name {
              inherit src;
              SYMBOL_GENERATED_DIR = generatedSources;
              inherit nativeBuildInputs;
            } ''
              cd "$src"
              ${command}
              touch "$out"
            '';
          sdkMock = sdkCheck "symbol-sdk-mock" [ pkgs.nodejs ] ''
            node tests/sdk/mock-server.mjs
          '';
          sdkJs = sdkCheck "symbol-sdk-js" [ pkgs.nodejs ] ''
            node tests/sdk/js/runtime.mjs
          '';
          sdkJsReal = sdkCheck "symbol-sdk-js-real" [ pkgs.nodejs ] ''
            export SYMBOL_BIN="${package}/bin/symbol"
            node tests/sdk/js/real.mjs
          '';
          sdkTs = sdkCheck "symbol-sdk-ts" [
            pkgs.nodejs
            pkgs.typescript
          ] ''
            export TSC="${pkgs.typescript}/bin/tsc"
            export SYMBOL_BIN="${package}/bin/symbol"
            node tests/sdk/ts/run.mjs
          '';
          sdkBrowser = sdkCheck "symbol-sdk-browser" (
            [ pkgs.nodejs ]
            ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.chromium ]
          ) ''
            ${lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
              export CHROMIUM_BIN="${pkgs.chromium}/bin/chromium"
            ''}
            node tests/sdk/js/browser-smoke.mjs
          '';
          named = posix // {
            inherit package;
            generated-sources = generatedSources;
            generated-provenance = provenance;
            public-api-freeze = publicApiFreeze;
            sdk-mock = sdkMock;
            sdk-js = sdkJs;
            sdk-js-real = sdkJsReal;
            sdk-ts = sdkTs;
            sdk-browser = sdkBrowser;
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
            pkgs.nodejs
            pkgs.typescript
          ] ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.chromium ];
        };
      });

      overlays.default = final: _prev: {
        symbol = self.packages.${final.stdenv.hostPlatform.system}.symbol;
      };
    };
}
