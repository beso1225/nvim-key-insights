# Release readiness

Status: release candidate ready; initial release blocked pending explicit approval.

Tracking issue: [#29](https://github.com/beso1225/nvim-key-insights/issues/29)

## Objective

Validate the completed privacy-first workflow with deliberate local forward
tests, tune deterministic recommendations without encoding one person's habits
as universal policy, and prepare an auditable first release candidate.

This release-readiness work does not weaken the existing collection or AI boundaries. Personal
session logs, generated private reports, local paths, authentication material,
and raw Codex responses must never be committed. Raw JSONL and local reports
must never be sent to an AI service. Creating a tag or GitHub release remains a
separate, explicitly approved operation after the release-candidate gate.

## Forward-test contract and synthetic harness

Status: complete.

- add an offline harness that runs the installed/public command workflow only on
  generated synthetic sessions;
- emit a bounded machine-readable inspection manifest without copying JSONL,
  report text, paths, session IDs, or other private content into the repository;
- scan each local and AI-boundary artifact with boundary-specific canaries and
  fail closed on missing, malformed, aliased, oversized, or unsafe artifacts;
- prove the harness requires an explicit temporary/private output location and
  never invokes Codex, accesses the network, or modifies release state;
- keep ordinary CI synthetic and deterministic.

## Deliberate local usage inspection

Status: complete. The local-only inspection harness and synthetic contract
tests are implemented, and a deliberate human inspection of real local usage
completed successfully. The private inspection manifest remains outside the
repository.

- document an explicit opt-in collection window using the shipped privacy
  defaults;
- require a human inspection of private JSONL, sanitized summary, local report,
  and canonical Codex preview as distinct boundaries;
- record only aggregate pass/fail observations and tool versions in the
  inspection manifest;
- reject attempts to place private logs or generated reports inside the source
  tree;
- keep all real-usage execution local and outside ordinary CI.

## Deterministic threshold tuning

Status: complete. A real local aggregate exposed a candidate-cap ordering
problem, and a synthetic regression now preserves observed evidence before
absence-only mapping evidence without changing thresholds or bounds.

- compare real observations with public synthetic fixtures before changing a
  threshold or ranking rule;
- add a failing public regression fixture for every accepted tuning change;
- distinguish product-wide correctness from one user's preferences;
- preserve deterministic output, measurement evidence, collision checks, and
  the options to learn an existing operation or make no change;
- document rejected tuning ideas as local observations rather than silently
  changing defaults.

Accepted tuning change under review:

- A real local aggregate exposed a cap-ordering failure: absence-only mapping
  candidates could fill the shared 100-row limit before observed repeated
  motion evidence was emitted. The public regression fixture reproduces this
  with synthetic data only.
- Ergonomics contract version 2 gives observed `repeated_motion` candidates
  deterministic priority over absence-only
  `current_mapping_unobserved_in_sample` candidates. Thresholds, candidate
  bounds, collision checks, and the existing `learn_existing` / `no_change`
  choices remain unchanged.
- No raw local logs, paths, identifiers, or personal key preferences are
  recorded in this plan.

## Performance forward tests

Status: complete. The public contract test and local-only analyzer measurement
harness are implemented, and a real-session measurement completed on the
current host. The private performance manifest remains outside the repository.

- measure collector callback telemetry and analyzer resource use on deliberate
  local sessions without persisting typed text or paths in measurements;
- compare observations with the deterministic callback, queue, scan,
  descriptor, byte, and cardinality contracts;
- add public synthetic regressions before changing any enforced budget;
- treat machine-specific timing and RSS values as telemetry rather than portable
  correctness thresholds;
- keep measurement artifacts private and bounded.

The callback side reuses the synthetic resource suite: deterministic
operation-count, queue, byte, and cardinality contracts remain enforcement,
while callback timing is printed as machine-specific telemetry. The local
harness measures only aggregate analyzer resource observations from finalized
sessions and removes its temporary report artifacts before returning.

## Optional Codex forward test

Status: complete for the explicit local run on 2026-09-02. The canonical
preview was inspected, the confirmation gate was exercised, and the real
Codex response passed the local evidence/collision validator and deterministic
Markdown renderer. The response schema was adjusted to the Codex structured
output subset; action-specific semantics remain enforced locally.

- require an explicit user invocation and preview of the exact sanitized payload;
- send only the canonical payload produced by `key-insights preview`;
- use saved file-based authentication, the existing empty working directory,
  cleared environment, read-only permission profile, output schema, timeout,
  process-group shutdown, and output bounds;
- validate the response against the exact summary and snapshot before rendering
  deterministic Markdown;
- prohibit raw JSONL, report text, adjacent files, network enrichment, and
  private canaries from entering the subprocess boundary;
- keep a real Codex call out of ordinary CI and require separate opt-in approval.

## Release-candidate audit

Status: complete on 2026-09-02. The explicit Codex decision was exercised on
clean commit `dfafcf3`; the full project check, all-systems flake check, release
contract, and an unpublished plugin artifact build/checksum audit all pass.
Installation, upgrade, schema compatibility, retention, purge, crash recovery,
authentication-boundary, resource, and native-platform contracts were reviewed.

- rerun the full offline project, flake, package, plugin, schema, release, and
  native-platform contracts;
- audit installation, upgrade, schema compatibility, retention, purge, crash
  recovery, authentication limits, and platform support documentation;
- perform context-light adversarial reviews focused on privacy, data loss,
  concurrency, resource exhaustion, recommendation quality, and misleading
  release claims;
- prepare the changelog and version only through the existing release tooling;
- produce reproducible artifacts and checksums without publishing them.

## Initial release

Status: blocked on explicit approval after the audit.

- require a clean, reviewed, merged release-candidate commit;
- require every protected native CI and release-contract check to pass;
- request explicit approval for the exact version, tag, artifacts, checksums,
  changelog, and release notes;
- create the first tag and GitHub release only after that approval;
- verify the published assets and document any rollback or follow-up action.

## Completion gate

Release readiness is complete only when deliberate local forward testing finds no
privacy-boundary regression, every accepted tuning change is backed by a public
deterministic fixture, the release-candidate audit is clean, and the user has
explicitly approved and verified the first release. Until then, the repository
may be release-ready but must not claim that an initial release was published.
