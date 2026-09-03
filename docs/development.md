# Development

## Toolchain

Use the Nix development shell when possible. The repository uses Rust and Lua
for the analyzer and Neovim collector, `uv` for Python contract harnesses, and
`pkfire` for project tasks.

```sh
nix develop
pkf list
pkf run check
```

Useful focused tasks include `pkf run test:rust`, `pkf run test:lua`,
`pkf run test:e2e`, `pkf run test:forward`, and `pkf run test:release`.
The complete checks also include Rust formatting, Clippy, headless Neovim
tests, packaging contracts, Codex plugin contracts, and the release contract.

For a flake-level check, run:

```sh
nix flake check --all-systems --no-update-lock-file
```

## TDD workflow

Changes follow this loop:

1. Explore the existing implementation and public contracts.
2. Add a focused regression test and confirm it fails (Red).
3. Implement the smallest change that makes it pass (Green).
4. Refactor while keeping the full relevant suite green.

Rust changes should be formatted with `cargo fmt --all` and checked with
`cargo clippy --workspace --all-targets -- -D warnings`. Neovim behavior should
be exercised headlessly through the project tasks.

## Privacy and determinism

- Collection is explicit opt-in and must not start implicitly.
- Raw key logs, Insert-mode text, command/search contents, and file paths must
  not be introduced as default capture.
- Raw JSONL, local reports, private paths, authentication material, and raw
  Codex responses must remain outside the repository.
- AI integrations may receive only the bounded sanitized payload after explicit
  confirmation.
- Local analysis and report rendering must remain deterministic and independent
  of AI services.

Any change to collection, schemas, payload boundaries, or release behavior needs
corresponding contract coverage and documentation updates.

## Documentation and release status

Public documentation is in English. The [documentation index](README.md) lists
the supported contracts. The v0.1.0 release is published. Publication actions
for future releases remain explicit maintainer operations as described in
[releasing](releasing.md).
