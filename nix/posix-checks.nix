{ pkgs, lib, root }:

let
  src = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.unions [
      (root + "/check")
      (root + "/ops/restart.sh")
      (root + "/release-check")
      (root + "/static/install.sh")
      (root + "/static/symbol.sh")
      (root + "/tests/alias_transfer_e2e.sh")
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

  # python3 is a harness tool, not a client dependency: the tests use it to
  # validate JSON output and to stand up a fake server. static/symbol.sh itself
  # never calls it, so the client is still exercised on a bare POSIX shell.
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
      pkgs.python3
    ];
  } (builtins.readFile ./check-posix-dash.sh);
in
{
  posix-static = static;
  posix-dash-runtime = dashRuntime;
}
// lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
  # Same as the dash runtime: busybox provides the shell and the utilities the
  # client uses, python3 only the harness. This check pins PATH, so python3 has
  # to be on that list as well as in the inputs.
  posix-busybox-runtime = pkgs.runCommand "symbol-posix-shell-busybox-runtime" {
    inherit src;
    nativeBuildInputs = [ pkgs.busybox pkgs.python3 ];
    busyboxPath = lib.makeBinPath [ pkgs.busybox pkgs.python3 ];
  } (builtins.readFile ./check-posix-busybox.sh);
}
