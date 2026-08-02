# Adversarial Review Remediation Plan

This document tracks the remediation of findings from an independent review of
the complete `codex/deterministic-reporting` branch. It is the source of truth
for the order, scope, tests, and completion criteria of this hardening work.

## Working rules

- Address exactly one finding at a time in the order below.
- Use TDD for every finding: reproduce the failure, implement the smallest
  complete fix, then refactor and run the full project checks.
- Do not mark an item complete until its focused regression test and the full
  `pkf run check` task pass.
- Preserve the privacy-first contract. Recovery metadata must remain local,
  private, bounded, and must not contain editor input or raw logs.
- Prefer fail-closed behavior when filesystem identity or recovery state cannot
  be proven.
- Record any scope change or newly discovered dependency in this document
  before implementing it.

## Progress

| ID | Priority | Finding | Status |
| --- | --- | --- | --- |
| R1 | P1 | Recovery cleanup can become permanently unrecoverable after a second crash | Complete |
| R2 | P1 | Recovery sidecar creation is not failure-atomic | Complete |
| R3 | P1 | An output ancestor can be replaced after path resolution | Complete |
| R4 | P2 | Alias spellings can reverse lock order on case-insensitive filesystems | Complete |
| R5 | P2 | Total session duration silently saturates on overflow | Complete |
| R6 | P3 | Abrupt termination can leave staged output files indefinitely | Complete |

## R1: Make recovery cleanup restartable

### Failure sequence

1. A paired publication is interrupted after one public output is replaced.
2. A later invocation restores the last indexed destination.
3. Recovery removes that destination's index and only backup.
4. The process stops before removing the shared active transaction marker.
5. The next invocation sees an active transaction that claims a previous output
   existed, but no backup remains, and recovery fails permanently.

### Red test

Interrupt recovery after the final destination backup has been cleaned but
before shared journal cleanup. A subsequent invocation must complete cleanup
without requiring the already-consumed backup and without changing the restored
public outputs.

### Intended fix

Introduce an explicit durable rollback-complete transition before consuming the
last recovery evidence. Startup recovery must distinguish an active rollback
from a completed rollback whose remaining work is metadata cleanup. Every step
after that transition must be idempotent.

### Completion criteria

- Recovery can be interrupted after either destination restore and after either
  destination cleanup.
- Repeated recovery converges to the same previous output pair.
- No active marker can require a backup that an earlier recovery step already
  removed.

### Result

Completed with a durable pair-level rollback decision. Recovery publishes this
decision before restoring or consuming any destination backup. A destination
with a recovery index continues the selected rollback, while a leftover
rollback marker without indexes is treated as restartable metadata cleanup and
never requires an already-consumed backup. Focused tests interrupt recovery
after both the first and the last destination cleanup.

## R2: Publish sidecars failure-atomically

### Failure sequence

A short write, ENOSPC condition, or sync failure occurs while writing a recovery
index or marker directly at its final name. The error path restores outputs and
discards backups, but a partial sidecar remains and blocks every later run.

### Red test

Inject write and sync failures for both index and marker publication. No final
sidecar may become visible, and a retry must succeed without manual cleanup.

### Intended fix

Write and sync a private temporary sidecar, publish it with a non-replacing
atomic operation, and sync the containing directory. On failure, remove only
the temporary file; never replace an unrelated final entry.

### Completion criteria

- Partial index and marker contents are never visible at final paths.
- Occupied final sidecar names are preserved and cause a closed failure.
- Retry succeeds after every injected pre-publication failure.

### Result

Completed with private same-directory staging and atomic no-replace
publication. Sidecar contents are fully written and synced before the final
name can become visible. Production uses handle-relative `linkat` publication
followed by removal of the temporary link. The pathname-only test seam uses
`renameat2(RENAME_NOREPLACE)` on Linux, `renamex_np(RENAME_EXCL)` on macOS, and
non-replacing rename behavior on Windows; unsupported targets fail closed.
Focused tests cover a partial persistence failure followed by a successful
retry and preservation of an occupied final entry.

## R3: Bind output operations to resolved directories

### Failure sequence

After an output parent is canonicalized, another process renames that directory
and installs a same-name symlink. Later pathname-based create, link, or rename
operations follow the replacement ancestor and modify a different directory.

### Red test

Replace a resolved output ancestor between resolution and staging/publication.
The operation must fail without creating or replacing files in either the moved
directory or the symlink target.

### Intended fix

Perform output operations relative to verified directory handles where the
platform supports them, and verify directory identity immediately around every
publication transition. Unsupported platforms must fail closed rather than
claim equivalent protection.

### Completion criteria

- Ancestor replacement is detected for staging, backup, marker, and final
  publication operations.
- Leaf and ancestor symlink attacks are both covered.
- Existing output and input files survive unchanged on rejection.

### Result

Completed with verified directory handles retained from output resolution.
Staging, publication locks, backups, recovery indexes and markers, output
replacement, rollback, commit cleanup, and startup recovery now perform their
filesystem mutations relative to those handles. Directory identity is checked
at phase boundaries, while handle-relative operations prevent a replacement
ancestor from redirecting work between checks. Platforms without the required
directory-handle operations fail closed.

Regression tests replace an output ancestor before recovery, before staging,
between lock acquisitions, before publication, after the first output is
published, and after a recovery index is read. They verify that attacker
directories remain untouched and that interrupted output pairs are restored in
the originally opened directory. Existing leaf-symlink rejection tests remain
in place. A follow-up review also added post-open regular-file verification for
publication locks and post-link identity verification for captured backups.

### Implementation structure follow-up

The CLI entrypoint now contains several distinct filesystem responsibilities.
Defer the mechanical split until R4 is complete because R4 changes both the
lock representation and the filesystem-identity primitives that define the
module boundary. Before starting R5, extract the stable boundaries into
`secure_fs`, `publication`, and `recovery` modules in a behavior-neutral commit.
This keeps the R4 security diff reviewable without mixing it with file moves and
prevents R5 and R6 from extending the current monolithic entrypoint.

Status: complete. The entrypoint now retains CLI parsing and path resolution,
while directory-handle primitives, publication orchestration, recovery state,
and unit tests live in dedicated source files.

## R4: Derive lock order from filesystem identity

### Failure sequence

Two invocations use different case or normalization spellings for the same two
physical lock files. Bytewise pathname sorting produces opposite acquisition
orders, so each process holds one lock while waiting forever for the other.

### Red test

On filesystems that report the spellings as aliases, start two publishers with
opposite alias spellings and assert that both complete within a bounded time.
Skip only when the test filesystem proves that the names are distinct.

### Intended fix

Resolve each opened lock to a stable filesystem identity before blocking on the
complete set, or use a single destination-set lock whose identity is invariant
under filesystem aliasing. Never hold one blocking lock while discovering the
order of another.

### Completion criteria

- Lock ordering is identical for every spelling of the same physical files.
- Existing exact-path serialization remains covered.
- No unbounded wait is possible when two output sets overlap.

### Result

Completed by separating lock discovery from lock acquisition. Every candidate
lock is opened and revalidated before any blocking lock is taken. Candidates
are then sorted and deduplicated by their stable `(device, inode)` identity,
which gives every invocation the same total order regardless of pathname case
or normalization spelling. Lock opens use non-blocking file descriptors and
still reject symlinks, non-regular files, unsafe permissions, and path swaps.

The regression suite proves the physical order is strictly identity-sorted on
all supported test filesystems. On filesystems where case variants are aliases,
it also constructs two opposite pathname orders and verifies that both resolve
to the same physical lock order. Existing concurrent publication coverage
continues to verify exact-path serialization.

## R5: Define duration overflow behavior

### Failure sequence

Multiple individually valid sessions produce a mathematical duration total
larger than `u64::MAX`. Saturating addition silently reports `u64::MAX`, making
the deterministic summary inaccurate without an error.

### Red test

Analyze two valid sessions whose durations overflow `u64`. The analyzer must
return an explicit deterministic error and publish no outputs.

### Intended fix

Replace saturation with checked addition and a dedicated analysis error. Keep
the serialized summary field as `u64`; do not silently change the public schema.

### Completion criteria

- Exact totals through `u64::MAX` remain valid.
- Overflow is reported with a stable error variant and message.
- The CLI leaves existing output artifacts unchanged on overflow.

Status: complete. The accumulator uses checked addition and returns
`SessionDurationOverflow` before output staging. Regression coverage verifies
the exact `u64::MAX` boundary, deterministic overflow reporting, and preservation
of existing CLI outputs.

## R6: Bound retention of orphan staged outputs

### Failure sequence

The process is terminated after a staged output is synced but before normal
unwind cleanup. The private temporary file remains indefinitely. Repetition
retains report-derived data and consumes disk space.

### Red test

Create staged files that simulate an abruptly terminated prior process. A later
invocation must remove only provably owned, stale staging artifacts while
preserving live or unrelated files.

### Intended fix

Give staged artifacts a versioned, bounded naming/metadata contract and perform
safe startup scavenging under the publication locks. Cleanup must validate file
type, ownership assumptions, age or process liveness, and directory identity.

### Completion criteria

- Stale owned staging files are eventually removed.
- Current-process and unrelated files are never removed.
- Cleanup is bounded in both directory work and retained metadata.

Status: complete. Staged outputs use a bounded versioned name containing the
creating process ID. Startup recovery scans at most 1,024 entries per output
directory and removes at most 128 artifacts per pass while holding the
publication locks. Removal requires an exact versioned name, a dead process,
an age of at least 24 hours, current-user ownership, mode `0600`, a regular
file with one link, and identity revalidation through the opened directory
handle. Repeated passes provide eventual cleanup without unbounded retained
metadata.

## Final verification

After all items are complete:

1. Run `pkf run --no-cache check` inside the Nix development shell.
2. Run `nix flake check --no-update-lock-file`.
3. Review the complete `main...HEAD` diff again, including all crash-state
   transitions introduced by this plan.
4. Request a new context-light adversarial review before merging.
