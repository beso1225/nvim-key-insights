# Input aggregation

The collector converts `vim.on_key` callbacks into bounded, privacy-sanitized events. It never consumes editor input.

## Mode policy

- Normal, Visual, and Operator-pending modes produce `key_sequence` events.
- Insert, Replace, and Select modes produce `text_run` events containing only key count and duration.
- Command-line and Search modes produce mode transitions but discard all input content.
- Other modes do not produce input payload events.

The collector canonicalizes only the callback's pre-mapping `typed` value. It
does not persist the mapping-applied value, and callbacks with an empty `typed`
value are ignored as mapping expansions. When a bounded, freshly validated
mapping baseline identifies exactly one effective binding, the collector also
emits `mapping_use` with an opaque ID and the same canonical typed keys.
Ambiguity, mutation, unsupported modes, and excluded buffers reduce attribution
coverage instead of producing a guess.

## Sequence boundaries

A typed-key sequence ends when:

- the normalized mode changes;
- `sequence_timeout_ms` is greater than zero and the next key arrives after
  that interval;
- the sequence reaches `max_sequence_keys`;
- its encoded JSONL event would exceed the 64 KiB collector/analyzer limit;
- collection is paused, stopped, or explicitly flushed;
- input moves into an excluded buffer.

The defaults are:

```lua
require("key-insights").setup({
  collection = {
    sequence_timeout_ms = 1000,
    max_sequence_keys = 64,
  },
})
```

`sequence_timeout_ms` must be a finite non-negative integer; zero disables the
idle-time boundary. `max_sequence_keys`
must be an integer from 1 through 65,536. Byte-size splitting is always
enforced, including when `max_sequence_keys` is configured above its default. It
preserves every typed key and computes duration independently for each resulting
chunk.

An idle sequence is retained in memory until the next boundary; the collector
does not create a timer for every key. Pause, stop, explicit flush, and Neovim
shutdown all flush it. Deferred writes retain at most 1,024 events and 4 MiB.
If a synchronous input burst reaches either limit before Neovim services the
scheduled writer, collection records a fixed error and ignores later input until
pause or stop flushes the bounded batch. Pausing and starting again resumes the
same session after recovery. The batch that would cross a limit and any
unpublished aggregate tail are dropped together, so a finalized log remains an
accepted prefix rather than containing evidence from after a gap.

## Timing

All event timestamps are monotonic elapsed milliseconds relative to `session_start`. Sequence and text-run durations measure the interval between their first and last accepted typed callbacks. No wall-clock timestamp is stored.
