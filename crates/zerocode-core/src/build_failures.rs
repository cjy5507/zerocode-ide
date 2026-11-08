//! Why a failed step failed: the failures its own output names and the failed
//! test cases of the JUnit XML reports it left. The readers are data
//! (`build_failures.toml`), each named by the `tool` of the builds it reads, so
//! neither the runner nor the window knows a tool; a step of a tool with no
//! reader says only that it exited.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

use regex::Regex;
use serde::Deserialize;

use crate::builds::Failure;

/// The most failures one step keeps: the first are the ones to read.
pub const FAILURES_MOST: usize = 20;
/// The most of one failure's words kept.
const WHAT_MOST: usize = 240;
/// The most of a step's output read for its failures: its end.
pub const OUTPUT_MOST: u64 = 2 * 1024 * 1024;
/// A report larger than this is not read.
const REPORT_BYTES_MOST: u64 = 4 * 1024 * 1024;
/// The most report files read for one step.
const REPORTS_MOST: usize = 200;
/// How deep `**` walks under the folder the step ran in.
const DEPTH_MOST: usize = 8;
/// The names a pattern gives what it reads.
const WHAT: &str = "what";
const FILE: &str = "file";
const LINE: &str = "line";

#[derive(Debug)]
struct Row {
    tools: Vec<String>,
    lines: Vec<Regex>,
    at: Vec<Regex>,
    ignore: Vec<Regex>,
    reports: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    tools: Vec<String>,
    lines: Vec<String>,
    #[serde(default)]
    at: Vec<String>,
    #[serde(default)]
    ignore: Vec<String>,
    #[serde(default)]
    reports: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Table {
    reader: Vec<Written>,
}

fn table() -> Vec<Written> {
    toml::from_str::<Table>(include_str!("build_failures.toml"))
        .map(|table| table.reader)
        .unwrap_or_default()
}

fn patterns(written: &[String]) -> Vec<Regex> {
    written
        .iter()
        .filter_map(|one| Regex::new(one).ok())
        .collect()
}

static READERS: LazyLock<Vec<Row>> = LazyLock::new(|| {
    table()
        .into_iter()
        .map(|written| Row {
            lines: patterns(&written.lines),
            at: patterns(&written.at),
            ignore: patterns(&written.ignore),
            tools: written.tools,
            reports: written.reports,
        })
        .collect()
});

/// The failures a step of a build of `tool` names: in `output` (its own
/// output, colour already taken out or not), and in the JUnit XML reports
/// under `folder` (the folder the step ran in) written at or after `since`.
/// A file the step names relative to `folder` that stands there is given as an
/// absolute path, so the window can open it; one that does not stand keeps its
/// words. At most [`FAILURES_MOST`], in the order they were named.
#[must_use]
pub fn read(tool: &str, output: &str, folder: &Path, since: SystemTime) -> Vec<Failure> {
    let Some(row) = READERS
        .iter()
        .find(|row| row.tools.iter().any(|named| named == tool))
    else {
        return Vec::new();
    };
    let mut found = Found::default();
    for line in output.lines() {
        let line = crate::build_progress::plain(line.trim_end_matches('\r'));
        if row.ignore.iter().any(|pattern| pattern.is_match(&line)) {
            continue;
        }
        if let Some(caught) = row.lines.iter().find_map(|pattern| pattern.captures(&line)) {
            let what = caught
                .name(WHAT)
                .or_else(|| caught.name(FILE))
                .map_or("", |what| what.as_str());
            found.add(
                what,
                caught.name(FILE).map(|file| file.as_str()),
                line_of(&caught),
            );
        } else if let Some(caught) = row.at.iter().find_map(|pattern| pattern.captures(&line)) {
            found.place(
                caught.name(FILE).map(|file| file.as_str()),
                line_of(&caught),
            );
        }
    }
    for report in reports_under(folder, &row.reports, since) {
        for case in failed_cases(&report) {
            found.add(&case.what, case.file.as_deref(), case.line);
        }
    }
    found
        .failures
        .into_iter()
        .map(|failure| Failure {
            file: failure.file.map(|file| placed(folder, &file)),
            ..failure
        })
        .collect()
}

fn line_of(caught: &regex::Captures<'_>) -> Option<u32> {
    caught
        .name(LINE)
        .and_then(|line| line.as_str().parse().ok())
}

/// A file named relative to `folder` that stands there, as an absolute path;
/// otherwise the words the step said.
fn placed(folder: &Path, file: &str) -> String {
    let path = Path::new(file);
    if path.is_absolute() {
        return file.to_owned();
    }
    let joined = folder.join(path);
    if joined.is_file() {
        joined.display().to_string()
    } else {
        file.to_owned()
    }
}

#[derive(Default)]
struct Found {
    failures: Vec<Failure>,
    seen: HashSet<String>,
}

impl Found {
    fn add(&mut self, what: &str, file: Option<&str>, line: Option<u32>) {
        let what = what.trim();
        if what.is_empty() {
            return;
        }
        let what: String = what.chars().take(WHAT_MOST).collect();
        if let Some(held) = self.failures.iter_mut().find(|held| held.what == what) {
            if held.file.is_none() {
                held.file = file.map(str::to_owned);
                held.line = line;
            }
            return;
        }
        if self.failures.len() >= FAILURES_MOST || !self.seen.insert(what.clone()) {
            return;
        }
        self.failures.push(Failure {
            what,
            file: file.map(str::to_owned),
            line,
        });
    }

    fn place(&mut self, file: Option<&str>, line: Option<u32>) {
        if let Some(held) = self.failures.last_mut()
            && held.file.is_none()
        {
            held.file = file.map(str::to_owned);
            held.line = line;
        }
    }
}

/// The report files under `folder` that the written globs name, written at or
/// after `since` — at most [`REPORTS_MOST`].
fn reports_under(folder: &Path, globs: &[String], since: SystemTime) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for glob in globs {
        let parts: Vec<&str> = glob.split('/').filter(|part| !part.is_empty()).collect();
        walk(folder, &parts, 0, &mut found);
    }
    found.sort();
    found.dedup();
    found
        .into_iter()
        .filter(|path| {
            std::fs::metadata(path).is_ok_and(|meta| {
                meta.len() <= REPORT_BYTES_MOST && meta.modified().is_ok_and(|at| at >= since)
            })
        })
        .take(REPORTS_MOST)
        .collect()
}

fn walk(at: &Path, parts: &[&str], depth: usize, found: &mut Vec<PathBuf>) {
    if found.len() >= REPORTS_MOST * 4 || depth > DEPTH_MOST {
        return;
    }
    let Some((part, rest)) = parts.split_first() else {
        return;
    };
    if *part == "**" {
        walk(at, rest, depth, found);
        for child in dirs_of(at) {
            walk(&child, parts, depth + 1, found);
        }
        return;
    }
    let Ok(entries) = std::fs::read_dir(at) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !matches_part(part, name) {
            continue;
        }
        let path = entry.path();
        let kind = entry.file_type();
        if rest.is_empty() {
            if kind.is_ok_and(|kind| kind.is_file()) {
                found.push(path);
            }
        } else if kind.is_ok_and(|kind| kind.is_dir()) {
            walk(&path, rest, depth + 1, found);
        }
    }
}

fn dirs_of(at: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(at)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default()
}

/// One part of a glob against one name: `*` stands for any run of letters.
fn matches_part(part: &str, name: &str) -> bool {
    let pieces: Vec<&str> = part.split('*').collect();
    if pieces.len() == 1 {
        return part == name;
    }
    let (first, last) = (pieces[0], pieces[pieces.len() - 1]);
    if !name.starts_with(first) || !name.ends_with(last) || name.len() < first.len() + last.len() {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    for piece in &pieces[1..pieces.len() - 1] {
        match rest.find(piece) {
            Some(at) => rest = &rest[at + piece.len()..],
            None => return false,
        }
    }
    true
}

static CASE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<testcase\b([^>]*?)(/>|>(.*?)</testcase>)").expect("the test case pattern")
});
static ATTRIBUTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\b(name|classname|file|line)\s*=\s*"([^"]*)""#).expect("the attribute pattern")
});
static FAILED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<(failure|error)\b").expect("the failure pattern"));

/// The failed test cases of one JUnit XML report: a case holding a
/// `<failure>` or an `<error>`, named `classname.name`, placed by its `file`
/// and `line` attributes when the report gives them.
fn failed_cases(report: &Path) -> Vec<Failure> {
    let Ok(text) = std::fs::read_to_string(report) else {
        return Vec::new();
    };
    CASE.captures_iter(&text)
        .filter(|case| {
            case.get(3)
                .is_some_and(|body| FAILED.is_match(body.as_str()))
        })
        .filter_map(|case| {
            let mut name = None;
            let mut class = None;
            let mut file = None;
            let mut line = None;
            for attribute in ATTRIBUTE.captures_iter(&case[1]) {
                let value = unescape(&attribute[2]);
                match &attribute[1] {
                    "name" => name = Some(value),
                    "classname" => class = Some(value),
                    "file" => file = Some(value),
                    _ => line = value.parse().ok(),
                }
            }
            let name = name.filter(|name| !name.is_empty())?;
            Some(Failure {
                what: class
                    .filter(|class| !class.is_empty())
                    .map_or_else(|| name.clone(), |class| format!("{class}.{name}")),
                file,
                line,
            })
        })
        .collect()
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Every tool a reader names.
#[cfg(test)]
fn reader_tools() -> Vec<&'static str> {
    READERS
        .iter()
        .flat_map(|row| row.tools.iter().map(String::as_str))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_now(tool: &str, output: &str) -> Vec<Failure> {
        read(
            tool,
            output,
            Path::new("/nonexistent-folder"),
            SystemTime::now(),
        )
    }

    fn said(failures: &[Failure]) -> Vec<String> {
        failures
            .iter()
            .map(|failure| match (&failure.file, failure.line) {
                (Some(file), Some(line)) => format!("{} @ {file}:{line}", failure.what),
                (Some(file), None) => format!("{} @ {file}", failure.what),
                _ => failure.what.clone(),
            })
            .collect()
    }

    #[test]
    fn every_pattern_of_the_table_reads_and_every_reader_a_tool_the_presets_name() {
        let presets = crate::build_presets::tools();
        for (written, row) in table().iter().zip(READERS.iter()) {
            assert_eq!(
                written.lines.len(),
                row.lines.len(),
                "a line of {:?} is not a pattern",
                written.tools
            );
            assert_eq!(
                written.at.len(),
                row.at.len(),
                "an `at` of {:?} is not a pattern",
                written.tools
            );
            assert_eq!(
                written.ignore.len(),
                row.ignore.len(),
                "an `ignore` of {:?} is not a pattern",
                written.tools
            );
        }
        for tool in reader_tools() {
            assert!(presets.contains(&tool), "{tool} is no preset's tool");
        }
        assert_eq!(table().len(), READERS.len());
    }

    #[test]
    fn a_compiler_names_its_error_and_then_where_it_is() {
        let cargo = "   Compiling shop v0.1.0\nerror[E0425]: cannot find value `total` in this scope\n  --> src/order.rs:42:13\n   |\n\
             error: could not compile `shop` (lib) due to 1 previous error\n";
        assert_eq!(
            said(&read_now("cargo", cargo)),
            ["cannot find value `total` in this scope @ src/order.rs:42"]
        );
        let javac = "> Task :app:compileJava\n/repo/app/src/main/java/shop/Order.java:12: error: ';' expected\n";
        assert_eq!(
            said(&read_now("gradle", javac)),
            ["';' expected @ /repo/app/src/main/java/shop/Order.java:12"]
        );
        let tsc =
            "src/cart.ts(7,3): error TS2322: Type 'string' is not assignable to type 'number'.\n";
        assert_eq!(
            said(&read_now("pnpm", tsc)),
            ["TS2322: Type 'string' is not assignable to type 'number'. @ src/cart.ts:7"]
        );
    }

    #[test]
    fn a_test_runner_names_the_test_and_where_it_failed() {
        let go = "=== RUN   TestRefund\n--- FAIL: TestRefund (0.00s)\n    order_test.go:31: refunded twice\nFAIL\nFAIL\tshop/order\t0.01s\n";
        assert_eq!(said(&read_now("go", go)), ["TestRefund @ order_test.go:31"]);
        let pytest = "=========================== short test summary info ============================\n\
             FAILED tests/test_order.py::test_refund - AssertionError: assert 2 == 1\n";
        assert_eq!(
            said(&read_now("python", pytest)),
            ["test_refund @ tests/test_order.py"]
        );
        let rust = "test order::refund ... FAILED\n\nthread 'order::refund' panicked at src/order.rs:88:9:\n";
        assert_eq!(
            said(&read_now("cargo", rust)),
            ["order::refund @ src/order.rs:88"]
        );
        let vitest = " × src/cart.test.ts > adds a line 12ms\n";
        assert_eq!(
            said(&read_now("npm", vitest)),
            ["adds a line @ src/cart.test.ts"]
        );
    }

    #[test]
    fn a_tool_with_no_reader_names_nothing_and_a_long_list_keeps_its_first() {
        assert!(read_now("unheard-of", "error: everything").is_empty());
        let many: String = (0..50)
            .map(|n| format!("--- FAIL: Test{n} (0.00s)\n"))
            .collect();
        let failures = read_now("go", &many);
        assert_eq!(failures.len(), FAILURES_MOST);
        assert_eq!(failures[0].what, "Test0");
    }

    #[test]
    fn a_junit_report_written_since_the_step_began_names_each_failed_case() {
        let folder = tempfile::tempdir().expect("a folder");
        let reports = folder.path().join("build/test-results/test");
        std::fs::create_dir_all(&reports).expect("the reports folder");
        let before = SystemTime::now() - std::time::Duration::from_secs(5);
        std::fs::write(
            reports.join("TEST-shop.OrderTest.xml"),
            r#"<testsuite name="shop.OrderTest" tests="3" failures="1">
  <testcase name="adds" classname="shop.OrderTest" time="0.01"/>
  <testcase name="refunds &amp; voids" classname="shop.OrderTest" file="src/test/java/shop/OrderTest.java" line="42">
    <failure message="expected 1 but was 2">AssertionError</failure>
  </testcase>
  <testcase name="ships" classname="shop.OrderTest"></testcase>
</testsuite>"#,
        )
        .expect("a report");
        std::fs::create_dir_all(folder.path().join("src/test/java/shop"))
            .expect("the source folder");
        std::fs::write(
            folder.path().join("src/test/java/shop/OrderTest.java"),
            "class OrderTest {}",
        )
        .expect("the test");
        let failures = read("gradle", "", folder.path(), before);
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert_eq!(failures[0].what, "shop.OrderTest.refunds & voids");
        assert_eq!(failures[0].line, Some(42));
        assert_eq!(
            failures[0].file.as_deref(),
            Some(
                folder
                    .path()
                    .join("src/test/java/shop/OrderTest.java")
                    .display()
                    .to_string()
                    .as_str()
            ),
            "a file that stands in the folder is given whole, so the window can open it"
        );
        let later = SystemTime::now() + std::time::Duration::from_secs(60);
        assert!(
            read("gradle", "", folder.path(), later).is_empty(),
            "a report older than the step is another run's"
        );
    }

    #[test]
    fn a_glob_part_matches_its_stars_and_nothing_else() {
        assert!(matches_part("*.xml", "TEST-a.xml"));
        assert!(matches_part("TEST-*.xml", "TEST-a.xml"));
        assert!(!matches_part("TEST-*.xml", "a.xml"));
        assert!(matches_part("junit.xml", "junit.xml"));
        assert!(!matches_part("junit.xml", "junit.xml.bak"));
        assert!(matches_part("*", "anything"));
    }
}
