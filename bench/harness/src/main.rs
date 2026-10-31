//! Measurement harness for logreel.
//!
//! Every number is an absolute measurement on this machine, median of five
//! unless noted. The first run freezes bench/BUDGET.md; later runs compare
//! against it with a ten percent tolerance.
//!
//! Usage: cargo run -p logreel-bench --release -- [--write] [--freeze] [--check]

use std::hint::black_box;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use logreel::file::{FileStore, FileStoreConfig, SegmentReader, probe, segments};
use logreel::{
    BatchEncoder, BatchSink, Durability, EncodedBatch, Level, Record, Settings, SharedSink, Store,
    StoreError, format_utc, init, now_ns, scope,
};
use logreel_example_events::{DomainEvent, SessionClosed};

const EMIT_GATED_ITERATIONS: u64 = 5_000_000;
const EMIT_ENABLED_ITERATIONS: u64 = 200_000;
const BATCH_RECORDS: usize = 4096;
const BATCH_REPEATS: u64 = 200;
const FILE_RECORDS: u64 = 100_000;
const FILE_BATCH: usize = 256;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let write = args.iter().any(|arg| arg == "--write" || arg == "--freeze");
    let freeze = args.iter().any(|arg| arg == "--freeze");
    let check = args.iter().any(|arg| arg == "--check");

    let results = measure_all();
    let table = render_table(&results);
    println!("{table}");

    if check {
        match check_budget(&results) {
            Ok(()) => println!("budget: within tolerance"),
            Err(problems) => {
                eprintln!("budget: regressions detected\n{problems}");
                std::process::exit(1);
            }
        }
    }
    if write {
        let path = bench_dir().join("RESULTS.md");
        std::fs::write(&path, table).expect("write RESULTS.md");
        println!("wrote {}", path.display());
    }
    if freeze {
        let path = bench_dir().join("BUDGET.md");
        if path.exists() && !args.iter().any(|arg| arg == "--force") {
            eprintln!("BUDGET.md exists; pass --force to overwrite");
            std::process::exit(1);
        }
        std::fs::write(&path, render_budget(&results)).expect("write BUDGET.md");
        println!("wrote {}", path.display());
    }
}

fn bench_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("bench dir")
        .to_path_buf()
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(|left, right| left.partial_cmp(right).expect("finite"));
    values[values.len() / 2]
}

fn measure(mut run: impl FnMut() -> f64, samples: usize) -> f64 {
    let mut values = Vec::with_capacity(samples);
    for _ in 0..samples {
        values.push(run());
    }
    median(values)
}

fn event(index: u64) -> DomainEvent {
    DomainEvent::SessionClosed(SessionClosed {
        conn_id: index % 1000,
        bytes_out: index,
    })
}

fn measure_all() -> Vec<Metric> {
    let mut metrics = Vec::new();
    metrics.push(measure_emit_gated());
    metrics.push(measure_emit_enabled());
    metrics.push(measure_batch_encode());
    let (write_metric, bytes_metric, scan_metric, probe_metric) = measure_file();
    metrics.push(write_metric);
    metrics.push(bytes_metric);
    metrics.push(scan_metric);
    metrics.push(probe_metric);
    let (core, core_file, all) = measure_binary_sizes();
    metrics.push(Metric::new(
        "binary_core_kb",
        core,
        "KiB",
        "quickstart, core only",
    ));
    metrics.push(Metric::new(
        "binary_core_file_kb",
        core_file,
        "KiB",
        "quickstart, core + file",
    ));
    metrics.push(Metric::new(
        "binary_all_kb",
        all,
        "KiB",
        "quickstart, full features",
    ));
    metrics.push(measure_rss());
    metrics
}

fn measure_emit_gated() -> Metric {
    let logger = init::<DomainEvent>(
        Settings::builder()
            .min_level(Level::Error)
            .channel_capacity(1024)
            .build(),
        Box::new(SharedSink::new()),
    )
    .expect("init");
    let handle = Arc::clone(logger.handle());
    let template = event(7);
    let ns = measure(
        || {
            let start = Instant::now();
            for index in 0..EMIT_GATED_ITERATIONS {
                let _ = black_box(handle.emit(Level::Info, 0, index % 7, template.clone()));
            }
            start.elapsed().as_nanos() as f64 / EMIT_GATED_ITERATIONS as f64
        },
        5,
    );
    let _ = logger.shutdown();
    Metric::new(
        "emit_gated_ns",
        ns,
        "ns/record",
        "filtered out before the payload",
    )
}

struct CountingSink {
    records: Arc<AtomicU64>,
}

impl BatchSink for CountingSink {
    fn write_batch(&mut self, batch: &EncodedBatch) -> Result<(), StoreError> {
        self.records
            .fetch_add(u64::from(batch.record_count), Ordering::Relaxed);
        Ok(())
    }
}

fn measure_emit_enabled() -> Metric {
    let records = Arc::new(AtomicU64::new(0));
    let logger = init::<DomainEvent>(
        Settings::builder()
            .min_level(Level::Trace)
            .channel_capacity(1 << 20)
            .batch_max(4096)
            .flush_interval(Duration::from_secs(60))
            .build(),
        Box::new(CountingSink {
            records: Arc::clone(&records),
        }),
    )
    .expect("init");
    let handle = Arc::clone(logger.handle());
    let ns = measure(
        || {
            let start = Instant::now();
            for index in 0..EMIT_ENABLED_ITERATIONS {
                assert!(handle.emit(Level::Info, 0, index % 1000, event(index)));
            }
            start.elapsed().as_nanos() as f64 / EMIT_ENABLED_ITERATIONS as f64
        },
        5,
    );
    let report = logger.shutdown();
    let _ = (records.load(Ordering::Relaxed), report.dropped);
    Metric::new(
        "emit_enabled_ns",
        ns,
        "ns/record",
        "filter pass, timestamp, seq, channel send",
    )
}

fn measure_batch_encode() -> Metric {
    let records: Vec<Record<DomainEvent>> = (0..BATCH_RECORDS)
        .map(|index| {
            Record::new(
                Level::Info,
                0,
                1,
                1_700_000_000_000_000_000 + index as u64,
                index as u32,
                index as u64 % 1000,
                event(index as u64),
            )
        })
        .collect();
    let mut encoder = BatchEncoder::new();
    let ns = measure(
        || {
            let start = Instant::now();
            for _ in 0..BATCH_REPEATS {
                let batch = encoder.encode(&records, b"dc=1");
                black_box(&batch);
            }
            start.elapsed().as_nanos() as f64 / (BATCH_REPEATS * BATCH_RECORDS as u64) as f64
        },
        5,
    );
    Metric::new(
        "batch_encode_ns",
        ns,
        "ns/record",
        "envelope plus prost payload",
    )
}

fn temp_dir(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("logreel-bench-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("temp dir");
    path
}

fn measure_file() -> (Metric, Metric, Metric, Metric) {
    let directory = temp_dir("file");
    let mut store = FileStore::open(FileStoreConfig {
        directory: directory.clone(),
        producer_id: 1,
        ..FileStoreConfig::new(&directory)
    })
    .expect("open store");
    let records: Vec<Record<DomainEvent>> = (0..FILE_BATCH)
        .map(|index| {
            Record::new(
                Level::Info,
                0,
                1,
                1_700_000_000_000_000_000 + index as u64,
                index as u32,
                index as u64,
                event(index as u64),
            )
        })
        .collect();
    let mut encoder = BatchEncoder::new();
    let mut batches = Vec::new();
    while batches.len() < FILE_RECORDS as usize / FILE_BATCH {
        batches.push(encoder.encode(&records, b"dc=1"));
    }

    let start = Instant::now();
    for batch in &batches {
        store.append(batch).expect("append");
    }
    let elapsed = start.elapsed().as_secs_f64();
    Store::sync(&mut store, Durability::Immediate).expect("sync");
    drop(store);
    let file = segments(&directory).expect("segments").remove(0);
    let bytes = std::fs::metadata(&file).expect("metadata").len();
    let write_mb = bytes as f64 / elapsed / (1024.0 * 1024.0);
    let bytes_per_record = bytes as f64 / (batches.len() * FILE_BATCH) as f64;
    let write_metric = Metric::new(
        "file_write_mb_s",
        write_mb,
        "MiB/s",
        "append only, no fsync",
    );
    let bytes_metric = Metric::new(
        "file_bytes_per_record",
        bytes_per_record,
        "B/record",
        "header and frame amortised",
    );

    let scan_mb = measure(
        || {
            let start = Instant::now();
            let mut reader = SegmentReader::open(&file).expect("reader");
            let mut count = 0u64;
            while let Some(batch) = reader.next_batch() {
                let batch = batch.expect("batch");
                count += u64::from(batch.record_count);
            }
            assert!(count > 0);
            let seconds = start.elapsed().as_secs_f64();
            bytes as f64 / seconds / (1024.0 * 1024.0)
        },
        5,
    );
    let scan_metric = Metric::new("scan_mb_s", scan_mb, "MiB/s", "batch validation, no decode");

    let probe_us = measure(
        || {
            let start = Instant::now();
            let value = probe(&file).expect("probe");
            assert!(value.records > 0);
            start.elapsed().as_nanos() as f64 / 1000.0
        },
        5,
    );
    let probe_metric = Metric::new(
        "probe_us",
        probe_us,
        "us",
        "header, frames, hash, histogram",
    );
    let _ = std::fs::remove_dir_all(&directory);
    (write_metric, bytes_metric, scan_metric, probe_metric)
}

fn build_quickstart(features: &str) -> Option<f64> {
    let harness = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let mut command = std::process::Command::new(harness);
    command
        .current_dir(workspace_root())
        .args([
            "build",
            "-p",
            "logreel-quickstart",
            "--release",
            "--no-default-features",
        ])
        .args(if features.is_empty() {
            Vec::new()
        } else {
            vec!["--features", features]
        })
        .env("CARGO_NET_OFFLINE", "true")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let status = command.status().ok()?;
    if !status.success() {
        return None;
    }
    let path = workspace_root()
        .join("target")
        .join("release")
        .join("logreel-quickstart");
    std::fs::metadata(path)
        .ok()
        .map(|meta| meta.len() as f64 / 1024.0)
}

fn measure_binary_sizes() -> (f64, f64, f64) {
    let core = build_quickstart("").unwrap_or(f64::NAN);
    let core_file = build_quickstart("file").unwrap_or(f64::NAN);
    let all = build_quickstart("full").unwrap_or(f64::NAN);
    (core, core_file, all)
}

fn rss_kb() -> f64 {
    let mut text = String::new();
    if std::fs::File::open("/proc/self/statm")
        .and_then(|mut file| file.read_to_string(&mut text))
        .is_err()
    {
        return f64::NAN;
    }
    let pages: f64 = text
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(f64::NAN);
    pages * 4096.0 / 1024.0
}

fn measure_rss() -> Metric {
    let logger = init::<DomainEvent>(
        Settings::builder()
            .min_level(Level::Trace)
            .batch_max(4096)
            .build(),
        Box::new(SharedSink::new()),
    )
    .expect("init");
    let handle = Arc::clone(logger.handle());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_time()
        .build()
        .expect("runtime");
    let baseline = rss_kb();
    let after = runtime.block_on(async move {
        let mut tasks = Vec::new();
        for conn in 0..1000u64 {
            let handle = Arc::clone(&handle);
            tasks.push(tokio::spawn(scope(conn, handle, async move {
                std::future::pending::<()>().await;
            })));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        rss_kb()
    });
    let _ = logger.shutdown();
    Metric::new(
        "rss_1000_scopes_kb",
        after - baseline,
        "KiB",
        "1000 live task-local scopes (informational)",
    )
    .informational()
}

struct Metric {
    name: &'static str,
    value: f64,
    unit: &'static str,
    note: &'static str,
    budgeted: bool,
}

impl Metric {
    fn new(name: &'static str, value: f64, unit: &'static str, note: &'static str) -> Self {
        Self {
            name,
            value,
            unit,
            note,
            budgeted: true,
        }
    }

    fn informational(mut self) -> Self {
        self.budgeted = false;
        self
    }

    fn format(&self) -> String {
        if self.value.is_nan() {
            "UNVERIFIED".to_owned()
        } else if self.value >= 1000.0 {
            format!("{:.0}", self.value)
        } else if self.value >= 10.0 {
            format!("{:.1}", self.value)
        } else {
            format!("{:.3}", self.value)
        }
    }
}

fn cpu_model() -> String {
    let Ok(text) = std::fs::read_to_string("/proc/cpuinfo") else {
        return "unknown".to_owned();
    };
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("model name") {
            return value.trim_start_matches([':', ' ', '\t']).to_owned();
        }
    }
    "unknown".to_owned()
}

fn rustc_version() -> String {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map_or_else(|| "unknown".to_owned(), |text| text.trim().to_owned())
}

fn render_table(results: &[Metric]) -> String {
    let mut out = String::new();
    out.push_str("# logreel measurements\n\n");
    out.push_str("Generated by cargo run -p logreel-bench --release -- --write.\n\n");
    let mut date = String::new();
    format_utc(now_ns(), &mut date);
    out.push_str(&format!("- date: {date}\n"));
    out.push_str(&format!("- cpu: {}\n", cpu_model()));
    out.push_str(&format!("- toolchain: {}\n", rustc_version()));
    out.push_str("- profile: release, median of 5, same machine\n");
    out.push_str("- feature set: file, zstd-capable reader\n\n");
    out.push_str("| metric | value | unit | note |\n|---|---:|---|---|\n");
    for metric in results {
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            metric.name,
            metric.format(),
            metric.unit,
            metric.note
        ));
    }
    out.push('\n');
    out.push_str("A number that is not reproduced is UNVERIFIED, never guessed.\n");
    out
}

fn render_budget(results: &[Metric]) -> String {
    let mut out = String::new();
    out.push_str("# Frozen budget\n\n");
    out.push_str("First measurement frozen on 2026-10-31 with a 10 percent tolerance.\n");
    out.push_str("The check runs on the same machine and compares medians of 5.\n\n");
    out.push_str("tolerance = 0.10\n\n");
    for metric in results.iter().filter(|metric| metric.budgeted) {
        out.push_str(&format!("{} = {}\n", metric.name, metric.format()));
    }
    out
}

fn check_budget(results: &[Metric]) -> Result<(), String> {
    let path = bench_dir().join("BUDGET.md");
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut tolerance = 0.10;
    let mut failures = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().parse::<f64>();
        if key == "tolerance" {
            if let Ok(value) = value {
                tolerance = value;
            }
            continue;
        }
        let Ok(frozen) = value else { continue };
        let Some(metric) = results.iter().find(|metric| metric.name == key) else {
            continue;
        };
        if metric.value.is_nan() {
            failures.push_str(&format!("{key}: UNVERIFIED\n"));
            continue;
        }
        let ratio = metric.value / frozen;
        if (ratio - 1.0).abs() > tolerance {
            failures.push_str(&format!(
                "{key}: {} vs frozen {} (ratio {:.2})\n",
                metric.format(),
                frozen,
                ratio
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures)
    }
}
