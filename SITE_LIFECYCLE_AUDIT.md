# Site lifecycle plan audit

Revalidated on 2026-08-24 against commit `3ac356a` and the current worktree.

Plan: `/home/samuel/.cursor/plans/site_lifecycle_workflows_e7dfcc24.plan.md`

## Verdict

The remediation work closes most of the original audit, but the plan is not
complete as written.

Current disposition of the original 31 findings:

- 18 resolved;
- 12 partially resolved;
- 1 unresolved.

`./check` currently passes:

- 119 Rust tests;
- 18 isolated shell-client tests;
- 20 real installed-client lifecycle E2E workflows;
- formatting and strict Clippy;
- 19 API route markers and 6 parsed JSON examples.

Those passing checks do not prove the remaining response-loss, expiry-rendering,
conformance, documentation, and release-gate requirements described below.

## Unresolved finding

### 15 — COPY/remix creator claims are not persisted before the request

**Status: unresolved.**

The plan requires the client to generate and persist a creator claim before a
retry-sensitive creation request, then send it as `Creator-Claim`. Current COPY
does this in the wrong order:

1. generate the claim (`static/symbol.sh:1123`);
2. send the HTTP request (`static/symbol.sh:1129`);
3. derive the resulting name and persist the claim
   (`static/symbol.sh:1130-1138`).

If the server commits COPY but the response is lost, the process does not know
the generated destination and has not persisted the claim or idempotency
recovery state. Explicit-destination COPY also waits until after the response
despite already knowing the destination.

The managed-remix E2E test proves successful-response attachment, not
response-loss recovery.

Required fix:

- persist pending claim and idempotency state before COPY;
- key generated-destination pending state by an invocation/idempotency record
  until the destination is learned;
- move it atomically to the destination claim record after a successful
  response;
- add a test that commits the server operation and drops the response before
  the client receives it.

## Partially resolved findings

### 3 — mTLS and Tailscale identity are trusted-header adapters

The typed identity kinds now exist and headers are accepted only from configured
trusted peer IPs. This is a meaningful improvement.

It does not literally implement the plan's direct identity-provider contract:

- mTLS identity is supplied through a configured header rather than read from a
  client-certificate fingerprint on the connection;
- Tailscale identity is supplied through a configured header rather than
  resolved through the trusted local daemon/LocalAPI;
- only one identity provider may be configured at startup
  (`src/main.rs:278-280`).

Either implement direct resolvers or revise the plan/API to explicitly define
trusted reverse-proxy adapters as the intended providers.

### 8 — undo kind handling still has compatibility ambiguity

Path PUT now emits `put_file`, and 128-bit hexadecimal undo tokens exceed the
entropy requirement. Those parts are fixed.

Remaining issue: unknown stored undo-kind integers fall back to `Put`
(`src/store.rs:2721-2730`). A migrated or future kind can therefore be
silently mislabeled instead of rejected as unsupported data.

Required fix: make unknown values a checked conversion error and add a migration
compatibility test.

### 10 — secret-free logging is enforced only by omission

Application request spans now record only method and URI, and a unit test
asserts that selected secret header fields are absent. This closes the original
application tracing gap substantially.

Still missing:

- an explicit proxy log policy/filter;
- coverage for all secret-bearing request and response headers;
- coverage proving one-time response bodies/tokens cannot enter logs;
- a regression test at the configured tracing/proxy boundary rather than only
  checking span metadata.

### 12 — ordinary creation claims are sent before creation but persisted after it

Ordinary PUT now generates a client claim and sends `Creator-Claim`, so it no
longer relies on a server-generated receipt.

Without an existing matching manifest, persistence still happens after the
response:

1. generate claim (`static/symbol.sh:998-1002`);
2. send PUT (`static/symbol.sh:1038-1045`);
3. persist recovery claim (`static/symbol.sh:1064-1065`).

The E2E assertion at `tests/lifecycle_e2e.sh:87-89` checks state after a normal
response and is incorrectly described as proving pre-persistence. It does not
test response loss.

Required fix: persist pending claim/idempotency state before sending PUT,
including generated-name PUT, and test a committed operation with a dropped
response.

### 18 — client expiry rendering does not support the complete server shape

Inherited-cap rendering and the `--never` message were added. The server now
correctly returns an `ExpirySiteReport` with an `entries` array.

Remaining client gaps:

- `print_expire_report` treats JSON as one flat report and does not iterate a
  site-wide `entries` array;
- the client never reads `limited_by`;
- multi-policy site reports can display only an arbitrary first matching field;
- `--never` does not clearly name the policy that still limits the effective
  expiry.

Required fix: add typed/specialized site-report and target-report renderers,
selected once, plus tests for multiple site/folder/file policies and
`limited_by`.

### 19 — undo output still differs from the frozen canonical format

Remaining durations are now humanized, but:

- mutation hints print `undo: ... (expires RFC3339)` rather than the frozen
  `undo within 4h: ...` form;
- stack output leaves absolute expiry as raw RFC3339 rather than the planned
  compact UTC presentation;
- tests verify behavior but not the complete golden output.

This is low severity, but it remains a plan/output-contract mismatch until the
plan or implementation is changed.

### 22 — API conformance is only a shallow static check

`tests/api_contract.py` now checks 19 documentation section strings, selected
router source markers, and JSON syntax. Rust handler tests and E2E workflows
provide useful independent coverage.

It still does not implement the Phase 8 gate:

- no generated route/method/header/status inventory;
- no comparison against a typed implementation contract registry;
- no validation of documented response headers and status codes;
- no execution of the documented curl examples unchanged;
- non-JSON examples are not parsed or exercised.

Required fix: define one typed contract inventory consumed by routing/tests and
compare `API.md` against it; execute documented request examples against the
temporary server.

### 23 — command coverage duplicates the registry instead of deriving from it

Exact commands/aliases and representative prefix/substring cases are tested.

The test defines a second hand-maintained registry
(`tests/symbol_client.sh:125-142`) instead of reading
`command_registry()` (`static/symbol.sh:674-693`). It tests only selected
abbreviations and one ambiguity, so a new command can change the full
abbreviation matrix without failing the suite.

Required fix: expose the canonical registry in a machine-readable test mode and
generate every exact, prefix, substring, identity-collapse, and ambiguity
expectation from that source.

### 25 — deletion restoration lacks isolated coverage

The real E2E suite now proves:

- file-delete undo;
- site-pop undo;
- create-then-undo.

That resolves the behavioral concern. The plan explicitly requested focused
Store/handler fixtures; file-delete and create-undo still lack isolated Rust
tests. Keep this partial only as a test-localization/diagnostic-quality gap.

### 27 — the release gate exists but is conditional and not fully documented

`release-check` now combines `./check`, release build, and Nix build. Restart and
live endpoint checks run only with `SYMBOL_DEPLOY_CHECK=1`.

Remaining gaps:

- README documents `./check`, not `release-check` or the deploy flag;
- production restart/live smoke is optional, so a default “release gate passed”
  does not prove the complete Phase 7 gate;
- this revalidation ran `./check`, not the sudo Nix build or production deploy
  path.

Required fix: document release levels clearly and ensure release sign-off uses
the full deploy-enabled invocation where the plan requires it.

### 28 — public documentation still misses plan-required lifecycle topics

`API.md` is comprehensive and the public guide links to it. Keeping the public
page concise is explicitly desirable.

However, plan line 1323 specifically requires concise coverage in
`static/docs.md`. It still omits:

- canonical client `copy`, `remix`, and `move` commands;
- undo execution;
- operational expiry modes;
- management claim/rotate/release;
- token precedence;
- identity-provider behavior;
- sanitization limits;
- conditional PUT and MANAGE curl examples;
- destination-conflict and inverse-action guidance.

Either add concise coverage or amend the plan to move these requirements
exclusively to `API.md`.

### 29 — management/API discovery is improved but incomplete

The public guide now links to `API.md` and mentions
`symbol manage hello --status`.

It still does not provide the management basics explicitly required by the
plan: claim, rotate, release, token precedence, identity behavior, and token
sanitization. This overlaps finding 28 but remains a distinct management
discoverability gap.

## Resolved findings

The following original findings are resolved in the current worktree:

1. site-level expiry inventory shape;
2. incremental expiry path aggregates;
4. management audit source IP;
5. configurable archive limits;
6. unsupported archive suffix response;
7. content-negotiated plain/HTML/JSON FILES output;
9. no-op PUT plain `changed: false` result;
11. successful managed-remix credential attachment;
13. explicit PUT checkout-baseline refresh;
14. remix destination validation before COPY;
16. detailed sync conflict recovery output;
17. sync progress/count/deletion hints;
20. normalized mutation output;
21. dead PUT helper removal;
24. broad real-client lifecycle E2E coverage;
26. v2 migration fixture;
30. duplicate asset-tree removal;
31. service identity configuration examples.

## Additional remediation risks

These are not reopenings of the original findings, but should be tracked:

- partial expiry can call `rebuild_aggregates_locked`, producing an O(n) scan
  after a bulk/partial expiry operation;
- audit IP records the direct socket peer; behind a reverse proxy this is the
  proxy address unless a separately trusted audit-IP design is added;
- unsupported-archive detection relies on dots being forbidden in site names;
- a failed PUT can leave a prewritten claim sidecar in an existing manifest;
- managed remix still intentionally leaves the server copy with cleanup
  guidance if clone fails after COPY;
- `save_response_secrets` remains redundant for remix because post-clone
  sidecar attachment is authoritative.

## Verification evidence and limits

Executed during this revalidation:

- `./check` — passed;
- 119 Rust tests — passed;
- 18 isolated client tests — passed;
- 20 installed-client E2E workflows — passed;
- API checker — reported 19 route groups and 6 JSON examples;
- `git diff --check` — passed;
- shell/Python syntax checks — passed.

Not executed during this revalidation:

- `release-check`;
- sudo Nix build;
- production restart;
- live production endpoint smoke.

Do not claim those gates passed based only on this report.

## Required handoff message

> Read `SITE_LIFECYCLE_AUDIT.md` completely before editing anything. Do not
> rewrite the audit to declare findings resolved. Fix the implementation and
> tests that the audit identifies. A finding may be marked resolved only after
> the exact failure mode is covered by a test that would have failed before the
> fix. In particular, simulate committed operations with dropped responses for
> PUT and COPY claim recovery; do not call a normal successful-response test
> “pre-persistence.” Generate API and command matrices from canonical typed
> sources instead of duplicating strings in tests. Fix multi-entry expiry and
> `limited_by` rendering. Complete or formally amend the public-doc and release
> requirements. Run `./check`, then the full documented release gate. Report
> code changes and test evidence; do not edit this audit except to append
> evidence-backed dispositions.

