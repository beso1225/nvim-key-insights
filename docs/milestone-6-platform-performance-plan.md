# Milestone 6 platform and performance plan

Status: in progress on `codex/m6-platform-performance`.

Roadmap issue: [#27](https://github.com/beso1225/nvim-key-insights/issues/27)

## Objective

Complete Milestone 6 by enforcing the resource and platform contracts of the
privacy-safe workflow delivered in Milestone 6-A. This slice runs the existing
supported systems natively in CI, bounds large-directory cleanup and analyzer
resources, and replaces the misleading collector callback benchmark with
representative deterministic and coarse timing gates.

All fixtures are synthetic and offline. This slice does not collect real usage,
invoke a real Codex service, tune ergonomic thresholds from personal logs,
publish a release, or add a new supported platform. Those activities remain
Milestone 7 work.

## Existing baseline and gaps

Milestone 6-A exercises the public Neovim commands, shutdown, crash recovery,
retention and purge, local reporting, and a mocked Codex workflow. The flake
already publishes `x86_64-linux`, `aarch64-linux`, and `aarch64-darwin`. Before
this slice, CI executed only on `x86_64-linux`; the native matrix added here now
executes all three existing systems. The analyzer has deterministic cardinality
and byte limits, discovery and purge have scan limits, and the collector pending
queue is bounded. The remaining implementation gaps are:

- retention scans and deletions are unbounded per finalization;
- CLI discovery can retain one open file descriptor per accepted session;
- analyzer resource bounds are not exercised together at the supported limit;
- the callback timing test overflows its pending queue and then measures early
  no-op returns instead of the intended mapped-key path.

## S1: contracts, issue, and CI shape

Status: complete.

- record this plan and track it in a dedicated issue;
- add failing structural tests for native CI coverage of every existing flake
  system and for dedicated resource-test tasks;
- keep the existing Linux project-check job name stable for branch protection;
- reject mutable macOS runner labels, implicit lock updates, credential
  persistence, platform exclusions, and write permissions;
- do not add `x86_64-darwin` or otherwise broaden public platform support.

## S2: native Linux and macOS execution

Status: complete.

- add native `aarch64-linux` and `aarch64-darwin` jobs while preserving the
  existing `x86_64-linux` job;
- run the same lock-preserving flake and uncached project gates on every native
  platform;
- assert that each runner's evaluated Nix system matches the declared system;
- execute Darwin/Linux atomic rename, descriptor-relative filesystem,
  process-group, case-alias, retention, purge, and workflow regressions on their
  native implementations;
- keep the Neovim 0.10 lower-bound job on Linux.

## S3: bounded retention and large directories

Status: complete.

- add Red tests for a hard directory-entry scan budget and a per-finalization
  mutation budget;
- count every directory entry before filtering and stop without retaining an
  unbounded candidate set;
- sort eligible age and count candidates deterministically before mutation;
- defer excess cleanup with a categorical private warning while allowing the
  current session to finalize and unlock;
- prove later finalizations retry and converge while live locks, current logs,
  incomplete artifacts, unrelated entries, and identity replacements survive;
- retain the existing descriptor-relative, quarantine-recovery, owner, mode,
  link-count, and identity checks.

The initial contract uses the existing discovery and purge scale: at most 8,192
scanned entries and 512 deletions in one retention pass. Temporarily exceeding
`max_sessions` is allowed when cleanup is deferred; later finalizations retry.

## S4: analyzer resource contracts

Status: pending.

- add a low-file-descriptor Red test proving that thousands of accepted inputs
  do not require one simultaneously open file per session;
- open and securely revalidate session inputs sequentially or through an
  explicitly bounded handle set;
- exercise 4,096 synthetic sessions without constructing one giant fixture and
  reject the 4,097th session at the documented boundary;
- verify small-chunk streaming, early termination after malformed or oversized
  input, aggregate cardinality limits, the 1 MiB retained-token budget, and the
  100-item report ranking cap;
- assert byte-identical deterministic summary and report output across supported
  input chunking and discovery forms.

Resource correctness is expressed primarily through deterministic handle,
buffer, byte, and cardinality bounds. Shared CI does not use fragile RSS or
microsecond measurements as the sole correctness oracle.

## S5: collector callback budgets

Status: pending.

- replace the queue-overflow benchmark with warmups and measured batches that
  remain in the recording state and process every intended event;
- add deterministic operation-count contracts for excluded, unmapped Normal,
  mapped Normal, sequence-boundary, and Insert aggregation paths;
- prove the callback performs no storage I/O, schedules at most one pending
  flush, returns `nil`, and stays within pending-event and pending-byte limits;
- retain only generous batch-level timing smoke gates and print timing telemetry
  for Linux and macOS without relying on per-callback tail latency under shared
  runner preemption.

## S6: completion gate and documentation

Status: pending.

- run `pkf run --no-cache check` and
  `nix flake check --no-update-lock-file`;
- require green native jobs for all three existing flake systems and the Neovim
  lower-bound job;
- update the implementation roadmap and local workflow documentation with the
  bounded-retention and resource contracts;
- perform branch-wide context-light adversarial reviews focused on privacy,
  data loss, concurrency, resource exhaustion, CI drift, and flaky test oracles;
- mark Milestone 6 complete only after every gate passes.

## Completion gate

Milestone 6 is complete when one reproducible offline project check validates
the command-surface privacy workflow, native Linux and macOS implementations,
bounded directory cleanup, bounded analyzer handles and retained data, and
representative collector callback work without reading private logs or using the
network.
