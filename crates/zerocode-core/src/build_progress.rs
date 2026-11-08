//! How far a running step is, read from its own output: the build card's
//! in-step progress. The readers are data (`build_progress.toml`), each named
//! by the `tool` of the builds it reads, so neither the runner nor the window
//! knows a tool; a step of a tool with no reader says no more than its steps.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;

use crate::builds::Progress;

/// The most of one line looked at; the rest of a longer line is not.
const LINE_MOST: usize = 4 * 1024;
/// The names a pattern gives the numbers a line says itself: how many begun of
/// how many, or how far in hundredths (`[ 61%]`).
const DONE: &str = "done";
const OF: &str = "of";
const PERCENT: &str = "percent";
/// The whole a percent is of.
const HUNDRED: u32 = 100;
/// The escape that begins a colour, and the bracket of its sequence.
const ESCAPE: char = '\u{1b}';
const SEQUENCE: char = '[';

#[derive(Debug)]
struct Row {
    tools: Vec<String>,
    lines: Vec<Regex>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    tools: Vec<String>,
    lines: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Table {
    reader: Vec<Written>,
}

fn table() -> Vec<Written> {
    toml::from_str::<Table>(include_str!("build_progress.toml"))
        .map(|table| table.reader)
        .unwrap_or_default()
}

static READERS: LazyLock<Vec<Row>> = LazyLock::new(|| {
    table()
        .into_iter()
        .map(|written| Row {
            tools: written.tools,
            lines: written
                .lines
                .iter()
                .filter_map(|line| Regex::new(line).ok())
                .collect(),
        })
        .collect()
});

/// Reads one running step's output as it grows, into how far the step is.
#[derive(Debug)]
pub struct Reader {
    lines: &'static [Regex],
    progress: Progress,
    /// The output read so far that is not yet a whole line.
    partial: Vec<u8>,
}

impl Reader {
    /// The reader of a step of a build of `tool`, whose whole is `usual` units
    /// while its lines do not say; none when the tool has no reader.
    #[must_use]
    pub fn of_tool(tool: &str, usual: Option<u32>) -> Option<Self> {
        READERS
            .iter()
            .find(|row| row.tools.iter().any(|named| named == tool))
            .map(|row| Self {
                lines: &row.lines,
                progress: Progress {
                    done: 0,
                    of: usual.filter(|of| *of > 0),
                },
                partial: Vec::new(),
            })
    }

    /// Reads `bytes`, the next of the step's output: whether it moved on.
    pub fn read(&mut self, bytes: &[u8]) -> bool {
        let mut moved = false;
        let mut pieces = bytes.split(|byte| *byte == b'\n').peekable();
        while let Some(piece) = pieces.next() {
            let room = LINE_MOST.saturating_sub(self.partial.len());
            self.partial
                .extend_from_slice(&piece[..piece.len().min(room)]);
            if pieces.peek().is_some() {
                let line = std::mem::take(&mut self.partial);
                moved |= self.line(&line);
                self.partial = line;
                self.partial.clear();
            }
        }
        moved
    }

    fn line(&mut self, line: &[u8]) -> bool {
        let line = String::from_utf8_lossy(line);
        let line = plain(line.trim_end_matches('\r'));
        let Some(found) = self
            .lines
            .iter()
            .find_map(|pattern| pattern.captures(&line))
        else {
            return false;
        };
        // A number may be grouped by commas (Bazel's `[1,234 / 5,678]`).
        let said = |name| {
            found
                .name(name)
                .and_then(|number| number.as_str().replace(',', "").parse::<u32>().ok())
        };
        self.progress = match (said(DONE), said(OF), said(PERCENT)) {
            (Some(done), Some(of), _) => Progress { done, of: Some(of) },
            (_, _, Some(percent)) => Progress {
                done: percent.min(HUNDRED),
                of: Some(HUNDRED),
            },
            _ => Progress {
                done: self.progress.done.saturating_add(1),
                ..self.progress
            },
        };
        true
    }

    /// How far the step is, by what has been read.
    #[must_use]
    pub fn progress(&self) -> Progress {
        self.progress
    }
}

/// `line` without the escapes a tool colours it with — the failure reader's
/// too ([`crate::build_failures`]).
pub(crate) fn plain(line: &str) -> Cow<'_, str> {
    if !line.contains(ESCAPE) {
        return Cow::Borrowed(line);
    }
    let mut kept = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(char) = chars.next() {
        if char != ESCAPE {
            kept.push(char);
        } else if chars.clone().next() == Some(SEQUENCE) {
            // The sequence ends at its first letter.
            let _ = chars.find(char::is_ascii_alphabetic);
        }
    }
    Cow::Owned(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(tool: &str, usual: Option<u32>, output: &str) -> Progress {
        let mut reader = Reader::of_tool(tool, usual).expect("a reader");
        reader.read(output.as_bytes());
        reader.progress()
    }

    #[test]
    fn every_line_of_the_table_is_a_pattern_and_every_reader_a_tool_the_presets_name() {
        let presets = crate::build_presets::tools();
        for (written, row) in table().iter().zip(READERS.iter()) {
            assert_eq!(
                written.lines.len(),
                row.lines.len(),
                "a line of {:?} is not a pattern",
                written.tools
            );
            for tool in &row.tools {
                assert!(
                    presets.contains(&tool.as_str()),
                    "{tool} is no preset's tool"
                );
            }
        }
        assert_eq!(table().len(), READERS.len());
        assert!(READERS.len() >= 4, "the table lost its readers");
    }

    #[test]
    fn a_line_that_says_how_far_in_hundredths_or_in_grouped_numbers_says_it_itself() {
        let pytest =
            "tests/test_cart.py ....        [ 40%]\ntests/test_order.py ..F.       [ 61%]\n";
        assert_eq!(
            read("python", None, pytest),
            Progress {
                done: 61,
                of: Some(100)
            }
        );
        let cmake = "[ 12%] Building CXX object src/CMakeFiles/shop.dir/order.cpp.o\n[ 42%] Linking CXX executable shop\n";
        assert_eq!(
            read("cmake", None, cmake),
            Progress {
                done: 42,
                of: Some(100)
            }
        );
        let bazel = "[1,234 / 5,678] Compiling src/order.cc; 3s linux-sandbox\n";
        assert_eq!(
            read("bazel", None, bazel),
            Progress {
                done: 1234,
                of: Some(5678)
            }
        );
        let ninja = "[3/45] Compiling C object shop.p/order.c.o\n";
        assert_eq!(
            read("meson", None, ninja),
            Progress {
                done: 3,
                of: Some(45)
            }
        );
        let swift = "[12/30] Compiling Shop Order.swift\n";
        assert_eq!(
            read("swift", None, swift),
            Progress {
                done: 12,
                of: Some(30)
            }
        );
    }

    #[test]
    fn a_package_or_a_test_file_or_a_project_is_one_unit_begun() {
        let go =
            "ok  \tshop/cart\t0.012s\nFAIL\tshop/order\t0.020s\n?   \tshop/cmd\t[no test files]\n";
        assert_eq!(
            read("go", Some(10), go),
            Progress {
                done: 3,
                of: Some(10)
            }
        );
        let jest = "PASS src/cart.test.ts\nFAIL src/order.test.ts\n";
        assert_eq!(read("npm", None, jest).done, 2);
        let vitest = " \u{2713} src/cart.test.ts (3 tests) 12ms\n \u{276f} src/order.test.ts (2 tests | 1 failed) 20ms\n";
        assert_eq!(read("pnpm", None, vitest).done, 2);
        let dotnet = "  Shop -> /repo/src/Shop/bin/Debug/net8.0/Shop.dll\n  Shop.Tests -> /repo/tests/bin/Debug/net8.0/Shop.Tests.dll\n";
        assert_eq!(read("dotnet", None, dotnet).done, 2);
    }

    #[test]
    fn each_line_a_tool_begins_is_one_unit_of_the_whole_its_passing_runs_counted() {
        let gradle = "> Task :app:compileJava\nNote: some input files use unchecked operations.\n\
             > Task :app:processResources NO-SOURCE\n> Task :app:test\n";
        let maven = "[INFO] Scanning for projects...\n\
             [INFO] --- resources:3.3.1:resources (default-resources) @ app ---\n\
             [INFO] --- compiler:3.11.0:compile (default-compile) @ app ---\n";
        let cargo = "\u{1b}[1m\u{1b}[32m   Compiling\u{1b}[0m serde v1.0.210\n\
             \u{1b}[1m\u{1b}[32m   Compiling\u{1b}[0m shop v0.1.0 (/repo)\n    Finished `dev` profile\n";

        assert_eq!(
            read("gradle", Some(37), gradle),
            Progress {
                done: 3,
                of: Some(37)
            }
        );
        assert_eq!(read("maven", None, maven), Progress { done: 2, of: None });
        assert_eq!(
            read("cargo", Some(0), cargo),
            Progress { done: 2, of: None }
        );
    }

    #[test]
    fn a_line_that_says_how_far_it_is_is_read_as_it_says() {
        let buildkit = "#1 [internal] load build definition from Dockerfile\n\
             #5 [1/4] FROM docker.io/library/eclipse-temurin:21\n\
             #7 [build 2/4] RUN ./gradlew bootJar\n#7 CACHED\n";
        let legacy = "Step 1/6 : FROM eclipse-temurin:21\nStep 3/6 : RUN ./gradlew bootJar\n";

        assert_eq!(
            read("docker", Some(9), buildkit),
            Progress {
                done: 2,
                of: Some(4)
            }
        );
        assert_eq!(
            read("compose", None, legacy),
            Progress {
                done: 3,
                of: Some(6)
            }
        );
    }

    #[test]
    fn output_is_read_in_any_pieces_and_a_tool_with_no_reader_has_none() {
        let mut reader = Reader::of_tool("gradle", None).expect("a reader");
        let pieces = [
            "> Task :a\r\n> Ta",
            "sk :b",
            "\n",
            &"x".repeat(LINE_MOST * 2),
            "\n> Task :c",
        ];
        let moved: Vec<bool> = pieces
            .iter()
            .map(|piece| reader.read(piece.as_bytes()))
            .collect();

        assert_eq!(moved, [true, false, true, false, false]);
        assert_eq!(
            reader.progress().done,
            2,
            "a line not yet ended is not read"
        );
        assert!(reader.read(b"\n"));
        assert_eq!(reader.progress().done, 3);
        assert!(
            reader.partial.capacity() <= LINE_MOST * 2,
            "a long line is not kept whole"
        );
        for tool in ["just", "make", "sbt", "mix", ""] {
            assert!(Reader::of_tool(tool, Some(3)).is_none(), "{tool}");
        }
    }
}
