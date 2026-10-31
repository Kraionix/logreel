# logreel segment format — specification v1

Status: **normative**. This document defines every byte a `logreel` segment
may contain. An implementation that disagrees with this document is wrong.

Audience: implementers of `logreel-file`, authors of third-party readers, and
the fixture generator (which is deliberately a second implementation).

Change rule: any change to a byte, an offset, a validation order, a limit or a
retention rule requires a format version bump, a new fixture directory, and an
ADR amendment in the same change. Existing fixtures keep passing.

## 1. Conventions

- All fixed-width integers are **little-endian**.
- `varint` is unsigned LEB128, at most 10 bytes for a `u64`, at most 5 bytes
  for a `u32`. A decoder rejects a varint that is longer than its type allows
  or whose value overflows the type.
- Timestamps are **Unix nanoseconds**, signed by meaning but stored in `u64`;
  values beyond 2^63 are reserved and rejected.
- `CRC-32C` is CRC-32 with the Castagnoli polynomial, reflected input and
  output, init `0xFFFF_FFFF`, final xor `0xFFFF_FFFF` (the iSCSI variant).
  Check value for `"123456789"` is `0xE306_9283`.
- `SHA-256` is FIPS 180-4, hex encoded in lower case.

## 2. File model

A segment is one append-only file per UTC day, named `YYYY-MM-DD.seg`.
It begins with one 56-byte header followed by zero or more frames. There is no
index, no footer, and no trailer. Readers stream; they never seek backwards.

A file is either a sequence of fully valid frames followed by an optional torn
tail, or it is corrupt. A torn tail is normal: a writer may be appending while
a reader reads, or a process may have died mid-write. Corruption is not
normal.

## 3. Header (56 bytes)

| off | size | field | value / notes |
|---:|---:|---|---|
| 0 | 8 | `magic` | ASCII `TGSLOG01` |
| 8 | 2 | `format` | `1` |
| 10 | 2 | `codec_id` | payload codec; one per file |
| 12 | 2 | `flags` | bit 0: the file may contain zstd frames; all other bits 0 |
| 14 | 2 | `kind_count` | informational: number of kinds in the producing domain |
| 16 | 16 | `schema_hash` | fingerprint of the producing event schema |
| 32 | 8 | `created_ns` | Unix ns at file creation |
| 40 | 4 | `producer_id` | who wrote the file; `0` means "unspecified" |
| 44 | 4 | `session_nonce` | random per writer session; diagnostics and loop detection |
| 48 | 4 | `header_len` | `56`; lets a future header grow |
| 52 | 4 | `header_crc32c` | CRC-32C over bytes `0..52` |

Validation order for a reader, stopping at the first failure:

1. the file is at least 56 bytes;
2. `magic` matches;
3. `format == 1`;
4. `header_len == 56` (a larger value is a future version: refuse by name);
5. `header_crc32c` matches bytes `0..52`;
6. `flags` has no bit outside `0b1`;
7. `kind_count`, `codec_id` and `schema_hash` are **not** validated. They are
   metadata. A mismatch degrades decoding to raw output, never a refusal.

The reference writer sets `created_ns` from its injected clock, `producer_id`
from configuration (default 0), and `session_nonce` from a process-wide
counter mixed with the start time. It never writes a `format` other than 1.

## 4. Frame

| off | size | field | notes |
|---:|---:|---|---|
| 0 | 4 | `frame_len` | bit 31: payload is zstd compressed; bits 0..30: stored payload length |
| 4 | 4 | `crc32c` | CRC-32C over the **uncompressed** payload |
| 8 | n | `payload` | batch (§5), optionally compressed |

Rules:

- `stored_len = frame_len & 0x7FFF_FFFF`. `stored_len` must be `<= 64 MiB`
  (`0x0400_0000`).
- `stored_len == 0` is corrupt: a batch always has a 24-byte header.
- If bit 31 is set, the file header flag bit 0 must also be set; otherwise the
  frame is a format error.
- If bit 31 is set and the reader is built without zstd support, the reader
  reports `Unsupported` at the frame offset. It does not guess.
- The CRC is verified **after** decompression, over the bytes as written by the
  batch encoder. A reader that cannot decompress cannot verify the CRC.
- A frame is valid when its length fits in the file and its CRC matches. The
  first invalid frame ends the valid prefix; a reader never resynchronises by
  scanning for a later plausible length.

## 5. Batch payload

| size | field | notes |
|---:|---|---|
| 4 | `record_count` | `u32` |
| 8 | `ts_first` | min `ts_ns` in the batch |
| 8 | `ts_last` | max `ts_ns` in the batch |
| 2 + m | `kind_mask` | `u16` length `m`, then `m` bytes; bit `i` set iff local kind `i` occurs |
| 2 + d | `domain` | `u16` length, then opaque bytes |
| ... | `records` | exactly `record_count` records, back to back |

- `m = ceil(kind_count / 8)` as chosen by the writer. A reader accepts any
  `m` and treats bits beyond the mask as zero. `m` may be 0 when
  `kind_count` is 0.
- Local kind `i` means `absolute_kind - BASE`, where `BASE` is the domain
  offset known to the producer. The mask is informational: a reader must not
  use it to skip records; it iterates records by `record_count`.
- `domain` is opaque to the library. Its meaning belongs to the producing
  application (for example a datacenter mask). Readers copy it, never parse it.
- After the last record the payload must end exactly. Trailing bytes are a
  corruption, not a future extension.
- `record_count == 0` is accepted by a reader and never emitted by the
  reference writer.

## 6. Record

| size | field | notes |
|---:|---|---|
| 1 | `level` | `0..=4`: trace, debug, info, warn, error |
| 1 | `category` | index into the producer's category table |
| 2 | `kind` | absolute `u16`; `0x8000..=0x8FFF` is reserved for foreign domains |
| 8 | `ts_ns` | Unix ns; may go backwards across producers |
| varint | `seq` | `u32`, per-process monotonic; the ordering key inside a batch |
| varint | `conn_id` | `u64`; `0` means not session-scoped |
| varint | `payload_len` | `u32`, `<= 64 MiB` |
| n | `payload` | codec bytes; never inspected by the library |

The envelope is 15 to 30 bytes; 17–19 is typical. A record with an unknown
kind is decoded by skipping exactly `payload_len` bytes and continuing.

## 7. Writer rules

### 7.1 Open and recover

1. Create the directory if missing.
2. Take an exclusive advisory lock for the file. The reference implementation
   creates `<file>.lock` with `O_EXCL`; a pre-existing lock is reported, not
   stolen.
3. If the file does not exist, write the header as one `write` call.
4. If the file exists, validate its header; then scan frames. If the tail is
   torn (incomplete frame at EOF), truncate to the last valid frame boundary.
   If a CRC or format error appears before EOF, refuse to open: a writer never
   truncates real corruption silently.
5. Never append to a file whose header `format` is not 1.

### 7.2 Append

- One flush writes one frame, and the write is a single buffer
  `frame_len || crc32c || payload` so the append is atomic at the syscall
  boundary for readers that see the file concurrently.
- The frame offset is the file length before the write. It is returned for
  diagnostics.
- A failed partial write leaves a torn tail; the next open repairs it.

### 7.3 Sync

`Durability` selects when `sync_data` is called:

| mode | behaviour |
|---|---|
| `Eventual` | sync at most once every 30 s |
| `Immediate` | sync before the flush returns |
| `OnWarn` | the caller requests `Immediate` when the batch contains `level >= Warn` |

The writer never calls `fsync`; `sync_data` is sufficient for append-only data.
A clean shutdown always syncs.

### 7.4 Rotation

- The active file is `YYYY-MM-DD.seg` for the **UTC day of the writer clock**
  at the moment of the append.
- When that day differs from the active file's day: sync, close, open the new
  day, write a header, then append.
- Rotation is day based only. A size trigger is explicitly not part of v1; the
  byte cap in §7.5 bounds the directory, not the file.

### 7.5 Retention

Retention runs after a successful rotation, never on every append.

Let `today` be the UTC day of the writer clock, `newest` the newest day among
files in the directory, and

```
window_end = min(today, newest + 1 day)
```

Only files whose day is **strictly before** `window_end` are candidates. The
active file is never a candidate. Candidates are deleted oldest first while
either condition holds:

- `count(files) > keep_days`, where `count` includes the active file; or
- `sum(len(file)) > max_total_bytes`.

`keep_days = 0` disables day-based deletion; `max_total_bytes = 0` disables
the byte cap. If both are zero, retention is a no-op.

The `window_end` rule is the clock guard: if the clock jumps backwards, files
newer than `today` are outside the window and are not deleted. If the clock
jumps forwards, a file dated ahead of `today` is also protected until the clock
catches up. Retention never consults `created_ns`; file names are the order.

## 8. Reader rules

### 8.1 Open

Readers open `O_RDONLY`, take no lock, copy nothing, and never write. A file
being appended to is a normal file; a partial last frame is ignored, not
reported as corruption.

### 8.2 Probe

`probe` needs no codec. It reports:

- header fields, or a refusal with the file name and the failing check;
- number of valid frames and records;
- first and last record timestamp, as `u64` plus a formatted UTC date;
- distinct `conn_id` values (bounded by a configured cap; the count is exact
  when the cap is not reached);
- kind histogram (absolute kind → count), including unknown kinds;
- number of records whose kind is `>= kind_count` (unknown count);
- `valid_end`: byte offset just past the last valid frame;
- `truncated_bytes`: file length minus `valid_end`;
- `tail`: `clean`, `truncated` (EOF mid-frame) or `corrupt` (bad CRC, bad
  length, bad batch);
- `sha256` of bytes `0..valid_end`.

A corrupt frame does not abort probing; it ends the valid prefix, and the tail
state says `corrupt` with the frame offset.

### 8.3 Scan

`scan` yields one `RecordView` per record:

```
level: u8, category: u8, kind: u16, ts_ns: u64, seq: u32,
conn_id: u64, payload: &[u8], frame_offset: u64
```

It stops at the first invalid frame. It never resynchronises. It never
allocates per record; the caller copies the payload if it keeps it.

### 8.4 Dump

`dump` is an application concern. The library exposes scan plus raw rendering:

- decoded mode is available only when the caller supplies a decoder;
- raw mode prints the envelope fields and the payload as lower-case hex, with
  no codec and no `schema_hash` check;
- a schema mismatch downgrades a file from decoded to raw, never to refused.

### 8.5 Unknown data

| situation | probe | scan / dump |
|---|---|---|
| unknown kind | counted | decoded by skipping the payload; raw if the caller cannot decode |
| unknown `codec_id` | succeeds, flagged | raw mode only; decoded mode refuses by id |
| unknown `schema_hash` | succeeds, flagged | raw mode only; decoded mode refuses by hash |
| unknown `format` / `header_len` | refused by name | refused by name |
| zstd frame, no zstd feature | succeeds for header and preceding frames | stops with `Unsupported` at that frame |

## 9. Error taxonomy

Every reader and writer error carries a kind, a message, and, where relevant,
a byte offset.

| kind | meaning | recoverable |
|---|---|---|
| `Io` | operating system error | caller decides |
| `Format` | header or batch structure violates this spec | no |
| `Corrupt` | CRC or length mismatch inside the valid range | no; valid prefix remains readable |
| `Truncated` | EOF in the middle of a frame or record | yes; normal for a live file |
| `Unsupported` | zstd frame without the feature, or a newer format | no |
| `Refused` | header invalid, or a decoder was requested for an unknown codec/schema | no |
| `Locked` | another writer holds the lock | retry later |

A read result is tri-state: `Record`, `End` (clean EOF), or `Error` with one
of the kinds above. `Truncated` at EOF is not an error for `probe`; it is the
`truncated` tail state.

## 10. Size budget

| component | bytes |
|---|---:|
| record envelope, typical | 17–19 |
| record envelope, range | 15–30 |
| batch header, typical | 24 + mask + domain |
| per-record batch amortisation at 256 records | < 1 |
| total per record, typical | 38–40 |

## 11. Conformance fixtures (v1)

`fixtures/v1` is normative. Each case is committed bytes plus an expected JSON
file. `cargo xtask fixtures gen` regenerates them from frozen constants;
`cargo xtask fixtures check` fails if any byte or hash differs.

| id | file | case | expected |
|---|---|---|---|
| F01 | `F01_normal.seg` | one batch, three kinds, mixed levels | full decode, clean tail |
| F02 | `F02_varints.seg` | three batches, varint boundaries, empty payload | full decode; `seq = u32::MAX`, `conn_id = u64::MAX` |
| F03 | `F03_unknown_kind.seg` | two batches, unknown kind `0x9001` in the middle | probe counts it; scan skips its payload and continues |
| F04 | `F04_schema_mismatch.seg` | header schema hash differs from the payload's producer | probe succeeds flagged; dump raw |
| F05 | `F05_torn_tail.seg` | header, one frame, six bytes of a second frame | tail `truncated`; scan yields the first batch |
| F06 | `F06_bad_crc.seg` | second frame CRC flipped | tail `corrupt` at the frame offset; first batch readable |
| F07 | `F07_empty.seg`, `F07_bad_magic.seg` | zero bytes; bad magic | both refused by name |
| F08 | `F08_clock_jump/*.seg` | files named by day; the newer file carries an older `created_ns` | retention deletes nothing on a backwards clock; deletes only the older day on a forward clock |
| F09 | `F09_zstd.seg` | one zstd frame, header flag set | decompressed and CRC checked; equal records to F01 |
| F10 | `F10_producer_zero.seg` | `producer_id = 0` | accepted; no producer attribute |

## 12. Limits

| limit | value |
|---|---|
| frame payload, stored and uncompressed | `64 MiB` |
| record payload | `64 MiB` |
| header | 56 bytes |
| segment count in a directory | unbounded; retention bounds it |
| distinct `conn_id` values tracked by probe | implementation cap, reported |

## 13. Non-goals

No index; no random access; no transactions; no multi-writer files; no
per-record checksum (the frame CRC covers the batch); no compression
dictionary; no cross-file ordering; no encryption; no authentication.
