# nvim-key-insights

[![CI](https://github.com/beso1225/nvim-key-insights/actions/workflows/ci.yml/badge.svg)](https://github.com/beso1225/nvim-key-insights/actions/workflows/ci.yml)

Privacy-first Neovim usage collection and deterministic local analysis.

## Overview

`nvim-key-insights` has three local-first components:

- a Neovim 0.10+ Lua collector;
- a Rust analyzer that produces deterministic `summary.json` and `report.md`;
- an optional Codex workflow that reads only a bounded sanitized summary.

Raw key logging, Insert-mode text, command/search text, and file paths are not
captured by the default workflow. Terminal, prompt, special, and sensitive
buffers are excluded from collection.

## Status

The v0.1.0 release is published. See the [GitHub Release](https://github.com/beso1225/nvim-key-insights/releases/tag/v0.1.0)
for the Codex plugin archive and checksum file, and the
[release-readiness audit](docs/release-readiness.md) for the verification
record.

## Quick start

For development:

```sh
nix develop
pkf run check
```

To analyze finalized sessions locally without an AI service:

```sh
cargo run --bin key-insights -- analyze session-1.jsonl session-2.jsonl \
  --summary summary.json \
  --report report.md
```

For plugin installation, configuration, Nix packages, and the optional Codex
skill, see [Installation and configuration](docs/installation.md).

## Neovim commands

The collector is explicit opt-in. The main commands are:

- `:KeyInsightsStart`, `:KeyInsightsPause`, `:KeyInsightsStop`, and
  `:KeyInsightsStatus` for collection control;
- `:KeyInsightsReport`, `:KeyInsightsOpenReport`, and
  `:KeyInsightsAnalyze` for local analysis and the optional confirmation-gated
  Codex workflow;
- `:KeyInsightsPurge` for the bounded, ownership-checked cleanup of collector
  artifacts.

See [Local collection and reporting](docs/local-workflow.md) for command
ordering, discovery, outputs, purge, and recovery behavior.

## Privacy boundary

Sessions are stored in the Neovim state directory with owner-only permissions.
Incomplete sessions remain excluded until their end marker is durable. Reports
are generated locally from aggregated data. Raw JSONL, local reports, private
paths, authentication material, and raw Codex responses must stay outside the
repository.

The optional Codex integration requires explicit confirmation and may receive
only the canonical sanitized payload. Local validators check the response
against the exact summary and mapping snapshot before deterministic Markdown is
rendered.

## Documentation

The [documentation index](docs/README.md) is the starting point for user and
maintainer guides.

- [Analyzer](docs/analyzer.md)
- [Collector lifecycle](docs/collector-lifecycle.md)
- [Development](docs/development.md)
- [Event schema](docs/event-schema.md)
- [Input aggregation](docs/input-aggregation.md)
- [Mapping attribution](docs/mapping-attribution.md)
- [Releasing](docs/releasing.md)
- [Schema compatibility](docs/schema-compatibility.md)
- [Storage and retention](docs/storage-retention.md)
- [Changelog](CHANGELOG.md)

## License

Dual-licensed under either the [MIT License](LICENSE-MIT) or the
[Apache License, Version 2.0](LICENSE-APACHE), at your option. See
[LICENSE](LICENSE) for the project-level notice.

## Repository layout

```text
lua/key-insights/             Neovim collector modules
plugin/                       Neovim command registration
crates/key-insights-cli/      Deterministic Rust analyzer
plugins/nvim-key-insights/    Optional inert Codex plugin and skill
tests/                        Rust, Lua, Python, packaging, and CI contracts
docs/                         Public user, data, and maintainer documentation
```
