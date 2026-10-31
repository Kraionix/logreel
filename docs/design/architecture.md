# Architecture

## Data flow

```
producer task                    writer thread                     storage
------------                     -------------                     -------
enabled(category, level)
build payload
stamp ts, seq
try_send  ── bounded channel ──▶  observer (optional)
                                  batch Vec<E>
                                  encode envelope + payload
                                  ───────────────────────────────▶  BatchSink / Store
```

The emit path is static: no trait objects, no locks, no allocation after warm
up, no formatting while the record is disabled. The writer thread owns every
fallible or blocking operation: encoding, `write`, `fsync`. Rendering a
record as a line is the observer's job, not the pipeline's.

## Seams

| seam | trait | frequency | dynamic |
|---|---|---|---|
| event | `Event` | per record | no |
| observer | `Observer` | per record | yes |
| batch sink | `BatchSink` | per batch | yes |
| store | `Store` | per batch / scan | yes |

## Invariants

1. One writer per handle; the handle is shared as `Arc`, never cloned.
2. An observer sees the same record values the sink eventually writes.
3. A full channel drops and counts; producers never block or fail.
4. The envelope is library-owned; the codec only touches payload bytes.
5. An unknown kind or a newer schema never aborts a read: probe works, dump
   degrades to raw.
6. Readers never write and never copy a segment.
7. Producers outside a scope emit nothing.
8. A build without storage still runs the whole pipeline.

## Crate graph

```
logreel ──▶ logreel-core
    │           ▲
    ├──▶ logreel-derive (tokens only)
    ├──▶ logreel-file ──▶ logreel-store
    └──▶ logreel-store-redb ──▶ logreel-store
```
