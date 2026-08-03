# Milestone 1 local workflow implementation plan

Status: complete

Roadmap milestone: [Complete the local analysis workflow](implementation-roadmap.md#milestone-1-complete-the-local-analysis-workflow)

## Delivery status

- S1 stateful multi-input analysis API: complete.
- S2 CLI positional multi-input support: complete.
- S3 bounded session-directory discovery: complete.
- S4 Neovim report commands: complete.
- S5 explicit purge: complete.
- S6 documentation and end-to-end test: complete.

## Objective

Connect the existing one-file-per-session collector to the deterministic analyzer
without requiring users to concatenate JSONL manually. Then expose that workflow
through safe Neovim commands and add an explicit, bounded purge operation.

This milestone does not add new event fields, ergonomic metrics, mapping
attribution, or Codex execution.

## User-visible outcome

After this milestone, a user can:

1. collect multiple finalized sessions;
2. run one local command to analyze them as a single deterministic dataset;
3. generate and open `summary.json` and `report.md` from Neovim;
4. inspect collector status and report locations;
5. explicitly purge collector-owned local session artifacts.

Collection remains stopped by default. Analysis and purge never send data over
the network.

## Contract decisions

### Explicit files remain the analyzer primitive

The CLI will first support one or more positional input files:

```text
key-insights analyze <input.jsonl>... --summary <summary.json> --report <report.md>
```

The existing one-input invocation remains valid. Inputs are resolved before any
analysis output is staged. Canonical duplicate inputs are rejected rather than
counted twice, and neither output may alias any input.

Inputs are analyzed in the order supplied. The same ordered inputs must produce
byte-identical outputs. Directory discovery will sort finalized collector files
before passing them to the same multi-input primitive.

### Validation spans the complete input set

Each input file must contain one or more complete sessions and must end with no
active session. Session IDs cannot be reused across files in the same analysis.
The existing 4,096-session limit, retained-token budget, cardinality limits, and
checked duration arithmetic apply to the complete input set rather than to each
file independently.

Validation errors should identify the failing input and its line within that
input. A later invalid or unreadable input must leave existing summary and report
artifacts unchanged.

### Directory discovery is a separate boundary

The CLI will add an explicit directory form after the multi-file primitive is
stable:

```text
key-insights analyze --session-dir <directory> \
  --summary <summary.json> --report <report.md>
```

The first version will discover only regular files matching the current
`nvim-key-insights-<session_id>.jsonl` finalized namespace. It will ignore
`.jsonl.part`, lock files, output artifacts, symlinks, subdirectories, and
unrelated names. Discovery will be bounded, sorted by the ASCII collector
filename, and performed without following a replaced directory or leaf entry.

Explicit files and `--session-dir` are mutually exclusive in one invocation.
Legacy filenames can still be analyzed explicitly; automatic legacy discovery
requires a separate compatibility decision because the collector only recognizes
legacy names safely in its owned default directory.

### Neovim command responsibilities

- `:KeyInsightsReport` discovers finalized sessions and runs deterministic local
  analysis. It does not pause or stop an active collector; the current `.part`
  session remains excluded until the user stops it explicitly.
- `:KeyInsightsOpenReport` opens the configured Markdown report without running
  analysis.
- `:KeyInsightsPurge` previews and confirms deletion of collector-owned session
  artifacts. A force form may skip confirmation but not ownership checks.
- `:KeyInsightsAnalyze` is reserved for the later optional Codex workflow and is
  not registered in this milestone.

The analyzer process will be launched with an argv API, not a shell command
string. The default execution should be asynchronous so Neovim editing remains
responsive. Concurrent report commands for one configured output pair should be
coalesced or rejected with a clear status rather than queued without bound.

### Output locations

Defaults will live under `stdpath("state")/key-insights/reports/` and remain
configurable. Paths are local configuration only and never enter event data or a
sanitized summary. Output directory creation must use private permissions where
the platform supports them.

## Delivery slices

### S1: stateful multi-input analysis API

Refactor validation into a state object that can consume several `BufRead`
sources while retaining global session-ID and limit state.

Red tests:

- two individually valid files produce the existing combined fixture output;
- a session ID reused in the second file is rejected;
- more than 4,096 sessions spread across files is rejected;
- a file ending with an active session fails before the next file is consumed;
- duration and token limits apply across file boundaries;
- the error reports the source index and source-local line.

Green implementation:

- add an analyzer entry point for an ordered iterator of readers;
- keep `analyze_jsonl` as a compatibility wrapper over one source;
- keep one accumulator for the complete input set;
- do not concatenate input bytes or retain event streams.

Refactor gate:

- the single-input and multi-input paths share one validator and accumulator;
- existing public error text remains stable where source context is not added.

### S2: CLI positional multi-input support

Red tests:

- the existing one-file syntax remains valid;
- two input paths produce a combined report;
- zero inputs, duplicate canonical inputs, `.jsonl.part`, and non-regular inputs
  fail;
- either output aliasing any input fails before recovery or publication;
- failure to open or validate the last input preserves existing outputs.

Green implementation:

- parse positional inputs until the first option;
- resolve all inputs and outputs before recovery and analysis;
- reject duplicates by filesystem identity and canonical path;
- open inputs read-only, pass ordered buffered readers to S1, and map returned
  source indices back to local paths in CLI errors.

Explicit input symlinks retain the current canonicalization behavior. The stricter
no-follow rule applies to automatic directory discovery, where the user did not
select each leaf explicitly.

### S3: bounded session-directory discovery

Red tests:

- finalized collector files are returned in lexical filename order;
- incomplete, lock, report, unrelated, symlink, FIFO, and directory entries are
  ignored without being opened;
- a replaced directory or discovered file fails closed;
- discovery work and accepted file count are bounded;
- an empty directory produces a stable no-finalized-sessions error.

Green implementation:

- add `--session-dir` as a mutually exclusive input source;
- reuse anchored directory-handle helpers where possible;
- return resolved, verified files to the S1 analysis API.

### S4: Neovim report commands

Red tests:

- setup registers the report and open-report commands exactly once;
- argv preserves spaces and special characters without shell interpolation;
- missing analyzer, non-zero exit, and malformed output produce useful
  notifications while preserving the previous report;
- status exposes an active report job without exposing local paths in events;
- two concurrent invocations do not create an unbounded process queue.

Green implementation:

- add a small process-runner module around `vim.system`;
- add report configuration and private default output directories;
- use `--session-dir` and the same deterministic CLI path as terminal users;
- open the report only after a successful process exit.

### S5: explicit purge

Red tests:

- preview selects only collector-owned `.jsonl`, `.jsonl.part`, and reservation
  artifacts covered by the storage contract;
- symlinks, hard-linked files, unexpected modes, subdirectories, and unrelated
  entries survive;
- deletion is bounded and refuses a changed directory or leaf;
- an active session and live concurrent owner remain protected;
- cancellation performs no writes.

Green implementation:

- share namespace parsing and identity checks with storage retention;
- separate pure target selection from mutation;
- require confirmation unless the user explicitly supplies the force form;
- report removed, protected, skipped, and failed counts.

### S6: documentation and end-to-end test

- Update README command examples and configuration.
- Document multi-input ordering, discovery exclusions, output locations, purge,
  and recovery behavior.
- Add one headless test that finalizes two collector sessions, invokes the Rust
  analyzer, and verifies the deterministic aggregate without network access.
- Add seeded secret checks across JSONL, summary, report, notifications, and
  subprocess argv.

## Security and privacy invariants

- Never pass `.jsonl.part` to the analyzer automatically.
- Never follow a symlink discovered inside the session directory.
- Never interpolate paths into a shell command.
- Never create outputs until every selected input validates.
- Never include session IDs, project IDs, source paths, Insert text, command or
  search contents, or mapping expansions in summary/report payloads.
- Never let purge delete an entry that is not provably collector-owned.
- Keep scans, accepted inputs, subprocess concurrency, retained validation state,
  and cleanup work bounded.
- Reuse the existing failure-atomic output publication and crash recovery rather
  than adding a second publication mechanism.

## Compatibility

- Event schema version 1 is unchanged.
- Summary schema version 1 is unchanged for the same event set.
- Existing `key-insights analyze <input> --summary ... --report ...` scripts keep
  working.
- Multi-input analysis changes only aggregate counts and rankings according to
  the additional explicitly selected sessions.
- No collector behavior or privacy default changes in S1 through S3.

## Verification for every slice

1. Run the focused Red test and record the expected failure.
2. Implement only enough behavior for the slice to pass.
3. Run `cargo fmt`, Rust unit/integration tests, and Clippy.
4. Run the headless Lua suite when Lua or command integration changes.
5. Run `nix develop path:. --command pkf run --no-cache check`.
6. Run `nix flake check` without changing `flake.lock`.
7. Review input boundaries, output preservation, path aliases, privacy payloads,
   and cleanup behavior.

## Milestone completion criteria

- Several finalized collector files can be analyzed through the CLI and Neovim
  without manual concatenation.
- Outputs are deterministic and remain failure-atomic.
- All automatic discovery and purge operations are bounded and link-safe.
- No data crosses an AI or network boundary.
- Public documentation matches the implemented commands and failure behavior.
- The complete project check and an independent context-light adversarial review
  pass before merge.
