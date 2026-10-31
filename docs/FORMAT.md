# Reading a logreel segment — implementer's checklist

This is the practical companion to `SPEC.md`. It is not normative; if the two
disagree, the specification wins.

## 1. Read the header

```text
len >= 56 ?
magic == "TGSLOG01" ?
format == 1 ?
header_len == 56 ?
crc32c(bytes[0..52]) == u32_le(bytes[52..56]) ?
```

If any check fails, stop and name the file. Do not try the next frame: there
is no resynchronisation in this format.

Keep `codec_id`, `schema_hash`, `kind_count`, `producer_id` and
`session_nonce`. They are metadata for the consumer, not for framing.

## 2. Iterate frames

```text
offset = 56
while offset + 8 <= file_len:
    frame_len = u32_le(file[offset..offset+4])
    stored    = frame_len & 0x7fff_ffff
    zstd      = frame_len & 0x8000_0000 != 0
    if stored > 64 MiB: corrupt at offset; stop
    if offset + 8 + stored > file_len: truncated at offset; stop
    crc = u32_le(file[offset+4..offset+8])
    payload = file[offset+8 .. offset+8+stored]
    if zstd:
        require header.flags & 1
        payload = zstd_decompress(payload)   // or stop with Unsupported
    if crc32c(payload) != crc: corrupt at offset; stop
    parse_batch(payload)
    offset += 8 + stored
```

`truncated` at the end of a live file is normal. `corrupt` is not.

## 3. Parse a batch

```text
record_count = u32_le
ts_first     = u64_le
ts_last      = u64_le
mask_len     = u16_le; skip mask_len bytes
domain_len   = u16_le; skip domain_len bytes   // opaque
repeat record_count times:
    level     = u8      (0..=4)
    category  = u8
    kind      = u16_le
    ts_ns     = u64_le
    seq       = varint u32
    conn_id   = varint u64
    payload_n = varint u32
    payload   = bytes[payload_n]
```

After the last record the batch must end exactly. `payload_n == 0` is legal
(an empty protobuf message). `seq` is the ordering key inside a batch; `ts_ns`
is wall-clock and may be non-monotonic.

## 4. Decode what you can

- `kind` is application-defined. If you do not know it, print the envelope and
  the payload as hex and continue. Never abort the file.
- `codec_id` tells you how the payload is encoded. One codec per file.
- `schema_hash` tells you which version of the event schema produced it. A
  mismatch means "raw mode", not "unreadable file".
- `domain` belongs to the producer. Copy it; do not parse it.

## 5. Probe without a codec

A useful `probe` reports:

- header fields and the refusal reason, if any;
- frames, records, first and last timestamp;
- distinct `conn_id` values, with the tracking cap stated;
- kind histogram, including unknown kinds;
- `valid_end` and `truncated_bytes`;
- tail state: clean / truncated / corrupt;
- SHA-256 of `bytes[0..valid_end]`.

`valid_end` is the only safe offset for a partial copy: bytes before it are
self-consistent even if the writer is still appending.

## 6. Common mistakes

1. Resynchronising after a bad frame. Do not: a plausible length inside a
   corrupt region is not a frame.
2. Reporting a partial last frame as corruption. A writer may be alive.
3. Treating `kind_mask` as the record list. It is informational; iterate
   `record_count` records.
4. Forgetting that the CRC covers the uncompressed payload.
5. Assuming timestamps are monotonic across batches or files.
6. Using `created_ns` to order retention. File names are the order.
