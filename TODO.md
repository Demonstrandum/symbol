# TODO

Review follow-ups. Items 1-6 are from the code review; 7-8 are the reported
cosmetic regressions in the HTML listing.

## 1. Provenance tests inherit the developer's global git config

`crates/symbol/generation/provenance_tests.rs:155` isolates `user.name` and
`user.email` but not `commit.gpgsign` or `core.hooksPath`, so `commit()` fails
on any machine that signs commits or installs global hooks.

- [x] Fully isolate `run_git` from user, global, and system git config.
- [x] `cargo test -p symbol --example symbol-generate` passes on a machine with
      `commit.gpgsign=true`.

## 2. An upgraded v11 database has a different schema than a fresh one

`database/schema.rs:915-932` migrates `sites.tree_hash` and
`undo_sites.tree_hash` with `ADD COLUMN` / `DROP COLUMN` / `RENAME COLUMN`,
which cannot express `NOT NULL DEFAULT x'00..'` and moves the column to the end
of the table. A fresh database declares both as
`blob NOT NULL DEFAULT x'0000...'` in their canonical position.

Nothing catches the drift: catalog validation only runs for v6, and there is no
fresh-vs-upgraded comparison at v11.

Constraint discovered while investigating: `PRAGMA foreign_keys=OFF` is silently
ignored inside a transaction, and with foreign keys on, `ALTER TABLE ... RENAME
TO` rewrites `REFERENCES` clauses in every other table. So rebuilding `sites`
requires rebuilding its five referencing tables in the same pass.

- [x] Rebuild `sites` and its referencing closure. Done as a new
      `normalize_v11_schema`, the v11 counterpart of the existing
      `normalize_v6_schema`, which had been written for exactly this defect one
      version earlier.

      Two deviations from the plan, both deliberate:

      - It **drops and recreates** rather than renames. `DROP TABLE` does not
        rewrite anyone's `REFERENCES` clause, so the closure problem disappears;
        rows are parked in unconstrained snapshot tables first because dropping
        `sites` would otherwise cascade into its descendants.
      - Recreation replays all of `tables()` and `indexes()`, every statement of
        which is `IF NOT EXISTS`. The untouched tables are no-ops and the repair
        cannot fall out of step with the canonical schema.

      It is kept **out of the hashed migration program**, like
      `normalize_v6_schema`. Folding it in would change
      `migration_program_hash` for every upgrade path and make databases already
      at v11 fail startup with "schema checksum drift".
- [x] Rebuild `undo_sites` the same way (nothing references it).
- [x] Add a test asserting a v10-upgraded catalog equals a fresh v11 catalog.
- [x] Add the same assertion for the v2, v6, v7, v8 and v9 paths.
- [x] Better than a test: `migrate` now runs the repair under a catalog
      comparison on every path, so "upgraded equals fresh" is enforced at
      runtime and a hard error if the repair is ever incomplete. A database that
      already matches pays only for building the reference schema in memory.
- [x] Extend `empty_catalog_at` to stage versions 9 and 10, which it could not
      reach before.

## 3. Wrong metadata key in the v10 migration arm

`database/migrations.rs:134` deletes `schema.migration.v11`, but `v11` is the
current record key (`migrations.rs:269`). By symmetry with the v6 arm (L93) and
v9 arm (L126) it should delete `schema.migration.v10`. Net effect: a stale v10
row survives forever, and deleting the current key bypasses the drift check
that `ensure_migration_record` exists to perform.

- [x] Delete `schema.migration.v10` in the v10 arm. Root-caused instead: the
      per-arm key was hand-maintained, so `ensure_migration_record` now clears
      the whole superseded set and the individual arms no longer carry a key.
      This also repairs deployments that already went through the buggy arm.
- [x] Regression test that a v10 record is removed and that a pre-existing
      current-version record still triggers drift detection.

## 4. `public_api_freeze.py`'s top-level hash assertion never runs

`tests/public_api_freeze.py:177` guards `baseline_contract_sha256` with
`if len(contract) == len(freeze.baseline_endpoint_sha256)` — 41 vs 31 — so it is
dead. Per-endpoint hashes still work, so the gate is weaker than it reads
rather than absent.

- [x] Make the aggregate hash cover the frozen set unconditionally. It now
      hashes the baseline subset rather than the whole contract, so approved
      additions no longer disable it, and `baseline_contract_sha256` was
      recomputed accordingly.
- [x] Keep the self-test that tampering is rejected, and add one for the case
      only the aggregate can catch: shrinking the frozen baseline.

## 5. `tree_hash` excludes expiry policies

`store/mutation.rs:674` finalizes the tree hash before the expiry section is
appended to the generated `symbol.toml`, so an expiry-only mutation rewrites the
manifest without changing the site ETag.

Investigated: this is coherent rather than broken. `tree_hash` covers user
content (files, allocated entries, aliases), deliberately excludes the generated
manifest, and `/FILES` inventory JSON excludes the manifest too — so the ETag
covers exactly what the inventory reports. Folding expiry into the hash would
change every existing site's tree hash and break stored `If-Match` values for a
non-content property.

- [x] Document the exclusion in `API.md` instead of changing the hash. Also
      confirmed while writing it that `set_expiry_secured` does not bump
      `content_revision` either, so the whole site ETag is untouched -- which
      makes the exclusion more clearly deliberate than it first looked.
- [x] Pin the behaviour with a regression test so it cannot drift silently. The
      test also asserts the manifest really did change, so it cannot pass by
      accident, and that an `If-Match` captured before the `EXPIRE` is still
      accepted.

## 6. Smaller cleanups

- [x] `blobs.bytes` is dead — always written as `Vec::new()` since the external
      blob migration, but still `NOT NULL` in the canonical schema. Documented
      as retired at its definition, with the reason it is kept: a database that
      has not yet run `external_blobs_v1` still has to match this schema.
- [x] `store/mod.rs:1462` loads every legacy inline blob into memory in one
      `load()`. Now reads the hash/size index first and fetches one payload at a
      time, so peak memory is one blob rather than the whole store.
- [x] `MigrationRecord.source_revision` is hardcoded `0`. Records
      `SYMBOL_API_REVISION`. Safe for existing rows: `ensure_migration_record`
      compares the schema and program hashes and never this field.
- [x] `tests/command_matrix.py` runs in `./check` but is absent from the Nix
      flake, so the canonical release gate never runs it. Added to `sdkDocs`.
- [x] `./check` hard-fails on macOS because `chromium` is required but the
      devShell only provides it on Linux. It now probes a few binary names and
      skips just the browser smoke test with an explicit message, matching the
      flake's own Linux gate.
- [x] `generation/mod.rs:493` parses the unrendered TypeScript template with SWC
      and discards the result — a redundant full parse every build. Removed;
      every placeholder is a string literal replaced by another string literal,
      so parsing the rendered source catches the same errors.

## 7. The HTML listing must not strip `.html`

Extension stripping is a URL-cleaning concern only. `browse.rs:299` renders the
pretty name as the visible label, so `about.html` is displayed as `about`. Plain
and JSON listings already keep the stored name.

- [x] Render stored names verbatim in the HTML listing. `listing_entry_name` is
      gone; `pretty_html_name` is now reachable only from `entry_href`, so the
      pretty form cannot leak back into a label.
- [x] Keep hrefs pretty.
- [x] Update `API.md`, which currently documented "pretty links and labels".

## 8. `index.html` should link to the directory

An `index.html` row currently links to `/{site}/{rel}/index` rather than the
directory URL that actually serves it.

- [x] Link `index.html` to the containing directory.
- [x] Link `index.htm` there too, but only when `index.html` is absent, since
      `find_index` (`main.rs:2559`) prefers `index.html`.

## Closing out

- [x] `cargo fmt --all -- --check`
- [x] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [x] `cargo test --workspace --all-targets --all-features`: 298 pass, 0 fail,
      up from 284 with 2 failing
- [x] `./api-version update` for the `API.md` and `generation/` edits:
      0.1.92 revision 93 to 0.1.96 revision 97. `schema.sql` needed no refresh,
      which is the expected result: item 2 changed migration behaviour, not the
      canonical schema.
- [x] `python3 tests/public_api_freeze.py`, `python3 tests/api_contract.py`,
      `SYMBOL_GENERATION_MODE=readonly ./api-version check`, `shellcheck check`,
      and `sh -n` over every shell script

### Canonical Nix gate

`sh nix/check.sh` (`nix flake check --impure`) passes end to end, exit 0, all
21 outputs green:

```
alias-transfer-e2e  all              concurrency-soak   generated-provenance
generated-sources   lifecycle-e2e    package            posix-dash-runtime
posix-static        production-guard public-api-freeze  sdk-docs
sdk-js              sdk-js-real      sdk-mock           sdk-py
sdk-ts              devShells.default overlays.default  packages.{default,symbol}
```

- [x] First run failed one check. `posix-static` runs
      `shellcheck -s sh -o require-variable-braces`, and the chromium probe
      added in item 6 used `$candidate` and `$skip_browser_smoke` unbraced.
      Fixed to `${candidate}` / `${skip_browser_smoke}`; the re-run is clean.
- [x] `command matrix: 31 exact spellings, 258 exact/prefix/substring cases`
      appears in the `sdk-docs` log, confirming the item 6 flake addition
      actually runs in the canonical gate rather than only in `./check`.
- [x] Inside the Nix sandbox the Rust suite reports the same 257 / 1 / 35 / 5 /
      0, so nothing depended on the host toolchain.
- [x] `sdk-browser` is absent on `aarch64-darwin` by design; the flake gates it
      to Linux, which is the same condition the `./check` change now follows.

This run covered everything the local sandbox could not: both e2e shell
suites, the production guard, the concurrency soak, all five SDK suites, the
POSIX static and dash-runtime checks, and the docs gates.

### Not reachable from the local sandbox

Only relevant to running `./check` directly on this machine; the Nix gate above
covers all of it:

- `tsc`, `biome` and `chromium` are not installed here. On macOS the last of
  those is expected, and is exactly what the item 6 change makes survivable.
- macOS `mktemp` with no template ignores `TMPDIR` and uses the confstr temp
  directory, which the sandbox denies, so the shell suites cannot run there.
- The node SDK suites need to bind a loopback socket.
