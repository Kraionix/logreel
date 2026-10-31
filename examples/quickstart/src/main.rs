//! Emit three events, write a segment, read it back.
//!
//! Run with: cargo run -p logreel-quickstart -- ./logs
//! The core-only build (--no-default-features) writes to memory and prints
//! the same counters, so the size comparison has a baseline.

use std::error::Error;

#[cfg(not(feature = "file"))]
use logreel::SharedSink;
#[cfg(feature = "text")]
use logreel::TextTarget;
#[cfg(feature = "file")]
use logreel::file::{FileStore, FileStoreConfig, probe, segments};
use logreel::{DecodeError, Event, Level, Settings, init};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ping {
    conn_id: u64,
    bytes: u64,
}

impl Event for Ping {
    const KINDS: u16 = 1;

    fn kind(&self) -> u16 {
        0
    }

    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.conn_id.to_le_bytes());
        out.extend_from_slice(&self.bytes.to_le_bytes());
    }

    fn decode(kind: u16, bytes: &[u8]) -> Result<Self, DecodeError> {
        if kind != 0 || bytes.len() != 16 {
            return Err(DecodeError::new(kind, "ping expects 16 bytes"));
        }
        let conn_id = u64::from_le_bytes(bytes[0..8].try_into().expect("8 bytes"));
        let value = u64::from_le_bytes(bytes[8..16].try_into().expect("8 bytes"));
        Ok(Self {
            conn_id,
            bytes: value,
        })
    }
}

#[cfg(feature = "file")]
fn render(event: &Ping, out: &mut String) {
    if cfg!(feature = "text") {
        out.push_str(&format!(
            "ping conn={} bytes={}",
            event.conn_id, event.bytes
        ));
    }
}

fn emit_three(handle: &logreel::Handle<Ping>) {
    for conn_id in 1..=3 {
        handle.emit(
            Level::Info,
            0,
            conn_id,
            Ping {
                conn_id,
                bytes: conn_id * 1024,
            },
        );
    }
}

#[cfg(feature = "file")]
fn main() -> Result<(), Box<dyn Error>> {
    let directory = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "./logs".to_owned());
    let builder = Settings::builder()
        .min_level(Level::Info)
        .domain(b"dc=1".to_vec());
    #[cfg(feature = "text")]
    let builder = builder.text(Level::Info, TextTarget::Stderr);
    let settings = builder.build();
    let config = FileStoreConfig {
        directory: directory.clone().into(),
        producer_id: 1,
        ..FileStoreConfig::new(&directory)
    };
    let logger = init::<Ping>(settings, Some(render), Box::new(FileStore::open(config)?))?;
    emit_three(logger.handle());
    let report = logger.shutdown();
    println!(
        "wrote {} records in {} batches",
        report.records, report.batches
    );
    for file in segments(directory.as_ref())? {
        let value = probe(&file)?;
        println!(
            "{}: {} frames, {} records, tail {:?}, sha256 {}",
            file.display(),
            value.frames,
            value.records,
            value.tail,
            &value.sha256_hex()[..12]
        );
    }
    Ok(())
}

#[cfg(not(feature = "file"))]
fn main() -> Result<(), Box<dyn Error>> {
    let settings = Settings::builder().min_level(Level::Info).build();
    let logger = init::<Ping>(settings, None, Box::new(SharedSink::new()))?;
    emit_three(logger.handle());
    let report = logger.shutdown();
    println!(
        "wrote {} records in {} batches",
        report.records, report.batches
    );
    Ok(())
}
