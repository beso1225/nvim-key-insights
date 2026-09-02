# Contributing

Thanks for helping improve `nvim-key-insights`. Read the [documentation index](docs/README.md)
and the [development guide](docs/development.md) before changing collector,
analyzer, or integration behavior.

## Development workflow

Use the Nix shell and run the project checks through `pkfire`:

```sh
nix develop
pkf run check
```

Follow the repository's TDD loop: explore the current contract, write a failing
regression test, implement the smallest passing change, and then refactor. Use
headless Neovim tests for collector behavior and Rust unit/integration tests for
analyzer behavior.

## Privacy requirements

Privacy is a product boundary, not just a documentation preference. Do not add
raw key logging, Insert-mode text, command/search contents, or file paths to the
default collection path. Do not commit local JSONL sessions, reports, private
paths, authentication material, or raw Codex responses. New sensitive capture
requires explicit opt-in and regression coverage.

Keep analysis deterministic and independent of AI services. Codex integrations
may receive only the bounded sanitized summary after explicit confirmation.

## Pull requests

Before opening a pull request:

- run the focused tests and `pkf run check` when practical;
- update the relevant public contract in `docs/`;
- keep README, documentation, issues, pull requests, changelog entries, and
  commit messages in English;
- describe privacy, compatibility, and user-visible effects clearly;
- avoid unrelated formatting or generated private artifacts.

Please report security-sensitive issues privately as described in
[SECURITY.md](SECURITY.md), rather than publishing private data in an issue.
