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
          pkgs.bazel_8
          pkgs.cargo-edit
          pkgs.gh
          pkgs.bazelisk
          pkgs.bazel-buildtools
          pkgs.biome
          pkgs.just
          pkgs.mdx-language-server
          pkgs.nodejs_22
          pkgs.openssh
          pkgs.pnpm
          pkgs.rsync
          pkgs.vtsls
        ];
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "bazel-language-server";
          version = "0.1.1";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
        };

        devShells.default = pkgs.mkShell {
          buildInputs = [ toolchain ] ++ commonBuildInputs;
        };

        devShells.ci = pkgs.mkShell {
          buildInputs = [ toolchain ] ++ commonBuildInputs;
        };
      }
    );
}
