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

Individual tasks are available through `pkf list`.

## Collector lifecycle

The collector can be loaded with lazy.nvim without starting collection automatically:

```lua
{
  "beso1225/nvim-key-insights",
  opts = {},
}
```

The current implementation provides these commands:

- `:KeyInsightsStart` starts a new session or resumes a paused session;
- `:KeyInsightsPause` detaches the input callback and flushes pending events;
- `:KeyInsightsStop` writes `session_end`, flushes, and detaches the callback;
- `:KeyInsightsStatus` displays the current lifecycle state.

Each session is written under `stdpath("state")/key-insights/sessions/` with owner-only file permissions. Incomplete sessions retain a `.jsonl.part` suffix and are not analyzer inputs; a log becomes `.jsonl` only after its `session_end` is durable. Collection never starts implicitly. A `VimLeavePre` handler closes an active session.

## Repository layout

```text
lua/key-insights/             Neovim collector modules
tests/lua/                    Headless Neovim tests
crates/key-insights-cli/      Deterministic Rust analyzer
docs/                         Public design contracts
```

## Status

The current implementation establishes privacy defaults, strict event construction, a bounded streaming JSONL validator, durable collector sessions, and privacy-safe input aggregation. Normal, Visual, and Operator-pending input is grouped into bounded typed-key sequences; Insert and Select input is reduced to text-run counts and timing. Command/search contents and mapping expansions are discarded. Mapping attribution, deterministic reporting, and Codex integration will be implemented incrementally with TDD.
