# Milestone 2 mapping attribution implementation plan

Status: in progress (M2-S1 through M2-S3 implemented; M2-S3 review in progress)

Roadmap milestone: [Mapping attribution and keymap snapshots](implementation-roadmap.md#milestone-2-mapping-attribution-and-keymap-snapshots)

Tracking issue: [#11](https://github.com/beso1225/nvim-key-insights/issues/11)

## Objective

Attribute observed Normal, Visual, and Operator-pending input to effective
Neovim mappings and provide the deterministic analyzer with a sanitized snapshot
of the mappings that may be relevant to those observations.

The implementation must not persist mapping right-hand sides, callback bodies,
descriptions, source locations, buffer numbers, buffer names, or file paths.
Attribution is supplemental evidence: typed input continues to contribute to
`key_sequence`, while a confirmed mapping also produces one `mapping_use` event.

## User-visible outcome

After this milestone, a user can:

1. collect opaque mapping-use counts alongside typed-key sequences;
2. inspect a deterministic, API-derived snapshot containing only mapping left-hand
   sides, modes, scopes, and opaque identities;
3. generate a local report that distinguishes observed mappings from mappings
   present only in the snapshot;
4. use collision evidence without exposing mapping implementations;
5. continue using old schema-v1 logs and run analysis without a snapshot.

This milestone does not recommend new mappings, assign ergonomic scores, collect
filetypes, or invoke Codex.

## Contract decisions

### Event schema v1 remains the collection boundary

The existing `mapping_use` event already contains the required fields:

```json
{
  "schema_version": 1,
  "event_type": "mapping_use",
  "session_id": "opaque-session-id",
  "elapsed_ms": 42,
  "mode": "normal",
  "mapping_id": "mapping-v1:<sha256>",
  "typed_keys": ["<Space>", "f"]
}
```

No new event field is planned. `typed_keys` uses the same canonical token policy
as `key_sequence`, and `mapping_use` is emitted only for non-text-bearing modes.
The mapping-applied value supplied by `vim.on_key` is callback-local evidence
only: it must not be canonicalized for storage, retained, hashed, formatted into
errors, or passed to another module as content.

### Mapping identity describes a key binding, not its implementation

`mapping_id` is the lowercase SHA-256 digest of a domain-separated,
length-prefixed canonical tuple containing:

- identity format version `mapping-v1`;
- normalized mode;
- scope, either `global` or `buffer`;
- canonical left-hand-side key tokens.

The serialized ID is `mapping-v1:<64 lowercase hexadecimal digits>`. The
preimage excludes the right-hand side, callback, description, script ID, source
line, buffer ID, filetype, and path. Length-prefixing is required so different
tuples cannot produce the same preimage through delimiter ambiguity.

Buffer-local mappings with the same mode and left-hand side intentionally share
an identity across buffers. This avoids creating a persistent buffer identifier
and treats them as the same user-facing binding for coverage purposes. Collision
analysis must therefore report that a buffer-local collision exists in at least
one observed context; it must not claim that the mapping exists in every buffer.

### The snapshot is a separate, bounded local input

The initial snapshot format is a JSON object with its own version and a bounded
list of sanitized mappings:

```json
{
  "snapshot_version": 1,
  "mappings": [
    {
      "mapping_id": "mapping-v1:<sha256>",
      "mode": "normal",
      "scope": "global",
      "lhs": ["<Space>", "f"]
    }
  ]
}
```

Entries are deduplicated by their complete sanitized tuple and sorted by mode,
canonical left-hand side, scope, then mapping ID using bytewise ordering. The
format contains no capture time, Neovim process identity, buffer identity, or
source metadata. Global mappings come from `nvim_get_keymap`; buffer-local
mappings come from `nvim_buf_get_keymap` for currently loaded, valid,
non-sensitive buffers only. The collector never opens a buffer merely to inspect
its mappings.

Snapshot collection is bounded by an explicit maximum number of inspected
buffers, API entries, canonical key tokens, and encoded bytes. Exceeding a bound
fails without publishing a partial snapshot. Snapshot files use private
permissions and failure-atomic replacement under the existing state directory.

### Snapshot lifetime is point-in-time and analysis is conservative

The snapshot represents mappings visible when `:KeyInsightsReport` starts. It is
not historical proof that a mapping existed for an entire session. Consequently:

- an observed mapping ID absent from the snapshot is reported as
  `observed_not_in_snapshot`, not discarded;
- a snapshot mapping with no matching event is `unobserved_in_sample`, never
  called unused;
- a global and buffer-local entry with the same mode and left-hand side is a
  potential shadowing collision;
- mappings added, removed, or changed during collection may reduce attribution
  coverage but must never cause speculative attribution.

The CLI accepts the snapshot through an optional `--keymap-snapshot <path>`
argument. Omitting it preserves the current event-only analysis behavior. The
summary schema is versioned independently from the event and snapshot schemas;
adding snapshot-derived output requires an explicit summary-schema decision and
fixture migration in the analyzer slice.

### Attribution fails closed

`vim.on_key` supplies a post-mapping key and pre-mapping typed keys, but empty
`typed` callbacks, recursive mappings, identical left- and right-hand sides,
prefix timeout resolution, and mappings changed during a session can make an
individual action ambiguous. The collector emits `mapping_use` only when the
tested callback trace and the effective API mapping state identify exactly one
mode, scope, and left-hand side.

Ambiguous, stale, unsupported, or over-limit cases remain ordinary
`key_sequence` observations without a `mapping_use` event. Attribution code must
return `nil` from the input callback and must not perform filesystem I/O or
unbounded mapping enumeration on the callback path.

## Delivery slices

### M2-S1: callback trace and contract tests

Explore real headless Neovim callback traces before selecting an attribution
state machine. Cover the oldest supported Neovim release in CI as well as the
development shell version where practical.

Red tests:

- record sanitized callback classifications for unmapped keys, multi-key
  mappings, ambiguous prefixes, recursive and non-recursive mappings,
  `<Plug>`, `<Nop>`, and identical left- and right-hand sides;
- cover Normal, Visual, and Operator-pending mode plus a buffer-local mapping;
- seed right-hand sides and callbacks with commands, inserted text, paths, and
  control bytes, then assert that no captured test artifact or error contains
  them;
- assert that the callback always returns `nil` and editing behavior is
  unchanged.

Green outcome:

- document the supported attribution traces and explicit fail-closed cases;
- define a small pure attribution interface that accepts sanitized mapping
  metadata and boolean/classified callback evidence rather than mapped content.

Implementation note: the observed trace contract and current fail-closed
interface are documented in [Mapping attribution contract](mapping-attribution.md).

### M2-S2: canonical mapping identity and snapshot model

Red tests:

- equivalent termcodes produce the same canonical key-token sequence;
- tuple boundaries, mode, and scope affect the identity deterministically;
- right-hand side, callback, description, script metadata, buffer ID, and path do
  not affect the identity and cannot appear in encoded output or errors;
- duplicate API entries collapse and output ordering is byte-stable;
- malformed, oversized, or unsupported mapping metadata fails closed.

Green implementation:

- add pure canonicalization, identity, validation, ordering, and encoding
  modules;
- query global and eligible buffer-local mappings through Neovim APIs;
- keep API dictionaries containing sensitive fields inside the narrow adapter
  and immediately project them to the allowlisted model.

### M2-S3: collector attribution

Red tests:

- confirmed dynamic, recursive, non-recursive, and buffer-local mappings emit one
  `mapping_use` with the expected ID and typed keys;
- ordinary typed keys still produce `key_sequence` data and are not consumed;
- ambiguous prefixes, mapping mutation, excluded buffers, text-bearing modes,
  unsupported traces, and resource-limit failures emit no mapping event;
- pause, resume, flush, stop, and mode changes cannot carry attribution state
  across a boundary;
- mapped secrets remain absent from pending state, JSONL, errors, status, and
  notifications.

Green implementation:

- add a bounded in-memory attribution state machine informed by M2-S1;
- resolve effective global versus buffer-local scope at the point of use;
- queue `mapping_use` through the existing schema-v1 writer without a second
  storage path.

Refactor gate:

- key tokenization is shared by sequences, mapping identity, and mapping events;
- callback work has explicit state and token bounds and performs no writes
  beyond the existing event queue.

### M2-S4: snapshot publication and Neovim report integration

Red tests:

- only loaded, valid, non-sensitive buffers contribute buffer-local mappings;
- a replaced output directory, symlink, oversized enumeration, encoding error,
  or publication failure leaves the previous snapshot unchanged;
- report argv passes the snapshot as one literal argument without shell
  interpolation;
- concurrent report requests cannot race snapshot publication or mismatch a
  report with another invocation's snapshot.

Green implementation:

- publish a private sanitized snapshot immediately before report launch;
- reuse existing anchored-directory, staging, and process-runner boundaries;
- add the optional CLI argument without changing event discovery.

### M2-S5: deterministic analyzer integration

Red tests:

- event-only analysis remains supported;
- snapshot parsing rejects unknown fields, versions, duplicate/conflicting IDs,
  invalid tokens, unsorted or over-limit inputs according to the documented
  contract;
- observed, `observed_not_in_snapshot`, and `unobserved_in_sample` mappings are
  deterministic with stable tie-breaking;
- global/buffer shadowing and exact left-hand-side collisions are surfaced
  conservatively;
- all selected inputs validate before output publication, and any invalid
  snapshot preserves existing summary and report files.

Green implementation:

- parse the snapshot as a separate bounded input;
- join mapping counts to snapshot entries by opaque ID;
- add snapshot-derived sections to `summary.json` and `report.md` with an
  explicit summary-schema version change if required;
- keep all conclusions descriptive; underuse thresholds belong to Milestone 3.

### M2-S6: end-to-end privacy regression and documentation

- Finalize sessions containing global and buffer-local mapping use, publish a
  snapshot, and analyze them in one headless test without network access.
- Seed secrets in mapping implementations and assert their absence from JSONL,
  snapshot, summary, report, argv, errors, and notifications.
- Cover a mapping removed between observation and snapshot and verify the
  conservative status.
- Update the event, aggregation, local-workflow, README, and configuration docs.
- Record measured callback overhead and enforce a practical regression budget.

## Security and privacy invariants

- Mapping implementations never cross the narrow Neovim API/callback adapter.
- Only an allowlisted sanitized mapping model may be encoded, logged, or passed
  to the analyzer.
- Snapshot enumeration never opens files or unloaded buffers and never scans
  user configuration text.
- Mapping IDs contain no secret salt or local identity and are stable only for
  the documented binding tuple.
- Buffer IDs, names, paths, filetypes, descriptions, script IDs, and source lines
  remain absent from mapping artifacts.
- Attribution uncertainty lowers coverage instead of guessing.
- Callback state, enumeration, parsing, retained strings, and output size are
  bounded.
- Existing collector exclusion and lifecycle boundaries apply before mapping
  attribution.

## Milestone completion gate

Milestone 2 is complete when one reproducible headless workflow proves that:

1. known mappings produce opaque usage events without changing editor input;
2. an API-derived sanitized snapshot is deterministic and contains no mapping
   implementation data;
3. the analyzer joins events and snapshot data conservatively and reports
   potential collisions;
4. ambiguous or changed mapping state fails closed;
5. old schema-v1 logs and event-only analysis remain supported;
6. `pkf run --no-cache check` and `nix flake check` pass on the full branch.

## Explicitly deferred work

- ergonomic scoring, minimum-use thresholds, and recommendations;
- filetype or project-context mapping segmentation;
- historical snapshots per session;
- Insert, Command-line, Search, terminal, prompt, or raw-key attribution;
- mapping right-hand-side classification;
- Codex invocation and AI-generated suggestions.
