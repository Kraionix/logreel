# ADR 0001 — one repository, six crates

Status: accepted (2026-10-31).

## Context

The format is shared by the event pipeline, the derive macro, the storage
contract, the file backend and an optional redb backend. A header field or an
envelope rule touches all of them in one change.

## Decision

One workspace, one version, six published crates:

- `logreel` — facade, feature gated re-exports.
- `logreel-core` — `Event`, envelope, filters, writer, text, scope.
- `logreel-derive` — `#[derive(Event)]`.
- `logreel-store` — `Store`, `EncodedBatch`, `Durability`, errors.
- `logreel-file` — segment format, rotation, retention, probe, scan.
- `logreel-store-redb` — alternative backend, same contract.

Edges: `core` depends on no storage crate; `file` and `store-redb` depend
only on `store`; the facade wires everything behind features. The proc-macro
crate emits tokens only and depends on no runtime crate.

## Consequences

- A contract change is one commit and one CI run.
- `cargo publish` still publishes six independent crates in dependency order.
- A backend can be split out later; merging separate histories is the failure
  mode this avoids.
- The proxy repository stays untouched until the integration stage.
