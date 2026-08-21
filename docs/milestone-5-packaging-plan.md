# Milestone 5 packaging and integration plan

Status: in progress on `codex/m5-packaging-integration`.

## Scope

This pull request establishes the first stable installation surface for the
existing collector, analyzer, and optional Codex workflow. It does not change
event or summary schemas and does not add any automatic collection or AI
invocation.

Included:

- fail-closed validation for privacy configuration that is not implemented as
  an explicit opt-in;
- a Nix flake package and app for the Rust `key-insights` CLI;
- a separate Nix package for the Neovim plugin;
- an overlay exposing both packages without coupling this repository to a
  downstream dotfiles layout;
- lazy.nvim, direct flake, and overlay installation documentation;
- a complete reference for the currently supported Lua configuration;
- package-level tests proving that loading the plugin does not start collection
  or launch analysis.

Deferred to later Milestone 5 pull requests:

- installable Codex plugin packaging and its standalone `SKILL.md` contract;
- release artifacts, changelog policy, versioning, and schema upgrade policy;
- downstream nix-dotfiles changes.

## S1: privacy configuration contract

Status: complete on `codex/m5-packaging-integration`.

Write failing headless tests proving that unsupported sensitive-capture flags
cannot be enabled and that special-buffer exclusion cannot be disabled. Make
configuration resolution reject those values explicitly instead of accepting
no-op or privacy-weakening settings.

## S2: Rust CLI package and app

Status: complete on `codex/m5-packaging-integration`.

Write a failing flake-output contract, then export `packages.key-insights`,
`packages.default`, `apps.key-insights`, and `apps.default` for every supported
system. Build only the Cargo workspace sources required by the CLI.

## S3: Neovim package and overlay

Status: complete on `codex/m5-packaging-integration`.

Package only `lua/` and `plugin/` as `packages.nvim-key-insights`. Export an
overlay with `pkgs.key-insights` and `pkgs.vimPlugins.nvim-key-insights`, using
local constructors rather than recursively referring to `self.packages`.

## S4: installation and configuration documentation

Status: complete on `codex/m5-packaging-integration`.

Document lazy.nvim command triggers, direct flake packages/apps, overlay usage,
supported systems, the explicit CLI/plugin separation, and every supported
configuration field and default. Do not describe raw capture as available.

## S5: packaged integration and final review

Status: in progress.

Add a flake check that loads the packaged plugin in headless Neovim, verifies
the commands exist, and confirms the collector remains stopped without any
report process. Verify package contents and executable wiring, run the full
project and flake checks without lock-file changes, then perform a context-light
adversarial review of the complete branch.

## Completion gate

This slice is complete when:

- `nix build --no-update-lock-file .#key-insights` succeeds;
- `nix build --no-update-lock-file .#nvim-key-insights` succeeds;
- `nix run --no-update-lock-file .#key-insights -- --help` reaches the packaged
  executable;
- the overlay evaluates and exposes both stable attributes;
- the packaged plugin loads headlessly without starting collection or analysis;
- `pkf run --no-cache check` and `nix flake check --no-update-lock-file` pass;
- a context-light adversarial review reports no remaining actionable P0-P2
  findings.
