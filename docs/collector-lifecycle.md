# Collector lifecycle

The collector is stopped by default and does not create a log until the user runs `:KeyInsightsStart`.

## States

- `stopped`: no active session and no `vim.on_key` callback;
- `recording`: one active session with a non-consuming callback;
- `paused`: the session remains open, but the callback is detached.

Running `:KeyInsightsStart` while paused resumes the same session. Running it after `:KeyInsightsStop` creates a new opaque session ID and a new `session_start` boundary. Pause and stop flush pending events, and Neovim shutdown stops an active or paused session through `VimLeavePre`.

## Storage

The default JSONL path is:

```text
stdpath("state")/key-insights/events.jsonl
```

The directory is created with owner-only permissions when possible, and the log file is opened as mode `0600`. Writes are append-only. A custom path can be supplied through `setup`:

```lua
require("key-insights").setup({
  storage = {
    path = vim.fn.expand("~/.local/state/key-insights/events.jsonl"),
  },
})
```

The path is local configuration and is never included in an event.

## Current collection boundary

The callback always returns `nil`, so it cannot consume or replace editor input. It checks the current buffer before any future input aggregation. Special buffers and sensitive filenames or filetypes are force-excluded.

This lifecycle slice records only `session_start` and `session_end`. It does not yet persist Normal-mode sequences, Insert-mode text-run metrics, or mapping usage. Those event producers will be added behind the same exclusion boundary.
