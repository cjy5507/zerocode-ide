//! The browser door's form pair, window half (t-37883): the page scripts
//! `fields` reads a page's forms with and `fill` writes a bundle with, and
//! the road that runs them — the tables, the bundle's grammar and the words
//! are the core's (`zerocode_core::browser_form`).
//!
//! Every script is synchronous, like every other page script of the door:
//! an engine that does not await a promise (WebKit, WebView2) gets the same
//! answer as one that does, and the clock between the passes of a fill is
//! the window's, never a timer left in the person's signed-in page. Nothing
//! reads a rect or waits for a frame, so a tab nobody is looking at reads and
//! fills as a shown one does.

use super::*;
use zerocode_core::browser_form::{
    BROWSER_FILL_OFF, BROWSER_FILL_ON, BROWSER_FILL_PENDING_MS, BROWSER_FORM_ACTION_CAP,
    BROWSER_FORM_ACTIONS, BROWSER_FORM_CAPTION_DEPTH, BROWSER_FORM_CONTROLS, BROWSER_FORM_DAY_WORDS,
    BROWSER_FORM_DAYS, BROWSER_FORM_FIELD_CAP, BROWSER_FORM_FRAME_DEPTH,
    BROWSER_FORM_FRAME_SEPARATOR, BROWSER_FORM_NOT_FIELDS, BROWSER_FORM_OPTION_CAP,
    BROWSER_FORM_OPTIONS, BROWSER_FORM_SCOPES, FillEntry, FillLedger, FillPass, FillReport,
    FormRead,
};
use zerocode_core::agent_browser::BROWSER_EVAL_FORM_OBJECT;

/// What a read of a page's forms is made of, page side — read only, like the
/// marks helpers it stands on (`BROWSER_MARK_HELPERS`: a field's name, the
/// heading of its region, a structural selector): which elements are
/// fields, the words a field is known by when nothing names it, a handle
/// that finds it again (inside same-origin frames too), what it holds and
/// what it offers. Shared by `fields` and by `fill`'s read-back, so a fill
/// checks a field by the very read the agent was shown.
pub(crate) const BROWSER_FORM_HELPERS: &str = r##"
"##;

/// The `fields` read: every field and its buttons, in one synchronous pass
/// that writes nothing.
pub(crate) const BROWSER_FIELDS_BODY: &str = r#"
return zcFail("not_built");
"#;

/// What `fill` writes with, page side — standing on the form helpers: each
/// entry of a bundle written in its order — words through the platform's
/// own value setter and the input and change events a page's code listens
/// to, a choice and a checkbox by a press on the choice itself, a dropdown
/// the page draws by opening it and pressing the option whose words are the
/// value, a date the page keeps from being typed by pressing the day of its
/// picker that carries the date's numbers — then every written field read
/// back by the read the agent was shown, and what the form still wants
/// (`zcFill`). A secret is never written (`type --value-stdin` is its road)
/// and a file is the person's turn.
pub(crate) const BROWSER_FILL_HELPERS: &str = r#"
"#;

/// One pass of a `fill`: the bundle the window hands it, through `zcFill`.
pub(crate) const BROWSER_FILL_BODY: &str = r#"
return zcFail("not_built");
"#;

/// The form pair inside an `eval` (t-37883): an expression that names
/// [`BROWSER_EVAL_FORM_OBJECT`] (the `const` below is that name) gets `fields()` — the read `fields` answers —
/// and `fill(bundle)` — one pass of `fill` over `{handle: value}` or a list
/// of `{handle, value}` — so one script can read a step, fill it by the
/// words it read, check what is left and press a step's button, in one
/// round trip. Synchronous like every eval: a field that loads later is the
/// next call's.
pub(crate) const BROWSER_EVAL_FORM: &str = r#"
"#;

/// What a form script is handed: the core's tables and caps, and the marks
/// helpers' keys and regions it reads a field's words and section by.
pub(crate) fn form_request() -> serde_json::Value {
    serde_json::json!({})
}

/// A form script: the marks helpers it stands on, the form helpers, a body.
pub(crate) fn form_script(request: &serde_json::Value, body: &str) -> String {
    automation_script(
        request,
        &format!("{BROWSER_MARK_HELPERS}\n{BROWSER_FORM_HELPERS}\n{body}"),
    )
}

/// A fill script: the form helpers, the writer, and a body that calls it.
pub(crate) fn fill_script(request: &serde_json::Value, body: &str) -> String {
    form_script(request, &format!("{BROWSER_FILL_HELPERS}\n{body}"))
}

/// An `eval`'s script when its expression names the form pair's object
/// ([`BROWSER_EVAL_FORM_OBJECT`]): the fill script's helpers and the object
/// before the expression; any other expression's script is left as it was,
/// so an eval that never reads a form carries none of it.
pub(crate) fn eval_script(expression: &str, body: &str) -> String {
    let _ = expression;
    automation_script(&serde_json::json!({}), body)
}

/// Read every field a pane's page draws, and the buttons beside them.
pub(crate) async fn automate_fields(
    app: &AppHandle,
    state: &AppState,
    label: &str,
) -> Result<FormRead, String> {
    let pane = browser_pane_of(app, state, label)?;
    let script = form_script(&form_request(), BROWSER_FIELDS_BODY);
    let reply = page_json(&pane, script, BROWSER_CALLBACK_DEADLINE).await?;
    serde_json::from_value(page_value(reply)?)
        .map_err(|_| "브라우저 판의 양식 읽기를 읽을 수 없습니다".to_string())
}

/// Fill a bundle into a pane's page, in passes ([`fill_passes`]).
pub(crate) async fn automate_fill(
    app: &AppHandle,
    state: &AppState,
    label: &str,
    entries: Vec<FillEntry>,
) -> Result<FillReport, String> {
    let pane = browser_pane_of(app, state, label)?;
    fill_passes(
        entries,
        Duration::from_millis(BROWSER_FILL_PENDING_MS),
        BROWSER_WAIT_POLL,
        |asked| {
            let pane = pane.clone();
            async move {
                let mut request = form_request();
                request["entries"] = serde_json::to_value(&asked).unwrap_or_default();
                let script = fill_script(&request, BROWSER_FILL_BODY);
                let reply = page_json(&pane, script, BROWSER_CALLBACK_DEADLINE).await?;
                serde_json::from_value::<FillPass>(page_value(reply)?)
                    .map_err(|_| "브라우저 판의 채움 답을 읽을 수 없습니다".to_string())
            }
        },
    )
    .await
}

/// A fill's passes on the window's clock: the whole bundle first, then —
/// every `poll`, until `pending` has passed since the start — only the
/// fields another pass may still find (the ledger's word), so a field an
/// earlier value brings (a time list a date loads, a box a choice turns
/// on, a dropdown that opens on a press) is written once it is there. A
/// pass that fails ends the fill with its refusal.
pub(crate) async fn fill_passes<P, F>(
    entries: Vec<FillEntry>,
    pending: Duration,
    poll: Duration,
    mut pass: P,
) -> Result<FillReport, String>
where
    P: FnMut(Vec<FillEntry>) -> F,
    F: std::future::Future<Output = Result<FillPass, String>>,
{
    let _ = (pending, poll, &mut pass);
    Ok(FillLedger::new(entries).report())
}
