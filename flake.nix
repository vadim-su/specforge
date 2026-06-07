{
  description = "SpecForge - spec-driven project state CLI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      lib = nixpkgs.lib;

      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      forAllSystems = lib.genAttrs systems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        rec {
          specforge = pkgs.rustPlatform.buildRustPackage {
            pname = "specforge";
            version = "0.1.0";

            src = lib.fileset.toSource {
              root = ./.;
              fileset = lib.fileset.unions [
                ./Cargo.toml
                ./Cargo.lock
                ./build.rs
                ./src
                ./prompts
                ./README.md
                ./LICENSE
              ];
            };

            cargoLock.lockFile = ./Cargo.lock;

            doCheck = true;

            meta = {
              description = "Spec-driven project state CLI";
              homepage = "https://github.com/kosto/specforge";
              license = lib.licenses.mit;
              mainProgram = "specforge";
            };
          };

          default = specforge;
        }
      );

      apps = forAllSystems (
        system:
        {
          specforge = {
            type = "app";
            program = lib.getExe self.packages.${system}.specforge;
            meta.description = "Run the SpecForge CLI";
          };

          default = self.apps.${system}.specforge;
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              clippy
              rustc
              rustfmt
            ];
          };
        }
      );

      formatter = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        pkgs.nixfmt
      );

      overlays.default = final: prev: {
        specforge = self.packages.${prev.stdenv.hostPlatform.system}.default;
      };

      nixosModules.default =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.programs.specforge;
        in
        {
          options.programs.specforge = {
            enable = lib.mkEnableOption "SpecForge";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
              defaultText = lib.literalExpression "inputs.specforge.packages.\${pkgs.stdenv.hostPlatform.system}.default";
              description = "SpecForge package to install.";
            };
          };

          config = lib.mkIf cfg.enable {
            environment.systemPackages = [ cfg.package ];
          };
        };
    };
}
