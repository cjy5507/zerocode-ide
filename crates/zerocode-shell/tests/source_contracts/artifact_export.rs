//! An artifact's 「내보내기」 also writes a PDF or a picture (t-18558), drawn by
//! the window's own WebKit. The browser suite (`ui/tests/artifact-gallery.mjs`,
//! `artifact-band`) drives the menu; the unit tests fake the renderer. These
//! contracts hold what neither sees from outside — that the window and the
//! backend speak the same three kinds and the same three commands, that nothing
//! waits for WebKit on the thread WebKit answers on, that the print job never
//! holds the person's window, and that every number the renderer decides by
//! stands in one table.

use zerocode_core::artifact_publish::ExportFormat;

use super::support::{WINDOW_PARTS, block_after, strip_comments, window_source};

const RENDER: &str = include_str!("../../src/artifact_render.rs");
const PLAN: &str = include_str!("../../src/artifact_render_plan.rs");
const WEBKIT: &str = include_str!("../../src/artifact_webkit.rs");
const COMMANDS: &str = include_str!("../../src/cmd/artifacts.rs");
const MAIN: &str = include_str!("../../src/main.rs");

/// A file's shipped half: everything before its first test-only item.
fn shipped(source: &str) -> &str {
    source.split("\n#[cfg(test)]").next().unwrap_or(source)
}

/// The window's menu offers the kinds the core writes, in the core's order, and
/// asks the backend by three commands the handler list carries — one to export,
/// one to ask which kinds this build can write, one to reveal what was written.
#[test]
fn the_window_and_the_backend_speak_the_same_kinds_and_the_same_commands() {
    let window = window_source();
    for kind in ExportFormat::ALL {
        assert!(
            window.contains(&format!(
                "{{ format: \"{}\", key: \"artifacts.strip.export",
                kind.word()
            )),
            "the export menu does not offer `{}`",
            kind.word()
        );
    }
    let kinds = window
        .split_once("const ARTIFACT_EXPORT_FORMATS = Object.freeze([")
        .and_then(|(_, rest)| rest.split_once("]);"))
        .expect("the export menu's table is gone")
        .0;
    let order: Vec<usize> = ExportFormat::ALL
        .iter()
        .map(|kind| {
            kinds
                .find(&format!("format: \"{}\"", kind.word()))
                .unwrap_or_else(|| panic!("`{}` left the menu's table", kind.word()))
        })
        .collect();
    assert!(
        order.windows(2).all(|pair| pair[0] < pair[1]),
        "the menu offers the kinds in another order than the core lists them"
    );
    for command in [
        "artifact_export",
        "artifact_export_formats",
        "artifact_export_reveal",
    ] {
        assert!(
            window.contains(&format!("invoke(\"{command}\"")),
            "the window never asks `{command}`"
        );
        assert!(
            MAIN.contains(&format!("            {command},\n")),
            "`{command}` is not in the handler list"
        );
    }
    let exporting = block_after(window, "async function exportArtifactVersion(");
    assert!(
        exporting
            .contains("invoke(\"artifact_export\", { id: facts.id, version, folder, format })"),
        "the chosen kind does not reach `artifact_export`:\n{exporting}"
    );
    assert!(
        exporting.matches("invoke(\"choose_project\"").count() == 1,
        "an export asks for its folder other than once:\n{exporting}"
    );
}

/// Nothing waits for WebKit on the thread WebKit answers on. The export command
/// is async, checks the caller first and hands the wait to a blocking thread; the
/// renderer's wait is the one `block_on`, and it is reached only from there and
/// from the door's own blocking thread.
#[test]
fn the_export_waits_for_webkit_on_a_blocking_thread_and_only_there() {
    let command = block_after(COMMANDS, "pub(crate) async fn artifact_export(");
    let gate = command
        .find("from_the_main_webview(&webview)?")
        .expect("a guest webview must not export");
    let hand_off = command
        .find("spawn_blocking(")
        .expect("the export waits for WebKit on the async worker");
    assert!(
        gate < hand_off,
        "the export hands off before it checks the caller"
    );
    assert!(
        command.contains("export_into("),
        "the command exports by another road than the store's"
    );
    for asking in [
        "pub(crate) fn artifact_export_formats(",
        "pub(crate) fn artifact_export_reveal(",
    ] {
        assert!(
            block_after(COMMANDS, asking).contains("from_the_main_webview(&webview)?"),
            "{asking} answers a guest webview"
        );
    }
    let render = shipped(RENDER);
    assert_eq!(
        render.matches("block_on(").count(),
        1,
        "the renderer waits in more than one place"
    );
    let waiting = block_after(render, "impl PageRenderer for WindowRenderer {");
    assert!(
        waiting.contains("tauri::async_runtime::block_on(render_page("),
        "the one wait is not the render itself"
    );
    // The reveal shows only what this window wrote.
    assert!(
        block_after(COMMANDS, "pub(crate) fn artifact_export_reveal(").contains("was_exported("),
        "the reveal road shows a path the window did not write"
    );
}

/// A print job holds its window for the life of the job. The renderer never
/// gives it the pane's window — in the app that is the person's own — and never
/// runs a print operation the synchronous way, which prints nothing for a WKWebView.
#[test]
fn the_print_job_never_holds_the_persons_window() {
    let render = shipped(RENDER);
    let webkit = shipped(WEBKIT);
    assert!(
        !render.contains("ns_window("),
        "the renderer reaches for the pane's window: the print job would hold it"
    );
    let print = block_after(webkit, "pub(crate) fn print_pdf(");
    assert!(
        print.contains("&print_window(mtm)"),
        "the export's print job is run for a window other than its own:\n{print}"
    );
    assert!(
        !webkit.contains(".runOperation()"),
        "a print operation is run synchronously; WebKit's job needs the run loop"
    );
    assert!(
        webkit.contains("setShowsPrintPanel(false)")
            && webkit.contains("setShowsProgressPanel(false)"),
        "the print job may show a panel over the person's window"
    );
    // A late job cleans up after itself.
    assert!(
        block_after(webkit, "fn deliver(").contains("remove_file("),
        "a print job that ends after its waiter left leaves its file behind"
    );
}

/// Every number the renderer decides by is in the table: paper, picture width and
/// cap, and every wait. No other file spells one, and the window reads the
/// picture's height from the answer instead of spelling the cap.
#[test]
fn the_renderer_takes_every_number_from_one_table() {
    for literal in ["595.276", "841.89", "1280.0", "8192.0"] {
        assert!(PLAN.contains(literal), "`{literal}` left the table");
        for (name, part) in [
            ("artifact_render.rs", shipped(RENDER)),
            ("artifact_webkit.rs", shipped(WEBKIT)),
            ("cmd/artifacts.rs", shipped(COMMANDS)),
        ] {
            assert!(
                !part.contains(literal),
                "`{literal}` is spelled in {name}, outside artifact_render_plan.rs"
            );
        }
    }
    // A wait is a named constant of the table, never a duration written where it is used.
    for (name, part) in [
        ("artifact_render.rs", shipped(RENDER)),
        ("artifact_webkit.rs", shipped(WEBKIT)),
    ] {
        for spelled in ["from_secs(", "from_millis("] {
            assert!(
                !part.contains(spelled),
                "{name} spells a wait with `{spelled}` instead of taking it from the table"
            );
        }
    }
    let window = strip_comments(window_source());
    let exporting = block_after(&window, "async function exportArtifactVersion(");
    for literal in ["8192", "1280", "16384"] {
        assert!(
            !exporting.contains(literal),
            "the export spells the picture's `{literal}` itself instead of reading it from the answer"
        );
    }
}

/// The words are `t()` in the four catalogs (the Korean source words stand beside
/// each `t()`; `no_label_reaches_the_window_hardcoded` holds those).
#[test]
fn the_export_words_stand_in_every_catalog() {
    let window = window_source();
    let code = strip_comments(window);
    for key in [
        "artifacts.strip.exportHtml",
        "artifacts.strip.exportPdf",
        "artifacts.strip.exportPng",
        "artifacts.strip.exportNoWebkit",
        "artifacts.strip.exporting",
        "artifacts.strip.exportedPdf",
        "artifacts.strip.exportedPng",
        "artifacts.strip.exportedPngCut",
    ] {
        for language in ["en", "ja", "zh", "es"] {
            let block = block_after(&code, &format!("  {language}: {{"));
            assert!(
                block.contains(&format!("\"{key}\":")),
                "`{key}` is missing from the `{language}` catalog"
            );
        }
        let doc = WINDOW_PARTS
            .iter()
            .find(|(name, _)| *name == "shell-doc.js")
            .map(|(_, text)| *text)
            .expect("shell-doc.js is a window part");
        assert!(
            doc.contains(&format!("\"{key}\"")),
            "`{key}` is in the catalogs but the artifact strip does not read it"
        );
    }
}
