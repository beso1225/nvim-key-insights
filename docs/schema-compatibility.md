# Schema compatibility and upgrades

Package versions and data-contract versions are independent. A package release
may keep every schema unchanged, and a schema change does not silently inherit
compatibility from SemVer. Every reader validates its own integer version and
unknown versions fail closed before output publication or a Codex handoff.

## Current contracts

| Contract | Current version | Persistence | Upgrade or regeneration path |
| --- | ---: | --- | --- |
| Event log | `1` | Durable collector input | Keep the private finalized JSONL and analyze it with a release that supports event schema 1. |
| Analysis summary | `3` | Derived private artifact | Regenerate from supported event logs; do not rewrite an old summary in place. |
| Keymap snapshot | `1` | Ephemeral report input | Capture a fresh snapshot from Neovim. |
| Codex payload | `1` | Bounded sanitized handoff | Regenerate with `key-insights preview` from a supported summary. |
| Codex suggestions | `1` | Untrusted model output | Request new JSON and validate it against the exact private summary with `key-insights suggestions`. |
| Ergonomics contract | `1` | Nested in summary 3 | Regenerate the containing summary. |
| Histogram layout | `1` | Nested in ergonomics 1 | Regenerate the containing summary. |
| Operation token set | `1` | Nested in ergonomics 1 | Regenerate the containing summary. |
| Count-prefix token set | `1` | Nested in ergonomics 1 | Regenerate the containing summary. |
| Directional-motion token set | `1` | Nested in ergonomics 1 | Regenerate the containing summary. |
| Candidate kind | `1` | Nested in ergonomics 1 | Regenerate the containing summary. |
| Mapping identity | `mapping-v1` | Durable event identity and derived references | Preserve the byte-length-prefixed preimage contract; a new identity prefix requires an explicit cross-version attribution design. |
| Mapping-underuse candidate identity | `mapping-unobserved-v1` | Derived summary and payload reference | Regenerate the summary; changing the prefix requires a candidate-kind version bump. |

The JSON Schemas bundled with the standalone Codex skill are byte-identical
copies of the canonical repository schemas. A copy is not an independent
compatibility authority.

## Durable event logs

Event schema 1 is the only durable input contract in the current release. The
analyzer accepts the supported version exactly; it does not guess field names,
drop unknown fields, or partially analyze a newer stream. Preserve finalized
event logs when upgrading if they may need to be analyzed again.

Removing an event reader requires a package major release. Before event schema
1 can be removed, a release must provide either a parallel reader covering the
documented transition window or an explicit offline converter with privacy,
ordering, resource-limit, and crash-recovery tests.

## Derived and ephemeral artifacts

`summary.json` and `report.md` are a deterministic pair published from the same
in-memory analysis. They are not migration inputs. When summary schema 3 is no
longer current, regenerate both files from retained event logs using a CLI that
supports those logs. Existing outputs remain untouched when regeneration
fails.

For compatibility with a configured older analyzer, Neovim's report freshness
check may recognize summary schemas 1 and 2 only to confirm that a fresh
summary/report pair was published. It does not interpret their nested data or
send them to Codex. `key-insights preview` and the Neovim Codex boundary require
the complete current summary schema 3 and fail closed for schemas 1, 2, or an
unknown version.

Keymap snapshots are captured at report time and are never persisted by the
default Neovim workflow. Capture a new snapshot after upgrading instead of
converting an old snapshot document.

Codex payloads and suggestion JSON are bounded exchange artifacts, not durable
records. Regenerate a payload with the matching CLI, inspect it again, and ask
for new suggestion-schema-1 JSON. Suggestion JSON is never trusted or rendered
until the local validator binds every evidence value and collision claim to the
exact private summary and optional sanitized snapshot.

## Future schema changes

A schema bump must include all of the following in one reviewed change:

1. a new integer version and strict reader or converter;
2. canonical valid, invalid, truncated, oversized, and unknown-version fixtures;
3. privacy regressions proving that paths, session/project identifiers, Insert
   text, command/search contents, mapping right-hand sides, and raw sequences do
   not cross a newly introduced boundary;
4. deterministic reporting and failure-preservation tests;
5. synchronized canonical and standalone Codex schemas when affected;
6. this compatibility table, the changelog, and explicit user upgrade steps.

Do not reuse an existing schema number for a changed wire shape or meaning. Do
not silently coerce an unknown version, and do not add speculative migration
code for an artifact that can be safely regenerated.
