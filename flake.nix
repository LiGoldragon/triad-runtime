{
  description = "triad-runtime — shared runtime support for schema-derived triad daemons";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs = { self, nixpkgs, flake-utils, fenix, crane }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        toolchain = fenix.packages.${system}.stable.withComponents [
          "cargo"
          "rustc"
          "rustfmt"
          "clippy"
          "rust-src"
        ];
        craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
        source = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter = path: type: craneLib.filterCargoSources path type;
        };
        commonArguments = {
          src = source;
          strictDeps = true;
        };
        cargoArtifacts = craneLib.buildDepsOnly commonArguments;
      in
      {
        packages.default = craneLib.buildPackage (commonArguments // {
          inherit cargoArtifacts;
        });

        checks = {
          test = craneLib.cargoTest (commonArguments // {
            inherit cargoArtifacts;
          });
          clippy = craneLib.cargoClippy (commonArguments // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets --all-features -- -D warnings";
          });
        };

        devShells.default = pkgs.mkShell {
          name = "triad-runtime";
          packages = [
            pkgs.jujutsu
            toolchain
          ];
        };
      }
    );
}
