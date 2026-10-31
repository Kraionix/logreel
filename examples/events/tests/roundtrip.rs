//! Roundtrip and byte-identity tests for the example domain.

use logreel::{BatchEncoder, Event, Level, Record, Render};
use logreel_example_events::{
    BytesOut, DomainEvent, Handshake, SessionClosed, SessionOpened, Tick,
};

fn encode(event: &DomainEvent) -> Vec<u8> {
    let mut out = Vec::new();
    Event::encode(event, &mut out);
    out
}

fn decode(kind: u16, bytes: &[u8]) -> DomainEvent {
    <DomainEvent as Event>::decode(kind, bytes).expect("decode")
}

#[test]
fn kinds_are_the_domain_offset() {
    assert_eq!(
        DomainEvent::SessionOpened(SessionOpened {
            conn_id: 1,
            remote: String::new()
        })
        .kind(),
        0
    );
    assert_eq!(
        DomainEvent::SessionClosed(SessionClosed {
            conn_id: 1,
            bytes_out: 2
        })
        .kind(),
        1
    );
    assert_eq!(
        DomainEvent::BytesOut(BytesOut {
            conn_id: 1,
            bytes: 2
        })
        .kind(),
        2
    );
    assert_eq!(
        DomainEvent::Handshake(Handshake {
            conn_id: 1,
            protocol: 3
        })
        .kind(),
        3
    );
    assert_eq!(DomainEvent::Tick(Tick { now_ns: 4 }).kind(), 4);
    assert_eq!(DomainEvent::KINDS, 5);
    assert_eq!(DomainEvent::CODEC_ID, 1);
    assert_eq!(
        DomainEvent::SCHEMA_HASH,
        [
            0x9f, 0x2c, 0x4d, 0x3a, 0x1b, 0x0e, 0x5c, 0x6d, 0x7a, 0x8b, 0x9c, 0x0d, 0x1e, 0x2f,
            0x3a, 0x4b
        ]
    );
}

#[test]
fn derived_and_prost_encoding_are_identical() {
    let value = SessionOpened {
        conn_id: 7,
        remote: "127.0.0.1:443".to_owned(),
    };
    let event = DomainEvent::SessionOpened(value.clone());
    let mut via_event = Vec::new();
    Event::encode(&event, &mut via_event);
    let mut via_prost = Vec::new();
    prost::Message::encode(&value, &mut via_prost).expect("prost encode");
    assert_eq!(via_event, via_prost);
}

#[test]
fn every_variant_roundtrips() {
    let values = [
        DomainEvent::SessionOpened(SessionOpened {
            conn_id: 1,
            remote: "a".to_owned(),
        }),
        DomainEvent::SessionClosed(SessionClosed {
            conn_id: 2,
            bytes_out: 1024,
        }),
        DomainEvent::BytesOut(BytesOut {
            conn_id: 3,
            bytes: 99,
        }),
        DomainEvent::Handshake(Handshake {
            conn_id: 4,
            protocol: 7,
        }),
        DomainEvent::Tick(Tick {
            now_ns: 1_700_000_000_000_000_000,
        }),
    ];
    for value in values {
        let encoded = encode(&value);
        let decoded = decode(value.kind(), &encoded);
        assert_eq!(decoded, value);
    }
}

#[test]
fn hand_written_event_has_a_fixed_layout() {
    let value = Tick {
        now_ns: 0x0102_0304_0506_0708,
    };
    let encoded = encode(&DomainEvent::Tick(value));
    assert_eq!(encoded, value.now_ns.to_le_bytes());
    assert_eq!(decode(4, &encoded), DomainEvent::Tick(value));
    assert!(<DomainEvent as Event>::decode(4, &[0u8; 7]).is_err());
}

#[test]
fn unknown_kind_does_not_panic() {
    let error = <DomainEvent as Event>::decode(99, &[]).expect_err("unknown kind");
    assert_eq!(error.kind(), 99);
}

#[test]
fn batch_envelope_matches_the_specification() {
    let events = [
        DomainEvent::SessionOpened(SessionOpened {
            conn_id: 42,
            remote: "r".to_owned(),
        }),
        DomainEvent::Tick(Tick { now_ns: 5 }),
    ];
    let records: Vec<Record<DomainEvent>> = events
        .iter()
        .enumerate()
        .map(|(index, event)| {
            Record::new(
                Level::Info,
                0,
                event.kind(),
                1_700_000_000_000_000_000 + index as u64,
                index as u32 + 1,
                42,
                event.clone(),
            )
        })
        .collect();
    let mut encoder = BatchEncoder::new();
    let batch = encoder.encode(&records, b"dc=1");
    assert_eq!(batch.record_count, 2);
    assert_eq!(batch.codec_id, 1);
    let payload = &batch.payload;
    assert_eq!(u32::from_le_bytes(payload[0..4].try_into().unwrap()), 2);
    let mask_len = usize::from(u16::from_le_bytes(payload[20..22].try_into().unwrap()));
    assert_eq!(mask_len, 1);
    let domain_start = 22 + mask_len;
    let domain_len = usize::from(u16::from_le_bytes(
        payload[domain_start..domain_start + 2].try_into().unwrap(),
    ));
    assert_eq!(
        &payload[domain_start + 2..domain_start + 2 + domain_len],
        b"dc=1"
    );
    let mut cursor = domain_start + 2 + domain_len;
    for (index, event) in events.iter().enumerate() {
        assert_eq!(payload[cursor], Level::Info.as_u8());
        assert_eq!(payload[cursor + 1], 0);
        assert_eq!(
            u16::from_le_bytes(payload[cursor + 2..cursor + 4].try_into().unwrap()),
            event.kind()
        );
        cursor += 12;
        // seq, conn_id, payload_len are one byte each here.
        assert_eq!(payload[cursor], index as u8 + 1);
        assert_eq!(payload[cursor + 1], 42);
        let payload_len = usize::from(payload[cursor + 2]);
        cursor += 3;
        assert_eq!(&payload[cursor..cursor + payload_len], encode(event));
        cursor += payload_len;
    }
    assert_eq!(cursor, payload.len());
}

#[test]
fn render_lines_are_stable() {
    let mut line = String::new();
    DomainEvent::SessionClosed(SessionClosed {
        conn_id: 7,
        bytes_out: 1024,
    })
    .render(&mut line);
    assert_eq!(line, "session closed conn=7 bytes_out=1024");
}
