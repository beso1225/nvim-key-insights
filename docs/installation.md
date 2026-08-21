# Installation and configuration

## Support boundary

nvim-key-insights requires Neovim 0.10 or newer. The flake currently exports
packages for `aarch64-darwin`, `aarch64-linux`, and `x86_64-linux`.

The Neovim plugin and the Rust analyzer are separate packages. Installing or
loading either package does not start collection, generate a report, or launch
Codex. Install the analyzer explicitly when using `:KeyInsightsReport` or
`:KeyInsightsAnalyze`.

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

## Nix flake packages

Run or install the deterministic analyzer directly:

```sh
nix run github:beso1225/nvim-key-insights#key-insights -- --version
nix profile install github:beso1225/nvim-key-insights#key-insights
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
- `pkgs.vimPlugins.nvim-key-insights` for Neovim.

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
