# nvim-key-insights

[![CI](https://github.com/beso1225/nvim-key-insights/actions/workflows/ci.yml/badge.svg)](https://github.com/beso1225/nvim-key-insights/actions/workflows/ci.yml)

Privacy-first Neovim usage collection and deterministic local analysis.

This repository is in its initial implementation phase. The intended system has three parts:

1. a Neovim 0.10+ Lua collector;
2. a local Rust analyzer that produces `summary.json` and `report.md`;
3. an optional Codex skill that reads only the sanitized summary.

Raw key logging, Insert-mode text, command/search text, and file paths are disabled by default. Terminal, prompt, special, and sensitive buffers are excluded from collection.

## Development

Enter the reproducible development shell and run the test suite:

```sh
nix develop
pkf run test
```

`pkf run test:e2e` builds the Rust analyzer and exercises the complete local
collector-to-report workflow in headless Neovim without network access.
Individual tasks are available through `pkf list`.

## Deterministic analyzer

Analyze a complete finalized JSONL stream without an AI service:

```sh
cargo run --bin key-insights -- analyze session-1.jsonl session-2.jsonl \
  --summary summary.json \
  --report report.md
```

One or more finalized inputs are accepted in the supplied order. The command validates every input before creating outputs and rejects duplicate filesystem identities. `summary.json` contains only aggregated counts and bounded rankings of sanitized tokens; it excludes session IDs, project IDs, raw sequences, Insert text, and command/search contents. `report.md` is rendered deterministically from the same in-memory summary.

To analyze every finalized collector session in its owned directory, use the
mutually exclusive discovery form:

```sh
cargo run --bin key-insights -- analyze \
  --session-dir /path/to/sessions \
  --summary summary.json \
  --report report.md
```

Directory discovery considers only private regular files in the current
`nvim-key-insights-<session_id>.jsonl` namespace. It is bounded and ordered by
ASCII filename, and it does not follow session-directory or entry symlinks.
Incomplete, lock, legacy, and unrelated entries are ignored. Legacy logs remain
available through explicit positional input paths.

## Collector lifecycle

The collector can be loaded with lazy.nvim without starting collection automatically:

```lua
{
  "beso1225/nvim-key-insights",
  opts = {
    report = {
      analyzer = "/path/to/key-insights",
      directory = "/path/to/private/reports",
    },
  },
}
```

The current implementation provides these commands:

- `:KeyInsightsStart` starts a new session or resumes a paused session;
- `:KeyInsightsPause` detaches the input callback and flushes pending events;
- `:KeyInsightsStop` writes `session_end`, flushes, and detaches the callback;
- `:KeyInsightsStatus` displays collection state and whether a report job is running;
- `:KeyInsightsReport` asynchronously analyzes finalized sessions and opens the new report;
- `:KeyInsightsOpenReport` opens the existing report without running analysis;
- `:KeyInsightsPurge` previews collector-owned session artifacts and asks before deletion;
- `:KeyInsightsPurge!` skips that prompt but retains every ownership and race check.

Each session is written under `stdpath("state")/key-insights/sessions/` with owner-only file permissions. Incomplete sessions retain a `.jsonl.part` suffix and are not analyzer inputs; a log becomes `.jsonl` only after its `session_end` is durable. Finalized logs are retained for at most 30 days and the newest 100 sessions by default. Collection never starts implicitly. A `VimLeavePre` handler closes an active session.

By default, the analyzer is `key-insights`, and reports live under
`stdpath("state")/key-insights/reports/`. The plugin verifies that directory and
sets owner-only permissions before passing paths as argv without a shell. It
allows one report process at a time. An active collector's
`.jsonl.part` file remains excluded. After a successful exit, the plugin opens
only fresh, bounded, valid outputs; analyzer errors keep the current editor view
and previously published artifacts.

Purge considers only private, single-linked regular files in the collector
namespace. Active sessions, live owners, malformed reservations, symlinks,
hard links, special modes, directories, and unrelated entries remain untouched.
The result reports removed, protected, skipped, and failed counts. Purge is
local-only and is refused while a report process is running.

See [Local collection and reporting](docs/local-workflow.md) for ordering,
discovery, output, purge, and recovery contracts.

## Repository layout

```text
lua/key-insights/             Neovim collector modules
tests/lua/                    Headless Neovim tests
crates/key-insights-cli/      Deterministic Rust analyzer
docs/                         Public design contracts
```

## Status

The current implementation covers privacy-safe collection, bounded retention and
validation, deterministic multi-session reports, asynchronous Neovim report
commands, explicit bounded purge, and a headless local-workflow privacy test.
Normal, Visual, and Operator-pending input becomes bounded typed-key sequences;
Insert and Select input becomes text-run counts and timing. Command/search
contents and mapping expansions are discarded. Mapping attribution, richer
ergonomic metrics, and Codex integration remain incremental TDD work.

See the [implementation roadmap](docs/implementation-roadmap.md) for the ordered
remaining milestones and the
[Milestone 2 implementation plan](docs/milestone-2-mapping-attribution-plan.md)
for the next mapping-attribution work.
