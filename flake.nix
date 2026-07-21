{
  description = "Privacy-first Neovim key usage insights";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    pkfire.url = "github:mizchi/pkfire";
    pkfire.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { nixpkgs, pkfire, ... }:
    let
      systems = [ "aarch64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in {
      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
        in {
          default = pkgs.mkShell {
            packages = [
              pkgs.actionlint
              pkgs.cargo
              pkgs.clippy
              pkgs.neovim
              pkgs.pkl
              pkgs.rustc
              pkgs.rustfmt
              pkfire.packages.${system}.default
            ];
          };
        });
    };
}
