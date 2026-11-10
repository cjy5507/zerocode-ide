//! `sheet-write` and `sheet-read` (t-102013): a table's cells without the
//! person's pointer, keys or front app, so one agent fills a spreadsheet
//! while the person and other agents keep the desk.
//!
//! Two roads. The file road writes and reads an `.xlsx` or `.csv` directly
//! (`xlsx` keeps every other part of a workbook), on every platform. The app
//! road writes an open Numbers or Excel document through the app's own
//! script on macOS: the app must already be running with the document open —
//! the script never starts it or brings it forward — and the front app is
//! sampled the whole time, so the answer says whether it stayed.
//!
//! The pure part — addresses, the cells, the path rule, the CSV text and the
//! script's arguments — is `zerocode_core::computer_sheet`.

mod xlsx;

use std::path::Path;
use std::time::Instant;

use serde_json::{Value, json};
use zerocode_core::computer_sheet::{
    CellRange, CsvTable, SheetFileKind, SheetRoots, cells_from_value, sheet_path, verification,
};
use zerocode_core::computer_use::{ComputerCommand, ComputerMethod};
use zerocode_core::computer_use_protocol::error_code;

use super::ComputerUseError;

/// The largest sheet file the file road opens.
pub(crate) const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

fn invalid(message: impl Into<String>) -> ComputerUseError {
    ComputerUseError::new(error_code::INVALID_ARGUMENT, message)
}

fn word<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
}

/// A sheet verb's answer: the file road when it names `--path`, the app road
/// when it names `--app`. `cwd` is where the asking shell stands, for a
/// relative path.
///
/// # Errors
/// What the road refused, said in words; a cell's value is never in them.
pub(crate) fn answer(
    command: &ComputerCommand,
    cwd: Option<&Path>,
) -> Result<Value, ComputerUseError> {
    let params = &command.params;
    let started = Instant::now();
    let writes = command.method == ComputerMethod::SheetWrite;
    let mut answer = match word(params, "path") {
        Some(path) => file_road(writes, params, path, cwd)?,
        None => app_road(writes, params)?,
    };
    answer["elapsedMs"] = json!(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
    Ok(answer)
}

fn roots() -> Result<SheetRoots, ComputerUseError> {
    Ok(SheetRoots {
        home: dirs::home_dir().ok_or_else(|| invalid("this account has no home folder"))?,
        temp: Some(std::env::temp_dir()),
    })
}

fn file_road(
    writes: bool,
    params: &Value,
    asked: &str,
    cwd: Option<&Path>,
) -> Result<Value, ComputerUseError> {
    let (path, kind) = sheet_path(asked, cwd, &roots()?).map_err(invalid)?;
    let sheet = word(params, "sheet");
    if kind == SheetFileKind::Csv && sheet.is_some() {
        return Err(invalid("a CSV file has no sheets; leave out --sheet"));
    }
    let existing = existing_bytes(&path)?;
    let format = match kind {
        SheetFileKind::Xlsx => "xlsx",
        SheetFileKind::Csv => "csv",
    };
    let mut answer =
        json!({ "road": "file", "path": path.display().to_string(), "format": format });
    if !writes {
        let bytes =
            existing.ok_or_else(|| invalid(format!("{} does not exist", path.display())))?;
        let range = word(params, "range")
            .map(CellRange::parse)
            .transpose()
            .map_err(invalid)?;
        let (cells, sheet_name, sheets, truncated) = read_file(kind, &bytes, sheet, range)?;
        answer["sheet"] = json!(sheet_name);
        if let Some(sheets) = sheets {
            answer["sheets"] = json!(sheets);
        }
        answer["count"] = json!(cells.len());
        answer["truncated"] = json!(truncated);
        answer["cells"] = Value::Array(cells.iter().map(|cell| cell.to_json()).collect());
        return Ok(answer);
    }

    let cells = cells_from_value(params.get("cells").unwrap_or(&Value::Null)).map_err(invalid)?;
    refuse_a_file_an_app_holds(&path)?;
    let created = existing.is_none();
    let (bytes, sheet_name) = match kind {
        SheetFileKind::Csv => {
            let mut table = match &existing {
                Some(bytes) => CsvTable::parse(
                    std::str::from_utf8(bytes)
                        .map_err(|_| invalid("the CSV file is not UTF-8 text"))?,
                )
                .map_err(invalid)?,
                // A byte-order mark: Excel reads a CSV without one as the
                // system's old code page, and Hangul arrives broken.
                None => CsvTable {
                    bom: true,
                    ..CsvTable::default()
                },
            };
            table.apply(&cells);
            (table.render().into_bytes(), None)
        }
        SheetFileKind::Xlsx => {
            let written = xlsx::write(existing.as_deref(), sheet, &cells).map_err(invalid)?;
            (written.bytes, Some(written.sheet))
        }
    };
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|error| invalid(format!("could not make {}: {error}", folder.display())))?;
    }
    let permissions = std::fs::metadata(&path).ok().map(|meta| meta.permissions());
    zerocode_core::atomic_file::write_atomically(&path, &bytes)
        .map_err(|error| invalid(format!("could not write {}: {error}", path.display())))?;
    if let Some(permissions) = permissions {
        let _ = std::fs::set_permissions(&path, permissions);
    }
    // What the file holds now, read again from the disk.
    let again = std::fs::read(&path)
        .map_err(|error| invalid(format!("could not read {} back: {error}", path.display())))?;
    let (read, _, _, _) = read_file(kind, &again, sheet_name.as_deref(), None)?;
    answer["sheet"] = json!(sheet_name);
    answer["created"] = json!(created);
    answer["written"] = json!(cells.len());
    merge(&mut answer, verification(&cells, &read));
    Ok(answer)
}

type FileCells = (
    Vec<zerocode_core::computer_sheet::ReadCell>,
    Option<String>,
    Option<Vec<String>>,
    bool,
);

fn read_file(
    kind: SheetFileKind,
    bytes: &[u8],
    sheet: Option<&str>,
    range: Option<CellRange>,
) -> Result<FileCells, ComputerUseError> {
    match kind {
        SheetFileKind::Csv => {
            let text = std::str::from_utf8(bytes)
                .map_err(|_| invalid("the CSV file is not UTF-8 text"))?;
            let mut cells = CsvTable::parse(text).map_err(invalid)?.cells(range);
            let truncated = cells.len() > zerocode_core::computer_sheet::SHEET_MAX_CELLS;
            cells.truncate(zerocode_core::computer_sheet::SHEET_MAX_CELLS);
            Ok((cells, None, None, truncated))
        }
        SheetFileKind::Xlsx => {
            let read = xlsx::read(bytes, sheet, range).map_err(invalid)?;
            Ok((
                read.cells,
                Some(read.sheet),
                Some(read.sheets),
                read.truncated,
            ))
        }
    }
}

fn existing_bytes(path: &Path) -> Result<Option<Vec<u8>>, ComputerUseError> {
    match std::fs::metadata(path) {
        Ok(meta) if !meta.is_file() => Err(invalid(format!("{} is not a file", path.display()))),
        Ok(meta) if meta.len() > MAX_FILE_BYTES => Err(invalid(format!(
            "{} is larger than {MAX_FILE_BYTES} bytes",
            path.display()
        ))),
        Ok(_) => std::fs::read(path)
            .map(Some)
            .map_err(|error| invalid(format!("could not read {}: {error}", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(invalid(format!(
            "could not read {}: {error}",
            path.display()
        ))),
    }
}

/// A file an app has open, or one the person made read-only, is not written
/// under them: the app's next save would put back what it holds, and a
/// read-only file is the person's word that it stays. LibreOffice marks an
/// open file with `.~lock.<name>#` beside it, Excel and Word with `~$<name>`.
fn refuse_a_file_an_app_holds(path: &Path) -> Result<(), ComputerUseError> {
    if std::fs::metadata(path).is_ok_and(|meta| meta.permissions().readonly()) {
        return Err(invalid(format!("{} is read-only", path.display())));
    }
    let (Some(folder), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str()))
    else {
        return Ok(());
    };
    for lock in [format!(".~lock.{name}#"), format!("~${name}")] {
        if folder.join(&lock).exists() {
            return Err(invalid(format!(
                "{} is open in an app ({lock} beside it); write it through --app, or close it there first",
                path.display()
            )));
        }
    }
    Ok(())
}

fn merge(answer: &mut Value, more: Value) {
    if let (Some(answer), Value::Object(more)) = (answer.as_object_mut(), more) {
        answer.extend(more);
    }
}

#[cfg(not(target_os = "macos"))]
fn app_road(_writes: bool, params: &Value) -> Result<Value, ComputerUseError> {
    Err(ComputerUseError::new(
        error_code::UNSUPPORTED_CAPABILITY,
        format!(
            "{} is written through its script only on macOS; here, write the file with --path",
            word(params, "app").unwrap_or("the app")
        ),
    ))
}

#[cfg(target_os = "macos")]
fn app_road(writes: bool, params: &Value) -> Result<Value, ComputerUseError> {
    use zerocode_core::computer_sheet::{
        CellAddress, SheetApp, SheetInApp, parse_script_cells, read_script_argv, write_script_argv,
    };
    let app = SheetApp::parse(word(params, "app").unwrap_or_default()).map_err(invalid)?;
    let target = SheetInApp {
        app,
        document: word(params, "document").unwrap_or_default().to_string(),
        sheet: word(params, "sheet").unwrap_or_default().to_string(),
        table: word(params, "table").unwrap_or_default().to_string(),
    };
    let watch = front::Watch::start(app.bundle_id());
    let mut answer = json!({
        "road": "app",
        "app": app.name(),
        "document": target.document,
    });
    let outcome = (|| {
        if writes {
            let cells =
                cells_from_value(params.get("cells").unwrap_or(&Value::Null)).map_err(invalid)?;
            script::run(
                &target,
                &write_script_argv(&target, &cells).map_err(invalid)?,
            )?;
            let corner = |pick: fn(u32, u32) -> u32| {
                cells.iter().map(|(at, _)| *at).reduce(|a, b| CellAddress {
                    row: pick(a.row, b.row),
                    column: pick(a.column, b.column),
                })
            };
            let block = corner(u32::min)
                .zip(corner(u32::max))
                .map(|(first, last)| CellRange { first, last });
            let read = parse_script_cells(&script::run(
                &target,
                &read_script_argv(&target, block).map_err(invalid)?,
            )?)
            .map_err(invalid)?;
            answer["written"] = json!(cells.len());
            merge(&mut answer, verification(&cells, &read));
        } else {
            let range = word(params, "range")
                .map(CellRange::parse)
                .transpose()
                .map_err(invalid)?;
            let read = parse_script_cells(&script::run(
                &target,
                &read_script_argv(&target, range).map_err(invalid)?,
            )?)
            .map_err(invalid)?;
            answer["count"] = json!(read.len());
            answer["cells"] = Value::Array(read.iter().map(|cell| cell.to_json()).collect());
        }
        Ok(())
    })();
    let front = watch.stop();
    outcome?;
    answer["foreground"] = front;
    Ok(answer)
}

/// The app's script, run by `osascript` with its words as arguments.
#[cfg(target_os = "macos")]
mod script {
    use std::io::Read as _;
    use std::time::{Duration, Instant};

    use zerocode_core::computer_sheet::{
        SCRIPT_NO_DOCUMENT, SCRIPT_NO_SHEET, SCRIPT_NO_SUCH_APP, SCRIPT_NO_TABLE,
        SCRIPT_NOT_RUNNING, SheetInApp,
    };
    use zerocode_core::computer_use_protocol::error_code;

    use super::ComputerUseError;

    const OSASCRIPT: &str = "/usr/bin/osascript";
    /// How long one script may take: a modal sheet in the app would hold it
    /// forever.
    const BUDGET: Duration = Duration::from_secs(30);
    /// How often a running script reads the operator's stop.
    const POLL: Duration = Duration::from_millis(20);
    /// AppleScript's "not allowed to send Apple events" (TCC Automation).
    const NOT_ALLOWED: i64 = -1743;

    /// The script's answer on its standard output.
    pub(super) fn run(target: &SheetInApp, argv: &[String]) -> Result<String, ComputerUseError> {
        let mut child = crate::proc::quiet_command(OSASCRIPT)
            .args(argv)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| {
                ComputerUseError::new(
                    error_code::ACTION_NOT_SUPPORTED,
                    format!("osascript did not start: {error}"),
                )
            })?;
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let out = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(pipe) = stdout.as_mut() {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        });
        let err = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(pipe) = stderr.as_mut() {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        });
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().ok().flatten() {
                break Some(status);
            }
            let stopped = crate::computer_use::guard::stopped_reason();
            if started.elapsed() >= BUDGET || stopped.is_some() {
                let _ = child.kill();
                let _ = child.wait();
                let _ = (out.join(), err.join());
                return Err(match stopped {
                    Some(reason) => crate::computer_use::guard::refusal(&reason),
                    None => ComputerUseError::new(
                        error_code::ACTION_TIMEOUT,
                        format!(
                            "{} did not answer within {} s — is it asking the person something?",
                            target.app.name(),
                            BUDGET.as_secs()
                        ),
                    ),
                });
            }
            std::thread::sleep(POLL);
        };
        let (out, err) = (
            out.join().unwrap_or_default(),
            err.join().unwrap_or_default(),
        );
        if status.is_some_and(|status| status.success()) {
            return Ok(out);
        }
        Err(refusal(target, &err))
    }

    /// The script's error number: the last `(N)` on its error line.
    pub(super) fn error_number(stderr: &str) -> Option<i64> {
        let open = stderr.rfind('(')?;
        let close = open + stderr[open..].find(')')?;
        stderr[open + 1..close].trim().parse().ok()
    }

    /// What went wrong, by the script's number; the words of an unknown
    /// error lose what they quote, since a value written may be in them.
    pub(super) fn refusal(target: &SheetInApp, stderr: &str) -> ComputerUseError {
        let app = target.app.name();
        let document = &target.document;
        match error_number(stderr) {
            Some(SCRIPT_NOT_RUNNING) => ComputerUseError::new(
                error_code::APP_NOT_FOUND,
                format!(
                    "{app} is not running; open the document in it first — a sheet verb never starts an app or brings it forward"
                ),
            ),
            Some(SCRIPT_NO_SUCH_APP) => ComputerUseError::new(
                error_code::APP_NOT_FOUND,
                format!("{app} is not installed on this Mac; write the file with --path"),
            ),
            Some(SCRIPT_NO_DOCUMENT) => ComputerUseError::new(
                error_code::WINDOW_NOT_FOUND,
                format!(
                    "{app} has no open document named `{document}`; name it as its window title shows it"
                ),
            ),
            Some(SCRIPT_NO_SHEET) => ComputerUseError::new(
                error_code::INVALID_ARGUMENT,
                format!("`{document}` has no sheet `{}`", target.sheet),
            ),
            Some(SCRIPT_NO_TABLE) => ComputerUseError::new(
                error_code::INVALID_ARGUMENT,
                format!(
                    "that sheet of `{document}` has no table{}",
                    if target.table.is_empty() {
                        String::new()
                    } else {
                        format!(" `{}`", target.table)
                    }
                ),
            ),
            Some(NOT_ALLOWED) => ComputerUseError::new(
                error_code::PERMISSION_DENIED,
                format!(
                    "ZeroCode may not control {app}: allow it in System Settings › Privacy & Security › Automation"
                ),
            ),
            number => ComputerUseError::new(
                error_code::ACTION_NOT_SUPPORTED,
                format!(
                    "{app}'s script refused{}: {}",
                    number.map(|n| format!(" ({n})")).unwrap_or_default(),
                    unquoted(stderr.trim())
                ),
            ),
        }
    }

    /// The words with every quoted stretch replaced by `"…"`.
    pub(super) fn unquoted(words: &str) -> String {
        let mut out = String::new();
        let mut quoted: Option<char> = None;
        for c in words.chars() {
            match (quoted, c) {
                (None, '"') => {
                    quoted = Some('"');
                    out.push_str("\"…");
                }
                (None, '“') => {
                    quoted = Some('”');
                    out.push_str("\"…");
                }
                (Some(close), c) if c == close => {
                    quoted = None;
                    out.push('"');
                }
                (Some(_), _) => {}
                (None, c) => out.push(c),
            }
        }
        out
    }
}

/// The front app, sampled while a script runs: the proof the person's desk
/// was not taken.
#[cfg(target_os = "macos")]
mod front {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use serde_json::{Value, json};

    const LSAPPINFO: &str = "/usr/bin/lsappinfo";
    const EVERY: Duration = Duration::from_millis(20);

    /// The front app's session number (`ASN:…`), no name: which app it is
    /// is not this answer's business, only whether it changed.
    fn front() -> Option<String> {
        let out = crate::proc::quiet_command(LSAPPINFO)
            .arg("front")
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| session_number(&String::from_utf8_lossy(&out.stdout)))
            .flatten()
    }

    /// `ASN:0x0-0x11b8fb7e:` and `ASN:0x0-0x11b8fb7e-"Name":` alike as
    /// `0x0-0x11b8fb7e`: `front` and `find` spell one app two ways.
    pub(super) fn session_number(said: &str) -> Option<String> {
        let rest = said.trim().strip_prefix("ASN:")?;
        let mut parts = rest.split(['-', ':']);
        let (high, low) = (parts.next()?, parts.next()?);
        (!high.is_empty() && !low.is_empty()).then(|| format!("{high}-{low}"))
    }

    /// The running app's session number, by its bundle.
    fn session_of(bundle: &str) -> Option<String> {
        let out = crate::proc::quiet_command(LSAPPINFO)
            .args(["find", &format!("bundleid={bundle}")])
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| session_number(&String::from_utf8_lossy(&out.stdout)))
            .flatten()
    }

    pub(super) struct Watch {
        before: Option<String>,
        app: Option<String>,
        stop: Arc<AtomicBool>,
        samples: std::thread::JoinHandle<Vec<Option<String>>>,
    }

    impl Watch {
        pub(super) fn start(bundle: &str) -> Self {
            let before = front();
            let app = session_of(bundle);
            let stop = Arc::new(AtomicBool::new(false));
            let stopping = Arc::clone(&stop);
            let samples = std::thread::spawn(move || {
                let mut seen = Vec::new();
                while !stopping.load(Ordering::Relaxed) {
                    seen.push(front());
                    std::thread::sleep(EVERY);
                }
                seen
            });
            Self {
                before,
                app,
                stop,
                samples,
            }
        }

        /// `{unchanged, samples, changedSamples, appCameForward}`: whether
        /// every sample, and the front app after, is the one before; and,
        /// when it was not, whether the sheet's own app was ever in front —
        /// the script's doing — or only another app the person moved to.
        pub(super) fn stop(self) -> Value {
            self.stop.store(true, Ordering::Relaxed);
            let mut seen = self.samples.join().unwrap_or_default();
            seen.push(front());
            let changed = seen.iter().filter(|sample| **sample != self.before).count();
            let came_forward =
                self.app.is_some() && self.app != self.before && seen.contains(&self.app);
            json!({
                "unchanged": self.before.is_some() && changed == 0,
                "samples": seen.len(),
                "changedSamples": changed,
                "appCameForward": came_forward,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn a_script_error_reads_by_its_number_and_never_repeats_a_value() {
        use zerocode_core::computer_sheet::{SheetApp, SheetInApp};
        let target = SheetInApp {
            app: SheetApp::Numbers,
            document: "Budget".into(),
            sheet: "Q1".into(),
            table: String::new(),
        };
        let code = |stderr: &str| script::refusal(&target, stderr).code;
        assert_eq!(
            code("123:456: execution error: Numbers is not running (4001)"),
            error_code::APP_NOT_FOUND
        );
        assert_eq!(
            code("syntax error: can't get application id \"com.microsoft.Excel\". (-1728)"),
            error_code::APP_NOT_FOUND
        );
        assert_eq!(
            code("execution error: no open document (4002)"),
            error_code::WINDOW_NOT_FOUND
        );
        assert_eq!(
            code("execution error: no such sheet (4003)"),
            error_code::INVALID_ARGUMENT
        );
        assert_eq!(
            code("execution error: Not authorized to send Apple events to Numbers. (-1743)"),
            error_code::PERMISSION_DENIED
        );
        let unknown = script::refusal(
            &target,
            "execution error: Can’t make “secret-ish 1234” into type number. (-1700)",
        );
        assert_eq!(unknown.code, error_code::ACTION_NOT_SUPPORTED);
        assert!(!unknown.message.contains("1234"), "{}", unknown.message);
        assert!(unknown.message.contains("(-1700)"));
        assert_eq!(script::unquoted(r#"a "b" c "d"#), r#"a "…" c "…"#);
        assert_eq!(script::error_number("no number"), None);
        assert_eq!(
            front::session_number("ASN:0x0-0x11b8fb7e:\n").as_deref(),
            Some("0x0-0x11b8fb7e")
        );
        assert_eq!(
            front::session_number("ASN:0x0-0x11cc4cb3-\"Numbers\":").as_deref(),
            Some("0x0-0x11cc4cb3")
        );
        assert_eq!(front::session_number("nothing"), None);
    }

    fn command(method: ComputerMethod, argv: &[&str]) -> ComputerCommand {
        let mut words = vec![method.verb_name().to_string()];
        words.extend(argv.iter().map(|word| (*word).to_string()));
        zerocode_core::computer_use::parse_command(&words).expect("parses")
    }

    #[test]
    fn the_file_road_writes_reads_and_refuses_a_file_an_app_holds() {
        let home = dirs::home_dir().expect("home");
        let folder = tempfile::Builder::new()
            .prefix(".zerocode-sheet-test-")
            .tempdir_in(&home)
            .expect("a folder in home");
        let path = folder.path().join("new/t.csv");
        let written = answer(
            &command(
                ComputerMethod::SheetWrite,
                &[
                    "--path",
                    &path.to_string_lossy(),
                    "--cells",
                    r#"{"A1":"품목","B2":3,"C3":"=SUM(B2:B2)","D1":"-1"}"#,
                ],
            ),
            None,
        )
        .expect("written");
        assert_eq!(written["verified"], json!(true), "{written}");
        assert_eq!(written["created"], json!(true));
        assert_eq!(written["written"], json!(4));
        let text = std::fs::read_to_string(&path).expect("file");
        assert_eq!(text, "\u{feff}품목,,,'-1\n,3\n,,=SUM(B2:B2)\n");

        let read = answer(
            &command(
                ComputerMethod::SheetRead,
                &["--path", &path.to_string_lossy(), "--range", "A1:B2"],
            ),
            None,
        )
        .expect("read");
        assert_eq!(
            read["cells"],
            json!([{"cell":"A1","value":"품목"},{"cell":"B2","value":3.0}])
        );

        std::fs::write(folder.path().join("new/.~lock.t.csv#"), "").expect("lock");
        let held = answer(
            &command(
                ComputerMethod::SheetWrite,
                &["--path", &path.to_string_lossy(), "--cells", r#"{"A1":1}"#],
            ),
            None,
        )
        .expect_err("held");
        assert!(held.message.contains("open in an app"), "{}", held.message);

        let sheet_on_csv = answer(
            &command(
                ComputerMethod::SheetRead,
                &["--path", &path.to_string_lossy(), "--sheet", "x"],
            ),
            None,
        )
        .expect_err("csv has no sheets");
        assert!(sheet_on_csv.message.contains("no sheets"));

        let outside = answer(
            &command(
                ComputerMethod::SheetWrite,
                &[
                    "--path",
                    "/etc/zerocode-sheet.csv",
                    "--cells",
                    r#"{"A1":1}"#,
                ],
            ),
            None,
        );
        #[cfg(unix)]
        assert!(outside.expect_err("outside").message.contains("outside"));
        #[cfg(not(unix))]
        let _ = outside;
    }

    #[test]
    fn the_xlsx_road_makes_a_workbook_and_changes_one_cell_of_it() {
        let folder = tempfile::tempdir_in(std::env::temp_dir()).expect("temp");
        let path = folder.path().join("t.xlsx");
        let path = path.to_string_lossy();
        let first = answer(
            &command(
                ComputerMethod::SheetWrite,
                &[
                    "--path",
                    &path,
                    "--sheet",
                    "매출",
                    "--cells",
                    r#"{"A1":"품목","B1":2,"B2":"=B1*2"}"#,
                ],
            ),
            None,
        )
        .expect("made");
        assert_eq!(
            (first["verified"].clone(), first["sheet"].clone()),
            (json!(true), json!("매출")),
            "{first}"
        );
        let second = answer(
            &command(
                ComputerMethod::SheetWrite,
                &["--path", &path, "--cells", r#"{"B1":5}"#],
            ),
            None,
        )
        .expect("changed");
        assert_eq!(
            (second["verified"].clone(), second["created"].clone()),
            (json!(true), json!(false))
        );
        let read = answer(
            &command(ComputerMethod::SheetRead, &["--path", &path]),
            None,
        )
        .expect("read");
        assert_eq!(
            read["cells"],
            json!([{"cell":"A1","value":"품목"},{"cell":"B1","value":5.0},{"cell":"B2","value":null,"formula":"=B1*2"}])
        );
        assert_eq!(read["sheets"], json!(["매출"]));
    }

    /// The 11-10 measurement (t-102013), run by hand: the vault's 12-cell
    /// table into a new xlsx and through Numbers, one cell of a formatted
    /// workbook, five rounds each. `ZEROCODE_SHEET_MEASURE` names a folder
    /// holding `formatted.xlsx`; `ZEROCODE_SHEET_NUMBERS` names an open
    /// Numbers document the run may write. Each answer is one JSON line.
    #[test]
    #[ignore = "measures files and a live Numbers document; run by hand"]
    fn measure_the_three_roads() {
        let Ok(folder) = std::env::var("ZEROCODE_SHEET_MEASURE") else {
            return;
        };
        let table = r#"{"A1":"품목","B1":"수량","C1":"단가","A2":"사과","B2":3,"C2":1200,"A3":"배","B3":2,"C3":2500,"A4":"합계","B4":"=SUM(B2:B3)","C4":"=SUMPRODUCT(B2:B3,C2:C3)"}"#;
        let say = |label: &str, value: &Value| println!("MEASURE {label} {value}");
        for round in 1..=5 {
            let fresh = format!("{folder}/a-new-{round}.xlsx");
            let made = answer(
                &command(
                    ComputerMethod::SheetWrite,
                    &["--path", &fresh, "--sheet", "주문", "--cells", table],
                ),
                None,
            );
            say(
                "a-write",
                &made
                    .map_err(|e| e.message)
                    .unwrap_or_else(|e| json!({ "error": e })),
            );
            let read = answer(
                &command(ComputerMethod::SheetRead, &["--path", &fresh]),
                None,
            );
            say(
                "a-read",
                &read
                    .map_err(|e| e.message)
                    .unwrap_or_else(|e| json!({ "error": e })),
            );
            let copy = format!("{folder}/b-edited-{round}.xlsx");
            std::fs::copy(format!("{folder}/formatted.xlsx"), &copy).expect("copy");
            let one = answer(
                &command(
                    ComputerMethod::SheetWrite,
                    &["--path", &copy, "--cells", r#"{"B2":4}"#],
                ),
                None,
            );
            say(
                "b-write",
                &one.map_err(|e| e.message)
                    .unwrap_or_else(|e| json!({ "error": e })),
            );
        }
        let Ok(document) = std::env::var("ZEROCODE_SHEET_NUMBERS") else {
            return;
        };
        for round in 1..=5 {
            let wrote = answer(
                &command(
                    ComputerMethod::SheetWrite,
                    &[
                        "--app",
                        "Numbers",
                        "--document",
                        &document,
                        "--cells",
                        table,
                    ],
                ),
                None,
            );
            say(
                &format!("c-write-{round}"),
                &wrote
                    .map_err(|e| e.message)
                    .unwrap_or_else(|e| json!({ "error": e })),
            );
        }
        let read = answer(
            &command(
                ComputerMethod::SheetRead,
                &[
                    "--app",
                    "Numbers",
                    "--document",
                    &document,
                    "--range",
                    "A1:C4",
                ],
            ),
            None,
        );
        say(
            "c-read",
            &read
                .map_err(|e| e.message)
                .unwrap_or_else(|e| json!({ "error": e })),
        );
        let missing = answer(
            &command(
                ComputerMethod::SheetRead,
                &[
                    "--app",
                    "Numbers",
                    "--document",
                    "no such document t-102013",
                ],
            ),
            None,
        );
        say(
            "c-missing",
            &missing.unwrap_or_else(|e| json!({ "code": e.code, "message": e.message })),
        );
        let excel = answer(
            &command(
                ComputerMethod::SheetRead,
                &["--app", "Excel", "--document", "Book1"],
            ),
            None,
        );
        say(
            "c-excel",
            &excel.unwrap_or_else(|e| json!({ "code": e.code, "message": e.message })),
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn the_app_road_says_why_it_is_not_here() {
        let refused = answer(
            &command(
                ComputerMethod::SheetRead,
                &["--app", "Excel", "--document", "Book1"],
            ),
            None,
        )
        .expect_err("not here");
        assert_eq!(refused.code, error_code::UNSUPPORTED_CAPABILITY);
    }
}
