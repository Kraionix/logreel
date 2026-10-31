//! Proves that the emit path performs no allocation after warm-up.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use logreel_core::{DecodeError, Event, Level, Settings, init};
use logreel_store::SharedSink;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

struct Counting;

fn counting() -> bool {
    COUNTING.try_with(Cell::get).unwrap_or(false)
}

fn record(size: usize) {
    ALLOCS.fetch_add(1, Ordering::Relaxed);
    BYTES.fetch_add(size, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if counting() {
            record(layout.size());
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if counting() {
            record(new_size);
        }
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ping(u64);

impl Event for Ping {
    const KINDS: u16 = 1;

    fn kind(&self) -> u16 {
        0
    }

    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.0.to_le_bytes());
    }

    fn decode(_kind: u16, bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut array = [0u8; 8];
        array.copy_from_slice(bytes);
        Ok(Self(u64::from_le_bytes(array)))
    }
}

#[test]
fn emit_path_does_not_allocate_after_warm_up() {
    let logger = init::<Ping>(
        Settings::builder()
            .min_level(Level::Trace)
            .channel_capacity(8192)
            .batch_max(4096)
            .flush_interval(Duration::from_secs(60))
            .flush_on_warn(false)
            .build(),
        None,
        Box::new(SharedSink::new()),
    )
    .expect("init");
    let handle = Arc::clone(logger.handle());

    for value in 0..64 {
        assert!(handle.emit(Level::Info, 0, value, Ping(value)));
    }
    ALLOCS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);

    COUNTING.with(|flag| flag.set(true));
    for value in 0..1024 {
        assert!(handle.emit(Level::Info, 0, value, Ping(value)));
    }
    COUNTING.with(|flag| flag.set(false));

    let allocations = ALLOCS.load(Ordering::Relaxed);
    let bytes = BYTES.load(Ordering::Relaxed);
    assert_eq!(
        allocations, 0,
        "emit path allocated {allocations} times ({bytes} bytes)"
    );
    let _ = logger.shutdown();
}

#[test]
fn disabled_emit_does_not_allocate() {
    let logger = init::<Ping>(
        Settings::builder()
            .min_level(Level::Error)
            .channel_capacity(8)
            .build(),
        None,
        Box::new(SharedSink::new()),
    )
    .expect("init");
    let handle = Arc::clone(logger.handle());
    assert!(!handle.emit(Level::Info, 0, 0, Ping(1)));
    ALLOCS.store(0, Ordering::Relaxed);
    COUNTING.with(|flag| flag.set(true));
    for value in 0..1024 {
        assert!(!handle.emit(Level::Info, 0, 0, Ping(value)));
    }
    COUNTING.with(|flag| flag.set(false));
    assert_eq!(ALLOCS.load(Ordering::Relaxed), 0);
    let _ = logger.shutdown();
}
