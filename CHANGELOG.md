# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
- Refusal to append compressed frames to a file whose header says it carries
  none: the reader would have no reason to expect them.

### Changed

- `SettingsBuilder::min_level` now also moves the sink floor until
  `sink_min_level` is called, so "log at debug" still means the segment.

## [0.1.0] - 2026-10-31

### Added

- Initial public surface: typed events, scoped emission, text projection,
  append-only segment files, probing and scanning readers.
