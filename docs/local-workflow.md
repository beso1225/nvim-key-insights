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

For a deliberate local usage window, run `:KeyInsightsStart` only after you
have reviewed the configured storage directory and privacy defaults. Use the
editor normally for the chosen window, then run `:KeyInsightsStop` explicitly.
Do not copy the resulting JSONL into the repository, and do not enable any raw
capture option; raw capture options are intentionally unavailable. A paused or
active session is not a finalized input until it is stopped.

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
  --keymap-snapshot -
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
Neovim APIs. The JSON payload contains only canonical left-hand sides,
normalized modes, global/buffer-local scope, and opaque IDs. Mapping
implementations, descriptions, source metadata, buffer IDs, names, filetypes,
and paths are excluded.
The plugin writes this payload directly to the analyzer's standard input. The
analyzer reads at most 1 MiB before strict parsing, and no snapshot file or
cleanup lifecycle exists. Manual CLI use can pass a private file path instead of
`-`.

## Input ordering and discovery

Explicit CLI inputs are analyzed in the supplied order:

```text
key-insights analyze first.jsonl second.jsonl \
  --summary summary.json --report report.md
```

The same ordered inputs produce byte-identical output. Every input is resolved
before analysis; duplicate filesystem identities, incomplete inputs, output
aliases, and an invalid later input fail without replacing existing outputs. On
supported Unix systems, resolved input descriptors are closed after validation
and reopened one at a time with identity revalidation during streaming analysis,
so the supported 4,096-session boundary does not require 4,096 open files.

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

Deletion first moves the selected file to a private, identity-bound hidden
quarantine name under the held collector-directory handle. If Neovim exits
between that move and deletion, the next purge or retention pass recovers only
a regular, owner-only, single-linked quarantine whose current identity matches
the identity encoded in its name. Changed or malformed quarantines are skipped.
Legacy-name quarantines are recoverable only in the default collector directory,
under the same rule as their original legacy logs.

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

The suite drives the public Neovim commands in isolated child processes. It
exercises pause, resume, restart, recording and paused `VimLeavePre`, crash
artifacts, age-before-count retention, purge cancellation and force mode, and
known-good report preservation after analyzer failure. It also approves one
repository-owned mock Codex process and cancels a second request. The tests bind
the exact preview bytes, process arguments, two-variable child environment,
structured response, and deterministic Markdown while scanning the applicable
JSONL, summary, local report, notifications, Codex boundary, and rendered output
for seeded private values. No real Codex service or network connection is used.

## Resource and native-platform gates

Run the complete offline project contract with:

```sh
nix develop path:. --command pkf run --no-cache check
nix flake check --no-update-lock-file
```

The resource suite counts every callback-path dependency and persisted event for
excluded, ordinary Normal, mapped Normal, sequence-boundary, and Insert paths.
No session-writer method may run inside the callback, callback bursts coalesce
their deferred flush, and the real mapping resolver remains below a generous
batch-median timing ceiling. Timing output is telemetry; deterministic call,
event, queue, and byte bounds are the primary contract.

On Unix, the CLI validates all selected inputs before publication and then
reopens and consumes one input at a time, rechecking its filesystem identity and
discovery privacy policy. The supported 4,096-session boundary is exercised
without retaining one input descriptor per session. See
[Analyzer CLI](analyzer.md) for the trusted same-user concurrency precondition.

Retention inventories at most 8,192 directory entries and performs at most 512
identity-safe deletions per finalization. Excess work is deferred without
invalidating the newly finalized session and converges on later finalizations.
See [Storage retention](storage-retention.md) for overflow and external-recovery
behavior.

CI runs the same uncached project gate natively on the three existing flake
systems: `x86_64-linux`, `aarch64-linux`, and `aarch64-darwin`. The Linux
Neovim 0.10 lower-bound job separately runs both the normal Lua suite and the
callback/retention resource suite. These checks use synthetic local fixtures
only and never invoke a real Codex service or read private usage logs.

## Synthetic offline forward test

Run the synthetic forward-test contract with:

```sh
nix develop path:. --command pkf run --no-cache test:forward
```

The task builds the local analyzer and runs generated sessions in a fresh
owner-only temporary directory outside the repository. It exercises the public
`analyze` and `preview` commands with an empty child environment, bounded
private artifacts, schema checks, and private canaries. It does not inspect real
usage, invoke Codex, access the network, or change release state.

The temporary directory contains the generated JSONL, local report, sanitized
summary and payload, plus a compact `inspection-manifest.json`. The manifest
contains only contract versions, artifact byte sizes and SHA-256 digests, and
boolean boundary results. It never contains artifact text, paths, session IDs,
or seeded private canaries. These temporary artifacts remain local and must not
be copied into the repository.

To inspect an individual run, first build the analyzer, then provide an empty
absolute directory owned by the current user with mode `0700`:

```sh
cargo build -p key-insights
workspace="$(mktemp -d)"
chmod 0700 "$workspace"
python3 scripts/forward_test.py \
  --workspace "$workspace" \
  --key-insights-bin "$PWD/target/debug/key-insights"
```

Delete the temporary directory after the deliberate local inspection. Real
usage inspection is a separate opt-in step and is not part of this synthetic
contract.

## Deliberate local usage inspection

After an explicit collection window, use the local inspection harness on the
private finalized session directory. It runs the public analyzer and canonical
preview commands locally, never invokes Codex, and requires a human to inspect
each boundary before it writes a manifest. The session directory must be the
owner-only directory used by the plugin, while the report directory must be a
new empty owner-only directory outside the source tree:

```sh
inspection_root="$(mktemp -d)"
chmod 700 "$inspection_root"
mkdir "$inspection_root/reports"
chmod 700 "$inspection_root/reports"

python3 scripts/local_forward_test.py \
  --session-dir "/path/to/stdpath/state/key-insights/sessions" \
  --report-dir "$inspection_root/reports" \
  --manifest "$inspection_root/inspection-manifest.json" \
  --key-insights-bin "$(realpath "$(command -v key-insights)")" \
  --nvim-bin "$(realpath "$(command -v nvim)")"
```

`realpath` resolves Nix profile and Home Manager symlinks to the regular binary
files required by the harness. The harness rejects relative, aliased,
non-private, non-empty, or source-tree paths. It creates only
`summary.json`, `report.md`, and `payload.json` in the private report
directory. At four separate prompts, inspect the private JSONL, sanitized
summary, local report, and canonical Codex preview. Type `yes` only after each
inspection is complete; refusing any boundary leaves the manifest unpublished.

The resulting `inspection-manifest.json` contains only contract versions, tool
version strings, a finalized-session count, and aggregate boolean checks. It
does not contain session IDs, paths, JSONL, summary/report/payload text, or
authentication material. This command is deliberately excluded from ordinary
CI when pointed at real usage; the checked-in contract test uses only a
synthetic session in a temporary directory. Keep the private artifacts local,
review the manifest, and remove the temporary report directory when finished.

## Local performance forward test

The callback resource suite is synthetic and deterministic. Run it to collect
platform telemetry for the four production callback paths while keeping the
operation-count, queue, and byte contracts as the primary oracle:

```sh
nix develop path:. --command pkf run test:resource:lua
```

For analyzer telemetry, first build or install the current repository version
so its ergonomics contract matches the checked-in version. Then provide the
private finalized session directory and a new private manifest location:

```sh
performance_root="$(mktemp -d)"
chmod 700 "$performance_root"
python3 scripts/local_performance_test.py \
  --session-dir "/path/to/stdpath/state/key-insights/sessions" \
  --manifest "$performance_root/performance-manifest.json" \
  --key-insights-bin "$(realpath "$(command -v key-insights)")" \
  --nvim-bin "$(realpath "$(command -v nvim)")"
```

The harness measures one local analyzer run and stores only the finalized
session count, elapsed milliseconds, child maximum RSS, and generated artifact
byte sizes. Temporary summary/report files are private and removed on exit;
the manifest contains no paths, session IDs, key tokens, JSONL, report text, or
other input content. RSS and elapsed time are machine-specific telemetry, not
portable correctness thresholds. Keep the manifest private and remove
`performance_root` after review.
