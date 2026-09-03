# Release procedure

This procedure prepares and validates a tagged GitHub release. It does not
publish to crates.io, and the repository keeps `publish = false` in the Cargo
manifest. Creating a tag or GitHub release is always a deliberate maintainer
operation.

The repository is dual-licensed under either the MIT License or the Apache
License, Version 2.0, at the user's option. The v0.1.0 release was published
after explicit approval. Future releases still require explicit approval for
the publication steps below; the release tooling does not choose or modify
usage rights.

## Release surfaces

A release consists of:

- the immutable Git tag and GitHub-generated source archives;
- the Nix CLI, Neovim plugin, and inert Codex plugin packages evaluated from
  that tag;
- one deterministic versioned Codex plugin archive;
- a sorted `SHA256SUMS` covering the explicitly published archive;
- release notes derived from the matching changelog entry.

Prebuilt CLI binaries, crates.io publication, platform signing, and provenance
attestations are not current release surfaces.

## Prepare a release commit

Start from a clean branch based on current `main`. Review `CHANGELOG.md` and
keep all pending release notes under `## [Unreleased]` until the release date is
known.

If the package version changes, update only the synchronized mirrors:

```sh
UV_CACHE_DIR="${TMPDIR:-/tmp}/nvim-key-insights-uv-cache" \
  uv run --python-preference only-system python scripts/release.py \
  bump --from 0.1.0 --to 0.2.0
```

The command updates Cargo, Cargo.lock, and the Codex plugin manifest. Nix reads
the Cargo version directly. It does not commit, tag, or publish. Inspect the
complete diff before continuing.

Move the reviewed Unreleased notes under a dated release heading:

```sh
UV_CACHE_DIR="${TMPDIR:-/tmp}/nvim-key-insights-uv-cache" \
  uv run --python-preference only-system python scripts/release.py prepare-changelog \
  --version 0.1.0 --date 2026-08-22
```

This command is also non-publishing and refuses a mismatched version, invalid
date, duplicate release entry, concurrent edit, symlink, or malformed
changelog. A hard process termination cannot be made transactional across a
multi-file version bump; if a machine stops during `bump`, use Git to inspect
and restore the working tree before retrying.

Run every local gate before committing the release preparation:

```sh
nix develop --no-update-lock-file --command pkf run --no-cache check
nix flake check --no-update-lock-file
uv run --python-preference only-system python scripts/release.py \
  check --tag v0.1.0 \
  --nix-system aarch64-darwin \
  --nix-system aarch64-linux \
  --nix-system x86_64-linux
```

Review version/schema mutations, privacy boundaries, archive allowlists,
workflow permissions, failure preservation, and the generated checksums. Commit
the reviewed release preparation on a pull request and merge it before tagging.

Build the release assets from the reviewed tree with the commit timestamp as the
reproducible archive epoch. The output directory must not already exist:

```sh
epoch="$(git show -s --format=%ct HEAD)"
nix develop --no-update-lock-file --command \
  uv run --python-preference only-system python scripts/release.py build-artifacts \
  --version 0.1.0 --epoch "$epoch" --output-dir dist
(cd dist && shasum -a 256 --check SHA256SUMS)
```

`release.py build-artifacts` resolves one Git `HEAD` commit, disables replacement
objects, and reads the five inert Codex plugin files from that immutable object
database. It requires the plugin working tree to match that commit, normalizes
an uncompressed USTAR archive, and atomically publishes the complete versioned
archive plus `SHA256SUMS` without replacing an existing output directory. It
never packages the CLI, Neovim collector, private logs, reports, local
configuration, repository metadata, or build outputs.

## Create and publish the tag

Create an annotated tag on the reviewed merge commit:

```sh
git tag -a "v0.1.0" -m "nvim-key-insights 0.1.0"
git push origin "v0.1.0"
```

The tag-only release workflow re-runs the version, schema, test, artifact, and
permission contracts with read-only access. Only its dependent publication job
receives `contents: write`. It refuses a tag/version mismatch and refuses to
replace an existing GitHub release. The tagged commit must already be reachable
from `main`; do not tag an unmerged release-preparation branch.

Before pushing a release tag, configure one active repository tag ruleset named
`immutable-release-tags`. It must target `refs/tags/v*.*.*`, have no bypass
actors or exclusions, and enable both deletion and non-fast-forward update
protection. The publication job verifies this ruleset through the GitHub API and
fails closed when it is absent or weaker; this closes the tag check/use race.
Create a protected GitHub environment named `release` with required reviewers,
and add an expiring fine-grained secret named `RELEASE_RULESET_TOKEN`. The token
needs repository Administration write permission because GitHub hides
`bypass_actors` from callers without ruleset write access; grant no Contents
permission. It is exposed only to the environment-gated, checkout-free ruleset
verification step. The separate write-capable publication job also contains one
repository-independent shell step and does not run a checkout or external
action.

Confirm the published archive checksum and release notes before announcing the
release. Do not move or reuse a pushed release tag.

## Failure recovery

If validation fails before publication, leave the tag and release untouched,
fix the release preparation through another reviewed commit, and use a new
version/tag when the original tag has already been pushed. An unpushed local tag
may be deleted and recreated after its target is corrected.

If publication partially fails, inspect the workflow logs and the existing
GitHub release. The workflow must not overwrite it on rerun. Complete recovery
with an explicitly reviewed GitHub operation; do not weaken checks, reuse an
asset name, or expose the write token to build/test steps.

## Upgrade and rollback

Users should pin the same tag for the CLI, Neovim plugin, and optional Codex
plugin source. Review the [schema compatibility policy](schema-compatibility.md)
before upgrading. Preserve private finalized event logs until the new analyzer
has regenerated and published a valid report pair.

For rollback, pin the previous immutable tag again and regenerate derived
summary/report/Codex artifacts with that matching toolchain. Do not convert a
newer unknown schema by editing its version number, and do not reuse untrusted
suggestions across different summaries.
