#!/usr/bin/env bash
set -euo pipefail

system="$(nix eval --raw --impure --expr 'builtins.currentSystem')"

test "$(nix eval --no-update-lock-file --raw ".#packages.${system}.key-insights.pname")" = "key-insights"
test "$(nix eval --no-update-lock-file --raw ".#packages.${system}.nvim-key-insights.pname")" = "nvim-key-insights"
test "$(nix eval --no-update-lock-file --raw ".#packages.${system}.nvim-key-insights-codex-plugin.pname")" = "nvim-key-insights-codex-plugin"
test "$(nix eval --no-update-lock-file --raw ".#packages.${system}.default.pname")" = "key-insights"
package_version="$(nix eval --no-update-lock-file --raw ".#packages.${system}.key-insights.version")"

cli_source="$(nix eval --no-update-lock-file --raw ".#packages.${system}.key-insights.src")"
for entry in "${cli_source}"/*; do
  case "$(basename "${entry}")" in
    Cargo.lock|Cargo.toml|codex|crates) ;;
    *) echo "unexpected CLI package source: ${entry}" >&2; exit 1 ;;
  esac
done

plugin_source="$(nix eval --no-update-lock-file --raw ".#packages.${system}.nvim-key-insights.src")"
for entry in "${plugin_source}"/*; do
  case "$(basename "${entry}")" in
    codex|lua|plugin) ;;
    *) echo "unexpected Neovim package source: ${entry}" >&2; exit 1 ;;
  esac
done
test -e "${plugin_source}/codex/suggestions.schema.json"

codex_plugin="$(nix build --no-update-lock-file --no-link --print-out-paths ".#packages.${system}.nvim-key-insights-codex-plugin")"
test -f "${codex_plugin}/.codex-plugin/plugin.json"
test -f "${codex_plugin}/skills/analyze-neovim-usage/SKILL.md"
test -f "${codex_plugin}/skills/analyze-neovim-usage/agents/openai.yaml"
test -f "${codex_plugin}/skills/analyze-neovim-usage/references/payload.schema.json"
test -f "${codex_plugin}/skills/analyze-neovim-usage/references/suggestions.schema.json"
test "$(find "$codex_plugin" -type f | wc -l | tr -d ' ')" = 5
test -z "$(find "$codex_plugin" -type l -print -quit)"
test -z "$(find "$codex_plugin" -type f -perm -111 -print -quit)"

cli_program="$(nix eval --no-update-lock-file --raw ".#apps.${system}.key-insights.program")"
default_program="$(nix eval --no-update-lock-file --raw ".#apps.${system}.default.program")"
test "${cli_program}" = "${default_program}"
case "${cli_program}" in
  */bin/key-insights) ;;
  *) echo "unexpected key-insights app program: ${cli_program}" >&2; exit 1 ;;
esac

test "$(nix eval --no-update-lock-file --raw --impure --expr '
  let
    flake = builtins.getFlake (toString ./.);
    pkgs = import flake.inputs.nixpkgs {
      system = builtins.currentSystem;
      overlays = [ flake.overlays.default ];
    };
  in pkgs.key-insights.pname
')" = "key-insights"

test "$(nix eval --no-update-lock-file --raw --impure --expr '
  let
    flake = builtins.getFlake (toString ./.);
    pkgs = import flake.inputs.nixpkgs {
      system = builtins.currentSystem;
      overlays = [ flake.overlays.default ];
    };
  in pkgs.vimPlugins.nvim-key-insights.pname
')" = "nvim-key-insights"

test "$(nix eval --no-update-lock-file --raw --impure --expr '
  let
    flake = builtins.getFlake (toString ./.);
    pkgs = import flake.inputs.nixpkgs {
      system = builtins.currentSystem;
      overlays = [ flake.overlays.default ];
    };
  in pkgs.nvim-key-insights-codex-plugin.pname
')" = "nvim-key-insights-codex-plugin"

test "$(nix run --no-update-lock-file .#key-insights -- --version)" = "key-insights ${package_version}"
nix run --no-update-lock-file .#key-insights -- --help >/dev/null

for option in \
  privacy.raw_keylog \
  privacy.capture_insert_text \
  privacy.capture_command_text \
  privacy.capture_search_text \
  privacy.store_file_paths \
  collection.exclude_special_buffers \
  collection.max_sequence_keys \
  collection.sequence_timeout_ms \
  storage.directory \
  storage.retention.max_age_days \
  storage.retention.max_sessions \
  report.analyzer \
  report.directory \
  report.codex.binary \
  report.codex.output_schema \
  report.codex.working_directory
do
  grep -Fq "\`${option}\`" docs/installation.md
done

echo "Nix packaging contract: ok"
