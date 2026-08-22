# Milestone 5 release contract plan

Status: in progress on `codex/m5-release-contract`.

## Objective

Finish Milestone 5 with a deterministic, privacy-safe release contract for the
already packaged CLI, Neovim plugin, and optional Codex plugin. A release tag
must identify one package version, document every supported persisted and
derived schema, produce an allowlisted reproducible Codex plugin archive, and
publish only after read-only validation succeeds.

This slice does not publish the first release. It makes a future `vX.Y.Z` tag a
reviewable, fail-closed operation.

## Scope

This branch will:

- define the Cargo package version as the canonical release version;
- verify that Cargo.lock, the Nix packages, the CLI, and the Codex plugin
  manifest expose the same version;
- add an explicit, non-destructive version update command;
- document the compatibility and regeneration policy for event schema 1,
  summary schema 3, keymap snapshot 1, Codex payload 1, and Codex suggestions 1;
- add a changelog and tagged install, upgrade, rollback, and release procedures;
- build a deterministic archive from the existing five-file Codex plugin
  allowlist and generate sorted checksums;
- add a tag-only GitHub Actions workflow that validates with read-only
  permissions before a separate publication job receives `contents: write`;
- update the roadmap only after every completion gate passes.

The following work remains outside this slice:

- choosing a repository license, because that changes public usage rights and
  requires an explicit owner decision;
- publishing the initial release or creating a tag;
- prebuilt multi-platform CLI archives, platform signing, and provenance
  attestations;
- crates.io publication;
- macOS CI, performance budgets, and large-session-directory tests from
  Milestone 6;
- real-log tuning and release approval from Milestone 7;
- new schema versions, migration converters, raw capture, filetype capture, or
  project identity collection.

## Release and compatibility decisions

### Version authority

`crates/key-insights-cli/Cargo.toml` is the canonical package version. The Nix
flake reads that TOML value instead of maintaining a second literal. The Codex
plugin manifest must retain a static SemVer value because Codex consumes the
JSON directly, but contract tests keep it byte-for-byte synchronized with the
Cargo version. Cargo.lock and the CLI `--version` output are also checked.

Repository releases use stable `X.Y.Z` versions and exact `vX.Y.Z` tags. The
marketplace references the local plugin manifest and must not introduce another
version field.

### Schema lifecycle

Package SemVer and data schema versions are independent. Unknown schema or
contract versions continue to fail closed rather than being coerced.

- Event logs are durable inputs. Event schema 1 remains readable until a future
  release provides a tested parallel reader or an explicit converter. Removing
  support requires a package major release.
- Summaries and Markdown reports are derived artifacts. Regenerate them from
  supported event logs instead of migrating the private derived files.
- Keymap snapshots are ephemeral report-time inputs. Capture a fresh snapshot.
- Codex payloads and suggestions are bounded handoff artifacts. Regenerate the
  payload with the matching CLI and validate new suggestions against the exact
  private summary.

Any future schema bump must land its reader, fixtures, privacy regression,
compatibility-table update, changelog entry, and upgrade instructions together.

### Release artifacts

The initial automated artifact is a versioned archive of the inert Codex plugin
tree plus a sorted `SHA256SUMS`. Archive entries come from an exact allowlist;
symlinks, executables, collector logs, summaries, reports, suggestions, local
configuration, repository metadata, and build outputs are forbidden.

Tagged source archives and the existing Nix packages remain the CLI and Neovim
plugin distribution surface until prebuilt binary signing and cross-platform
release testing are designed.

## TDD slices

### S1: version and release contract

Red:

- require valid stable SemVer across Cargo, Cargo.lock, Nix, plugin, and CLI;
- require an exact `v${version}` tag when release validation is requested;
- reject duplicate marketplace version state;
- require release documentation, artifact tooling, and a tag workflow.

Green:

- parse the canonical Cargo manifest structurally;
- derive the Nix version from Cargo TOML;
- add one release-contract validator and a safe version update command;
- replace duplicated regex checks with the shared contract.

Review the mutation paths for partial version updates, malformed SemVer,
unexpected Cargo.lock entries, and dirty working-tree preservation.

### S2: schema compatibility contract

Red:

- require every public schema and nested contract version in one compatibility
  table;
- compare the table with Rust constants, JSON Schema `const` values, and the
  byte-identical standalone skill copies;
- preserve explicit unknown-version rejection tests at each boundary.

Green:

- add the compatibility, regeneration, deprecation, and future-bump policy;
- link it from analyzer, event-schema, installation, and release docs.

Review that no migration path reads more private data or relaxes strict parsing.

### S3: changelog and public release documentation

Red:

- require `CHANGELOG.md`, release instructions, supported installation forms,
  rollback instructions, and a release entry matching any requested tag;
- reject the current untagged-install wording from a tag release.

Green:

- add a Keep a Changelog compatible history beginning with `Unreleased`;
- document version preparation, review, tag creation, release failure recovery,
  Nix/lazy.nvim/Codex upgrades, and rollback;
- keep `publish = false` and state that crates.io is not a release surface.

Review every claim against implemented commands and package outputs.

### S4: deterministic allowlisted artifact

Red:

- build twice with the same version and epoch and compare bytes;
- verify canonical entry order, path prefix, timestamps, uid/gid, modes, and
  plugin manifest version;
- reject symlinks, executable files, unexpected files, and version mismatch;
- verify sorted checksums cover exactly the published archive.

Green:

- implement a repository-local artifact builder using only the five-file Codex
  plugin allowlist;
- make output staging atomic and avoid overwriting an existing artifact on
  validation failure.

Review archive extraction paths and all filesystem race/failure paths.

### S5: restricted tag workflow

Red:

- require a `v*.*.*` tag trigger and reject pull-request release execution;
- require SHA-pinned checkout/upload/download actions with checkout credentials
  disabled;
- require `contents: read` for validation and isolate `contents: write` to the
  publication job;
- reject tag/version mismatch and implicit Nix lock updates before upload;
- refuse to overwrite an existing release.

Green:

- validate and test in a read-only job;
- transfer only the contract-validated archive and checksum file through a
  GitHub artifact;
- publish in a dependent minimal-permission job with `gh release create`.

Review token reachability, untrusted code execution, artifact substitution,
re-run behavior, and partial publication.

### S6: completion

- run `pkf run --no-cache check` in the Nix development shell;
- run `nix flake check --no-update-lock-file`;
- run release-contract mutation and reproducibility tests independently;
- perform context-light adversarial review and repair every P0-P2 finding;
- mark Milestone 5 complete only after the final review is clean.

## Completion gate

One command validates version synchronization, schema compatibility, changelog
coverage, deterministic artifact contents, workflow permissions, and all
existing project tests without publishing, tagging, using network access, or
reading collector data.
