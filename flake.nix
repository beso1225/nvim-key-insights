{
  description = "Privacy-first Neovim key usage insights";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    pkfire.url = "github:mizchi/pkfire";
    pkfire.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, pkfire, ... }:
    let
      systems = [ "aarch64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      version = "0.1.0";

      cliSource = lib:
        lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.unions [
            ./Cargo.toml
            ./Cargo.lock
            ./codex/suggestions.schema.json
            ./crates
          ];
        };

      pluginSource = lib:
        lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.unions [
            ./codex/suggestions.schema.json
            ./lua
            ./plugin
          ];
        };

      mkCli = pkgs:
        pkgs.rustPlatform.buildRustPackage {
          pname = "key-insights";
          inherit version;
          src = cliSource pkgs.lib;
          cargoLock.lockFile = ./Cargo.lock;
          meta = {
            description = "Deterministic local analyzer for nvim-key-insights";
            homepage = "https://github.com/beso1225/nvim-key-insights";
            mainProgram = "key-insights";
          };
        };

      mkPlugin = pkgs:
        pkgs.vimUtils.buildVimPlugin {
          pname = "nvim-key-insights";
          inherit version;
          src = pluginSource pkgs.lib;
          meta = {
            description = "Privacy-first Neovim key usage collector";
            homepage = "https://github.com/beso1225/nvim-key-insights";
          };
        };

      packagesFor = pkgs:
        let
          cli = mkCli pkgs;
          plugin = mkPlugin pkgs;
        in {
          key-insights = cli;
          nvim-key-insights = plugin;
          default = cli;
        };
    in {
      packages = forAllSystems (system:
        packagesFor (import nixpkgs { inherit system; }));

      apps = forAllSystems (system:
        let
          program = "${self.packages.${system}.key-insights}/bin/key-insights";
          app = {
            type = "app";
            inherit program;
            meta.description = "Run the deterministic key-insights analyzer";
          };
        in {
          key-insights = app;
          default = app;
        });

      overlays.default = final: prev: {
        key-insights = mkCli final;
        vimPlugins = prev.vimPlugins // {
          nvim-key-insights = mkPlugin final;
        };
      };

      checks = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          packages = packagesFor pkgs;
        in {
          inherit (packages) key-insights nvim-key-insights;
          packaged-plugin = pkgs.runCommand "nvim-key-insights-packaged-plugin-check" {
            nativeBuildInputs = [ pkgs.neovim ];
          } ''
            export HOME="$TMPDIR/home"
            export XDG_CACHE_HOME="$TMPDIR/cache"
            export XDG_CONFIG_HOME="$TMPDIR/config"
            export XDG_DATA_HOME="$TMPDIR/data"
            export XDG_STATE_HOME="$TMPDIR/state"
            mkdir -p "$HOME" "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME"
            nvim --headless -u NONE \
              --cmd "set runtimepath^=${packages.nvim-key-insights}" \
              -c "lua require('key-insights').setup(); assert(require('key-insights').status().state == 'stopped'); assert(require('key-insights').status().report_running == false); assert(vim.fn.exists(':KeyInsightsStart') == 2); assert(vim.fn.exists(':KeyInsightsAnalyze') == 2)" \
              -c qa
            test -e "${packages.nvim-key-insights}/codex/suggestions.schema.json"
            test ! -e "$XDG_STATE_HOME/nvim/key-insights"
            test ! -e "$XDG_CACHE_HOME/nvim/key-insights"
            touch "$out"
          '';
        });

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
