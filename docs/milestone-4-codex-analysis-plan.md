# Milestone 4 optional Codex analysis plan

Status: in progress

Roadmap milestone: [Optional Codex analysis](implementation-roadmap.md#milestone-4-optional-codex-analysis)

## Objective

Add an explicitly invoked Codex workflow without making collection or
deterministic analysis depend on a network service. The workflow may consume
only a bounded, previewable payload derived from the already sanitized
`summary.json` and an optional sanitized keymap snapshot.

The default path must remain local-only. Raw JSONL, report Markdown, session
identifiers, project identifiers, paths, buffer names, command/search text,
and mapping implementations must not cross the subprocess boundary.

## Non-goals

- no API-key configuration or provider abstraction in this milestone;
- no automatic analysis after collection or report generation;
- no raw-log or report upload;
- no new raw-key, Insert-text, command-text, or search-text capture;
- no claim that a new mapping is inherently better than learning an existing
  operation.

## Contract decisions

### Payload

The Codex input is a versioned JSON object containing only:

- a payload schema version;
- the deterministic summary schema-v3 object;
- an optional sanitized keymap snapshot;
- explicit instructions that suggestions are evidence-bound and privacy-safe.

The payload is rendered in canonical key order with bounded serialized size.
It contains no Markdown report, source paths, session IDs, project IDs, or
collector JSONL records. The optional snapshot retains only its existing
sanitized mode, scope, canonical left-hand side, and opaque mapping identity.

### Preview gate

Before any process launch, the exact canonical payload is available as a local
preview. The first implementation exposes this as a pure library contract;
the CLI and Neovim command will use the same renderer rather than reconstruct
the payload independently.

### Suggestion contract

Codex output must eventually classify each proposal as one of
`learn_existing`, `add_mapping`, `change_mapping`, or `no_change`, cite one or
more deterministic measurements, and account for keymap collisions. Structured
output validation belongs to a later slice; this milestone does not trust free
form model output.

## TDD slices

### M4-S1: sanitized payload and canonical preview

Status: complete on `codex/codex-analysis-contract`.

- add Red tests for the exact payload shape, deterministic serialization, size
  limits, and omission of forbidden fields;
- introduce a dedicated Rust payload module with a versioned public structure;
- derive the payload from `AnalysisSummary` and the optional sanitized
  `KeymapSnapshot` without accepting raw log or path inputs;
- add seeded-secret regression tests over JSON output and preview text;
- document the boundary and update the roadmap.

### M4-S2: preview command and explicit approval boundary

Status: complete on `codex/codex-analysis-contract`.

- add a CLI command that reads only existing sanitized artifacts and prints the
  exact payload to a caller-selected destination or stdout;
- reject output aliases, unsafe files, and oversized payloads before writing;
- add Neovim command wiring that shows the preview without launching Codex;
- preserve previous reports on every preview failure.

### M4-S3: mocked Codex exec runner

Status: complete on `codex/codex-analysis-contract`.

- invoke `codex exec` only after explicit user confirmation;
- use saved ChatGPT authentication, `--ephemeral`, read-only sandbox, stdin,
  bounded timeout, and a strict output schema;
- pass the preview bytes through stdin and never interpolate user paths into a
  shell command;
- test success, timeout, malformed output, non-zero exit, and output
  publication failure with a mocked executable.

### M4-S4: structured suggestion validation

Status: complete on `codex/codex-analysis-contract`.

- define a bounded suggestion schema and reject unknown fields, unsupported
  action kinds, missing evidence, and collision-blind mapping proposals;
- render Markdown only from validated structured output;
- bind evidence values and collision IDs to the exact deterministic summary and
  keymap snapshot used for the request;
- keep deterministic summary and report artifacts unchanged.

### M4-S5: Neovim analyze workflow

- add `:KeyInsightsAnalyze` as an explicit preview-then-confirm workflow;
- make cancellation, running-process state, and stale output handling explicit;
- display the exact sanitized payload before subprocess launch;
- add headless privacy and lifecycle tests.

### M4-S6: integration, documentation, and adversarial review

- document installation and saved-auth assumptions without requesting API keys;
- exercise the full mocked workflow in CI and keep real Codex invocation opt-in;
- seed secrets, paths, reports, raw JSONL, and mapping implementations and
  assert that none cross the process boundary;
- perform context-light adversarial review focused on privacy, subprocess
  injection, output publication, and misleading recommendations.

## Definition of done

Each slice starts with a failing contract test, is implemented with the
smallest compatible change, and receives a focused adversarial review before
the next slice starts. The complete milestone is done only when the default
local workflow remains network-independent and every Codex payload is
previewable, bounded, structured, and derived exclusively from sanitized
artifacts.
