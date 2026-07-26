# Storage retention

The collector bounds finalized local logs by age and session count. Retention runs only after a session has been durably written and atomically published as `.jsonl`.

## Defaults

```lua
require("key-insights").setup({
  storage = {
    retention = {
      max_age_days = 30,
      max_sessions = 100,
    },
  },
})
```

Both values must be positive integers. A log is age-expired only when its modification time is strictly older than `max_age_days`; a log exactly on the boundary remains eligible for count-based retention.

## Deterministic pruning

The collector:

1. scans regular files whose names match the opaque session-log format;
2. removes age-expired finalized logs;
3. orders the remaining logs by modification time and then filename;
4. removes the oldest entries until at most `max_sessions` remain.

The session being finalized is always protected. A finalized log with a corresponding `.lock` is also protected so concurrent Neovim processes cannot delete one another's in-progress publications. This can temporarily leave more than `max_sessions` logs; a later successful finalization converges the directory to the configured bound.

Retention never removes `.jsonl.part`, `.lock`, symlinks, or unrelated files. Incomplete artifacts require explicit recovery or purge handling rather than age-based deletion.

## Failure behavior

Pruning failures are reported as finalization failures. The completed `.jsonl` and its reservation remain intact, and retrying stop resumes pruning without rewriting or republishing the session. All successful publication, pruning, and reservation changes are covered by the final parent-directory sync.
