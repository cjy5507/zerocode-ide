//! `session-corpus <root> [--sessions N] [--seed S]` writes `<root>/sessions/`
//! so `ZO_SESSION_ROOT=<root>` points a `zo` (or the measurement tests) at it.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use session_corpus::{corpus_bytes, generate, CorpusSpec};

const USAGE: &str = "usage: session-corpus <root> [--sessions N] [--seed S]";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next().map(PathBuf::from) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let mut spec = CorpusSpec::REAL_MIX;
    while let Some(flag) = args.next() {
        let value = args.next().and_then(|v| v.parse::<u64>().ok());
        match (flag.as_str(), value) {
            ("--sessions", Some(n)) => spec = spec.with_sessions(usize::try_from(n).unwrap_or(usize::MAX)),
            ("--seed", Some(s)) => spec = spec.with_seed(s),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let dir = root.join("sessions");
    let started = Instant::now();
    match generate(&dir, &spec).and_then(|manifest| Ok((manifest.len(), corpus_bytes(&manifest)?))) {
        Ok((count, bytes)) => {
            println!(
                "wrote {count} sessions ({} KB) under {} in {} ms",
                bytes / 1_024,
                dir.display(),
                started.elapsed().as_millis()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("session-corpus: {error}");
            ExitCode::FAILURE
        }
    }
}
