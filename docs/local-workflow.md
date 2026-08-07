# Local collection and reporting

This document describes the complete local workflow from opt-in collection to a
deterministic report and explicit cleanup. None of these operations require a
network connection, an AI service, or an API key.

## Configure and collect

The plugin remains stopped after setup. A minimal lazy.nvim configuration is:

```lua
{
  "beso1225/nvim-key-insights",
  opts = {
    storage = {
      directory = vim.fn.stdpath("state") .. "/key-insights/sessions",
      retention = {
        max_age_days = 30,
        max_sessions = 100,
      },
    },
    report = {
      analyzer = "key-insights",
      directory = vim.fn.stdpath("state") .. "/key-insights/reports",
    },
  },
}
```

Use `:KeyInsightsStart`, `:KeyInsightsPause`, and `:KeyInsightsStop` to control
one session boundary. Pause preserves the current session; starting from the
paused state resumes it. Stop publishes the completed `.jsonl` file. Collection
also stops during `VimLeavePre`. `:KeyInsightsStatus` shows the lifecycle state
and whether a local report process is running.

The current session remains a `.jsonl.part` file and is never selected by
automatic analysis. Run `:KeyInsightsStop` before reporting when the current
session should be included.

## Analyze and open reports

`:KeyInsightsReport` launches the configured analyzer asynchronously with an
argument vector equivalent to:

```text
key-insights analyze --session-dir <sessions> \
  --summary <reports>/summary.json \
  --report <reports>/report.md \
  --keymap-snapshot <reports>/keymap-snapshot-<opaque>.json
```

Paths are passed as separate arguments without shell interpolation. Only one
report process may run for a configured plugin instance. A successful process
must publish fresh, bounded, structurally valid outputs before the Markdown file
is opened. A launch error, validation error, or non-zero exit leaves the editor
view and previously published report pair unchanged. `:KeyInsightsOpenReport`
validates and opens the existing Markdown report without invoking the analyzer.

The default output directory is
`stdpath("state")/key-insights/reports/`. The plugin creates and verifies it as
owner-only where the platform supports Unix permissions. Paths are local
configuration and are not written to events or summaries.

Immediately before launch, the plugin derives a bounded mapping snapshot from
Neovim APIs. The invocation-specific file contains only canonical left-hand
sides, normalized modes, global/buffer-local scope, and opaque IDs. Mapping
implementations, descriptions, source metadata, buffer IDs, names, filetypes,
and paths are excluded.

## Input ordering and discovery

Explicit CLI inputs are analyzed in the supplied order:

```text
key-insights analyze first.jsonl second.jsonl \
  --summary summary.json --report report.md
```

The same ordered inputs produce byte-identical output. Every input is resolved
before analysis; duplicate filesystem identities, incomplete inputs, output
aliases, and an invalid later input fail without replacing existing outputs.

The mutually exclusive `--session-dir` form scans at most 8,192 entries, accepts
at most 4,096 finalized sessions, sorts accepted files by ASCII collector
filename, and then uses the same multi-input analyzer. Discovery accepts only
current `nvim-key-insights-<session_id>.jsonl` files that are regular,
single-linked, owner-only, and owned by the current user. It ignores incomplete
files, reservations, legacy names, symlinks, directories, unexpected modes, and
unrelated entries. Replaced directories or accepted leaves fail closed. Legacy
logs remain available as explicitly selected CLI inputs.

## Explicit purge

`:KeyInsightsPurge` performs a bounded preview and lists the selected collector
artifact names before asking for confirmation. Cancelling performs no writes.
`:KeyInsightsPurge!` skips the prompt only; it does not bypass namespace,
ownership, permission, link-count, active-session, lock, scan-limit, or identity
checks.

Eligible names are finalized `.jsonl`, incomplete `.jsonl.part`, and `.lock`
reservation files in the collector namespace. The default collector directory
also recognizes the previous 32-character lowercase hexadecimal `.jsonl`
namespace. A custom directory does not opt into legacy-name deletion.

The purge engine protects all artifacts for the active session, a live owner
process, or an unreadable, oversized, empty, malformed, or otherwise unknown
lock. A valid lock whose process is known to be absent is stale. Symlinks, hard
links, special permission modes, entries owned by another user, directories,
and unrelated names are skipped. If current-user ownership cannot be verified,
purge fails closed. Directory and leaf identities are rechecked after the
preview and immediately around bounded mutation. A verified parent-directory
handle is held across mutation; deletion is relative to that handle, and the
same handle is synchronized before success is reported. Platforms without
descriptor-relative deletion support fail closed. The final notification
reports removed, protected, skipped, and failed counts. One unlink failure does
not make the engine broaden its selection.

Purge is refused while a report process is running. It never removes report
outputs, analyzer recovery sidecars, or files outside the configured session
directory.

## Recovery boundaries

A crash may leave a `.jsonl.part` file and reservation. Automatic analysis
ignores both. A live or ambiguous owner remains protected; stale artifacts can
be inspected and removed through explicit purge. Retention removes only
finalized collector logs and never acts as incomplete-session recovery.

The analyzer stages both output files, serializes cooperative writers with
private sidecar locks, and recovers an interrupted paired publication on the
next invocation. Recovery completes before new analysis output is published.
Do not manually rename analyzer sidecars into place.

## Privacy boundaries

Collector JSONL remains local and contains session boundaries, sanitized typed
Normal/Visual/Operator-pending key tokens, and aggregate text-run counts. It does
not contain Insert text, Command text, Search text, mapping expansions, buffer
paths, or sensitive-buffer input. `summary.json` and `report.md` additionally
exclude session IDs and project IDs. Only a user-previewed `summary.json` is
intended for a later optional Codex workflow; the local report commands do not
send any artifact over the network.

Run the complete default-workflow privacy regression with:

```sh
nix develop --command pkf run test:e2e
```

The test finalizes two real collector sessions, exercises pause and resume,
launches the built Rust analyzer through the asynchronous report workflow,
checks byte-identical repeated output, and searches JSONL, summary, report,
notifications, and analyzer arguments for seeded private values.
