{
  description = "Minimal Wayland Activate Linux overlay";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
  };

  outputs = inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];

      perSystem = { config, pkgs, ... }:
        let
          commonBuildInputs = with pkgs; [
            cairo
            wayland
          ];
        in
        {
          packages.default = pkgs.rustPlatform.buildRustPackage {
            pname = "activate-linux";
            version = "0.1.0";
            src = ./.;

            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = with pkgs; [
              pkg-config
            ];

            buildInputs = commonBuildInputs;
          };

          apps.default = {
            type = "app";
            program = "${config.packages.default}/bin/activate-linux";
          };

          devShells.default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              clippy
              pkg-config
              rustc
              rustfmt
            ];

            buildInputs = commonBuildInputs;
          };
        };
    };
}
