{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };
        toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        commonBuildInputs = [
          # bazel_8 is offline; bazelisk fetches whatever .bazelversion asks for.
          pkgs.bazel_8
          pkgs.bazelisk
          pkgs.bazel-buildtools
          pkgs.gh
          pkgs.jq
          pkgs.just
          pkgs.protobuf
          pkgs.shellcheck
          pkgs.shfmt
        ];
      in
      {
        # No `packages.default` yet: starlark-cst is a path dependency pointing
        # outside this tree, which a nix build cannot see. Restore this once
        # starlark-cst is on crates.io and the path dep becomes a version.

        devShells.default = pkgs.mkShell {
          buildInputs = [ toolchain ] ++ commonBuildInputs;
        };
      }
    );
}
