# Event schema contract

The collector writes newline-delimited JSON (JSONL). Each line is one complete event. Schema version `1` is the initial compatibility boundary between the Lua collector and Rust analyzer. Its support lifetime and any future upgrade path are defined in the [schema compatibility policy](schema-compatibility.md).

## Envelope

Every event will contain:

- `schema_version`: integer, currently `1`;
- `event_type`: a stable event discriminator;
- `session_id`: a random identifier created for one Neovim collection session;
- `elapsed_ms`: monotonic milliseconds since the session began.

The default event stream does not contain an absolute timestamp or file path. Project identity, when enabled, is an anonymized local identifier rather than a directory name.

## Initial event types

- `session_start` and `session_end` define hard aggregation boundaries. `session_start` may include an anonymized `project_id`.
- `key_sequence` represents a completed Normal, Visual, or Operator-pending sequence with canonical pre-mapping typed `keys` and `duration_ms`.
- `text_run` records Insert-, Replace-, or Select-mode `key_count` and `duration_ms`, never its text.
- `mode_transition` records `from` and `to` modes.
- `mapping_use` records a collector-generated opaque `mapping_id` and `typed_keys`. It does not record the mapping right-hand side because that value may contain commands, paths, or inserted text.

Every event rejects unknown fields. Key sequences and mapping key lists must be non-empty and cannot contain empty tokens, mapping IDs must be non-empty, and a key sequence's `duration_ms` cannot exceed its `elapsed_ms` within the session. Both collector and analyzer enforce a 64 KiB encoded event-line limit, and session IDs are limited to 128 bytes. The collector losslessly splits a large key sequence before it reaches that shared line boundary.

The streaming validator accepts up to 4,096 complete sessions across one analysis input set, but requires matching session IDs and non-decreasing `elapsed_ms` values within each session. The session limit bounds memory retained for reuse detection. It rejects nested sessions, reused session IDs across files, events outside a session, and the end of any input file with an unclosed session.

The collector stores one session per finalized `.jsonl` file. The validator accepts multiple ordered input readers and multiple complete sessions per reader without weakening session-boundary checks or concatenating their bytes. Files ending in `.jsonl.part` are incomplete collector artifacts and must not be analyzed.

## Privacy invariants

- Raw per-key logging is disabled unless explicitly opted in.
- Insert-mode text and command/search contents are never present under default settings.
- Terminal, prompt, `nofile`, and other special buffers are excluded.
- Sensitive filenames and filetypes are force-excluded and cannot be enabled through ordinary configuration.
- File paths are absent by default.
- AI integrations receive only a previewed, sanitized `summary.json`; they do not receive JSONL logs by default.
- A parser must reject unsupported schema versions and malformed session boundaries rather than merging ambiguous data.
