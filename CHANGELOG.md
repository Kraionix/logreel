# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `NullSink`: a `BatchSink` that accepts every batch and keeps none, so a
  process that writes no segment satisfies the pipeline without a test sink
  (which retains the batches it is given).

## [0.3.0] - 2026-11-01

### Added

- `with_current(|handle, conn_id| ...)`: the scoped path used to clone the
  `Arc` and downcast it for every record. It now borrows the handle, and
  `current` and `try_emit` are built on it.
- `RetentionAnchor` and `FileStoreConfig::retention_anchor`: the window can
  start at the clock or at the newest segment already in the directory.
- `FileStoreConfig::on_warning` and `WarningSink`: the store reports a clock
  that disagrees with the archive as a ready-made line; the application keeps
  its own wording.
- `today`, `day_name`, `day_of_name`, `segment_day`, `SEGMENT_SUFFIX`:
  the day calendar and the segment naming are public, so a consumer no longer
  carries a second copy of the arithmetic.

### Changed

- A day the current run wrote is never deleted by retention, whatever the
  window or the clock says: a router that boots reading 1970 would otherwise
  delete the segment that boot had just written.
- `format_utc` prints microseconds, the precision every consumer shows.
- `decode_batch` returns the records instead of a `DecodedBatch` with a
  copied payload; `SegmentReader::next_decoded_batch` keeps the wrapper.
- `init(settings, sink)` and `init_with(settings, observer, sink)` no longer
  take a render function.

### Removed

- The text projection and the ring buffer, with the `text` and `ring`
  features: `Render`, `TextSettings`, `TextTarget`, `RingSettings`,
  `Settings::text`, `Settings::ring`, `Handle::dump_ring` and
  `Handle::ring_dropped`. An application renders a record and keeps its own
  bounded buffer in its [`Observer`], which is the same seam with the
  application's own rules.
- `ShutdownReport::text_errors` and `ShutdownReport::ring_dropped`.

## [0.2.0] - 2026-10-31

### Added

- `Observer<E>`: a second consumer the writer thread owns, with
  `record`, `flush`, `dump` and `overflow`. An application that already has a
  view of its records -- a renderer with state, a ring of its own, a line a
  deployment parses -- supplies one instead of asking the library for a hook
  per use.
- `init_with(settings, render, observer, sink)`; `init` keeps its signature and
  is the same call with no observer.
- `Handle::request_dump()`: the writer drains the channel, flushes the batch
  and then calls the observer's `dump`, so a dump taken from a signal shows
  what was queued when the signal arrived.
- `Settings::sink_min_level` and `Settings::sink_filter_overrides` (builder:
  `sink_min_level`, `sink_filter`): a second level table consulted on the
  writer thread. The producer gate stays the union of every consumer, so the
  text view and the ring still see what the segment drops.
- `BatchEncoder::encode_where`: encode only the records the sink table keeps,
  without copying the batch into a second vector.
- `Logger::shutdown_with(quiet, limit)`: keep draining until the channel has
  been quiet for `quiet` or `limit` has passed, then flush the tail. `shutdown`
  is the same with both zero.
- `FileStoreConfig::zstd_level` (default 3) for builds with `zstd`.
- `FileStore::prime`: create or validate today's file and take its lock
  without writing a frame, so a second writer fails at startup rather than at
  the first append.
- `SegmentReader::next_decoded_batch`, `DecodedBatch` and `OwnedRecord`: a
  batch with its records decoded and their payloads copied out, for readers
  that print records and their enclosing batch together.
- Refusal to append compressed frames to a file whose header says it carries
  none: the reader would have no reason to expect them.

### Changed

- `SettingsBuilder::min_level` now also moves the sink floor until
  `sink_min_level` is called, so "log at debug" still means the segment.

## [0.1.0] - 2026-10-31

### Added

- Initial public surface: typed events, scoped emission, text projection,
  append-only segment files, probing and scanning readers.
