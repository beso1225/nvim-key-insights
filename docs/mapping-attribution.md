# Mapping attribution contract

Status: M2-S1 callback contract implemented; collector attribution is not yet
enabled.

This document records the observed `vim.on_key` behavior used by the
privacy-safe attribution design. It complements the
[Milestone 2 implementation plan](milestone-2-mapping-attribution-plan.md).

## Callback observations

Headless contract tests exercise the development-shell Neovim version with real
input processing rather than calling the collector callback directly.

| Case | Pre-mapping `typed` behavior | Post-mapping `key` behavior |
| --- | --- | --- |
| Unmapped key | contains the typed key | may equal `typed` |
| Multi-key mapping | contains the complete resolved left-hand side | contains mapped output |
| Recursive or `<Plug>` mapping | contains the original left-hand side | contains the fully resolved output |
| Buffer-local mapping | follows the same grouping as a global mapping | contains mapped output |
| Visual mapping | contains the resolved left-hand side | callback mode is Visual |
| Operator-pending mapping | contains the resolved left-hand side | callback mode is Operator-pending |
| Identity mapping | appears on the first output callback | later output callbacks may have empty `typed` |
| `<Nop>` or Lua callback | contains the resolved left-hand side | representation is not a stable public identity |

The post-mapping value is therefore evidence that input processing occurred, not
a safe or stable mapping identifier. Equality between `key` and `typed` does not
prove that no mapping ran, and inequality does not identify which mapping ran.
Empty `typed` callbacks cannot begin a new attribution.

These observations are regression-tested for Normal, Visual, and
Operator-pending input, direct and recursive mappings, `<Plug>`, `<Nop>`, Lua
callbacks, and buffer-local scope. CI must continue to run the same test suite on
the project-supported Neovim toolchain. Expanding the supported version matrix
requires confirming this contract on the oldest version before relying on new
trace behavior.

## Content-blind callback evidence

The narrow callback adapter accepts `key` and `typed`, rejects either value above
4 KiB, and immediately reduces them to one of five values:

- `typed_same`: non-empty typed input and an equal non-empty post-mapping value;
- `typed_different`: non-empty typed input and a different non-empty
  post-mapping value;
- `untyped`: an expansion callback with no new typed input;
- `typed_without_output`: typed input with no post-mapping value;
- `unsupported`: invalid or oversized callback values.

No returned evidence contains either input string. In particular, mapped
commands, paths, inserted text, callback encodings, and control bytes cannot
cross this adapter as content.

## Fail-closed decision boundary

Content-blind evidence is insufficient by itself. A later API-backed resolver
may confirm an attribution only when it supplies exactly one candidate that is:

- an exact left-hand-side match for the callback's typed input;
- stable across the resolver's observation window;
- in Normal, Visual, or Operator-pending mode;
- global or buffer-local;
- within the documented key-count and token-size limits.

The initial decision limits are 64 left-hand-side tokens and 256 encoded bytes
per token. Later canonicalization may tighten a representation but must not make
these hot-path bounds unbounded.

The decision function returns a new allowlisted object containing only mode,
scope, and left-hand-side tokens. Extra candidate fields are discarded. Zero,
multiple, sparse, unstable, inexact, malformed, text-bearing, or oversized
candidates return no attribution.

M2-S2 will define canonical tokens and mapping identities. M2-S3 will connect
the resolver and decision function to the collector. Until then, the collector
continues to discard mapping expansions and emits no `mapping_use` events.
