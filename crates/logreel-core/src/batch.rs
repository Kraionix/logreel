use logreel_store::EncodedBatch;

use crate::Event;
use crate::Record;

/// Encodes records into the batch payload of the specification.
///
/// The encoder reuses its internal buffers; one allocation per batch remains
/// for the owned payload that the sink receives. That allocation is on the
/// writer thread and amortised over the whole batch.
#[derive(Debug)]
pub struct BatchEncoder {
    body: Vec<u8>,
    payload: Vec<u8>,
    scratch: Vec<u8>,
}

impl BatchEncoder {
    /// Create an encoder with empty reusable buffers.
    #[must_use]
    pub fn new() -> Self {
        Self {
            body: Vec::with_capacity(16 * 1024),
            payload: Vec::with_capacity(16 * 1024),
            scratch: Vec::with_capacity(256),
        }
    }

    /// Encode a batch.
    ///
    /// Returns an empty batch (zero records) when the input is empty. The
    /// caller is expected to skip the sink in that case.
    pub fn encode<E: Event>(&mut self, records: &[Record<E>], domain: &[u8]) -> EncodedBatch {
        if records.is_empty() {
            return EncodedBatch::new(
                E::CODEC_ID,
                E::SCHEMA_HASH,
                0,
                0,
                0,
                domain.to_vec().into_boxed_slice(),
                Box::new([]),
            );
        }

        let mut first_ts = u64::MAX;
        let mut last_ts = 0u64;
        let mask_len = usize::from(E::KINDS).div_ceil(8);
        let mut mask = vec![0u8; mask_len];
        self.body.clear();

        for record in records {
            first_ts = first_ts.min(record.ts_ns);
            last_ts = last_ts.max(record.ts_ns);
            let local = record.kind.saturating_sub(E::BASE);
            let index = usize::from(local);
            if index < usize::from(E::KINDS) {
                mask[index / 8] |= 1 << (index % 8);
            }

            self.scratch.clear();
            record.event.encode(&mut self.scratch);

            self.body.push(record.level.as_u8());
            self.body.push(record.category);
            self.body.extend_from_slice(&record.kind.to_le_bytes());
            self.body.extend_from_slice(&record.ts_ns.to_le_bytes());
            put_varint(&mut self.body, u64::from(record.seq));
            put_varint(&mut self.body, record.conn_id);
            put_varint(&mut self.body, self.scratch.len() as u64);
            self.body.extend_from_slice(&self.scratch);
        }

        self.payload.clear();
        self.payload
            .extend_from_slice(&(records.len() as u32).to_le_bytes());
        self.payload.extend_from_slice(&first_ts.to_le_bytes());
        self.payload.extend_from_slice(&last_ts.to_le_bytes());
        self.payload
            .extend_from_slice(&(mask.len() as u16).to_le_bytes());
        self.payload.extend_from_slice(&mask);
        let domain_len = domain.len().min(usize::from(u16::MAX));
        self.payload
            .extend_from_slice(&(domain_len as u16).to_le_bytes());
        self.payload.extend_from_slice(&domain[..domain_len]);
        self.payload.extend_from_slice(&self.body);

        let payload = std::mem::take(&mut self.payload);
        EncodedBatch::new(
            E::CODEC_ID,
            E::SCHEMA_HASH,
            records.len() as u32,
            first_ts,
            last_ts,
            domain.to_vec().into_boxed_slice(),
            payload.into_boxed_slice(),
        )
    }
}

impl Default for BatchEncoder {
    fn default() -> Self {
        Self::new()
    }
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}

#[cfg(test)]
mod tests {
    use super::BatchEncoder;
    use crate::{DecodeError, Event, Level, Record};

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum TestEvent {
        Number(u64),
        Empty,
    }

    impl Event for TestEvent {
        const KINDS: u16 = 2;

        fn kind(&self) -> u16 {
            match self {
                Self::Number(_) => 0,
                Self::Empty => 1,
            }
        }

        fn encode(&self, out: &mut Vec<u8>) {
            if let Self::Number(value) = self {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }

        fn decode(kind: u16, bytes: &[u8]) -> Result<Self, DecodeError> {
            match kind {
                0 if bytes.len() == 8 => {
                    let mut array = [0u8; 8];
                    array.copy_from_slice(bytes);
                    Ok(Self::Number(u64::from_le_bytes(array)))
                }
                1 if bytes.is_empty() => Ok(Self::Empty),
                _ => Err(DecodeError::new(kind, "bad payload")),
            }
        }
    }

    fn record(seq: u32, kind: u16, event: TestEvent) -> Record<TestEvent> {
        Record::new(
            Level::Info,
            0,
            kind,
            1_700_000_000_000_000_000,
            seq,
            7,
            event,
        )
    }

    #[test]
    fn header_carries_count_timestamps_and_mask() {
        let mut encoder = BatchEncoder::new();
        let batch = encoder.encode(
            &[
                record(1, 0, TestEvent::Number(42)),
                record(2, 1, TestEvent::Empty),
            ],
            b"dc=1",
        );
        assert_eq!(batch.record_count, 2);
        assert_eq!(batch.first_ts_ns, 1_700_000_000_000_000_000);
        assert_eq!(batch.last_ts_ns, 1_700_000_000_000_000_000);
        assert_eq!(&batch.payload[0..4], &2u32.to_le_bytes());
        let mask_len = u16::from_le_bytes([batch.payload[20], batch.payload[21]]);
        assert_eq!(mask_len, 1);
        assert_eq!(batch.payload[22], 0b11);
        assert_eq!(&batch.payload[23..25], &4u16.to_le_bytes());
        assert_eq!(&batch.payload[25..29], b"dc=1");
    }

    #[test]
    fn empty_input_produces_an_empty_batch() {
        let mut encoder = BatchEncoder::new();
        let batch = encoder.encode::<TestEvent>(&[], b"");
        assert_eq!(batch.record_count, 0);
        assert!(batch.is_empty());
        assert!(batch.payload.is_empty());
    }

    #[test]
    fn varints_are_short_for_small_values() {
        let mut encoder = BatchEncoder::new();
        let batch = encoder.encode(&[record(1, 0, TestEvent::Number(0))], b"");
        // 4 + 8 + 8 + 2 + 1 + 2 = 25 header bytes;
        // record: 12 fixed + 3 varints + 8 payload bytes.
        assert_eq!(batch.payload.len(), 25 + 12 + 3 + 8);
    }
}
