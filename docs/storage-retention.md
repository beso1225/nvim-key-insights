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

The collector performs one bounded inventory and then:

1. recovers eligible interrupted quarantine artifacts in filename order;
2. orders finalized logs by modification time and then filename;
3. removes age-expired finalized logs in that order;
4. removes the oldest remaining entries until at most `max_sessions` remain.

A retention inventory examines at most 8,192 directory entries, counting every
entry before namespace or file-type filtering. Observing an 8,193rd entry aborts
the inventory before any retention mutation. One finalization performs at most
512 identity-safe deletions across quarantine recovery, age pruning, and count
pruning. These are internal safety budgets rather than user configuration.

The session being finalized is always protected. A finalized log whose versioned lock identifies a live owner process is also protected so concurrent Neovim processes cannot delete one another's in-progress publications. Live locks or the per-pass deletion budget can temporarily exceed `max_sessions`; a later successful finalization retries and converges to the configured bound. A directory above the scan budget requires explicit purge or external removal of unrelated entries before automatic retention can resume. Stale, empty, or malformed locks do not exempt a finalized log from retention. A reused process ID can conservatively delay pruning until that process exits, but it cannot cause an unrelated file to be deleted.

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

Retention is best-effort after the current `.jsonl` has been published. A scan
overflow, exhausted deletion budget, unavailable identity-safe rename primitive,
or another pruning failure does not interrupt session finalization: the current
reservation is released, the parent directory is synchronized, and Neovim
reports the categorical warning `key-insights: retention cleanup was deferred`
without exposing filesystem details. A pre-mutation failure preserves every
candidate, while a failure or budget deferral after earlier deletions may leave
the completed subset in place. The next session finalization retries the
remaining eligible logs when the directory can be scanned within its bound. The
collector never substitutes a race-prone pathname unlink for the identity-safe
cleanup protocol.
