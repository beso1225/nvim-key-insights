# Milestone 3 deterministic ergonomic metrics plan

Status: in progress

Roadmap milestone: [Deterministic ergonomic metrics](implementation-roadmap.md#milestone-3-deterministic-ergonomic-metrics)

## Objective

Turn the existing privacy-sanitized event stream into bounded, explainable
ergonomic evidence. The analyzer must remain deterministic and useful without
AI. It may identify observations and cautious candidates, but it must not claim
that a mapping or workflow change is automatically better.

Milestone 3 uses schema-v1 collector events only. It does not add raw key logs,
per-keystroke timestamps, file paths, buffer names, project identifiers, command
or search contents, or mapping implementations.

## Contract decisions

### Summary schema

Adding ergonomic metrics changes both generated artifacts. All Milestone 3
outputs use summary schema version 3, with or without a keymap snapshot. Event
schema version 1 remains unchanged. The migration is intentional and must be
covered by exact JSON and Markdown fixtures.

The ergonomic section contains only aggregate counts, fixed histograms, bounded
rankings, thresholds, sample guards, and stable candidate identifiers. It must
not contain session IDs or raw sequences.

### Observation boundary

Metrics may use:

- complete session duration from `session_end`;
- canonical typed keys and aggregate duration from `key_sequence`;
- aggregate Insert/Replace/Select counts from `text_run`;
- normalized `from` and `to` values from `mode_transition`;
- opaque mapping IDs and the optional sanitized snapshot.

Mapping events duplicate typed-key evidence already present in key sequences, so
operation and motion metrics use `key_sequence` only. Session and sequence
boundaries are hard boundaries: no run, prefix, latency sample, or candidate may
span them.

### Fixed aggregate distributions

Raw timing samples are never emitted. The summary records counts in versioned,
fixed buckets:

- session duration: `0-1s`, `1-10s`, `10-60s`, `1-5m`, and `over-5m`;
- sequence length: `1`, `2`, `3-4`, `5-8`, `9-16`, `17-32`, and `33-plus`;
- average sequence inter-key latency for sequences with at least two keys:
  `0-50ms`, `50-100ms`, `100-250ms`, `250-500ms`, and `over-500ms`.

Bucket boundaries are lower-inclusive and upper-exclusive except for the first
zero boundary and the final open-ended bucket. Average latency uses integer
division of `duration_ms / (keys - 1)` and never reconstructs a per-key timeline.

### Operation evidence

Versioned token sets count only canonical Normal/Visual/Operator-pending tokens:

- undo: `u`;
- redo: `<C-R>`;
- repeat: `.`;
- search start: `/` and `?`;
- search navigation: `n`, `N`, `*`, and `#`.

These are observations, not command-semantic reconstruction. Mapping use is
reported separately and is not double-counted as a built-in operation.

### Counts and repeated motions

A count prefix is a non-zero ASCII digit followed by zero or more ASCII digits
and then a member of a versioned countable-operation token set. The analyzer
records prefix occurrences and digit presses but does not retain the numeric
value. A zero without an earlier non-zero digit remains a motion, not a prefix.

A repeated-motion opportunity is a consecutive run of at least three identical
tokens from a conservative directional-motion set. It must be contained in one
key sequence. The summary aggregates the motion token, run count, and presses;
rankings use descending run count, descending press count, then lexical token.

### Candidate guards

Candidates are deterministic evidence records, not recommendations. Every
candidate includes its versioned kind, measurements, and the guard that made it
eligible. Initial guards require all of:

- at least 3 complete sessions;
- at least 100 sequence keys in the complete sample;
- at least 3 qualifying observations for the candidate.

Insufficient samples still produce aggregate metrics but no candidate. Mapping
underuse evidence additionally requires a sanitized snapshot and never labels a
mapping as useless; it reports only that the current bounded sample did not
observe it. For this candidate kind, `observations` means the number of complete
sampled sessions, while measurements record `observed_uses = 0`. It does not
claim that the currently snapped mapping existed throughout those sessions.
The three-observation guard therefore requires three complete sampled sessions,
not three invented absence events.

### Bounds and arithmetic

- Histogram and operation fields have fixed cardinality.
- Ranked ergonomic token tables use the existing 100-row output cap and shared
  retained-token budget.
- Candidate output is capped at 100 rows with deterministic ordering.
- Counters use checked arithmetic where input-controlled totals could overflow;
  invalid totals fail before output publication.
- Thresholds and token sets are serialized with a version so identical inputs
  cannot silently change meaning after an implementation update.

## Deferred work

- Filetype distribution requires a separate privacy review and collector schema
  decision; it is not part of this milestone.
- Raw inter-key latency and per-keystroke timelines remain out of scope.
- Command/search contents remain unavailable and must not be inferred.
- Ergonomic recommendations belong to the optional Codex milestone. The local
  analyzer emits evidence only.

## TDD slices

### M3-S1: summary-v3 contract and module boundary

- Add exact Red fixtures for schema version 3 and an empty ergonomic section.
- Introduce a dedicated Rust ergonomics module with versioned thresholds, token
  sets, bounded public structures, and deterministic rendering order.
- Preserve event schema version 1 and strict JSONL validation.

### M3-S2: session and sequence distributions

- Add boundary fixtures for every duration, length, and average-latency bucket.
- Aggregate session duration and sequence metrics without retaining samples.
- Prove checked arithmetic and byte-identical multi-input output.

### M3-S3: operation and transition counts

- Count undo, redo, repeat, search-start, and search-navigation evidence.
- Aggregate normalized mode-transition pairs in lexical order.
- Prove that mapping events do not double-count typed-key operations.

### M3-S4: count-prefix evidence

- Add threshold and ambiguous-zero fixtures for Normal, Visual, and
  Operator-pending sequences.
- Keep prefix parsing within individual sequences and emit no numeric values or
  raw sequences.

### M3-S5: repeated-motion opportunities and guards

- Recognize conservative repeated motions of length three or more.
- Apply session, sequence-key, and observation guards.
- Add session-boundary, mode-boundary, ranking, cardinality, and escaping tests.

### M3-S6: mapping coverage evidence

- Derive cautious observed/unobserved coverage from schema-v2 snapshot joins.
- Apply the same sample guards and keep collision reporting unchanged.
- Prove that no short sample produces an underuse candidate.

### M3-S7: integration, documentation, and privacy regression

- Update exact JSON/Markdown fixtures, CLI documentation, roadmap status, and
  the headless local workflow.
- Seed sensitive values and prove their absence from every new field.
- Run callback, analyzer resource, Nix, Neovim 0.10, and context-light
  adversarial reviews before completion.

## Completion gate

For identical validated inputs, the analyzer produces byte-identical schema-v3
JSON and Markdown containing bounded ergonomic aggregates. Every emitted
candidate cites measurements and its sample guard, no candidate crosses a
session or sequence boundary, and no new sensitive collector field is required.
