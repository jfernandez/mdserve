{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    utils.url = "github:numtide/flake-utils";
    fenix.url = "github:nix-community/fenix";
    naersk.url = "github:nix-community/naersk/master";
  };

  outputs =
    {
      self,
      nixpkgs,
      utils,
      fenix,
      naersk
    }:
    utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        rustToolchain =
          with fenix.packages.${system};
          combine [
            (stable.withComponents [
              "rustc"
              "cargo"
              "rustfmt"
              "clippy"
              "rust-src"
              "rust-analyzer"
            ])
          ];
        naersk-lib = pkgs.callPackage naersk { };
      in
      {
        defaultPackage = naersk-lib.buildPackage ./.;
        devShell =
          with pkgs;
          mkShell {
            buildInputs = [
              rustToolchain
            ];
          };
      }
    );
}
