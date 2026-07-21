# Event schema contract

The collector writes newline-delimited JSON (JSONL). Each line is one complete event. Schema version `1` is the initial compatibility boundary between the Lua collector and Rust analyzer.

## Envelope

Every event will contain:

- `schema_version`: integer, currently `1`;
- `event_type`: a stable event discriminator;
- `session_id`: a random identifier created for one Neovim collection session;
- `elapsed_ms`: monotonic milliseconds since the session began.

The default event stream does not contain an absolute timestamp or file path. Project identity, when enabled, is an anonymized local identifier rather than a directory name.

## Initial event types

- `session_start` and `session_end` define hard aggregation boundaries.
- `key_sequence` represents a completed Normal, Visual, or Operator-pending sequence.
- `text_run` records Insert-mode length and timing, never its text.
- `mode_transition` records mode changes.
- `mapping_use` records typed and mapping-applied key notation for a resolved mapping.

The exact per-event fields will be introduced with executable collector/analyzer contract tests before persistence is implemented.

## Privacy invariants

- Raw per-key logging is disabled unless explicitly opted in.
- Insert-mode text and command/search contents are never present under default settings.
- Terminal, prompt, `nofile`, and other special buffers are excluded.
- Sensitive filenames and filetypes are force-excluded and cannot be enabled through ordinary configuration.
- File paths are absent by default.
- AI integrations receive only a previewed, sanitized `summary.json`; they do not receive JSONL logs by default.
- A parser must reject unsupported schema versions and malformed session boundaries rather than merging ambiguous data.
