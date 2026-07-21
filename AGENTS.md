# Contributor Instructions

## Development workflow

- Use test-driven development: explore, write a failing test, make it pass, then refactor.
- Keep collection privacy-first. New sensitive capture must be explicit opt-in and covered by regression tests.
- Keep analysis deterministic and independent of AI services.
- Do not send raw logs to AI integrations by default. Only sanitized summaries may cross that boundary.

## Tooling

- Run project tasks with pkfire: `pkf run <task>`.
- Use the Nix flake development shell when possible: `nix develop`.
- Keep the existing Rust and Lua implementation languages.
- Use `cargo fmt` and `cargo clippy` for Rust changes.
- Run Neovim collector tests headlessly.

## Public repository language

Write README files, documentation, contributor guides, changelogs, issues, pull requests, and commit messages in English.

## Change safety

- Preserve user changes and avoid destructive Git operations.
- Ask before making a choice that materially changes public behavior, compatibility, licensing, or data handling.
