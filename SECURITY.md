# Security and privacy

`nvim-key-insights` is designed to keep collection and analysis local. Collection
is opt-in, raw key logging and text-bearing input are not enabled by default,
and the optional Codex workflow is confirmation-gated and accepts only a bounded
sanitized payload.

## Reporting a vulnerability

Do not include raw JSONL, generated reports, private filesystem paths,
authentication material, or other sensitive data in a public issue. If GitHub
Security Advisories are enabled for this repository, use the private advisory
workflow. Otherwise, contact a repository maintainer privately before opening a
public issue and share only the minimum reproduction details needed to
investigate.

Include the affected version or revision, operating system, Neovim version,
reproduction steps, and a proposed impact assessment when safe to do so.

## Privacy reports

Reports about unexpected collection or boundary violations should identify the
command or component involved without attaching local session data. The
[collector lifecycle](docs/collector-lifecycle.md), [event schema](docs/event-schema.md),
and [schema compatibility policy](docs/schema-compatibility.md) define the
intended data boundaries.
