//! Repository tasks: ci, fixtures, bench.

mod codec;
mod fixtures;

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("fixtures") => match args.next().as_deref() {
            Some("gen") => fixtures::generate_to_disk(),
            Some("check") => fixtures::check(),
            _ => Err("usage: cargo xtask fixtures gen|check".to_owned()),
        },
        Some("ci") => ci(),
        Some("bench") => {
            let rest: Vec<String> = args.collect();
            bench(&rest)
        }
        Some("version") => {
            println!("logreel {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => Err("usage: cargo xtask <ci|fixtures gen|fixtures check|bench|version>".to_owned()),
    }
}

fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn step(program: &str, args: &[&str]) -> Result<(), String> {
    println!("xtask: {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(workspace_root())
        .status()
        .map_err(|error| format!("{program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} {}: {status}", args.join(" ")))
    }
}

fn ci() -> Result<(), String> {
    step("cargo", &["fmt", "--all", "--", "--check"])?;
    step(
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    step("cargo", &["test", "--workspace", "--all-features"])?;
    let status = Command::new("cargo")
        .args(["doc", "--workspace", "--no-deps", "--all-features"])
        .env("RUSTDOCFLAGS", "-D warnings")
        .current_dir(workspace_root())
        .status()
        .map_err(|error| format!("cargo doc: {error}"))?;
    if !status.success() {
        return Err(format!("cargo doc: {status}"));
    }
    fixtures::check()
}

fn bench(args: &[String]) -> Result<(), String> {
    println!(
        "xtask: cargo run --release -p logreel-bench -- {}",
        args.join(" ")
    );
    let status = Command::new("cargo")
        .args(["run", "--release", "-p", "logreel-bench", "--"])
        .args(args)
        .current_dir(workspace_root())
        .status()
        .map_err(|error| format!("cargo run: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("bench: {status}"))
    }
}
