//! redb-backed store for logreel.
//!
//! The segment format is unchanged: this backend stores the same encoded
//! batch payload in a redb table keyed by a monotonic sequence number. It is
//! an alternative to the file backend, not a second format.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::path::Path;

use logreel_store::{BatchScan, Durability, EncodedBatch, ErrorKind, Store, StoreError};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

const BATCHES: TableDefinition<'static, u64, &'static [u8]> = TableDefinition::new("batches");

/// A [Store] implementation over a redb database.
#[derive(Debug)]
pub struct RedbStore {
    database: Database,
    next: u64,
}

impl RedbStore {
    /// Open or create a database at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let database = Database::create(path)
            .map_err(|error| StoreError::new(ErrorKind::Io, format!("redb: {error}")))?;
        let next = match database.begin_read() {
            Ok(read) => match read.open_table(BATCHES) {
                Ok(table) => match table.last() {
                    Ok(Some(entry)) => entry.0.value().saturating_add(1),
                    Ok(None) => 0,
                    Err(error) => {
                        return Err(StoreError::new(
                            ErrorKind::Io,
                            format!("redb last: {error}"),
                        ));
                    }
                },
                Err(redb::TableError::TableDoesNotExist(_)) => 0,
                Err(error) => {
                    return Err(StoreError::new(
                        ErrorKind::Io,
                        format!("redb open table: {error}"),
                    ));
                }
            },
            Err(error) => {
                return Err(StoreError::new(
                    ErrorKind::Io,
                    format!("redb read: {error}"),
                ));
            }
        };
        Ok(Self { database, next })
    }

    /// Number of batches appended so far.
    #[must_use]
    pub const fn len(&self) -> u64 {
        self.next
    }

    /// True when no batch was appended.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.next == 0
    }
}

fn serialize(batch: &EncodedBatch) -> Vec<u8> {
    let mut out = Vec::with_capacity(40 + batch.domain.len() + batch.payload.len());
    out.extend_from_slice(&batch.codec_id.to_le_bytes());
    out.extend_from_slice(&batch.schema_hash);
    out.extend_from_slice(&batch.kind_count.to_le_bytes());
    out.extend_from_slice(&batch.record_count.to_le_bytes());
    out.extend_from_slice(&batch.first_ts_ns.to_le_bytes());
    out.extend_from_slice(&batch.last_ts_ns.to_le_bytes());
    out.extend_from_slice(&(batch.domain.len() as u32).to_le_bytes());
    out.extend_from_slice(&batch.domain);
    out.extend_from_slice(&(batch.payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&batch.payload);
    out
}

fn deserialize(bytes: &[u8]) -> Result<EncodedBatch, StoreError> {
    let mut pos = 0usize;
    let mut take = |len: usize| -> Result<&[u8], StoreError> {
        let end = pos + len;
        let slice = bytes
            .get(pos..end)
            .ok_or_else(|| StoreError::new(ErrorKind::Format, "redb batch is truncated"))?;
        pos = end;
        Ok(slice)
    };
    let codec_id = u16::from_le_bytes(take(2)?.try_into().expect("2 bytes"));
    let mut schema_hash = [0u8; 16];
    schema_hash.copy_from_slice(take(16)?);
    let kind_count = u16::from_le_bytes(take(2)?.try_into().expect("2 bytes"));
    let record_count = u32::from_le_bytes(take(4)?.try_into().expect("4 bytes"));
    let first_ts_ns = u64::from_le_bytes(take(8)?.try_into().expect("8 bytes"));
    let last_ts_ns = u64::from_le_bytes(take(8)?.try_into().expect("8 bytes"));
    let domain_len = u32::from_le_bytes(take(4)?.try_into().expect("4 bytes")) as usize;
    let domain = take(domain_len)?.to_vec();
    let payload_len = u32::from_le_bytes(take(4)?.try_into().expect("4 bytes")) as usize;
    let payload = take(payload_len)?.to_vec();
    if pos != bytes.len() {
        return Err(StoreError::new(
            ErrorKind::Format,
            "redb batch has trailing bytes",
        ));
    }
    Ok(EncodedBatch::new(
        codec_id,
        schema_hash,
        kind_count,
        record_count,
        first_ts_ns,
        last_ts_ns,
        domain.into_boxed_slice(),
        payload.into_boxed_slice(),
    ))
}

impl Store for RedbStore {
    fn append(&mut self, batch: &EncodedBatch) -> Result<(), StoreError> {
        let value = serialize(batch);
        let write = self
            .database
            .begin_write()
            .map_err(|error| StoreError::new(ErrorKind::Io, format!("redb write: {error}")))?;
        {
            let mut table = write
                .open_table(BATCHES)
                .map_err(|error| StoreError::new(ErrorKind::Io, format!("redb table: {error}")))?;
            table
                .insert(self.next, value.as_slice())
                .map_err(|error| StoreError::new(ErrorKind::Io, format!("redb insert: {error}")))?;
        }
        write
            .commit()
            .map_err(|error| StoreError::new(ErrorKind::Io, format!("redb commit: {error}")))?;
        self.next = self.next.saturating_add(1);
        Ok(())
    }

    fn sync(&mut self, _mode: Durability) -> Result<(), StoreError> {
        // Every append commits; there is nothing left to flush.
        Ok(())
    }

    fn scan(&self) -> Result<BatchScan<'_>, StoreError> {
        let read = self
            .database
            .begin_read()
            .map_err(|error| StoreError::new(ErrorKind::Io, format!("redb read: {error}")))?;
        let table = match read.open_table(BATCHES) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => {
                return Ok(Box::new(std::iter::empty()));
            }
            Err(error) => {
                return Err(StoreError::new(
                    ErrorKind::Io,
                    format!("redb table: {error}"),
                ));
            }
        };
        let mut batches = Vec::new();
        for entry in table
            .iter()
            .map_err(|error| StoreError::new(ErrorKind::Io, format!("redb iter: {error}")))?
        {
            let entry = entry
                .map_err(|error| StoreError::new(ErrorKind::Io, format!("redb entry: {error}")))?;
            batches.push(deserialize(entry.1.value())?);
        }
        Ok(Box::new(batches.into_iter().map(Ok)))
    }
}

#[cfg(test)]
mod tests {
    use super::RedbStore;
    use logreel_store::{Durability, EncodedBatch, Store};

    fn temp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "logreel-redb-{tag}-{}-{:?}.redb",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    fn batch(count: u32, ts: u64) -> EncodedBatch {
        EncodedBatch::new(
            3,
            [0x33; 16],
            8,
            count,
            ts,
            ts + 1,
            b"dc=2".to_vec().into_boxed_slice(),
            vec![0xAB; 16].into_boxed_slice(),
        )
    }

    #[test]
    fn append_scan_and_reopen() {
        let path = temp_path("roundtrip");
        let _ = std::fs::remove_file(&path);
        {
            let mut store = RedbStore::open(&path).expect("open");
            store.append(&batch(2, 10)).expect("append");
            store.append(&batch(3, 20)).expect("append");
            Store::sync(&mut store, Durability::Immediate).expect("sync");
            assert_eq!(store.len(), 2);
            let scanned: Vec<_> = store
                .scan()
                .expect("scan")
                .collect::<Result<_, _>>()
                .expect("batches");
            assert_eq!(scanned.len(), 2);
            assert_eq!(scanned[1].record_count, 3);
            assert_eq!(&*scanned[0].domain, b"dc=2");
        }
        let store = RedbStore::open(&path).expect("reopen");
        assert_eq!(store.len(), 2);
        assert!(!store.is_empty());
        let scanned: Vec<_> = store
            .scan()
            .expect("scan")
            .collect::<Result<_, _>>()
            .expect("batches");
        assert_eq!(scanned[0].first_ts_ns, 10);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn empty_database_scans_empty() {
        let path = temp_path("empty");
        let _ = std::fs::remove_file(&path);
        let store = RedbStore::open(&path).expect("open");
        assert!(store.is_empty());
        let scanned: Vec<_> = store
            .scan()
            .expect("scan")
            .collect::<Result<_, _>>()
            .expect("batches");
        assert!(scanned.is_empty());
        let _ = std::fs::remove_file(&path);
    }
}
