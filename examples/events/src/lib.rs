//! Example event domain: four prost events and one hand written event.
//!
//! The derived structs implement the logreel Event contract; the enum is the
//! single event type of the process and delegates kind, encode and decode.

use logreel::{Event, Render};
use prost::Message;

/// A connection was accepted.
#[derive(Clone, PartialEq, Message, Event)]
#[event(kind = 0, codec = "prost", crate = "logreel")]
pub struct SessionOpened {
    /// Connection id.
    #[prost(uint64, tag = "1")]
    pub conn_id: u64,
    /// Remote address, display form.
    #[prost(string, tag = "2")]
    pub remote: String,
}

/// A connection ended.
#[derive(Clone, PartialEq, Message, Event)]
#[event(kind = 1, codec = "prost", crate = "logreel")]
pub struct SessionClosed {
    /// Connection id.
    #[prost(uint64, tag = "1")]
    pub conn_id: u64,
    /// Bytes written to the client.
    #[prost(uint64, tag = "2")]
    pub bytes_out: u64,
}

/// Periodic byte counter.
#[derive(Clone, PartialEq, Message, Event)]
#[event(kind = 2, codec = "prost", crate = "logreel")]
pub struct BytesOut {
    /// Connection id.
    #[prost(uint64, tag = "1")]
    pub conn_id: u64,
    /// Bytes since the last report.
    #[prost(uint64, tag = "2")]
    pub bytes: u64,
}

/// Protocol negotiation finished.
#[derive(Clone, PartialEq, Message, Event)]
#[event(kind = 3, codec = "prost", crate = "logreel")]
pub struct Handshake {
    /// Connection id.
    #[prost(uint64, tag = "1")]
    pub conn_id: u64,
    /// Selected protocol number.
    #[prost(uint32, tag = "2")]
    pub protocol: u32,
}

/// A hand written event, no codec and no derive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tick {
    /// Current Unix time in nanoseconds.
    pub now_ns: u64,
}

impl Event for Tick {
    const KINDS: u16 = 5;

    fn kind(&self) -> u16 {
        4
    }

    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.now_ns.to_le_bytes());
    }

    fn decode(kind: u16, bytes: &[u8]) -> Result<Self, logreel::DecodeError> {
        if kind != 4 || bytes.len() != 8 {
            return Err(logreel::DecodeError::new(kind, "tick expects 8 bytes"));
        }
        let mut array = [0u8; 8];
        array.copy_from_slice(bytes);
        Ok(Self {
            now_ns: u64::from_le_bytes(array),
        })
    }
}

/// The single event type of the process.
#[derive(Clone, Debug, PartialEq, Event)]
#[event(
    codec_id = 1,
    kind_count = 5,
    schema_hash = "9f2c4d3a1b0e5c6d7a8b9c0d1e2f3a4b",
    crate = "logreel"
)]
pub enum DomainEvent {
    /// Session opened.
    #[event(kind = 0)]
    SessionOpened(SessionOpened),
    /// Session closed.
    #[event(kind = 1)]
    SessionClosed(SessionClosed),
    /// Byte counter.
    #[event(kind = 2)]
    BytesOut(BytesOut),
    /// Handshake done.
    #[event(kind = 3)]
    Handshake(Handshake),
    /// Clock tick.
    #[event(kind = 4)]
    Tick(Tick),
}

impl Render for DomainEvent {
    fn render(&self, out: &mut String) {
        use std::fmt::Write as _;
        match self {
            Self::SessionOpened(event) => {
                let _ = write!(
                    out,
                    "session opened conn={} remote={}",
                    event.conn_id, event.remote
                );
            }
            Self::SessionClosed(event) => {
                let _ = write!(
                    out,
                    "session closed conn={} bytes_out={}",
                    event.conn_id, event.bytes_out
                );
            }
            Self::BytesOut(event) => {
                let _ = write!(
                    out,
                    "bytes out conn={} bytes={}",
                    event.conn_id, event.bytes
                );
            }
            Self::Handshake(event) => {
                let _ = write!(
                    out,
                    "handshake conn={} protocol={}",
                    event.conn_id, event.protocol
                );
            }
            Self::Tick(event) => {
                let _ = write!(out, "tick now_ns={}", event.now_ns);
            }
        }
    }
}
