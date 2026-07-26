# Deterministic analyzer

The Rust analyzer validates collector JSONL and produces two local artifacts without calling an AI service:

- `summary.json`: stable structured aggregation for tools and optional sanitized AI input;
- `report.md`: a human-readable rendering of the same summary.

## Command

```sh
key-insights analyze <input.jsonl> \
  --summary <summary.json> \
  --report <report.md>
```

The input may contain one or more complete finalized `.jsonl` sessions accepted by the streaming schema validator. Incomplete `.jsonl.part` collector artifacts are rejected. The analyzer completes validation and aggregation before it creates either output. Inputs and existing outputs must be regular files, output symlinks are rejected, and the two output names must resolve to distinct entries under the target filesystem's case and Unicode-normalization rules.

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

The analyzer accepts at most 4,096 distinct keys, mapping IDs, and repeated-key identifiers per input. Key tokens and mapping IDs are limited to 256 encoded bytes each. It rejects inputs outside either bound instead of retaining unbounded aggregation state.

Both artifacts are fully staged in private same-directory temporary files before publication. Each completed file replaces its destination with an atomic rename, so validation and write failures do not truncate an existing artifact and a swapped output symlink is replaced rather than followed. If either publication fails, the CLI rolls back both destinations to their previous state.

## Privacy boundary

The outputs exclude session IDs, anonymized project IDs, file paths, raw sequences, mapping right-hand sides, Insert text, and command/search contents. Key rankings contain individual sanitized Normal/Visual/Operator-pending key tokens, and mapping rankings contain only collector-generated opaque IDs. Markdown treats these tokens as untrusted content and escapes them before rendering.

Only `summary.json`, after user preview, is intended to cross the optional Codex boundary. JSONL collector logs and `report.md` remain local by default.
