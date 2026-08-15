{
  description = "seasalt - fish-style autosuggestion and per-directory history for bash";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "seasalt";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          meta = {
            description = "fish-style inline autosuggestion and per-directory history for bash";
            mainProgram = "seasalt";
          };
        };
      });
}
