//! `asar-reader` — list or extract an Electron `asar` archive, read-only.
//!
//! ```text
//! asar-reader list    <archive> [--only <substring>]
//! asar-reader extract <archive> <out-dir> [--only <substring>]
//! ```

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut positional: Vec<&str> = Vec::new();
    let mut filter = String::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--only" => match iter.next() {
                Some(value) => filter = value.clone(),
                None => return fail("--only needs a value"),
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => positional.push(other),
        }
    }

    match positional.as_slice() {
        ["list", archive] => match asar_reader::open(Path::new(archive)) {
            Ok((_, parsed)) => {
                let mut total = 0u64;
                for entry in asar_reader::entries(&parsed) {
                    if !filter.is_empty() && !entry.path.contains(&filter) {
                        continue;
                    }
                    total += entry.size;
                    let tag = if entry.unpacked {
                        " [unpacked]"
                    } else if entry.is_link {
                        " [link]"
                    } else {
                        ""
                    };
                    println!("{:>10}  {}{}", entry.size, entry.path, tag);
                }
                eprintln!("total listed bytes: {total}");
                ExitCode::SUCCESS
            }
            Err(err) => fail(&err.to_string()),
        },
        ["extract", archive, out_dir] => {
            match asar_reader::extract(Path::new(archive), Path::new(out_dir), &filter) {
                Ok(count) => {
                    eprintln!("extracted {count} files to {out_dir}");
                    ExitCode::SUCCESS
                }
                Err(err) => fail(&err.to_string()),
            }
        }
        _ => fail(USAGE),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::FAILURE
}

const USAGE: &str = "usage:\n  asar-reader list    <archive> [--only <substring>]\n  asar-reader extract <archive> <out-dir> [--only <substring>]";
