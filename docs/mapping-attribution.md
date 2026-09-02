# Mapping attribution contract

Status: complete.

This document records the observed `vim.on_key` behavior used by the
privacy-safe attribution design. It complements the public
[event schema](event-schema.md) and the collector lifecycle contract.

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
- represented by canonical tokens that have passed the validation limits.

Until that canonicalizer exists, the decision function returns a new
allowlisted object containing only mode and scope. It deliberately does not
return the candidate left-hand side. Extra candidate fields are discarded. Zero,
multiple, sparse, unstable, inexact, or text-bearing-mode candidates return no
attribution.

The bounded canonical tokens and mapping identities below define the public
normalization boundary. The collector continues to discard mapping expansions
until a validated mapping resolver is connected to the decision function, and
emits no `mapping_use` events until then.

## Canonical mapping model

The snapshot adapter queries only `n`, `x`, and `o` mappings and derives the
normalized mode from that query, not from the API dictionary's `mode` field.
This prevents generic mapping metadata from broadening Visual collection into
text-bearing Select mode. Global entries come from `nvim_get_keymap`; eligible
loaded buffer-local entries come from `nvim_buf_get_keymap`.

Each raw API dictionary is immediately reduced to its `lhsraw` value. Fields
such as `rhs`, `callback`, `desc`, `sid`, `lnum`, `buffer`, and source metadata
are never copied, inspected, encoded, or included in errors. `lhsraw` passes
through `keytrans` and the same tokenizer used by collector events. Invalid
UTF-8, control bytes, unsplit tokens, malformed arrays, and resource-limit
violations fail the whole in-memory snapshot without returning a partial model.

The initial limits are:

- 256 listed buffers;
- 4,096 raw mapping entries across global and buffer-local queries;
- 4 KiB for one raw or canonical left-hand side;
- 64 canonical tokens per left-hand side;
- 256 encoded bytes per token;
- 1 MiB for the encoded snapshot.

Buffer enumeration does not load buffers. Invalid, unloaded, configured-excluded,
and unconditionally sensitive buffers are not queried for local mappings.

## Mapping identity

The `mapping-v1` digest preimage is the concatenation of byte-length-prefixed
values in this order:

1. the literal `mapping-v1`;
2. normalized mode;
3. scope;
4. decimal token count;
5. each canonical token in order.

For a value `s`, the length-prefixed representation is `<decimal byte
length>:<s>`. The ID is `mapping-v1:` followed by the complete 64-character
lowercase SHA-256 digest. Hash output with the wrong shape is rejected. If one
digest is encountered for two different sanitized tuples, collection fails with
a content-free identity-conflict error.

Identical tuples are deduplicated, including the same buffer-local binding seen
in several eligible buffers. Global and buffer-local bindings with the same mode
and left-hand side remain separate. Entries sort by normalized mode,
lexicographic token array, scope, and mapping ID. The encoder writes a fixed JSON
field order and a trailing newline, revalidates each ID against its tuple,
rejects duplicate or conflicting tuples, and canonically sorts a defensive copy.
Equivalent sanitized mapping sets therefore produce identical bytes regardless
of input order.

## Collector attribution lifecycle

Before attaching `vim.on_key`, the collector builds a bounded, sanitized
baseline for the current eligible buffer. The baseline contains all supported
global mappings and the current buffer's local mappings, with buffer-local
entries taking precedence. Both sides of a prefix family are marked ambiguous.
No right-hand side or other implementation metadata survives this step.

For an attributable callback, the collector canonicalizes `typed` once for the
ordinary `key_sequence` path and passes only those tokens and the normalized
mode to the resolver. The resolver performs one exact effective `maparg` lookup
and requires its canonical tuple and scope to match the baseline. Buffer changes,
API failures, malformed results, prefix ambiguity, and mapping mutation all
produce no mapping event. The post-mapping callback value is reduced to a
content-free evidence enum and never reaches the resolver.

If an exact lookup detects that its baseline became stale, the collector marks
attribution unavailable immediately and schedules at most one baseline refresh
outside `vim.on_key`. Input remains fail-closed until that refresh succeeds. A
transient refresh failure is retried only after later eligible input, without
performing Neovim API calls or storage I/O inside the input callback. Excluded
buffers do not trigger refresh attempts.

Start and resume establish fresh baselines outside the input callback. Explicit
flush establishes a fresh baseline after the pending event write; pause, stop,
and failed start discard it. A mapping added or changed after the last baseline
may therefore miss its first observation, but stale state is never used to
guess an attribution. Every confirmed action remains ordinary sequence evidence
and additionally emits exactly one schema-v1 `mapping_use` event.

## Report-time snapshot transport

Each report request collects and encodes a fresh sanitized model after the
private report directory has been verified. The plugin passes the bounded JSON
directly to the analyzer's standard input and supplies `--keymap-snapshot -` as
one literal argument. No snapshot pathname is created, retained, replaced, or
unlinked. The 1 MiB payload is fixed before process launch, and a concurrent
request on the same report instance is rejected before collecting another
snapshot.

Collection or encoding failures stop the launch with a content-free
notification. The CLI reads the complete bounded payload into memory before
strict parsing, so later editor state cannot mutate the analysis input. Manual
CLI use may instead provide an owner-only, single-linked regular file path.

All current summaries use schema version 3. Snapshot-aware summaries contain
all joined mapping bindings plus conservative `potential_buffer_shadowing`
collision groups and guarded mapping-coverage evidence. Event-only analysis
continues to accept schema-v1 logs and emits the same schema-v3 shape without
snapshot-derived attribution.

## Callback performance budget

The headless resource suite measures excluded, ordinary Normal, mapped Normal,
and Insert paths in warm batches. The mapped path uses a real registered mapping
and the production resolver, including live `maparg()` validation. Deterministic
operation-count checks are the primary contract: every intended callback must
reach the expected aggregation or mapping event, return `nil`, coalesce a burst
to one scheduled flush, and perform no session-writer method inside the callback.

CI also enforces a deliberately portable 500-microsecond median batch ceiling
for each path. The suite prints the operating system, architecture, Neovim
version, sample shape, and min/median/max batch averages as telemetry. Deferred
flush and report work remain outside the timed region, and individual tail
latency is not a shared-runner correctness oracle.
