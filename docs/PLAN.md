# DEVELOPMENT PLAN — logreel

status: **draft for sign-off** (2026-10-31) · execution NOT started
audience: the implementing AI agent; this file is the contract for every session
inputs: 41_ADR-001.md (decision), 51_SEGMENT_FORMAT.md + A1–A4 (format), 50_TARGET_ARCHITECTURE.md (design), 70_SYNTHESIS.md (borrows)
target: a standalone library repo, **outside** the proxy repo; no integration in this stage

## 0. Settled inputs

| item | decision |
|---|---|
| name | **logreel** (repo, umbrella crate, prefix for all crates) |
| repository | one new monorepo, separate git history, private on a personal GitHub account |
| crates | 6: `logreel` (facade), `logreel-core`, `logreel-derive`, `logreel-store`, `logreel-file`, `logreel-store-redb` |
| first milestone | spec + conformance fixtures before any implementation code |
| performance | absolute numbers only; no A/B against the proxy until integration; budgets frozen after the first measurement |
| MSRV | 1.98, pinned toolchain, edition 2024 |
| license | MIT |
| publication | no crates.io before 0.1.0; `cargo publish --dry-run` is part of M6 |
| proxy repo | read-only reference; not modified, not integrated |

## 1. Why one repository

Six crates that share one format change together: a header field touches the spec, the reader, the fixture set and the facade in the same commit. Five repositories would turn each contract change into a cross-repo sequence, which is exactly the failure mode observed in slog (`slog-async` never received core's `flush`) and logforth (47 releases, feature renames, two version streams). One repo, one version, one CI, one issue tracker; `cargo publish` still publishes six independent crates. If a backend ever needs a different cadence, it can be split later — splitting is cheap, merging is not.

The proxy repository stays untouched: integration is a later stage with its own plan.

## 2. Repository layout

```
logreel/
├── Cargo.toml                 workspace: 6 crates + xtask + bench + examples
├── Cargo.lock                 committed (library workspace, reproducible CI)
├── rust-toolchain.toml        channel = "1.98.0"
├── rustfmt.toml, clippy.toml, deny.toml
├── LICENSE                    MIT
├── README.md                  what it is, 20-line example, format promise
├── CHANGELOG.md               keep-a-changelog, filled at release
├── CONTRIBUTING.md            the agent rules from §5
├── docs/
│   ├── SPEC.md                normative format spec (51 + A1–A4), versioned
│   ├── FORMAT.md              reader's guide: how to write a third-party parser
│   ├── adr/0001-five-crates.md
│   ├── design/architecture.md (50 condensed)
│   └── research/              optional: 70 + six reports, for provenance
├── crates/
│   ├── logreel/               facade: re-exports + features
│   ├── logreel-core/          envelope, Event, Filter, scope, pipeline, Render
│   ├── logreel-derive/        proc-macro
│   ├── logreel-store/         BatchSink, Store, Scan, durability, errors, EncodedBatch
│   ├── logreel-file/          segments, frames, rotation, retention, probe, scan
│   └── logreel-store-redb/    optional backend
├── fixtures/
│   ├── v1/                    committed golden segments + expected outputs
│   └── README.md              how to regenerate, hash table
├── bench/
│   ├── harness/               measurement binary
│   ├── RESULTS.md             generated numbers
│   └── BUDGET.md              frozen budgets + tolerance
├── examples/
│   ├── events/                sample domain: 5 events, manual + derived
│   └── quickstart/            emit → file → dump in 30 lines
└── xtask/                     fixtures, ci, bench, release helpers
```

Workspace rules: single workspace version (`[workspace.package] version`), path dependencies carry `version = "=0.1.0"`, `resolver = "3"`, `edition = "2024"`, `rust-version = "1.98"`, `license = "MIT"`. Every crate starts with `#![forbid(unsafe_code)]` and `#![warn(missing_docs)]`. No dependency is added without a one-line justification in the commit message.

## 3. Format contract and fixtures (the core of the plan)

`docs/SPEC.md` is normative and self-contained: byte layout, invariants, reader guarantees, retention rules, error taxonomy. It is derived from 51 with amendments A1–A4 and must be implementable by someone who has never seen this repository.

**Fixture set v1** (each case is a committed `*.seg` plus expected `dump.txt` / `probe.json`):

| id | case | expected behaviour |
|---|---|---|
| F1 | one batch, three kinds, mixed levels | full decode |
| F2 | many batches, varint boundaries, empty payload | full decode |
| F3 | unknown kind `0x9001` | skip by `payload_len`, continue |
| F4 | schema_hash mismatch | probe ok; dump raw; no error |
| F5 | torn tail mid-frame | stop at last good frame, report offset |
| F6 | corrupted frame CRC | stop, report offset, do not resync |
| F7 | zero-length file, bad magic | refuse with the file name |
| F8 | clock jump: newer segment has older `created_ns` | retention deletes nothing |
| F9 | zstd frame (feature on) | decompress, CRC over uncompressed bytes |
| F10 | 56-byte header with `producer_id = 0` | accepted, raw attribute absent |

The generator is deterministic: fixed timestamps, ids, payloads, no randomness, no wall clock. `cargo xtask fixtures gen` rewrites `fixtures/v1`; `cargo xtask fixtures check` fails if the committed bytes or the hash table differ. A format change means: SPEC version bump → new fixture directory → old fixtures stay and keep passing.

## 4. Milestones

Estimates are indicative agent-days, not calendar promises. A milestone starts only when the previous one's acceptance passes.

| id | milestone | est. | exit criterion |
|---|---|---|---|
| M0 | bootstrap | 0.5–1 | `cargo xtask ci` green on an empty workspace; repo pushed |
| M1 | spec + fixtures | 2–3 | SPEC complete; F1–F10 committed; generator reproduces hashes |
| M2 | store + core | 4–6 | emit→batch→in-memory sink; filter, drop, shutdown tests pass |
| M3 | derive + example domain | 2–3 | derived and manual events are byte-identical; roundtrip properties pass |
| M4 | file backend | 5–7 | F1–F10 pass; crash matrix green; rotation and retention tested |
| M5 | measurements | 2–3 | `bench/RESULTS.md` + frozen `BUDGET.md`; size report |
| M6 | redb, facade, release | 2–4 | docs build; dry-run publish of 6 crates in order; tag `v0.1.0` |

Total: roughly 18–27 agent-days.

### M0 — bootstrap

1. `git init -b main` in `logreel/`; `.gitignore` (`/target`, `*.seg.tmp`).
2. Workspace manifest, toolchain, lints, rustfmt/clippy/deny config.
3. Six empty crates with `lib.rs` doc comments and the dependency edges from §2; `xtask` with `ci`, `fixtures`, `bench` subcommands (stubs).
4. `LICENSE` (MIT), `README.md`, `CONTRIBUTING.md` with §5 verbatim.
5. Copy `SPEC.md` draft, `docs/adr/0001`, `docs/design/architecture.md` from the research artifacts (they are the source of truth for now).
6. CI: `fmt`, `clippy -D warnings`, `test`, `doc`, `deny`; later jobs stubbed.
7. Acceptance: `cargo xtask ci` passes; `git log` has one signed-off bootstrap commit.

Scaffold commands (agent may run these):

```bash
mkdir -p logreel && cd logreel && git init -b main
cargo new --lib crates/logreel-store --name logreel-store
cargo new --lib crates/logreel-core --name logreel-core
cargo new --lib crates/logreel-derive --name logreel-derive
cargo new --lib crates/logreel-file --name logreel-file
cargo new --lib crates/logreel-store-redb --name logreel-store-redb
cargo new --lib crates/logreel --name logreel
cargo new --lib xtask --name xtask
```

### M1 — spec and fixtures

1. Rewrite `SPEC.md` from 51 + A1–A4: every field, offset, endianness, varint encoding, CRC polynomial and coverage, header validation order, frame validation order, unknown-kind skip, raw mode contract, retention order, durability modes.
2. Add `docs/FORMAT.md`: a third-party implementer's checklist ("read header, verify CRC, iterate frames, decode batch, decode record").
3. Implement `xtask fixtures gen|check`: a small encoder that does **not** depend on `logreel-file` (independent implementation is the point — two implementations, one spec).
4. Produce F1–F10 with expected outputs; commit hashes.
5. Acceptance: `fixtures check` is deterministic across runs and machines; SPEC has no unresolved `TODO`.

### M2 — store and core

1. `logreel-store`: `EncodedBatch`, `BatchSink`, `Store` (`append`, `sync`, `durability`), `Scan` item types, `StoreError` tri-state, `Durability`.
2. `logreel-core`: `Event` trait; `Filter` + `[Filter; CATEGORY_COUNT]` + interest generation; `Handle`; `log_event!`; bounded channel; writer loop with batch encoding; `Render`; `Trap`; `LogOverflow`; `Scope` + `ScopeState`.
3. In-memory sink for tests only.
4. Tests: gate on/off; drop counting at a full channel; shutdown drains and joins; seq is monotonic per handle; batch boundaries at size/time; no allocation after warm-up (use a counting allocator in the test).
5. Acceptance: the above pass; `cargo xtask ci` green.

### M3 — derive and example domain

1. `#[derive(Event)]` with attributes `kind`, `category`, `level`, `codec = "prost"`.
2. Generated code: consts, `kind()`, `encode`, `decode` dispatch; no prost dependency in the macro crate.
3. `examples/events`: five events, one written by hand and four derived.
4. Tests: encode→decode roundtrip (proptest or a deterministic generator), duplicate-kind compile-fail test, kind-range check.
5. Acceptance: hand-written and derived events produce identical bytes for the same values.

### M4 — file backend

1. Writer: header (56 B, A1), frames with `len` and CRC32C (in-crate implementation with known test vectors), batch payload passthrough.
2. Rotation: day boundary plus a **disabled** size seam; retention: day count + total-byte cap, oldest-first, active file protected; clock guard.
3. Durability: `Eventual` (write), `Immediate` (write + `sync_data`), `OnWarn` (sync on level warn and above).
4. Reader: `probe` (header + histograms), `scan` iterator, `dump` with raw fallback; tri-state errors.
5. Crash matrix: helper binary writes a segment, parent kills it with SIGKILL at deterministic offsets, reader must satisfy: no panic, no infinite loop, either full records or a reported offset.
6. Acceptance: F1–F10 and the crash matrix pass; a 24-hour simulated rotation test (injected clock) passes.

### M5 — measurements

1. Harness measures: emit ns/record (gated and enabled), batch encode ns/record, write MB/s and bytes/record, scan MB/s, probe µs, binary size of `examples/quickstart` for feature sets (`core`, `core+file`, `all`), RSS with 1000 scopes.
2. `bench/RESULTS.md` generated, with CPU model, toolchain, feature set, median of 5.
3. `bench/BUDGET.md`: freeze the first measurement ±10%; CI runs a smoke benchmark only, full runs are manual.
4. Acceptance: numbers reproducible within the stated tolerance on the same machine; a regression check script exists.

### M6 — redb, facade, release

1. `logreel-store-redb` behind the `redb` feature; same `Store` contract; no format change.
2. Facade `logreel` re-exports core, derive, file behind `text`, `ring`, `file`, `redb`, `zstd`; `default = []`.
3. Docs: README with a working 30-line example, crate-level docs, `cargo doc` clean.
4. `cargo publish --dry-run` in dependency order: store → core → derive → file → store-redb → logreel.
5. Tag `v0.1.0`; CHANGELOG filled.
6. Acceptance: dry-run succeeds for all six; docs build without warnings; tag exists.

## 5. Agent operating rules

These are part of the repository contract; copy them into `CONTRIBUTING.md` at M0.

1. **One milestone per session.** Read this plan, `docs/SPEC.md` and the milestone section; then implement only that milestone.
2. **Never touch** `repo/` (the proxy), the router, or any deployment configuration. The proxy is a read-only reference for behavior, not a build target.
3. **Never push, publish, tag, or create a GitHub repository** without an explicit instruction in the current session.
4. **No new dependency** without a one-line justification in the commit message; no `cargo install`; no network installs. Prefer the standard library.
5. **Commit small, conventional commits**, one concern per commit; never rewrite pushed history.
6. **Format changes are gated**: SPEC version bump + new fixtures + ADR amendment in the same change; old fixtures keep passing.
7. **Stop at acceptance.** Do not gold-plate, do not start the next milestone, do not refactor adjacent code.
8. **Report format**: `DONE Mx <commit range> <n tests> <bench|n/a> <next>` or `BLOCKED Mx: <reason>`. No silent partial work.
9. **Ask before**: adding a crate, changing a public signature after M4 freeze, changing the format, adding a backend, or anything that touches CI secrets.
10. **Numbers are recorded, not claimed**: if a benchmark is not run, write UNVERIFIED in `RESULTS.md`.

## 6. Git, GitHub, release

Branching: trunk-based. `main` is protected (linear history, required CI, squash merges). Short-lived branches `m2-store-core`, `m4-crash-matrix`. Tags: annotated `v0.1.0`.

Creation, once the user approves and provides the account:

```bash
# option A: gh CLI authenticated as the user
gh repo create <user>/logreel --private --source=. --remote=origin --push

# option B: empty repo created in the browser
git remote add origin git@github.com:<user>/logreel.git
git push -u origin main
```

GitHub Actions: `ci.yml` (fmt, clippy, test on 1.98, doc, deny, semver on PRs), `cross.yml` (armv7-musleabihf build + size), `bench.yml` (`workflow_dispatch` only). Secrets: `CARGO_REGISTRY_TOKEN` at M6 only.

Release checklist: CHANGELOG → version bump in one place → `cargo xtask ci` → `cargo publish --dry-run` ×6 → tag → `cargo publish` ×6 in order → GitHub release notes.

## 7. Performance policy

Absolute numbers only, per the decision. The proxy baseline is deliberately out of scope until integration, so "within noise" cannot be evaluated here. Therefore: the first measurement becomes the budget (±10%), and every later run is compared against that budget, not against another implementation. This is weaker than D20 and is recorded as such; the A/B gate moves to the integration stage.

## 8. Risks and mitigations

| risk | mitigation |
|---|---|
| spec drifts from code | the fixture generator is an independent implementation; two implementations must agree |
| scope creep through borrows | P0 borrows only in M2–M4; P1/P2 wait for a written reason |
| benchmark noise | median of 5, same machine, tolerance ±10%, CI smoke only |
| API frozen too early | public API freeze at M4; before that, internal churn is allowed |
| redb churn | isolated crate, feature-gated, no format coupling |
| agent wandering | milestone acceptance + stop rule + report protocol |

## 9. Out of scope for this stage

Integration into tgwsproxy; router deployment; crates.io publication; additional backends (JSONL, sqlite, syslog); Windows support; mmap; compression by default; spans or distributed tracing; multi-producer files.

## 10. Definition of done for v0.1.0

- SPEC v1 frozen; F1–F10 committed and passing.
- Six crates compile on 1.98; `cargo xtask ci` green; clippy and deny clean.
- Emit path: no allocation after warm-up, drop-and-count verified by test.
- File backend: crash matrix green; retention and clock-guard tests green.
- `bench/RESULTS.md` and `bench/BUDGET.md` exist with real numbers.
- Facade example works from a clean checkout in ≤30 lines.
- Dry-run publish succeeds in dependency order; `v0.1.0` tagged.

## 11. Immediate next action

On the user's go: execute **M0** only, then stop and report. M0 needs two facts from the user: the GitHub account name and whether the repository should be created by the agent (`gh` authenticated) or by the user (agent pushes to an existing remote).
