# Milestone 6 command-surface privacy E2E plan

Status: in progress on `codex/m6-workflow-privacy-e2e`.

Roadmap issue: [#25](https://github.com/beso1225/nvim-key-insights/issues/25)

## Objective

Exercise the supported Neovim command surface from collection through local
reporting and optional mocked Codex analysis in isolated child processes. The
completion gate must prove that lifecycle, recovery, and privacy invariants hold
across component boundaries without invoking a real AI service or using the
network.

This slice extends existing focused tests. It does not replace the collector,
storage, purge, report, or Rust publication suites with one large scenario.

## Existing coverage and missing seams

The repository already has strong component coverage for collector state
transitions, storage durability and retention, purge races, paired report
publication, Codex payload and suggestion validation, callback cost, and a
collector-to-report privacy scenario. The current E2E constructs collector and
report objects directly, however, and therefore does not prove the singleton
plugin wiring, public commands, `VimLeavePre`, confirmation UI, real process
runner, or command-level purge behavior.

The first threat review also found that a configured Codex executable currently
inherits the complete Neovim environment. Codex tool configuration limits the
environment inherited by tools that Codex starts; it does not sanitize the
environment delivered to the Codex process itself. M6-A must close that boundary
before adding a happy-path subprocess E2E.

Cross-platform CI expansion and performance or resource budgets remain the
second Milestone 6 slice. Real usage, threshold tuning, and release approval
remain Milestone 7 work.

## Boundary model

Tests use an isolated temporary `HOME`, `XDG_STATE_HOME`, `XDG_CACHE_HOME`, and
`XDG_CONFIG_HOME`. They may launch only the built analyzer, child Neovim
processes, and repository-owned mock executables. They must not contact the
network or read the developer's Neovim or Codex configuration.

Canaries are boundary-specific:

- collector JSONL keeps a local session boundary but must not contain paths,
  text-bearing input, mapping right-hand sides, callbacks, descriptions, or
  source metadata;
- deterministic summaries and reports must additionally omit session and
  project identities;
- preview bytes, Codex stdin, structured suggestions, rendered Markdown,
  diagnostics, and subprocess environments must contain only their explicit
  allowlisted data;
- status output may display the active local session identity and is not treated
  as an AI-boundary artifact.

Raw and decoded JSON checks cover plain, escaped, Unicode, and path-shaped
canaries. Exact-string scanning alone is not the semantic privacy oracle.

## S1: subprocess environment boundary

Status: complete.

- add a failing real-process test that seeds an unrelated credential-like
  environment canary and records the environment received by a mock Codex
  executable;
- define the minimum environment allowlist needed to locate and run the
  configured executable while reusing saved Codex authentication;
- launch the Codex process with an explicit environment instead of inheriting
  the Neovim process environment;
- keep analyzer execution local and separately document its environment
  boundary;
- cover synchronous callbacks, timeout cleanup, and diagnostics without
  exposing removed values.

## S2: public command lifecycle and normal shutdown

Status: pending.

- load the plugin with `require("key-insights").setup` in an isolated child
  Neovim;
- exercise real `:KeyInsightsStart`, pause, resume, status, stop, report, and
  open-report commands;
- feed real Neovim input and prove paused input is not collected;
- exit separate recording and paused child processes normally and verify that
  `VimLeavePre` produces one private finalized log with one session end and no
  remaining partial or lock artifact;
- analyze the finalized sessions with the built CLI.

## S3: crash, retention, and public purge recovery

Status: pending.

- terminate a child collector without `VimLeavePre` and retain its private
  partial artifact and stale lock;
- prove analyzer discovery ignores incomplete collector artifacts;
- run non-bang purge cancellation followed by `:KeyInsightsPurge!` through the
  public command surface;
- retain active, live-locked, malformed or unknown, unsafe, report, and unrelated
  artifacts while removing only stale collector-owned targets;
- add final-check-to-unlink mutation regressions for purge and retention so a
  replaced leaf is never deleted;
- exercise deterministic age and count retention during public finalization.

## S4: mocked Codex command E2E and canary matrix

Status: pending.

- generate the source sessions through the public lifecycle from S2;
- run real `:KeyInsightsReport` and `:KeyInsightsAnalyze` commands;
- replace only the confirmation UI and point the configured Codex binary at a
  repository-owned mock executable;
- capture the exact preview, process arguments, allowlisted environment, and
  standard input, then return schema-valid structured suggestions;
- pass the response through the Rust contextual validator and deterministic
  Markdown renderer;
- scan every persisted, displayed, diagnostic, and AI-boundary artifact using
  the boundary-specific canary rules;
- prove confirmation precedes launch and cancellation launches no Codex process.

## S5: failure publication and fixture refactor

Status: pending.

- preserve a known-good report pair across analyzer failure, timeout, malformed
  or oversized output, and stale or one-sided publication;
- exercise Codex timeout, descendant cleanup, invalid suggestions, renderer
  failure, late callbacks, and shutdown through the command workflow;
- block report, analyze, reconfiguration, and purge mutations during every
  incompatible running phase, then prove recovery after completion or cancel;
- share isolated-process, canary, and mock-executable fixtures without copying
  production validators into tests;
- include every helper and fixture in the corresponding pkfire task inputs.

## S6: completion

Status: pending.

- update public workflow documentation to describe the command-surface E2E
  accurately;
- run `pkf run --no-cache check` in the Nix development shell;
- run `nix flake check --no-update-lock-file`;
- perform a context-light adversarial review after every slice and a final review
  of the complete branch;
- repair every actionable P0-P2 finding before opening the pull request.

## Completion gate

One reproducible command must validate the default command workflow, normal and
crashed shutdown behavior, retention and purge boundaries, deterministic local
reporting, and optional mocked Codex analysis without network access. No seeded
private value may cross a boundary where the product contract forbids it.
