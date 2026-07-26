# Input aggregation

The collector converts `vim.on_key` callbacks into bounded, privacy-sanitized events. It never consumes editor input.

## Mode policy

- Normal, Visual, and Operator-pending modes produce `key_sequence` events.
- Insert, Replace, and Select modes produce `text_run` events containing only key count and duration.
- Command-line and Search modes produce mode transitions but discard all input content.
- Other modes do not produce input payload events.

The collector canonicalizes only the callback's pre-mapping `typed` value. It does not persist the mapping-applied value, and callbacks with an empty `typed` value are ignored as mapping expansions. Mapping attribution will use opaque mapping identities in a later implementation.

## Sequence boundaries

A typed-key sequence ends when:

- the normalized mode changes;
- the next key arrives after `sequence_timeout_ms`;
- the sequence reaches `max_sequence_keys`;
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

`sequence_timeout_ms` must be a non-negative integer, and `max_sequence_keys` must be a positive integer.

An idle sequence is retained in memory until the next boundary; the collector does not create a timer for every key. Pause, stop, explicit flush, and Neovim shutdown all flush it.

## Timing

All event timestamps are monotonic elapsed milliseconds relative to `session_start`. Sequence and text-run durations measure the interval between their first and last accepted typed callbacks. No wall-clock timestamp is stored.
