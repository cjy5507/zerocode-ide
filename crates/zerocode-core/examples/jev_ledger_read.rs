//! Compare Jev ledger reader time and peak RSS on the same synthetic history.
//! Run each mode as a separate release process under the platform's RSS meter.

use std::hint::black_box;
use std::io::{self, BufWriter, Write};
use std::time::Instant;

use serde_json::{Value, json};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "stream".to_string());
    if !matches!(mode.as_str(), "stream" | "whole") {
        return Err("expected stream or whole, then row count and payload bytes".into());
    }
    let count: usize = args.next().map_or(Ok(20_000), |arg| arg.parse())?;
    let width: usize = args.next().map_or(Ok(1_024), |arg| arg.parse())?;
    if args.next().is_some()
        || count > 1_000_000
        || width > 1_048_576
        || count
            .checked_mul(width.saturating_add(160))
            .is_none_or(|bytes| bytes > 256 * 1024 * 1024)
    {
        return Err("invalid benchmark size or extra argument".into());
    }
    let source = tempfile::NamedTempFile::new()?;
    {
        let mut out = BufWriter::new(source.as_file());
        let padding = "x".repeat(width);
        for at in 0..count {
            serde_json::to_writer(
                &mut out,
                &json!({
                    "at": at, "outcome": "answered", "model": "jev-bench",
                    "rubricVersion": 1, "chosen": "keep", "confidence": 0.8,
                    "payload": &padding
                }),
            )?;
            out.write_all(b"\n")?;
        }
        out.flush()?;
    }
    let began = Instant::now();
    let rows: Vec<Value> = if mode == "stream" {
        zerocode_core::jev::journal::read(source.path())?
    } else {
        let text = std::fs::read_to_string(source.path())?;
        text.lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    };
    let elapsed = began.elapsed();
    if rows.len() != count {
        return Err(io::Error::other("the reader lost a valid row").into());
    }
    let checksum: usize = rows
        .iter()
        .filter_map(|row| row["payload"].as_str())
        .map(str::len)
        .sum();
    if checksum != count * width {
        return Err(io::Error::other("the reader changed a payload").into());
    }
    println!(
        "{}",
        json!({"mode": mode, "rows": rows.len(), "payloadBytes": black_box(checksum),
        "elapsedMicros": elapsed.as_micros(), "fileBytes": source.as_file().metadata()?.len()})
    );
    black_box(rows);
    Ok(())
}
