use std::fmt;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use logreel_store::{BatchSink, Durability};

use crate::batch::BatchEncoder;
use crate::filter::Filters;
use crate::ring::Ring;
use crate::settings::{InternalError, InternalErrorKind, Settings};
use crate::text::{TextSink, format_record};
use crate::time::now_ns;
use crate::{Event, Level, Record};

enum Msg<E> {
    Record(Record<E>),
    Flush,
    Shutdown,
}

/// Shared producer handle.
///
/// The handle is never cloned; it is shared as an Arc. It carries the bounded
/// sender, the filter table, the sequence counter and the drop counter. A
/// handle outside any configured scope still works; the scoped API is a
/// convenience on top.
pub struct Handle<E: Event> {
    tx: SyncSender<Msg<E>>,
    filters: Arc<Filters>,
    dropped: AtomicU64,
    seq: AtomicU32,
    ring: Option<Arc<Mutex<Ring>>>,
}

impl<E: Event> Handle<E> {
    /// True when a record at this category and level is wanted.
    #[must_use]
    pub fn enabled(&self, category: u8, level: Level) -> bool {
        self.filters.enabled(category, level)
    }

    /// Filter table, for live reconfiguration.
    #[must_use]
    pub fn filters(&self) -> &Filters {
        &self.filters
    }

    /// Try to emit one record.
    ///
    /// Returns true when the record was queued, false when the filter
    /// rejected it or the channel was full. A full channel increments the
    /// drop counter and never blocks the producer.
    pub fn emit(&self, level: Level, category: u8, conn_id: u64, event: E) -> bool {
        if !self.filters.enabled(category, level) {
            return false;
        }
        let record = Record::new(
            level,
            category,
            event.kind(),
            now_ns(),
            self.seq.fetch_add(1, Ordering::Relaxed),
            conn_id,
            event,
        );
        match self.tx.try_send(Msg::Record(record)) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Ask the writer to flush the current batch.
    #[must_use]
    pub fn flush(&self) -> bool {
        self.tx.try_send(Msg::Flush).is_ok()
    }

    /// Records dropped because the channel was full.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Next sequence number that will be assigned.
    #[must_use]
    pub fn sequence(&self) -> u32 {
        self.seq.load(Ordering::Relaxed)
    }

    /// Retained ring lines, oldest first.
    #[must_use]
    pub fn dump_ring(&self) -> Vec<String> {
        match &self.ring {
            Some(ring) => ring.lock().map(|ring| ring.snapshot()).unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// Ring lines dropped by the byte or count cap.
    #[must_use]
    pub fn ring_dropped(&self) -> u64 {
        match &self.ring {
            Some(ring) => ring.lock().map(|ring| ring.dropped()).unwrap_or(0),
            None => 0,
        }
    }
}

impl<E: Event> fmt::Debug for Handle<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Handle")
            .field("dropped", &self.dropped())
            .field("sequence", &self.sequence())
            .field("ring", &self.ring.is_some())
            .finish()
    }
}

/// Counters collected by the writer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShutdownReport {
    /// Records handed to a sink.
    pub records: u64,
    /// Batches handed to a sink.
    pub batches: u64,
    /// Payload bytes plus frame headers.
    pub bytes: u64,
    /// Records dropped by a full channel.
    pub dropped: u64,
    /// Sink write or sync failures.
    pub sink_errors: u64,
    /// Text write failures.
    pub text_errors: u64,
    /// Ring lines dropped by its caps.
    pub ring_dropped: u64,
    /// Last sink error, if any.
    pub error: Option<String>,
}

/// Owner of the writer thread.
///
/// Dropping the logger without calling [Logger::shutdown] disconnects the
/// channel and lets the writer drain in the background; the thread is not
/// joined. Call shutdown for a deterministic stop.
pub struct Logger<E: Event> {
    handle: Arc<Handle<E>>,
    join: Option<JoinHandle<WriterResult>>,
}

impl<E: Event> Logger<E> {
    /// Shared producer handle.
    #[must_use]
    pub fn handle(&self) -> &Arc<Handle<E>> {
        &self.handle
    }

    /// Stop the writer deterministically: drain, flush, sync, join.
    #[must_use]
    pub fn shutdown(mut self) -> ShutdownReport {
        // A blocking send: the channel may be full, and the writer is the
        // only consumer. It drains records before it observes the shutdown.
        let _ = self.handle.tx.send(Msg::Shutdown);
        let result = self
            .join
            .take()
            .and_then(|join| join.join().ok())
            .map(ShutdownReport::from)
            .unwrap_or_default();
        ShutdownReport {
            dropped: self.handle.dropped(),
            ring_dropped: self.handle.ring_dropped(),
            ..result
        }
    }
}

impl<E: Event> fmt::Debug for Logger<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Logger")
            .field("handle", &self.handle)
            .finish()
    }
}

/// Configuration error detected before the writer starts.
#[derive(Debug)]
pub enum InitError {
    /// The channel capacity is zero.
    ZeroCapacity,
    /// The batch size is zero.
    ZeroBatch,
    /// The flush interval is zero.
    ZeroFlushInterval,
    /// Text or ring is enabled without a render function.
    MissingRender,
    /// Text projection is configured but the text feature is off.
    TextDisabled,
    /// The writer thread could not be spawned.
    Spawn(String),
}

impl fmt::Display for InitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroCapacity => formatter.write_str("channel capacity must be non-zero"),
            Self::ZeroBatch => formatter.write_str("batch size must be non-zero"),
            Self::ZeroFlushInterval => formatter.write_str("flush interval must be non-zero"),
            Self::MissingRender => {
                formatter.write_str("text or ring is enabled without a render function")
            }
            Self::TextDisabled => formatter
                .write_str("text projection is configured but the logreel text feature is off"),
            Self::Spawn(message) => write!(formatter, "writer thread: {message}"),
        }
    }
}

impl std::error::Error for InitError {}

/// Start the pipeline.
///
/// The render function is required when text or the ring is enabled and is
/// ignored otherwise. It is a plain function pointer, monomorphised per
/// event type: no per-record trait object.
pub fn init<E: Event + Send + 'static>(
    settings: Settings,
    render: Option<fn(&E, &mut String)>,
    sink: Box<dyn BatchSink>,
) -> Result<Logger<E>, InitError> {
    if settings.channel_capacity == 0 {
        return Err(InitError::ZeroCapacity);
    }
    if settings.batch_max == 0 {
        return Err(InitError::ZeroBatch);
    }
    if settings.flush_interval.is_zero() {
        return Err(InitError::ZeroFlushInterval);
    }
    if (settings.text.is_some() || settings.ring.is_some()) && render.is_none() {
        return Err(InitError::MissingRender);
    }
    if !cfg!(feature = "text") && settings.text.is_some() {
        return Err(InitError::TextDisabled);
    }

    let filters = Arc::new(Filters::new(settings.min_level));
    for &(category, level) in &settings.filter_overrides {
        filters.set(category, level);
    }
    let ring = settings
        .ring
        .map(|settings| Arc::new(Mutex::new(Ring::new(settings))));
    let (tx, rx) = sync_channel(settings.channel_capacity);
    let handle = Arc::new(Handle {
        tx,
        filters,
        dropped: AtomicU64::new(0),
        seq: AtomicU32::new(0),
        ring: ring.clone(),
    });
    let join = thread::Builder::new()
        .name("logreel-writer".to_owned())
        .spawn(move || writer_loop(rx, settings, render, sink, ring))
        .map_err(|error| InitError::Spawn(error.to_string()))?;
    Ok(Logger {
        handle,
        join: Some(join),
    })
}

#[derive(Default)]
struct WriterResult {
    records: u64,
    batches: u64,
    bytes: u64,
    sink_errors: u64,
    text_errors: u64,
    error: Option<String>,
}

struct Writer<E: Event> {
    settings: Settings,
    render: Option<fn(&E, &mut String)>,
    sink: Box<dyn BatchSink>,
    encoder: BatchEncoder,
    batch: Vec<Record<E>>,
    text: Option<TextSink>,
    ring: Option<Arc<Mutex<Ring>>>,
    line: String,
    result: WriterResult,
}

impl<E: Event> Writer<E> {
    fn flush(&mut self) {
        if self.batch.is_empty() {
            return;
        }
        let mut has_warn = false;
        for record in &self.batch {
            if record.level >= Level::Warn {
                has_warn = true;
            }
            let text_wants = self
                .text
                .as_ref()
                .is_some_and(|text| text.wants(record.level));
            let ring_wants = self.ring.is_some();
            if !text_wants && !ring_wants {
                continue;
            }
            if let Some(render) = self.render {
                format_record(record, render, &mut self.line);
                if text_wants && let Some(text) = self.text.as_mut() {
                    text.push(&self.line);
                }
                if ring_wants && let Some(ring) = &self.ring {
                    ring.lock()
                        .expect("ring lock")
                        .push(record.level, &self.line);
                }
            }
        }
        if let Some(text) = self.text.as_mut()
            && let Err(error) = text.commit()
        {
            self.result.text_errors += 1;
            self.report(InternalErrorKind::Text, error.to_string());
        }

        let encoded = self.encoder.encode(&self.batch, &self.settings.domain);
        self.result.records += u64::from(encoded.record_count);
        self.result.batches += 1;
        self.result.bytes += (encoded.payload_len() + 8) as u64;

        if let Err(error) = self.sink.write_batch(&encoded) {
            self.result.sink_errors += 1;
            self.result.error = Some(error.to_string());
            self.report(InternalErrorKind::Sink, error.to_string());
        }

        let mode = match self.settings.durability {
            Durability::OnWarn if has_warn => Durability::Immediate,
            other => other,
        };
        if let Err(error) = self.sink.sync(mode) {
            self.result.sink_errors += 1;
            self.result.error = Some(error.to_string());
            self.report(InternalErrorKind::Sink, error.to_string());
        }
        self.batch.clear();
    }

    fn finish(&mut self) {
        if let Err(error) = self.sink.flush() {
            self.result.sink_errors += 1;
            self.result.error = Some(error.to_string());
            self.report(InternalErrorKind::Sink, error.to_string());
        }
        if let Err(error) = self.sink.sync(Durability::Immediate) {
            self.result.sink_errors += 1;
            self.result.error = Some(error.to_string());
            self.report(InternalErrorKind::Sink, error.to_string());
        }
    }

    fn report(&self, kind: InternalErrorKind, message: String) {
        if let Some(callback) = &self.settings.on_internal {
            callback(InternalError { kind, message });
        }
    }
}

fn writer_loop<E: Event>(
    rx: Receiver<Msg<E>>,
    settings: Settings,
    render: Option<fn(&E, &mut String)>,
    sink: Box<dyn BatchSink>,
    ring: Option<Arc<Mutex<Ring>>>,
) -> WriterResult {
    let text = settings.text.map(TextSink::new);
    let mut writer = Writer {
        batch: Vec::with_capacity(settings.batch_max),
        settings,
        render,
        sink,
        encoder: BatchEncoder::new(),
        text,
        ring,
        line: String::with_capacity(256),
        result: WriterResult::default(),
    };
    let _ = Instant::now();

    loop {
        match rx.recv_timeout(writer.settings.flush_interval) {
            Ok(Msg::Record(record)) => {
                let warn = record.level >= Level::Warn;
                writer.batch.push(record);
                if writer.batch.len() >= writer.settings.batch_max
                    || (writer.settings.flush_on_warn && warn)
                {
                    writer.flush();
                }
            }
            Ok(Msg::Flush) => writer.flush(),
            Ok(Msg::Shutdown) => {
                while let Ok(msg) = rx.try_recv() {
                    if let Msg::Record(record) = msg {
                        writer.batch.push(record);
                    }
                }
                writer.flush();
                writer.finish();
                break;
            }
            Err(RecvTimeoutError::Timeout) => writer.flush(),
            Err(RecvTimeoutError::Disconnected) => {
                writer.flush();
                writer.finish();
                break;
            }
        }
    }
    writer.result
}

impl From<WriterResult> for ShutdownReport {
    fn from(result: WriterResult) -> Self {
        Self {
            records: result.records,
            batches: result.batches,
            bytes: result.bytes,
            dropped: 0,
            sink_errors: result.sink_errors,
            text_errors: result.text_errors,
            ring_dropped: 0,
            error: result.error,
        }
    }
}
