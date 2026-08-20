# Deterministic analyzer

The Rust analyzer validates collector JSONL and produces two local artifacts without calling an AI service:

- `summary.json`: stable structured aggregation for tools and optional sanitized AI input;
- `report.md`: a human-readable rendering of the same summary.

## Command

```sh
key-insights analyze <input.jsonl>... \
  --summary <summary.json> \
  --report <report.md> \
  [--keymap-snapshot <snapshot.json|->]
```

Each input must contain one or more complete finalized `.jsonl` sessions accepted by the streaming schema validator. Inputs are processed in the supplied order with shared session-identity, cardinality, retained-byte, and duration limits. Canonical or hard-linked duplicate inputs and incomplete `.jsonl.part` collector artifacts are rejected. The analyzer completes validation and aggregation before it creates either output. Inputs and existing outputs must be regular files, output symlinks are rejected, neither output may alias any input, and the two output names must resolve to distinct entries under the target filesystem's case and Unicode-normalization rules.

Alternatively, discover the collector's current finalized namespace:

```sh
key-insights analyze --session-dir <directory> \
  --summary <summary.json> \
  --report <report.md>
```

The directory form and explicit inputs are mutually exclusive. Discovery scans
at most 8,192 directory entries and accepts at most 4,096 finalized sessions,
then opens `nvim-key-insights-<session_id>.jsonl` files in ASCII filename order.
Accepted IDs contain 1–128 ASCII alphanumeric, `_`, or `-` bytes. Discovered
files must be owner-only (`0600`), owned by the current user, regular, and
single-linked. Unsafe matching entries, `.jsonl.part`, locks, legacy names,
outputs, and unrelated entries are ignored. The directory itself and every
accepted leaf are held and verified through anchored handles without following
symlinks; replacement during discovery fails the command before output recovery
or publication. An empty discovery set is an error. Legacy filenames can still
be selected explicitly.

## Current metrics

- session and event counts;
- total session duration;
- key-sequence and sanitized sequence-key counts;
- Insert/Replace/Select text-run counts without text content;
- sequence counts and keys by Normal, Visual, and Operator-pending mode;
- mode-transition and opaque mapping-use counts;
- deterministic key and mapping frequency rankings, capped at 100 rows each;
- consecutive repeated-key runs within each collected sequence;
- versioned fixed histograms for session duration, sequence length, and average
  sequence inter-key latency;
- versioned undo, redo, repeat, search, count-prefix, repeated-motion, and
  mode-transition evidence;
- cautious, sample-guarded repeated-motion and current-mapping coverage
  candidates, capped at 100 total rows.

Every analysis emits summary schema v3. Collector events remain schema v1 and
the optional snapshot document remains version 1; these are independent
contracts. With a snapshot, the summary joins every observed and snapshotted
mapping as `observed`, `observed_not_in_snapshot`, or
`unobserved_in_sample`. It reports only potential global/buffer shadowing for an
exact mode and canonical LHS match; it does not claim that a buffer-local entry
was active in every context. Current mappings that were unobserved may become
candidates only after three complete sessions and 100 sequence keys; this does
not claim that the current snapshot existed throughout the sample. Omitting the
snapshot emits the same schema-v3 structure with unavailable mapping coverage.
Snapshot JSON is limited to 1 MiB and 4,096 canonical,
strictly ordered mappings. Unknown fields, unsupported versions, malformed or
inconsistent IDs, invalid tokens, duplicates, and noncanonical ordering fail
before output publication. `-` reads the automatic Neovim payload from stdin;
an explicit path must be an owner-readable (`0400` or `0600`), single-linked
regular file with no group, other, execute, or special permission bits.
Snapshot-derived outputs remain bounded by the validated snapshot and event
cardinality budgets; the Neovim workflow reads each generated artifact up to
16 MiB before accepting it.

Histogram buckets, token sets, candidate kinds, thresholds, and caps are
versioned in the summary. Repeated-motion rankings use descending run count,
descending presses, then lexical motion. Other rankings use descending count
with lexical identifiers as the tie-break. The summary retains total
unique-token cardinalities when ranked rows are truncated. Mode rows use
lexical mode order. JSON object layout and Markdown sections are stable for
identical validated input.

The analyzer accepts at most 4,096 distinct keys, mapping IDs, and repeated-key identifiers across the complete input set and retains at most 1 MiB of unique token data across those categories. Individual schema-v1 tokens remain valid up to the existing 64 KiB event-line boundary. The analyzer rejects inputs outside the aggregate bounds instead of retaining unbounded state.

Both artifacts are fully staged in private same-directory temporary files before publication. Each completed file replaces its destination with an atomic rename, so validation and write failures do not truncate an existing artifact and a swapped output symlink is rejected rather than followed. If either publication fails, the CLI rolls back both destinations to their previous state.

Publication acquires persistent, private sidecar lock files for both destinations in stable path order. Cooperative analyzer processes targeting either output are therefore serialized, and operating-system lock release handles process termination. Existing outputs remain linked at their public paths while rollback hard links are prepared, and the containing directory is synced before replacement, so an interruption before replacement does not make an artifact disappear.

## Privacy boundary

The outputs exclude session IDs, anonymized project IDs, file paths, raw
sequences, raw timing samples, mapping right-hand sides, Insert text, and
command/search contents. Key rankings contain individual sanitized
Normal/Visual/Operator-pending key tokens, and mapping rankings and candidates
contain only collector-generated opaque IDs. Markdown treats event-derived
tokens as untrusted content and escapes them before rendering.

Only `summary.json`, after user preview, is intended to cross the optional Codex boundary. JSONL collector logs and `report.md` remain local by default.

## Optional Codex payload preview

The CLI exposes the same renderer through an explicit preview command:

```sh
key-insights preview <summary.json> [--keymap-snapshot <snapshot.json|->] [--output <payload.json|->]
```

The summary must be an owner-only regular file. A snapshot may be supplied as
an owner-only file or through stdin (`-`); stdout is also selected with
`--output -` and receives the exact compact JSON bytes without a trailing
newline. File outputs are staged atomically and cannot alias either input.
The Rust library and CLI renderer accept only an in-memory schema-v3 `AnalysisSummary`
and an optional parsed keymap snapshot, emits compact deterministic JSON, and
rejects unsupported summary versions or payloads larger than 256 KiB. The
payload contains a fixed purpose, evidence and collision-check instructions,
the summary, and—when supplied—the snapshot's version, mode, scope, canonical
LHS, and opaque mapping IDs. It does not accept paths, JSONL, report Markdown,
or mapping implementations, and it omits those fields from the serialized
boundary by construction. No Codex process is launched by this renderer. The
Neovim `:KeyInsightsAnalyze` command opens the bounded JSON in a scratch buffer
and requests explicit confirmation; only confirmation launches the non-shell
`codex exec` runner. Cancelling leaves the workflow local-only, and raw
JSONL/report artifacts are never sent.

## Optional Codex exec runner

The library also provides a non-shell `codex exec` runner for the next explicit
approval step. It inherits saved Codex authentication, supplies `--ephemeral`,
ignores user configuration and rules, clears the shell environment inheritance,
starts in an owner-only empty directory, and selects a permission profile that
denies filesystem-root and tool-network access while retaining only Codex's
minimal runtime reads. Approval prompts are disabled for this bounded
non-interactive run. `--output-schema` constrains the response, and the exact
previewed payload is written only to stdin. Input and output are bounded to
256 KiB, stderr is not returned as a publishable result, and timeout/overflow
termination kills the dedicated process group. The runner fails closed on
non-Unix platforms until equivalent process-tree termination is available. It
is wired into
`:KeyInsightsAnalyze`; real Codex invocations remain opt-in and are not run by
ordinary CI.

Codex responses are accepted only through the structured suggestion validator.
The validator bounds the document, rejects duplicate JSON keys and unknown
fields, requires measured evidence and a completed collision check, and binds
every metric value and reported mapping ID to the exact summary and keymap
snapshot used for the request. Markdown is rendered only from that validated
boundary; raw response text is never published as a report.

The same validation-and-render boundary is exposed for local orchestration:

```sh
key-insights suggestions <summary.json> [--input <suggestions.json|->] [--output <report.md|->]
```

The command reconstructs the report-time snapshot from sanitized attribution,
requires mapping proposals to report exact and prefix collisions in either
direction, and emits deterministic Markdown only after contextual validation.
`:KeyInsightsAnalyze` uses this command after Codex exits; it never opens the
raw wire-format JSON as the user-facing result.

Private input readers retain separate fail-closed bounds: 16 MiB for generated
summaries, 1 MiB for keymap snapshots, and 256 KiB for Codex suggestions. This
keeps large valid attributed summaries usable without broadening the AI input
boundary.

Rendered Markdown has a separate 1 MiB bound because escaping can expand a
valid 256 KiB structured response; the Neovim process runner captures that
output with the same explicit limit.
