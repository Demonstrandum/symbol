{ pkgs, lib, root }:

let
  src = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.unions [
      (root + "/check")
      (root + "/ops/restart.sh")
      (root + "/release-check")
      (root + "/crates/symbol/static/install.sh")
      (root + "/crates/symbol/static/symbol.sh")
      (root + "/tests/concurrency_soak.sh")
      (root + "/tests/lifecycle_e2e.sh")
      (root + "/tests/production_guard.sh")
      (root + "/tests/symbol_client.sh")
    ];
  };

  static = pkgs.runCommand "symbol-posix-shell-static" {
    inherit src;
    nativeBuildInputs = [
      pkgs.checkbashisms
      pkgs.dash
      pkgs.shellcheck
    ];
  } (builtins.readFile ./check-posix-static.sh);

  dashRuntime = pkgs.runCommand "symbol-posix-shell-dash-runtime" {
    inherit src;
    nativeBuildInputs = [
      pkgs.coreutils
      pkgs.dash
      pkgs.diffutils
      pkgs.findutils
      pkgs.gawk
      pkgs.gnused
      pkgs.gnutar
      pkgs.gzip
    ];
  } (builtins.readFile ./check-posix-dash.sh);
in
{
  posix-static = static;
  posix-dash-runtime = dashRuntime;
}
// lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
  posix-busybox-runtime = pkgs.runCommand "symbol-posix-shell-busybox-runtime" {
    inherit src;
    nativeBuildInputs = [ pkgs.busybox ];
    busyboxPath = lib.makeBinPath [ pkgs.busybox ];
  } (builtins.readFile ./check-posix-busybox.sh);
}
