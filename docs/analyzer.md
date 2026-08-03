# Deterministic analyzer

The Rust analyzer validates collector JSONL and produces two local artifacts without calling an AI service:

- `summary.json`: stable structured aggregation for tools and optional sanitized AI input;
- `report.md`: a human-readable rendering of the same summary.

## Command

```sh
key-insights analyze <input.jsonl>... \
  --summary <summary.json> \
  --report <report.md>
```

Each input must contain one or more complete finalized `.jsonl` sessions accepted by the streaming schema validator. Inputs are processed in the supplied order with shared session-identity, cardinality, retained-byte, and duration limits. Canonical or hard-linked duplicate inputs and incomplete `.jsonl.part` collector artifacts are rejected. The analyzer completes validation and aggregation before it creates either output. Inputs and existing outputs must be regular files, output symlinks are rejected, neither output may alias any input, and the two output names must resolve to distinct entries under the target filesystem's case and Unicode-normalization rules.

Alternatively, discover the collector's current finalized namespace:

```sh
key-insights analyze --session-dir <directory> \
  --summary <summary.json> \
  --report <report.md>
```

The directory form and explicit inputs are mutually exclusive. Discovery scans
at most 8,192 directory entries and accepts at most 4,096 finalized sessions,
then opens `nvim-key-insights-<session_id>.jsonl` files in ASCII filename order.
Accepted IDs contain 1–128 ASCII alphanumeric, `_`, or `-` bytes. Discovered
files must be owner-only (`0600`), owned by the current user, regular, and
single-linked. Unsafe matching entries, `.jsonl.part`, locks, legacy names,
outputs, and unrelated entries are ignored. The directory itself and every
accepted leaf are held and verified through anchored handles without following
symlinks; replacement during discovery fails the command before output recovery
or publication. An empty discovery set is an error. Legacy filenames can still
be selected explicitly.

## Current metrics

- session and event counts;
- total session duration;
- key-sequence and sanitized sequence-key counts;
- Insert/Replace/Select text-run counts without text content;
- sequence counts and keys by Normal, Visual, and Operator-pending mode;
- mode-transition and opaque mapping-use counts;
- deterministic key and mapping frequency rankings, capped at 100 rows each;
- consecutive repeated-key runs within each collected sequence.

Rankings use descending count with lexical identifiers as the tie-break. The summary retains total unique-token cardinalities when ranked rows are truncated. Mode rows use lexical mode order. JSON object layout and Markdown sections are stable for identical validated input.

The analyzer accepts at most 4,096 distinct keys, mapping IDs, and repeated-key identifiers across the complete input set and retains at most 1 MiB of unique token data across those categories. Individual schema-v1 tokens remain valid up to the existing 64 KiB event-line boundary. The analyzer rejects inputs outside the aggregate bounds instead of retaining unbounded state.

Both artifacts are fully staged in private same-directory temporary files before publication. Each completed file replaces its destination with an atomic rename, so validation and write failures do not truncate an existing artifact and a swapped output symlink is rejected rather than followed. If either publication fails, the CLI rolls back both destinations to their previous state.

Publication acquires persistent, private sidecar lock files for both destinations in stable path order. Cooperative analyzer processes targeting either output are therefore serialized, and operating-system lock release handles process termination. Existing outputs remain linked at their public paths while rollback hard links are prepared, and the containing directory is synced before replacement, so an interruption before replacement does not make an artifact disappear.

## Privacy boundary

The outputs exclude session IDs, anonymized project IDs, file paths, raw sequences, mapping right-hand sides, Insert text, and command/search contents. Key rankings contain individual sanitized Normal/Visual/Operator-pending key tokens, and mapping rankings contain only collector-generated opaque IDs. Markdown treats these tokens as untrusted content and escapes them before rendering.

Only `summary.json`, after user preview, is intended to cross the optional Codex boundary. JSONL collector logs and `report.md` remain local by default.
