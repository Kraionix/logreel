# AGENTS.md

Guidance for AI coding agents (and humans) working in this repository.

## What this project is

`logreel` is a typed, allocation-free event-logging library for Rust services, with an append-only
binary segment format that other programs can read without your codec. It is a record pipeline, not a
formatting framework:

- one `Event` type per process, a `u16` kind per variant, `encode`/`decode` from `#[derive(Event)]`;
- a filtered, bounded channel from many producers to one writer thread, drop-and-count on overflow;
- no rendering and no ring in the library — an application that wants a text line or a dump supplies
  its own `Observer`.

The one real consumer is [`tg-ws-proxy-rs`](https://github.com/Kraionix/tg-ws-proxy-rs), which vendors
a pinned copy of four crates under `vendor/logreel/` and implements `Event` by hand over its prost
schema. This repository is the source of truth for that copy: `tools/sync-logreel.sh` there copies
exactly what is committed here, then applies two documented patches (no redb backend, no derive macro).
A change the proxy needs is delivered through that script, never by editing the vendored tree.

## Repository layout

```
crates/logreel-core/       event contract, record envelope, filters, scopes, the writer pipeline, Observer
crates/logreel-derive/     #[derive(Event)]: kind mapping and encode/decode dispatch, token-only
crates/logreel-store/      the storage boundary: EncodedBatch, Durability, Store, error taxonomy (no deps)
crates/logreel-file/       .seg backend: 56-byte header, CRC32C frames, day rotation, retention, probe, scan
crates/logreel-store-redb/ the same encoded batch in a redb table — an alternative backend, not a second format
crates/logreel/            facade: one import, features file/zstd/redb, none of them on by default
xtask/                     ci, fixtures gen|check, bench, version
bench/harness/             the measurements quoted in bench/RESULTS.md; BUDGET.md is the frozen baseline
examples/quickstart/       the README example end to end, including probing
examples/events/           the shared example event types the bench and the tests use
tests/alloc-harness/       proves the gated emit path allocates nothing
fixtures/v1/               the normative conformance corpus (bytes, expected output, hashes)
docs/SPEC.md               the byte layout, normative
docs/FORMAT.md             the same for a reader written in another language
docs/PLAN.md               the development plan; one milestone per session
docs/RELEASE.md            publish order and the pre-tag checklist
docs/adr/, docs/design/    the decisions behind the six crates and the pipeline
```

## Build, test, lint

The toolchain is pinned in `rust-toolchain.toml` (1.98, edition 2024). One command runs what CI runs:

```bash
cargo xtask ci                                  # fmt --check, clippy -D warnings, tests, docs, fixtures
cargo xtask fixtures check                      # fixture bytes, expected output and hashes are unchanged
cargo run -p logreel-bench --release -- --check # within ten percent of bench/BUDGET.md
cargo xtask bench                               # manual; rewrites bench/RESULTS.md
```

CI (`.github/workflows/ci.yml`) runs the same gates on `main` and on every pull request: `fmt`,
`clippy --workspace --all-targets --all-features -D warnings`, `test --workspace --all-features`
plus the fixture check, and `doc` with `RUSTDOCFLAGS=-D warnings`. The `deny` job is advisory — it
skips when `cargo-deny` is not installed. `cargo fmt --all -- --check` is the style gate; do not
reformat unrelated files in a change.

## The format is the contract

- **Any change to the bytes requires three things in the same change**: a format version bump (the
  `FORMAT_VERSION` constant lives in `logreel-store`), a new fixture directory under `fixtures/`,
  and an amendment to the relevant ADR. Older fixture directories keep passing forever; a reader that
  cannot parse them is a regression.
- **A reader never resynchronises.** It stops at the first invalid frame and reports the offset. A
  torn tail is expected; a bad CRC in the middle is a corrupt file, not a cue to scan for the next
  frame.
- **The header is 56 bytes** — `TGSLOG01` magic, codec id, flags, kind count, a 16-byte schema
  fingerprint, creation time, producer id and a per-session nonce, per `docs/SPEC.md`. Frames are
  `u32 length | u32 crc32c | payload`; batches carry the compact envelope. `probe` must keep working
  with no codec at all.
- **Unknown kinds and unknown schemas degrade to raw output**, never to a refused file. A newer
  producer must not make an older reader throw a segment away.
- **`fixtures/v1` is normative.** `xtask`'s generator is written against `docs/SPEC.md`, not against
  `logreel-file`, so `fixtures check` is an independent parser: when the two disagree, the code is
  wrong, not the fixture.
- **Day rotation and retention are public API** (`today`, `day_name`, `day_of_name`, `segment_day`,
  `SEGMENT_SUFFIX`, `RetentionAnchor`): consumers must not carry a second copy of the arithmetic.
  Retention never deletes a segment the current run wrote, whatever the clock says — a router that
  boots reading 1970 would otherwise delete the file that boot had just written.

## Conventions to follow

- **`CONTRIBUTING.md` is the operating contract** — one milestone per session, no publishing or
  tagging without an explicit instruction, no rewriting pushed history, ask before adding a crate or
  changing the format. This file is the map; that one is the rules.
- **No new dependency without a one-line justification** in the commit body: what it buys and why the
  standard library is not enough. No `cargo install`, no network installs.
- **Conventional commits**, imperative subject, one concern per commit, no trailing period
  (`feat(core): …`, `fix(file): …`, `docs(spec): …`).
- **The emit path stays allocation-free.** Anything convenient — formatting, a `String`, a
  per-record `Vec` — belongs on the writer side or in an `Observer`. `tests/alloc-harness` exists to
  keep that honest: a gated call must not allocate even when the record is dropped.
- **No unused dependency surface.** Keep `file`, `zstd` and `redb` independently switchable and keep
  the default feature set empty, so a dependency that only needs records does not link a storage stack.
- **Do not add rendering or a ring to the library.** The application owns how a record looks and what
  it keeps; `NullSink` is the escape hatch for a process that writes no segment.
- **MSRV is 1.98 and the workspace forbids `unsafe`** (`unsafe_code = "forbid"` in
  `[workspace.lints.rust]`, repeated as `#![forbid(unsafe_code)]` per crate); `missing_docs` is a
  warning, so a new public item needs a doc comment.
- **Numbers are recorded, not claimed.** If a benchmark was not run, `bench/RESULTS.md` says
  UNVERIFIED. Do not edit `bench/BUDGET.md` to make a regression pass.
- **Publishing is a release act, not a CI side effect.** The publish order, the dry-run limitation and
  the pre-tag checklist are in `docs/RELEASE.md`; publishing needs an explicit instruction in the
  session.
