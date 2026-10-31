//! Deterministic fixture generation and the independent reference parser.
//!
//! The encoder and the parser in this module are written against
//! docs/SPEC.md, not against logreel-file. cargo xtask fixtures check fails
//! when the committed bytes, the expected outputs or the hash table drift.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::codec::{crc32c, hex, put_varint, sha256};

/// ASCII magic at offset 0.
pub(crate) const MAGIC: &[u8; 8] = b"TGSLOG01";
/// Only supported format version.
pub(crate) const FORMAT: u16 = 1;
/// Header size in bytes.
pub(crate) const HEADER_LEN: usize = 56;
/// Maximum stored and uncompressed frame payload.
pub(crate) const MAX_PAYLOAD: usize = 64 * 1024 * 1024;

/// Header fields of a generated segment.
#[derive(Clone, Debug)]
pub(crate) struct Header {
    /// Payload codec id.
    pub(crate) codec_id: u16,
    /// File level flags; only bit 0 is defined.
    pub(crate) flags: u16,
    /// Informational kind count.
    pub(crate) kind_count: u16,
    /// Schema fingerprint.
    pub(crate) schema_hash: [u8; 16],
    /// Creation time in Unix nanoseconds.
    pub(crate) created_ns: u64,
    /// Producer identity; 0 means unspecified.
    pub(crate) producer_id: u32,
    /// Per-session nonce.
    pub(crate) session_nonce: u32,
}

impl Header {
    fn new(kind_count: u16, schema_hash: [u8; 16], created_ns: u64) -> Self {
        Self {
            codec_id: 1,
            flags: 0,
            kind_count,
            schema_hash,
            created_ns,
            producer_id: 7,
            session_nonce: 0x5EED_0001,
        }
    }

    fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0..8].copy_from_slice(MAGIC);
        out[8..10].copy_from_slice(&FORMAT.to_le_bytes());
        out[10..12].copy_from_slice(&self.codec_id.to_le_bytes());
        out[12..14].copy_from_slice(&self.flags.to_le_bytes());
        out[14..16].copy_from_slice(&self.kind_count.to_le_bytes());
        out[16..32].copy_from_slice(&self.schema_hash);
        out[32..40].copy_from_slice(&self.created_ns.to_le_bytes());
        out[40..44].copy_from_slice(&self.producer_id.to_le_bytes());
        out[44..48].copy_from_slice(&self.session_nonce.to_le_bytes());
        out[48..52].copy_from_slice(&(HEADER_LEN as u32).to_le_bytes());
        let crc = crc32c(&out[0..52]);
        out[52..56].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

/// One record as the fixture generator understands it.
#[derive(Clone, Debug)]
pub(crate) struct Rec {
    /// Level 0..=4.
    pub(crate) level: u8,
    /// Category index.
    pub(crate) category: u8,
    /// Absolute kind.
    pub(crate) kind: u16,
    /// Unix nanoseconds.
    pub(crate) ts: u64,
    /// Per-process sequence.
    pub(crate) seq: u32,
    /// Connection id, 0 for none.
    pub(crate) conn: u64,
    /// Opaque payload bytes.
    pub(crate) payload: Vec<u8>,
}

/// One batch plus its frame options.
#[derive(Clone, Debug)]
pub(crate) struct FrameSpec {
    /// Records in wire order.
    pub(crate) records: Vec<Rec>,
    /// Opaque domain bytes.
    pub(crate) domain: Vec<u8>,
    /// Compress this frame with zstd.
    pub(crate) compressed: bool,
}

fn rec(level: u8, category: u8, kind: u16, ts: u64, seq: u32, conn: u64, payload: &[u8]) -> Rec {
    Rec {
        level,
        category,
        kind,
        ts,
        seq,
        conn,
        payload: payload.to_vec(),
    }
}

fn encode_batch(records: &[Rec], domain: &[u8], kind_count: u16) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(records.len() as u32).to_le_bytes());
    let first = records.iter().map(|r| r.ts).min().unwrap_or(0);
    let last = records.iter().map(|r| r.ts).max().unwrap_or(0);
    out.extend_from_slice(&first.to_le_bytes());
    out.extend_from_slice(&last.to_le_bytes());
    let mask_len = usize::from(kind_count).div_ceil(8);
    let mut mask = vec![0u8; mask_len];
    for record in records {
        let index = usize::from(record.kind);
        if index < usize::from(kind_count) {
            mask[index / 8] |= 1 << (index % 8);
        }
    }
    out.extend_from_slice(&(mask_len as u16).to_le_bytes());
    out.extend_from_slice(&mask);
    out.extend_from_slice(&(domain.len() as u16).to_le_bytes());
    out.extend_from_slice(domain);
    for record in records {
        out.push(record.level);
        out.push(record.category);
        out.extend_from_slice(&record.kind.to_le_bytes());
        out.extend_from_slice(&record.ts.to_le_bytes());
        put_varint(&mut out, u64::from(record.seq));
        put_varint(&mut out, record.conn);
        put_varint(&mut out, record.payload.len() as u64);
        out.extend_from_slice(&record.payload);
    }
    out
}

fn encode_frame(payload: &[u8], compressed: bool) -> Vec<u8> {
    let stored = if compressed {
        zstd::bulk::compress(payload, 3).expect("zstd fixture compression")
    } else {
        payload.to_vec()
    };
    let mut len = stored.len() as u32;
    if compressed {
        len |= 1 << 31;
    }
    let mut out = Vec::with_capacity(8 + stored.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&crc32c(payload).to_le_bytes());
    out.extend_from_slice(&stored);
    out
}

fn encode_segment(header: &Header, frames: &[FrameSpec]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&header.encode());
    for frame in frames {
        let payload = encode_batch(&frame.records, &frame.domain, header.kind_count);
        out.extend_from_slice(&encode_frame(&payload, frame.compressed));
    }
    out
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| "length overflow".to_owned())?;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or_else(|| "unexpected end of batch".to_owned())?;
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, String> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> Result<u64, String> {
        let bytes = self.take(8)?;
        let mut array = [0u8; 8];
        array.copy_from_slice(bytes);
        Ok(u64::from_le_bytes(array))
    }

    fn varint(&mut self, max_bytes: usize) -> Result<u64, String> {
        let mut value = 0u64;
        for shift in 0..max_bytes {
            let byte = self.u8()?;
            let part = u64::from(byte & 0x7F);
            if shift == 9 && part > 1 {
                return Err("varint overflow".to_owned());
            }
            value |= part << (shift * 7);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err("varint too long".to_owned())
    }
}

#[derive(Debug, Clone)]
struct ParsedRecord {
    frame: usize,
    offset: u64,
    level: u8,
    category: u8,
    kind: u16,
    ts: u64,
    seq: u32,
    conn: u64,
    payload: Vec<u8>,
}

#[derive(Debug, Default)]
struct Parsed {
    refused: Option<String>,
    header: Option<Header>,
    frames: u64,
    records: Vec<ParsedRecord>,
    valid_end: u64,
    tail: &'static str,
    corrupt_offset: Option<u64>,
    truncated_bytes: u64,
    unknown_records: u64,
    kind_hist: BTreeMap<u16, u64>,
    conns: BTreeSet<u64>,
    first_ts: Option<u64>,
    last_ts: Option<u64>,
}

fn parse_header(bytes: &[u8]) -> Result<Header, String> {
    if bytes.len() < HEADER_LEN {
        return Err(format!(
            "header: file is {} bytes, shorter than 56",
            bytes.len()
        ));
    }
    if &bytes[0..8] != MAGIC {
        return Err("header: bad magic".to_owned());
    }
    let format = u16::from_le_bytes([bytes[8], bytes[9]]);
    if format != FORMAT {
        return Err(format!("header: unsupported format {format}"));
    }
    let header_len = u32::from_le_bytes([bytes[48], bytes[49], bytes[50], bytes[51]]);
    if header_len != HEADER_LEN as u32 {
        return Err(format!("header: unsupported header length {header_len}"));
    }
    let crc = u32::from_le_bytes([bytes[52], bytes[53], bytes[54], bytes[55]]);
    if crc32c(&bytes[0..52]) != crc {
        return Err("header: header crc mismatch".to_owned());
    }
    let flags = u16::from_le_bytes([bytes[12], bytes[13]]);
    if flags & !1 != 0 {
        return Err(format!("header: unknown flags 0x{flags:04x}"));
    }
    let mut schema_hash = [0u8; 16];
    schema_hash.copy_from_slice(&bytes[16..32]);
    Ok(Header {
        codec_id: u16::from_le_bytes([bytes[10], bytes[11]]),
        flags,
        kind_count: u16::from_le_bytes([bytes[14], bytes[15]]),
        schema_hash,
        created_ns: u64::from_le_bytes(bytes[32..40].try_into().expect("8 bytes")),
        producer_id: u32::from_le_bytes(bytes[40..44].try_into().expect("4 bytes")),
        session_nonce: u32::from_le_bytes(bytes[44..48].try_into().expect("4 bytes")),
    })
}

fn parse_batch(
    payload: &[u8],
    frame: usize,
    frame_offset: u64,
    header: &Header,
    out: &mut Parsed,
) -> Result<(), String> {
    let mut cursor = Cursor::new(payload);
    let record_count = cursor.u32()?;
    let _first = cursor.u64()?;
    let _last = cursor.u64()?;
    let mask_len = usize::from(cursor.u16()?);
    cursor.take(mask_len)?;
    let domain_len = usize::from(cursor.u16()?);
    cursor.take(domain_len)?;
    for _ in 0..record_count {
        let record_start = cursor.pos;
        let level = cursor.u8()?;
        if level > 4 {
            return Err(format!("batch: level {level} out of range"));
        }
        let category = cursor.u8()?;
        let kind = cursor.u16()?;
        let ts = cursor.u64()?;
        let seq = cursor.varint(5)?;
        if seq > u64::from(u32::MAX) {
            return Err("batch: seq overflows u32".to_owned());
        }
        let conn = cursor.varint(10)?;
        let payload_len = cursor.varint(5)?;
        if payload_len > MAX_PAYLOAD as u64 {
            return Err("batch: payload length exceeds cap".to_owned());
        }
        let payload = cursor.take(payload_len as usize)?.to_vec();
        out.records.push(ParsedRecord {
            frame,
            offset: frame_offset + 8 + record_start as u64,
            level,
            category,
            kind,
            ts,
            seq: seq as u32,
            conn,
            payload,
        });
        out.conns.insert(conn);
        *out.kind_hist.entry(kind).or_insert(0) += 1;
        if kind >= header.kind_count {
            out.unknown_records += 1;
        }
        out.first_ts = Some(out.first_ts.map_or(ts, |value| value.min(ts)));
        out.last_ts = Some(out.last_ts.map_or(ts, |value| value.max(ts)));
    }
    if cursor.pos != payload.len() {
        return Err(format!(
            "batch: {} trailing bytes after the last record",
            payload.len() - cursor.pos
        ));
    }
    Ok(())
}

fn parse_segment(bytes: &[u8]) -> Parsed {
    let header = match parse_header(bytes) {
        Ok(header) => header,
        Err(reason) => {
            return Parsed {
                refused: Some(reason),
                tail: "refused",
                ..Parsed::default()
            };
        }
    };
    let mut out = Parsed {
        header: Some(header.clone()),
        valid_end: HEADER_LEN as u64,
        tail: "clean",
        ..Parsed::default()
    };
    let mut offset = HEADER_LEN;
    let mut frame_index = 0usize;
    while offset < bytes.len() {
        if bytes.len() - offset < 8 {
            out.tail = "truncated";
            out.valid_end = offset as u64;
            break;
        }
        let frame_len = u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("4 bytes"));
        let stored = (frame_len & 0x7FFF_FFFF) as usize;
        let compressed = frame_len & 0x8000_0000 != 0;
        let crc = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().expect("4 bytes"));
        if stored == 0 || stored > MAX_PAYLOAD {
            out.tail = "corrupt";
            out.corrupt_offset = Some(offset as u64);
            out.valid_end = offset as u64;
            break;
        }
        let end = offset + 8 + stored;
        if end > bytes.len() {
            out.tail = "truncated";
            out.valid_end = offset as u64;
            break;
        }
        let stored_bytes = &bytes[offset + 8..end];
        let payload = if compressed {
            if header.flags & 1 == 0 {
                out.tail = "corrupt";
                out.corrupt_offset = Some(offset as u64);
                out.valid_end = offset as u64;
                break;
            }
            let Ok(payload) = zstd::bulk::decompress(stored_bytes, MAX_PAYLOAD) else {
                out.tail = "corrupt";
                out.corrupt_offset = Some(offset as u64);
                out.valid_end = offset as u64;
                break;
            };
            payload
        } else {
            stored_bytes.to_vec()
        };
        if crc32c(&payload) != crc {
            out.tail = "corrupt";
            out.corrupt_offset = Some(offset as u64);
            out.valid_end = offset as u64;
            break;
        }
        if let Err(reason) = parse_batch(&payload, frame_index, offset as u64, &header, &mut out) {
            out.tail = "corrupt";
            out.corrupt_offset = Some(offset as u64);
            out.valid_end = offset as u64;
            out.refused = Some(reason);
            break;
        }
        out.frames += 1;
        frame_index += 1;
        offset = end;
        out.valid_end = offset as u64;
    }
    if out.tail == "truncated" || out.tail == "corrupt" {
        out.truncated_bytes = (bytes.len() as u64).saturating_sub(out.valid_end);
    }
    out
}

fn header_json(header: &Header) -> Value {
    json!({
        "magic": std::str::from_utf8(MAGIC).expect("ascii"),
        "format": FORMAT,
        "codec_id": header.codec_id,
        "flags": header.flags,
        "kind_count": header.kind_count,
        "schema_hash": hex(&header.schema_hash),
        "created_ns": header.created_ns,
        "producer_id": header.producer_id,
        "session_nonce": header.session_nonce,
        "header_len": HEADER_LEN,
    })
}

fn probe_json(bytes: &[u8], parsed: &Parsed) -> Value {
    let mut kinds = Map::new();
    for (kind, count) in &parsed.kind_hist {
        kinds.insert(kind.to_string(), json!(count));
    }
    json!({
        "frames": parsed.frames,
        "records": parsed.records.len(),
        "first_ts_ns": parsed.first_ts,
        "last_ts_ns": parsed.last_ts,
        "conn_ids": parsed.conns.iter().copied().collect::<Vec<_>>(),
        "kinds": kinds,
        "unknown_records": parsed.unknown_records,
        "valid_end": parsed.valid_end,
        "truncated_bytes": parsed.truncated_bytes,
        "tail": parsed.tail,
        "corrupt_offset": parsed.corrupt_offset,
        "sha256": hex(&sha256(&bytes[..parsed.valid_end as usize])),
    })
}

fn record_json(record: &ParsedRecord) -> Value {
    json!({
        "frame": record.frame,
        "offset": record.offset,
        "level": record.level,
        "category": record.category,
        "kind": record.kind,
        "ts_ns": record.ts,
        "seq": record.seq,
        "conn_id": record.conn,
        "payload_hex": hex(&record.payload),
    })
}

fn dump_text(name: &str, parsed: &Parsed, mode: &str) -> String {
    let mut out = String::new();
    out.push_str("# ");
    out.push_str(name);
    out.push('\n');
    if let Some(reason) = &parsed.refused {
        out.push_str("refused: ");
        out.push_str(reason);
        out.push('\n');
        return out;
    }
    out.push_str("mode: ");
    out.push_str(mode);
    out.push('\n');
    for record in &parsed.records {
        let _ = writeln!(
            out,
            "frame={} offset={} level={} category={} kind={} ts_ns={} seq={} conn={} payload={}",
            record.frame,
            record.offset,
            level_name(record.level),
            record.category,
            record.kind,
            record.ts,
            record.seq,
            record.conn,
            hex(&record.payload)
        );
    }
    out
}

fn level_name(level: u8) -> &'static str {
    match level {
        0 => "TRACE",
        1 => "DEBUG",
        2 => "INFO",
        3 => "WARN",
        4 => "ERROR",
        _ => "?",
    }
}

fn expectation(name: &str, file: &str, bytes: &[u8], mode: &str) -> (Value, String) {
    let parsed = parse_segment(bytes);
    let header = parsed.header.as_ref().map(header_json);
    let probe = if parsed.refused.is_some() {
        Value::Null
    } else {
        probe_json(bytes, &parsed)
    };
    let value = json!({
        "fixture": name,
        "file": file,
        "sha256": hex(&sha256(bytes)),
        "bytes": bytes.len(),
        "refused": parsed.refused.is_some(),
        "refusal_reason": parsed.refused,
        "dump": mode,
        "header": header,
        "probe": probe,
        "records": parsed.records.iter().map(record_json).collect::<Vec<_>>(),
    });
    (value, dump_text(file, &parsed, mode))
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = i64::from((month + 9) % 12);
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn day_ns(year: i64, month: u32, day: u32) -> u64 {
    (days_from_civil(year, month, day) * 86_400_000_000_000) as u64
}

const BASE_TS: u64 = 1_700_000_000_000_000_000;

fn header(kind_count: u16) -> Header {
    Header::new(kind_count, [0x11; 16], BASE_TS)
}

fn f01() -> Vec<u8> {
    let records = vec![
        rec(2, 0, 0, BASE_TS, 1, 42, b"opened:42"),
        rec(3, 0, 1, BASE_TS + 1, 2, 42, b"closed:42"),
        rec(4, 1, 2, BASE_TS + 2, 3, 7, b"disk failure"),
    ];
    encode_segment(
        &header(8),
        &[FrameSpec {
            records,
            domain: b"dc=1".to_vec(),
            compressed: false,
        }],
    )
}

fn f02() -> Vec<u8> {
    let mut records_a = Vec::new();
    for (index, value) in [0u32, 127, 128, 16_383, 16_384].into_iter().enumerate() {
        let payload: &[u8] = match index {
            0 => b"",
            1 => b"x",
            2 => b"yy",
            _ => b"xyz",
        };
        records_a.push(rec(
            2,
            index as u8,
            0,
            BASE_TS + index as u64,
            value,
            u64::from(value),
            payload,
        ));
    }
    let mut payload_max = Vec::new();
    for index in 0..300 {
        payload_max.push((index % 251) as u8);
    }
    let records_b = vec![
        rec(3, 1, 1, BASE_TS + 100, u32::MAX, u64::MAX, &payload_max),
        rec(2, 1, 1, BASE_TS + 101, u32::MAX, u64::MAX - 1, b""),
    ];
    let records_c = vec![
        rec(1, 2, 2, BASE_TS - 100, 1, 1, b"out of order"),
        rec(1, 2, 2, BASE_TS + 200, 2, 1, b"later"),
    ];
    encode_segment(
        &header(4),
        &[
            FrameSpec {
                records: records_a,
                domain: Vec::new(),
                compressed: false,
            },
            FrameSpec {
                records: records_b,
                domain: b"dc=2".to_vec(),
                compressed: false,
            },
            FrameSpec {
                records: records_c,
                domain: b"dc=2".to_vec(),
                compressed: false,
            },
        ],
    )
}

fn f03() -> Vec<u8> {
    let first = vec![
        rec(2, 0, 0, BASE_TS, 1, 1, b"known"),
        rec(2, 0, 1, BASE_TS + 1, 2, 1, b"known too"),
    ];
    let second = vec![
        rec(2, 0, 0, BASE_TS + 2, 3, 9, b"before"),
        rec(3, 0, 0x9001, BASE_TS + 3, 4, 9, &[0xDE, 0xAD, 0xBE, 0xEF]),
        rec(2, 0, 1, BASE_TS + 4, 5, 9, b"after"),
    ];
    encode_segment(
        &header(8),
        &[
            FrameSpec {
                records: first,
                domain: Vec::new(),
                compressed: false,
            },
            FrameSpec {
                records: second,
                domain: Vec::new(),
                compressed: false,
            },
        ],
    )
}

fn f04() -> Vec<u8> {
    let records = vec![
        rec(2, 0, 0, BASE_TS, 1, 42, b"opened:42"),
        rec(3, 0, 1, BASE_TS + 1, 2, 42, b"closed:42"),
    ];
    let mut header = header(8);
    header.schema_hash = [0xAA; 16];
    encode_segment(
        &header,
        &[FrameSpec {
            records,
            domain: b"dc=1".to_vec(),
            compressed: false,
        }],
    )
}

fn f05() -> Vec<u8> {
    let first_records = vec![
        rec(2, 0, 0, BASE_TS, 1, 42, b"opened:42"),
        rec(3, 0, 1, BASE_TS + 1, 2, 42, b"closed:42"),
        rec(4, 1, 2, BASE_TS + 2, 3, 7, b"disk failure"),
    ];
    let second_records = vec![rec(2, 0, 0, BASE_TS + 10, 4, 42, b"torn tail")];
    let first = encode_segment(
        &header(8),
        &[FrameSpec {
            records: first_records,
            domain: b"dc=1".to_vec(),
            compressed: false,
        }],
    );
    let second = encode_segment(
        &header(8),
        &[FrameSpec {
            records: second_records,
            domain: b"dc=1".to_vec(),
            compressed: false,
        }],
    );
    // Header (56) + first frame + the frame length and two CRC bytes of the
    // second frame: a reader must stop at the end of the first frame.
    let mut out = first;
    out.extend_from_slice(&second[HEADER_LEN..HEADER_LEN + 6]);
    out
}

fn f06() -> Vec<u8> {
    let first_records = vec![rec(2, 0, 0, BASE_TS, 1, 42, b"opened:42")];
    let second_records = vec![rec(3, 0, 1, BASE_TS + 1, 2, 42, b"closed:42")];
    let mut bytes = encode_segment(
        &header(8),
        &[
            FrameSpec {
                records: first_records,
                domain: b"dc=1".to_vec(),
                compressed: false,
            },
            FrameSpec {
                records: second_records,
                domain: b"dc=1".to_vec(),
                compressed: false,
            },
        ],
    );
    let first_frame_len = frame_len(&bytes, HEADER_LEN);
    let second_offset = HEADER_LEN + 8 + first_frame_len;
    bytes[second_offset + 4] ^= 0xFF;
    bytes
}

fn frame_len(bytes: &[u8], offset: usize) -> usize {
    let len = u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("4 bytes"));
    (len & 0x7FFF_FFFF) as usize
}

fn f09() -> Vec<u8> {
    let records = vec![
        rec(2, 0, 0, BASE_TS, 1, 42, b"opened:42"),
        rec(3, 0, 1, BASE_TS + 1, 2, 42, b"closed:42"),
        rec(4, 1, 2, BASE_TS + 2, 3, 7, b"disk failure"),
    ];
    let mut header = header(8);
    header.flags = 1;
    encode_segment(
        &header,
        &[FrameSpec {
            records,
            domain: b"dc=1".to_vec(),
            compressed: true,
        }],
    )
}

fn f10() -> Vec<u8> {
    let records = vec![rec(2, 0, 0, BASE_TS, 1, 0, b"anonymous producer")];
    let mut header = header(8);
    header.producer_id = 0;
    encode_segment(
        &header,
        &[FrameSpec {
            records,
            domain: Vec::new(),
            compressed: false,
        }],
    )
}

fn f08_files() -> Vec<(String, Vec<u8>)> {
    let day_01 = day_ns(2026, 10, 1);
    let day_02 = day_ns(2026, 10, 2);
    // The newer file carries the older creation timestamp on purpose.
    let mut first = header(8);
    first.created_ns = day_02;
    let mut second = header(8);
    second.created_ns = day_01;
    let first_bytes = encode_segment(
        &first,
        &[FrameSpec {
            records: vec![rec(2, 0, 0, day_01 + 1, 1, 1, b"day one")],
            domain: Vec::new(),
            compressed: false,
        }],
    );
    let second_bytes = encode_segment(
        &second,
        &[FrameSpec {
            records: vec![rec(2, 0, 0, day_02 + 1, 1, 2, b"day two")],
            domain: Vec::new(),
            compressed: false,
        }],
    );
    vec![
        ("F08_clock_jump/2026-10-01.seg".to_owned(), first_bytes),
        ("F08_clock_jump/2026-10-02.seg".to_owned(), second_bytes),
    ]
}

fn retention_cases(files: &[(String, Vec<u8>)]) -> Value {
    let day = |name: &str| -> i64 {
        let date = name
            .rsplit('/')
            .next()
            .expect("name")
            .trim_end_matches(".seg");
        let mut parts = date.split('-');
        let year = parts.next().expect("year").parse::<i64>().expect("year");
        let month = parts.next().expect("month").parse::<u32>().expect("month");
        let day = parts.next().expect("day").parse::<u32>().expect("day");
        days_from_civil(year, month, day)
    };
    let simulate = |today: i64, keep_days: usize, cap: u64| -> Value {
        let mut entries: Vec<(String, i64, u64)> = files
            .iter()
            .map(|(name, bytes)| (name.clone(), day(name), bytes.len() as u64))
            .collect();
        entries.sort_by_key(|entry| entry.1);
        let newest = entries.last().map_or(today, |entry| entry.1);
        let window_end = today.min(newest + 1);
        let mut deleted = Vec::new();
        loop {
            let total: u64 = entries.iter().map(|entry| entry.2).sum();
            let count = entries.len();
            let over_count = keep_days > 0 && count > keep_days;
            let over_cap = cap > 0 && total > cap;
            if !over_count && !over_cap {
                break;
            }
            let Some(index) = entries.iter().position(|entry| entry.1 < window_end) else {
                break;
            };
            let removed = entries.remove(index);
            deleted.push(removed.0);
        }
        let kept: Vec<String> = entries.into_iter().map(|entry| entry.0).collect();
        json!({
            "clock_day": today,
            "keep_days": keep_days,
            "max_total_bytes": cap,
            "deleted": deleted,
            "kept": kept,
        })
    };
    let first_size = files
        .iter()
        .find(|(name, _)| name.contains("2026-10-01"))
        .map_or(0, |(_, bytes)| bytes.len() as u64);
    json!({
        "fixture": "F08",
        "files": files.iter().map(|(name, bytes)| json!({
            "name": name,
            "bytes": bytes.len(),
            "sha256": hex(&sha256(bytes)),
        })).collect::<Vec<_>>(),
        "cases": [
            simulate(days_from_civil(2026, 10, 3), 1, 0),
            simulate(days_from_civil(2026, 9, 30), 1, 0),
            simulate(days_from_civil(2026, 10, 3), 10, first_size + 10),
        ],
    })
}

/// Generate every fixture as (relative path, bytes).
pub(crate) fn generate() -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let cases: [(&str, &str, Vec<u8>, &str); 9] = [
        ("F01", "F01_normal.seg", f01(), "decoded"),
        ("F02", "F02_varints.seg", f02(), "decoded"),
        ("F03", "F03_unknown_kind.seg", f03(), "decoded"),
        ("F04", "F04_schema_mismatch.seg", f04(), "raw"),
        ("F05", "F05_torn_tail.seg", f05(), "decoded"),
        ("F06", "F06_bad_crc.seg", f06(), "decoded"),
        ("F09", "F09_zstd.seg", f09(), "decoded"),
        ("F10", "F10_producer_zero.seg", f10(), "decoded"),
        ("F07", "F07_empty.seg", Vec::new(), "refused"),
    ];
    for (name, file, bytes, mode) in cases {
        let (expected, dump) = expectation(name, file, &bytes, mode);
        files.push((file.to_owned(), bytes));
        files.push((
            format!("expected/{name}.json"),
            serde_json::to_vec_pretty(&expected).expect("json"),
        ));
        files.push((format!("expected/{name}.dump.txt"), dump.into_bytes()));
    }
    // Bad magic: a valid header with the magic replaced.
    let mut bad = header(8).encode().to_vec();
    bad[0..8].copy_from_slice(b"BADMAGIC");
    let (expected, dump) = expectation("F07b", "F07_bad_magic.seg", &bad, "refused");
    files.push(("F07_bad_magic.seg".to_owned(), bad));
    files.push((
        "expected/F07b.json".to_owned(),
        serde_json::to_vec_pretty(&expected).expect("json"),
    ));
    files.push(("expected/F07b.dump.txt".to_owned(), dump.into_bytes()));

    let f08 = f08_files();
    let expected = retention_cases(&f08);
    files.extend(f08);
    files.push((
        "F08_clock_jump/expected.json".to_owned(),
        serde_json::to_vec_pretty(&expected).expect("json"),
    ));
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn fixtures_root() -> PathBuf {
    workspace_root().join("fixtures").join("v1")
}

fn hash_table(files: &[(String, Vec<u8>)]) -> String {
    let mut lines = String::new();
    for (path, bytes) in files {
        lines.push_str(&hex(&sha256(bytes)));
        lines.push_str("  ");
        lines.push_str(path);
        lines.push('\n');
    }
    lines
}

fn compare_trees(expected: &[(String, Vec<u8>)]) -> Result<(), String> {
    let root = fixtures_root();
    let mut problems = Vec::new();
    for (path, bytes) in expected {
        let on_disk = root.join(path);
        match fs::read(&on_disk) {
            Ok(actual) if actual == *bytes => {}
            Ok(actual) => problems.push(format!(
                "{path}: committed {} bytes, regenerated {} bytes",
                actual.len(),
                bytes.len()
            )),
            Err(error) => problems.push(format!("{path}: {error}")),
        }
    }
    let expected_paths: BTreeSet<&str> = expected.iter().map(|(path, _)| path.as_str()).collect();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let relative = path
                .strip_prefix(&root)
                .map_err(|error| error.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            if relative == "HASHES.txt" || relative == "README.md" {
                continue;
            }
            if !expected_paths.contains(relative.as_str()) {
                problems.push(format!("{relative}: not produced by the generator"));
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

fn check_hashes(files: &[(String, Vec<u8>)]) -> Result<(), String> {
    let path = fixtures_root().join("HASHES.txt");
    let on_disk =
        fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let expected = hash_table(files);
    if on_disk == expected {
        Ok(())
    } else {
        Err(format!(
            "{}: hash table differs from the regenerated one",
            path.display()
        ))
    }
}

/// Write fixtures/v1 from the generator.
pub(crate) fn generate_to_disk() -> Result<(), String> {
    let root = fixtures_root();
    if root.exists() {
        fs::remove_dir_all(&root).map_err(|error| format!("{}: {error}", root.display()))?;
    }
    fs::create_dir_all(root.join("expected"))
        .map_err(|error| format!("{}: {error}", root.display()))?;
    let files = generate();
    for (relative, bytes) in &files {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        fs::write(&path, bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    fs::write(root.join("HASHES.txt"), hash_table(&files)).map_err(|error| error.to_string())?;
    println!(
        "fixtures: wrote {} files to {}",
        files.len(),
        root.display()
    );
    Ok(())
}

/// Regenerate in memory and compare with the committed fixture tree.
pub(crate) fn check() -> Result<(), String> {
    let files = generate();
    compare_trees(&files)?;
    check_hashes(&files)?;
    println!("fixtures: {} files match", files.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{HEADER_LEN, crc32c, generate, parse_segment};
    use crate::codec::hex;
    use crate::codec::sha256;

    #[test]
    fn generated_fixtures_are_self_consistent() {
        let files = generate();
        assert!(files.len() >= 20);
        let f01 = files
            .iter()
            .find(|(path, _)| path == "F01_normal.seg")
            .map(|(_, bytes)| bytes.clone())
            .expect("F01");
        let parsed = parse_segment(&f01);
        assert_eq!(parsed.frames, 1);
        assert_eq!(parsed.records.len(), 3);
        assert_eq!(parsed.valid_end, f01.len() as u64);
        assert_eq!(parsed.tail, "clean");
        assert_eq!(parsed.header.expect("header").kind_count, 8);
    }

    #[test]
    fn torn_tail_is_reported_not_decoded() {
        let files = generate();
        let f05 = files
            .iter()
            .find(|(path, _)| path == "F05_torn_tail.seg")
            .map(|(_, bytes)| bytes.clone())
            .expect("F05");
        let parsed = parse_segment(&f05);
        assert_eq!(parsed.tail, "truncated");
        assert_eq!(parsed.records.len(), 3);
        assert_eq!(parsed.truncated_bytes, 6);
    }

    #[test]
    fn corrupt_crc_ends_the_prefix() {
        let files = generate();
        let f06 = files
            .iter()
            .find(|(path, _)| path == "F06_bad_crc.seg")
            .map(|(_, bytes)| bytes.clone())
            .expect("F06");
        let parsed = parse_segment(&f06);
        assert_eq!(parsed.tail, "corrupt");
        assert_eq!(parsed.records.len(), 1);
        assert!(parsed.corrupt_offset.unwrap() >= HEADER_LEN as u64);
    }

    #[test]
    fn fixture_sha_is_stable() {
        assert_eq!(hex(&sha256(b"logreel")), hex(&sha256(b"logreel")));
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    }
}
