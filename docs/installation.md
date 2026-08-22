# Installation and configuration

## Support boundary

nvim-key-insights requires Neovim 0.10 or newer. The flake currently exports
packages for `aarch64-darwin`, `aarch64-linux`, and `x86_64-linux`.

The Neovim plugin and the Rust analyzer are separate packages. Installing or
loading either package does not start collection, generate a report, or launch
Codex. Install the analyzer explicitly when using `:KeyInsightsReport` or
`:KeyInsightsAnalyze`.

Before upgrading across a schema change, review the
[schema compatibility and regeneration policy](schema-compatibility.md).
Durable event logs and derived summaries have intentionally different upgrade
paths.

## lazy.nvim

The following specification remains lazy until one of the commands is used:

```lua
{
  "beso1225/nvim-key-insights",
  version = false,
  cmd = {
    "KeyInsightsStart",
    "KeyInsightsPause",
    "KeyInsightsStop",
    "KeyInsightsStatus",
    "KeyInsightsReport",
    "KeyInsightsAnalyze",
    "KeyInsightsOpenReport",
    "KeyInsightsPurge",
  },
  opts = {
    report = {
      analyzer = "key-insights",
    },
  },
}
```

There is no release tag yet. Keep the revision recorded by the plugin-manager
lock file, and review changes before updating it.

After the first release is published, replace `version = false` with an exact
immutable tag such as `version = "v0.1.0"`. Do not mix a tagged Neovim plugin
with an analyzer or Codex plugin from an unrelated revision.

## Nix flake packages

Run or install the deterministic analyzer directly:

```sh
nix run github:beso1225/nvim-key-insights#key-insights -- --version
nix profile install github:beso1225/nvim-key-insights#key-insights
```

After a tag is published, pin it explicitly:

```sh
nix run 'github:beso1225/nvim-key-insights?ref=v0.1.0#key-insights' -- --version
nix profile install 'github:beso1225/nvim-key-insights?ref=v0.1.0#key-insights'
```

The flake also exports `packages.nvim-key-insights` for consumers that assemble
their own Neovim package set. Building it alone does not add it to Neovim:

```sh
nix build github:beso1225/nvim-key-insights#nvim-key-insights
```

## Overlay

Add the flake and apply its stable overlay to the package set used by a NixOS,
nix-darwin, or Home Manager configuration:

```nix
{
  inputs.nvim-key-insights.url = "github:beso1225/nvim-key-insights";

  outputs = { nixpkgs, nvim-key-insights, ... }:
    let
      system = "aarch64-darwin";
      pkgs = import nixpkgs {
        inherit system;
        overlays = [ nvim-key-insights.overlays.default ];
      };
    in {
      packages.${system}.default = pkgs.key-insights;
    };
}
```

The overlay exposes:

- `pkgs.key-insights` for the Rust CLI;
- `pkgs.vimPlugins.nvim-key-insights` for Neovim;
- `pkgs.nvim-key-insights-codex-plugin` for the inert Codex plugin tree.

For example, a Home Manager configuration can install the explicit pair:

```nix
{ pkgs, ... }:
{
  home.packages = [ pkgs.key-insights ];
  programs.neovim.plugins = [ pkgs.vimPlugins.nvim-key-insights ];
}
```

The overlay does not wrap the plugin or inject the CLI path. Keep
`report.analyzer = "key-insights"` when the CLI is on `PATH`, or set an explicit
absolute executable path in Lua configuration. The executable is resolved only
when a report or analysis command is used.

## Optional Codex plugin and standalone skill

The Codex plugin is optional. Installing it does not start Neovim collection,
read any report, invoke a model, send data, or require a plugin-specific API
key. The `key-insights` CLI remains a separate prerequisite for creating the
sanitized preview and validating the result.

Install from the repository marketplace with a current Codex CLI:

```sh
codex plugin marketplace add beso1225/nvim-key-insights
codex plugin add nvim-key-insights@nvim-key-insights
```

There is no release tag yet. These commands therefore track the selected Git
revision; review changes before upgrading the marketplace or plugin cache. To
use only the standalone skill, copy
`plugins/nvim-key-insights/skills/analyze-neovim-usage` into
`$CODEX_HOME/skills/`. That directory is self-contained and does not resolve
resources from the repository.

After a release, install the marketplace from the same immutable tag before
installing the plugin:

```sh
codex plugin marketplace add beso1225/nvim-key-insights@v0.1.0
codex plugin add nvim-key-insights@nvim-key-insights
```

The flake exposes the same inert plugin tree without installing it into Codex:

```sh
nix build github:beso1225/nvim-key-insights#nvim-key-insights-codex-plugin
```

Use the manual skill only with the output of `key-insights preview`:

```sh
chmod 600 summary.json
key-insights preview summary.json --output sanitized-preview.json
# Inspect sanitized-preview.json, then deliberately provide exactly that file
# to $analyze-neovim-usage in Codex Desktop or a new Codex task.
chmod 600 suggestions.json
key-insights suggestions summary.json \
  --input suggestions.json \
  --output codex-suggestions.md
```

Do not provide `summary.json`, collector JSONL, `report.md`, project files, or
dotfiles to the skill. It returns suggestion-schema-v1 JSON only. The output is
not trusted until `key-insights suggestions` binds every evidence value and
collision claim to the exact private summary and optional sanitized snapshot,
then renders Markdown locally.

Manual/Desktop skill invocation and `:KeyInsightsAnalyze` are intentionally
separate entry points. The Neovim-managed subprocess uses an empty working
directory and ignores user configuration and rules, so it does not load the
installed plugin. Conversely, an interactive Codex task may have ambient local
permissions; the skill contract instructs it not to inspect files or use tools
or the network to enrich the supplied preview. Neither entry point broadens the
sanitized payload boundary.

## Configuration reference

Call `require("key-insights").setup()` once. Unknown options are rejected so
misspelled privacy or storage settings cannot silently become no-ops.

| Option | Default | Contract |
| --- | --- | --- |
| `privacy.raw_keylog` | `false` | Must remain `false`; raw logging is not implemented. |
| `privacy.capture_insert_text` | `false` | Must remain `false`; Insert text is reduced to counts and timing. |
| `privacy.capture_command_text` | `false` | Must remain `false`; command contents are discarded. |
| `privacy.capture_search_text` | `false` | Must remain `false`; search contents are discarded. |
| `privacy.store_file_paths` | `false` | Must remain `false`; file paths are not stored. |
| `collection.exclude_special_buffers` | `true` | Must remain `true`; terminal, prompt, nofile, and other special buffers are force-excluded. |
| `collection.max_sequence_keys` | `64` | Integer from 1 through 65,536. |
| `collection.sequence_timeout_ms` | `1000` | Non-negative integer; `0` disables the idle-time boundary. |
| `storage.directory` | `nil` | Non-empty path or `nil` for `stdpath("state")/key-insights/sessions`. |
| `storage.retention.max_age_days` | `30` | Positive integer. |
| `storage.retention.max_sessions` | `100` | Positive integer. |
| `report.analyzer` | `"key-insights"` | Non-empty executable name or path. Passed as argv without a shell. |
| `report.directory` | `nil` | Non-empty path or `nil` for `stdpath("state")/key-insights/reports`. |
| `report.codex.binary` | `"codex"` | Non-empty executable name or path. Used only after preview and confirmation. |
| `report.codex.output_schema` | `nil` | Non-empty path or `nil` for the schema bundled with the plugin. |
| `report.codex.working_directory` | `nil` | Absolute path or `nil` for an owner-only empty cache directory. |

Example with every configurable local bound shown explicitly:

```lua
require("key-insights").setup({
  collection = {
    max_sequence_keys = 64,
    sequence_timeout_ms = 1000,
  },
  storage = {
    directory = vim.fn.stdpath("state") .. "/key-insights/sessions",
    retention = {
      max_age_days = 30,
      max_sessions = 100,
    },
  },
  report = {
    analyzer = "key-insights",
    directory = vim.fn.stdpath("state") .. "/key-insights/reports",
    codex = {
      binary = "codex",
      output_schema = nil,
      working_directory = nil,
    },
  },
})
```

Collection starts only after `:KeyInsightsStart`. Codex starts only after
`:KeyInsightsAnalyze` displays the exact sanitized payload and the user confirms
the subprocess launch.

## Upgrade and rollback

Pin one reviewed Git tag across lazy.nvim, Nix, and the optional Codex
marketplace. Before upgrading, read the
[changelog](../CHANGELOG.md) and
[schema compatibility policy](schema-compatibility.md). Keep finalized private
event logs until the new version has regenerated a valid summary/report pair.

To rollback, restore the prior tag in each package manager and regenerate
derived summaries, reports, payloads, and suggestions. Do not edit a schema
number to make a newer artifact appear compatible. The detailed maintainer and
failure-recovery procedure is in [Release procedure](releasing.md).
