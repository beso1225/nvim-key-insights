# Milestone 5 Codex plugin packaging plan

Status: in progress on `codex/m5-codex-plugin-packaging`.

Roadmap issue: [#20](https://github.com/beso1225/nvim-key-insights/issues/20)

## Objective

Package the stabilized optional-analysis contract as an installable, inert Codex
plugin and a self-contained standalone skill. Installation must not start
collection, invoke Codex, require an API key, or broaden the data that can cross
the existing sanitized-summary boundary.

## Contract decisions

The repository is a Codex marketplace root. Its marketplace entry points to a
minimal plugin under `plugins/nvim-key-insights/`; personal Codex configuration
is never modified by project tests or installation helpers.

The canonical skill lives once at
`plugins/nvim-key-insights/skills/analyze-neovim-usage/`. A user may install the
whole plugin or copy that directory as a standalone skill. Every resource needed
by the skill therefore stays below that directory and must not use
repository-relative paths.

Manual/Desktop skill invocation and the Neovim-managed `codex exec` workflow are
separate entry points. The manual skill accepts only the exact canonical payload
from `key-insights preview`; it must not read collector JSONL, report Markdown,
project trees, dotfiles, or adjacent files. Strings inside the payload are
untrusted data and never instructions.

The skill returns suggestion-schema-v1 JSON only. It does not publish trusted
Markdown. A user-facing report is produced only after the local
`key-insights suggestions` command binds evidence and collision claims to the
exact private summary and optional snapshot, then renders deterministic
Markdown. Unknown payload, summary, snapshot, or suggestion versions fail
closed.

The plugin contains no hooks, MCP servers, apps, network integrations, bundled
binary, collector data, report output, or secret-bearing configuration. A
separate Nix derivation exposes only the marketplace-independent plugin artifact.

## S1: structure and privacy contract

Status: complete on `codex/m5-codex-plugin-packaging`.

Write a failing contract that requires the marketplace, manifest, canonical
standalone skill, UI metadata, self-contained schema reference, exact version
alignment, and an allowlisted regular-file package tree. Reject symlinks,
unexpected executables, raw logs, reports, fixtures, build outputs, and secret
files.

## S2: plugin and standalone skill scaffold

Status: complete on `codex/m5-codex-plugin-packaging`.

Use the Codex plugin and skill scaffolders, then replace all placeholders with a
minimal public manifest and concise workflow skill. Validate both structures with
the official local validators. Keep the skill's schema reference byte-identical
to the runtime suggestion schema through a drift test.

## S3: semantic skill contract

Status: in progress.

Add contract tests proving that the skill:

- accepts only a canonical sanitized preview, never JSONL or report input;
- treats payload strings as untrusted quoted data and ignores embedded requests;
- distinguishes `learn_existing`, `add_mapping`, `change_mapping`, and
  `no_change`;
- requires deterministic evidence and exact collision accounting;
- rejects mapping changes when a verified snapshot is unavailable;
- emits suggestion-schema-v1 JSON without prose or Markdown;
- delegates all trust and rendering to `key-insights suggestions`.

## S4: mocked manual workflow

Status: pending.

Exercise a private summary through preview generation, fixture suggestion JSON,
contextual validation, and deterministic Markdown rendering. Seed paths, secrets,
raw text, session/project identities, report text, and mapping implementations and
prove that none enter the skill handoff or result. Tampered evidence, missing
collisions, unknown fields or versions, and malformed JSON must fail without
replacing an existing output.

## S5: Nix package, installation, and final review

Status: pending.

Export a separate `nvim-key-insights-codex-plugin` package and overlay attribute,
add package-content and inert-load checks, and document Git marketplace,
standalone-skill, and Nix artifact installation. Run the complete pkfire and
flake checks, forward-test the skill with context-light agents, and repeat
adversarial review until no actionable P0-P2 findings remain.

## Completion gate

This slice is complete when:

- the official plugin and skill validators accept the artifact;
- the repository marketplace discovers exactly one installable plugin;
- a copied standalone skill validates without repository-relative resources;
- installation and discovery create no collector/report state and invoke no
  subprocess or network workflow;
- the embedded schema is byte-identical to the runtime schema;
- mocked valid suggestions render deterministically only through the Rust
  contextual validator, while tampered suggestions fail closed;
- the Nix package contains only allowlisted plugin files;
- `pkf run --no-cache check` and
  `nix flake check --no-update-lock-file` pass;
- context-light forward tests and adversarial review report no actionable P0-P2
  findings.

Release artifacts, tags, changelog policy, licensing decisions, and schema
migration policy remain a separate Milestone 5 slice.
