{
  description = "Kinora — an agent-native knowledge system where ideas move, connect, and compose";

  inputs.jig.url = "github:edger-dev/jig";

  outputs = { self, jig }:
    jig.lib.mkWorkspace
      {
        pname = "kinora";
        src = ./.;
        extraDevPackages = pkgs: [
          # `kinora` wrapper: always runs the current workspace source via
          # `cargo run`. Cargo's mtime check skips compile when unchanged,
          # so the overhead on repeat calls is negligible.
          (pkgs.writeShellScriptBin "kinora" ''
            exec cargo run --quiet -p kinora-cli -- "$@"
          '')
        ];
      }
      {
        rust = {
          # buildPackages = [ "kinora-cli" ];  # omit to build whole workspace
          # wasm = true;
        };
      };
}
