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

The input may contain one or more complete sessions accepted by the streaming schema validator. The analyzer completes validation and aggregation before it creates either output. Output paths must be different from one another and from the input path.

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

## Privacy boundary

The outputs exclude session IDs, anonymized project IDs, file paths, raw sequences, mapping right-hand sides, Insert text, and command/search contents. Key rankings contain individual sanitized Normal/Visual/Operator-pending key tokens, and mapping rankings contain only collector-generated opaque IDs. Markdown treats these tokens as untrusted content and escapes them before rendering.

Only `summary.json`, after user preview, is intended to cross the optional Codex boundary. JSONL collector logs and `report.md` remain local by default.
