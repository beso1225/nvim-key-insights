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

1. scans regular files in the `nvim-key-insights-<session_id>.jsonl` namespace;
2. removes age-expired finalized logs;
3. orders the remaining logs by modification time and then filename;
4. removes the oldest entries until at most `max_sessions` remain.

The session being finalized is always protected. A finalized log whose versioned lock identifies a live owner process is also protected so concurrent Neovim processes cannot delete one another's in-progress publications. This can temporarily exceed `max_sessions`; after the owner exits, a later successful finalization converges to the configured bound. Stale, empty, or malformed locks do not exempt a finalized log from retention. A reused process ID can conservatively delay pruning until that process exits, but it cannot cause an unrelated file to be deleted.

Retention never removes `.jsonl.part`, `.lock`, symlinks, non-regular entries, or files outside the collector namespace. In the collector-owned default directory only, it also recognizes the previous 32-character lowercase hexadecimal filename format so upgrades do not exempt historical logs from the privacy boundary. Legacy-shaped files in a custom directory remain untouched. If a filesystem does not report directory-entry types, the collector uses `lstat` rather than following links. Incomplete artifacts require explicit recovery or purge handling rather than age-based deletion.

## Explicit purge

Retention and purge are separate operations. `:KeyInsightsPurge` previews and
confirms bounded removal of collector-owned finalized, incomplete, and
reservation artifacts. `:KeyInsightsPurge!` skips confirmation but not safety
checks. Active sessions, live or ambiguous owners, symlinks, hard links,
unexpected permission modes, entries owned by another user, subdirectories, and
unrelated names survive. See [Local collection and reporting](local-workflow.md#explicit-purge)
for the complete selection, race, and result-count contract.

## Failure behavior

Retention is best-effort after the current `.jsonl` has been published. An unavailable identity-safe rename primitive or another pruning failure does not interrupt session finalization: the current reservation is released, the parent directory is synchronized, and Neovim reports the categorical warning `key-insights: retention cleanup was deferred` without exposing filesystem details. A pre-mutation failure preserves that candidate, while a failure after earlier deletions may leave the completed subset in place. The next session finalization retries the remaining eligible logs. The collector never substitutes a race-prone pathname unlink for the identity-safe cleanup protocol.
