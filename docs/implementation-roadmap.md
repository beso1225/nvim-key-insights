# Implementation roadmap

This document records the planned work after the deterministic reporting
foundation merged into `main`. It is an execution plan, not a promise of release
dates. Each milestone should be delivered through small pull requests using the
explore, Red, Green, and Refactor cycle.

## Product boundary

The finished tool should let a user:

1. collect ordinary Neovim interaction without recording text-bearing input;
2. analyze one or more finalized local sessions deterministically;
3. inspect `summary.json` and `report.md` locally;
4. optionally ask Codex for evidence-backed ergonomic suggestions using only the
   previewed sanitized summary.

AI services must remain optional. Collection and deterministic analysis must be
fully useful without network access, an API key, or a Codex installation.

## Current baseline

The following foundation is complete:

- an opt-in Neovim 0.10+ collector with explicit start, pause, stop, and status
  commands;
- durable session boundaries and private `.jsonl.part` to `.jsonl`
  publication;
- forced exclusion of special and sensitive buffers;
- Normal, Visual, and Operator-pending sequence aggregation;
- content-free Insert, Replace, and Select text-run aggregation;
- bounded retention of finalized sessions;
- a bounded streaming schema-v1 validator and deterministic multi-input analyzer;
- bounded finalized-session discovery, asynchronous local report commands, and
  explicit collector-artifact purge;
- deterministic `summary.json` and `report.md` generation;
- failure-atomic paired output publication, cross-process locking, crash
  recovery, and bounded staged-output scavenging;
- reproducible Nix, pkfire, Rust, Lua, and GitHub Actions checks.

The current collector discards mapping expansions and attributes only a
fail-closed subset of typed actions to opaque mapping identities. It never
serializes mapping-applied values or implementation metadata.
The analyzer accepts ordered explicit inputs and bounded discovery of the
collector's finalized session directory. The complete local workflow has a
headless privacy regression. Richer metrics, Codex integration, and installation
surfaces are described below.

## Milestone 1: complete the local analysis workflow

Status: complete.

Make the existing collector and analyzer convenient to use together before
adding new metrics.

### Scope

- Define deterministic discovery and ordering for multiple finalized session
  files, either through repeated `--input` arguments, a session directory, or a
  manifest. Never include `.jsonl.part`, lock files, symlinks, or unrelated
  entries.
- Stream sessions without concatenating them into an unbounded in-memory buffer.
- Add `:KeyInsightsReport`, `:KeyInsightsOpenReport`, and
  `:KeyInsightsPurge` around the local deterministic workflow. Reserve
  `:KeyInsightsAnalyze` for the optional Codex workflow so the command does not
  change meaning after installation.
- Keep purge scope restricted to collector-owned artifacts and make the selected
  targets visible before deletion. Do not make raw-log deletion an implicit side
  effect of analysis.
- Define configuration for analyzer discovery and output locations without
  storing those paths in events or summaries.
- Improve actionable error reporting for missing binaries, invalid sessions,
  partial failures, and output publication failures.

### Red tests

- Multiple finalized files aggregate in a stable order and produce identical
  artifacts across repeated runs.
- Incomplete, non-regular, aliased, and unrelated directory entries are ignored
  or rejected according to the documented input contract.
- Neovim commands build the expected argument vector, preserve existing reports
  on failure, and never shell-interpolate user-controlled paths.
- Purge cannot escape the configured session directory and cannot follow links.

### Completion gate

A user can collect several sessions and produce or open one deterministic report
without manually concatenating files.

## Milestone 2: mapping attribution and keymap snapshots

Status: complete. See the
[detailed implementation plan](milestone-2-mapping-attribution-plan.md).

Measure mapping use without persisting mapping right-hand sides, callback bodies,
source files, or other potentially sensitive implementation details.

### Contract work

- Specify a stable opaque mapping identity derived from non-sensitive mapping
  metadata. Preserve schema-v1 compatibility; introduce a new schema version only
  if the event contract cannot be extended compatibly.
- Define a sanitized keymap snapshot format containing only fields required for
  collision and usage analysis, such as mode, canonical left-hand side, scope,
  and opaque identity.
- Document buffer-local mapping behavior and the limits of attributing mappings
  that are changed during a session.

### Implementation work

- Inspect actual mappings through Neovim APIs, including global and relevant
  buffer-local mappings, instead of scanning dotfiles with regular expressions.
- Use both values supplied to `vim.on_key` only in memory to attribute a typed
  action. Never serialize the mapping-applied value.
- Emit `mapping_use` with an opaque mapping ID and privacy-safe typed keys.
- Let the analyzer accept a sanitized keymap snapshot separately from event logs.
- Detect existing mapping collisions before suggesting new bindings and report
  observed, underused, and unobserved mappings without treating a short sample as
  proof that a mapping is useless.

### Red tests

- Dynamic, recursive, non-recursive, and buffer-local mappings are attributed
  without exposing their right-hand sides.
- Mapping expansions that contain commands, paths, or inserted text never enter
  JSONL, summaries, reports, fixtures, or failure messages.
- Snapshot ordering and opaque identities are deterministic.
- Mapping changes and ambiguous prefixes fail closed rather than being assigned
  to the wrong mapping.

### Completion gate

Reports can distinguish typed operations from known mapping use and can evaluate
keymap collisions using an API-derived sanitized snapshot.

## Milestone 3: deterministic ergonomic metrics

Status: complete. See the
[detailed implementation plan](milestone-3-deterministic-ergonomic-metrics-plan.md).

Add metrics only when they support an explainable recommendation and remain
stable for identical inputs.

### Initial metrics

- repeated motions and repeated single-key runs;
- count-prefix use and opportunities where repeated motions may indicate a count;
- long or high-latency Normal/Visual/Operator-pending sequences;
- undo, redo, repeat, and search invocation counts without search contents;
- mode-transition patterns;
- mapping coverage and cautious underuse candidates;
- session length distributions.

Latency outputs are bounded histograms, not raw per-keystroke timelines.
Filetype collection remains deferred pending an explicit privacy review and
must remain free of paths and buffer names. Project identity remains optional,
local, and anonymous.

### Analysis rules

- Version thresholds and algorithms in the summary contract.
- Include a sample-size or confidence guard with every candidate.
- Separate observations from recommendations. The deterministic analyzer may
  identify evidence, but it must not claim that a new mapping is automatically
  better.
- Keep rankings bounded with deterministic tie-breaking.

### Red tests

- Fixtures cover session boundaries, mode boundaries, count prefixes, repeated
  motions, and threshold edges.
- Concatenating sessions never creates a false cross-session sequence.
- Equivalent inputs produce byte-identical JSON and Markdown.
- High-cardinality and duration arithmetic remain bounded and checked.

### Completion gate

Every report candidate cites deterministic measurements and has enough context
for a human or Codex to choose among learning an existing operation, adding a
mapping, or making no change.

## Milestone 4: optional Codex analysis

Status: in progress. The sanitized payload contract, CLI preview, and
`:KeyInsightsAnalyze` scratch-buffer preview are complete; see the
[detailed implementation plan](milestone-4-codex-analysis-plan.md).

Add the AI boundary only after the sanitized summary contract and deterministic
evidence are stable.

### Skill

- Add an `analyze-neovim-usage` skill that reads `summary.json` and an optional
  sanitized keymap snapshot.
- Require each suggestion to cite measured evidence and classify its action as
  `learn_existing`, `add_mapping`, `change_mapping`, or `no_change`.
- Require collision checks and ergonomics reasoning. More mappings must not be
  treated as an inherently better outcome.
- Produce structured JSON conforming to a checked schema and a Markdown
  explanation derived from that structure.
- Keep `:KeyInsightsAnalyze` as the local preview gate; add subprocess
  invocation only after explicit confirmation in a later slice.

### CLI automation

- Invoke `codex exec` with saved ChatGPT authentication; do not request an API
  key.
- Use ephemeral execution and a read-only sandbox.
- Send only the previewed sanitized summary through standard input.
- Use an output schema, bounded execution time, explicit binary discovery, and
  atomic local result publication.
- Test the runner against a mocked Codex executable. Real Codex invocations are
  opt-in integration tests and must never run in ordinary CI.

### Privacy gate

Before process launch, show the exact payload or its canonical local preview.
Tests must prove that JSONL logs, session IDs, project IDs, paths, report contents,
and keymap right-hand sides cannot cross the subprocess boundary by default.

### Completion gate

The optional workflow works with saved Codex authentication and stable structured
output while the local analyzer remains independent of Codex.

## Milestone 5: packaging and integration surfaces

- Export the Rust CLI as a flake package and app for supported systems.
- Document lazy.nvim installation and all user-facing configuration.
- Provide a stable Nix module or overlay surface suitable for integration from
  the user's nix-dotfiles without coupling this repository to those dotfiles.
- Package the Codex skill as an installable Codex plugin only after its file and
  output contracts stabilize.
- Define release artifacts, versioning, changelog policy, and an upgrade path for
  event and summary schemas.

The package must not auto-start collection, auto-send data, or add raw capture.

## Milestone 6: end-to-end and privacy regression coverage

- Run a headless Neovim collection session through finalization and Rust analysis
  in a temporary isolated state directory.
- Exercise pause/resume, Neovim shutdown, interrupted writes, retention, mapping
  attribution, report publication, and optional mocked Codex analysis.
- Search every boundary artifact for seeded secrets, paths, Insert text,
  commands, searches, and mapping right-hand sides.
- Keep filesystem race and crash-recovery tests on both Linux and macOS CI where
  platform behavior differs.
- Add performance budgets for callback work, streaming analyzer memory, large
  finalized-session directories, and bounded cleanup.

### Completion gate

One reproducible command validates the complete default workflow and its privacy
invariants without network access.

## Milestone 7: forward testing and release readiness

- Collect real local usage with the privacy defaults and manually inspect JSONL,
  summary, report, and Codex payload boundaries.
- Tune deterministic thresholds using representative sessions without encoding
  one user's habits as universal rules.
- Measure collector overhead and analyzer resource use.
- Perform another context-light adversarial review focused on privacy, data loss,
  concurrency, and misleading recommendations.
- Publish an initial release only after documentation, upgrade behavior, support
  boundaries, and purge/recovery procedures are complete.

## Planned order and dependencies

| Order | Milestone | Depends on |
| --- | --- | --- |
| 1 | Local analysis workflow | Current collector and analyzer |
| 2 | Mapping attribution and snapshots | Stable multi-session workflow |
| 3 | Ergonomic metrics | Mapping and event contracts |
| 4 | Codex analysis | Stable sanitized summary and evidence |
| 5 | Packaging and integration | Stable commands and AI contracts |
| 6 | End-to-end privacy coverage | All integration surfaces |
| 7 | Forward testing and release | Complete default workflow |

Milestones may be split into smaller pull requests, but later work should not
bypass an earlier contract dependency.

## Cross-cutting definition of done

Every implementation pull request must:

- begin with a failing contract or regression test;
- preserve privacy-first defaults and deterministic local behavior;
- document public schema or command changes in English;
- preserve existing event-schema consumers or provide an explicit versioned
  migration;
- run `pkf run --no-cache check` inside the Nix development shell;
- run `nix flake check` without rewriting the lock file;
- include a self-review of changed data boundaries and failure paths;
- receive a context-light adversarial review before merge when the change affects
  privacy, recovery, subprocess execution, or public schemas.

## Decisions to make before implementation

The following choices should be resolved in their owning milestone, with the
decision and rationale added to this document or a dedicated design note:

1. the multi-session CLI input syntax and deterministic discovery rules;
2. synchronous versus asynchronous Neovim command behavior and cancellation;
3. whether new timing or filetype events require event schema version 2;
4. the summary-schema migration required by snapshot-derived output;
5. the exact underuse thresholds and minimum observation window;
6. Codex executable discovery, timeout, output locations, and retry behavior;
7. the installation boundary between the repository, nix-dotfiles, and a future
   installable Codex plugin.

Raw key logging and capture of Insert, Command, or Search contents are outside the
MVP. Adding any of them would require a separate explicit opt-in design, threat
model, retention policy, user-visible warning, and privacy regression suite.
