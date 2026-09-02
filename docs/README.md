# Documentation

This directory contains the user-facing contracts and maintainer documentation
for `nvim-key-insights`. Start with the installation guide if you are setting up
the plugin, or with the development guide if you are contributing.

## For users

- [Installation and configuration](installation.md) — lazy.nvim, Nix, Codex
  integration, options, and upgrade guidance.
- [Local collection and reporting](local-workflow.md) — command ordering,
  discovery, outputs, purge, and recovery.
- [Analyzer](analyzer.md) — deterministic summaries and reports from finalized
  JSONL sessions.
- [Collector lifecycle](collector-lifecycle.md) — opt-in collection states and
  shutdown behavior.
- [Storage and retention](storage-retention.md) — private storage, ownership,
  retention, and cleanup rules.

## Data and privacy contracts

- [Event schema](event-schema.md) — versioned collector events and allowed
  fields.
- [Input aggregation](input-aggregation.md) — content-blind input categories,
  bounds, and deterministic aggregation.
- [Mapping attribution](mapping-attribution.md) — API-backed mapping evidence
  without storing mapping implementations.
- [Schema compatibility](schema-compatibility.md) — compatibility rules for
  analyzer, plugin, and Codex payload schemas.

## Optional Codex integration

The Codex workflow is optional and confirmation-gated. The analyzer produces a
bounded sanitized payload locally; only that payload may cross the integration
boundary. See the Codex sections in [installation](installation.md) and
[analyzer](analyzer.md), and the verification record in
[release readiness](release-readiness.md).

## Maintainers

- [Development](development.md) — repository structure, TDD workflow, commands,
  and privacy requirements for changes.
- [Releasing](releasing.md) — release preparation, artifact checks, and
  publication steps.
- [Release readiness](release-readiness.md) — the completed v0.1.0 candidate
  audit and publication record.
- [Changelog](../CHANGELOG.md) — user-visible changes and release history.

The v0.1.0 release is published. Any tag, push, or release operation for a
future version remains an explicit maintainer decision.
