# Collector lifecycle

The collector is stopped by default and does not create a log until the user runs `:KeyInsightsStart`.

## States

- `stopped`: no active session and no `vim.on_key` callback;
- `recording`: one active session with a non-consuming callback;
- `paused`: the session remains open, but the callback is detached;
- `stopping`: finalization failed and `:KeyInsightsStop` can safely retry it without adding another `session_end`.

Running `:KeyInsightsStart` while paused resumes the same session. Running it after `:KeyInsightsStop` creates a new opaque session ID and a new `session_start` boundary. Pause and stop flush pending events, and Neovim shutdown stops an active or paused session through `VimLeavePre`.

## Storage

The default session directory is:

```text
stdpath("state")/key-insights/sessions/
```

Each Neovim session first reserves its opaque ID with an exclusively created `<session_id>.lock`, then writes `<session_id>.jsonl.part` as mode `0600`. A clean stop flushes `session_end`, closes the file, atomically renames it to `<session_id>.jsonl`, releases the reservation, and fsyncs the parent directory before reporting success. Concurrent Neovim processes therefore cannot interleave session boundaries, and a finalized ID cannot be reused through the collector. A crash can leave `.lock` and `.jsonl.part` files, which analyzers must ignore.

A custom directory can be supplied through `setup`:

```lua
require("key-insights").setup({
  storage = {
    directory = vim.fn.expand("~/.local/state/key-insights/sessions"),
  },
})
```

The directory is local configuration and is never included in an event.

Start is failure-atomic: storage or callback-registration failures remove the incomplete file and restore the stopped state. Stop records its closing transition, so retrying after a transient write failure flushes the existing `session_end` instead of appending a duplicate.

## Current collection boundary

The callback always returns `nil`, so it cannot consume or replace editor input. It checks the current buffer before aggregation. Special buffers and sensitive filenames or filetypes are force-excluded.

Normal, Visual, and Operator-pending typed keys are grouped into sequences. The mapping-applied callback value is never stored, preventing mapping right-hand sides from entering a sequence. Insert, Replace, and Select input is reduced to key count and duration; Command and Search input content is discarded. See [Input aggregation](input-aggregation.md) for sequence boundaries and configuration.
