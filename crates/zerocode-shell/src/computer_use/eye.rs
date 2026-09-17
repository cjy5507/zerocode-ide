//! The continuous eye's client (docs/design/computer-use-full-operator.md
//! §7.1, V1). The window opens a stream of the display on the first desktop
//! look and answers desktop looks from its newest frame; the look after an
//! act (`observe --settle`), `watch`, and an OCR `wait-for` read its repaints
//! instead of taking pictures. What the repaints mean is the core's
//! (`computer_use_protocol::eye`). The helper answers one request at a time,
//! so every wait is the window's: it asks at the table's pace. A provider
//! without the eye (Windows today, a denied permission, a stream that failed
//! to open) is looked at the way it always was.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use zerocode_core::computer_use::{
    EYE_BACKGROUND_MS, EYE_IDLE_STOP_MS, EYE_POLL_MS, EYE_QUIET_MS, EYE_SETTLE_MAX_MS,
    desktop_wait_for_ms, eye_table,
};
use zerocode_core::computer_use_protocol::error_code;
use zerocode_core::computer_use_protocol::eye::{
    AFTER_KEY, CHANGES_METHOD, Change, Changes, Cursor, FRAME_METHOD, FROM_KEY, HISTORY_LOST,
    Ignore, Mark, NOT_WATCHING, NOW_KEY, OWN_REGION_KEY, SEQ_KEY, START_METHOD, Settle, Until,
    Watched, own_region,
};
use zerocode_core::computer_use_protocol::marks::DesktopWindow;
use zerocode_core::computer_use_protocol::render::Rect;

use super::ComputerUseError;

pub type Call<'a> = &'a mut dyn FnMut(&str, Value) -> Result<Value, ComputerUseError>;

/// What the window remembers about the eye between looks: when it last could
/// not be had. Until the table's idle time has passed, looks capture the
/// display as before instead of asking again — a provider without the eye
/// pays one refused ask per idle time, not one per look.
pub struct Memory {
    refused: Mutex<Option<Instant>>,
}

impl Memory {
    pub(super) const fn new() -> Self {
        Self {
            refused: Mutex::new(None),
        }
    }

    fn refused_lately(&self) -> bool {
        self.refused
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some_and(|at| at.elapsed() < Duration::from_millis(EYE_IDLE_STOP_MS))
    }

    fn note(&self, refused: bool) {
        *self
            .refused
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = refused.then(Instant::now);
    }
}

/// The window's one memory of the eye.
pub static MEMORY: Memory = Memory::new();

fn on_display(display: Option<u64>) -> Map<String, Value> {
    let mut params = Map::new();
    if let Some(display) = display {
        params.insert("display".into(), display.into());
    }
    params
}

/// Open the eye on `display` with the table's numbers; one already open is
/// kept by the helper.
fn open(memory: &Memory, display: Option<u64>, call: Call<'_>) -> Result<(), ComputerUseError> {
    let mut params = on_display(display);
    params.extend(eye_table());
    let opened = call(START_METHOD, Value::Object(params));
    memory.note(opened.is_err());
    opened.map(|_| ())
}

/// Ask the eye; an eye that closed (idle, a new helper) is opened again
/// once. A provider that does not know the ask has no eye: remembered.
fn ask(
    memory: &Memory,
    method: &str,
    display: Option<u64>,
    extra: Map<String, Value>,
    call: Call<'_>,
) -> Result<Value, ComputerUseError> {
    let mut params = on_display(display);
    params.extend(extra);
    let answered = match call(method, Value::Object(params.clone())) {
        Err(error) if error.code == NOT_WATCHING => {
            open(memory, display, call)?;
            call(method, Value::Object(params))
        }
        answered => answered,
    };
    if answered
        .as_ref()
        .is_err_and(|error| error.code != NOT_WATCHING)
    {
        memory.note(true);
    }
    answered
}

/// The newest frame on `display`, as a desktop screenshot answers it: `None`
/// when the eye cannot be had, and the caller captures the display instead.
pub(super) fn frame(memory: &Memory, display: Option<u64>, call: Call<'_>) -> Option<Value> {
    if memory.refused_lately() {
        return None;
    }
    ask(memory, FRAME_METHOD, display, Map::new(), call).ok()
}

/// A desktop `screenshot` through the eye: the newest frame, answered the way
/// the helper answers a capture — `None` for a region or a zoom (the display
/// at its own resolution), and when the eye cannot be had.
#[must_use]
pub fn screenshot(params: &Value) -> Option<Value> {
    if params.get("region").is_some()
        || params.get("fullRes").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let display = params.get("display").and_then(Value::as_u64);
    let mut answer = frame(&MEMORY, display, &mut super::call)?;
    if let Some(answer) = answer.as_object_mut() {
        answer.remove(SEQ_KEY);
    }
    Some(answer)
}

/// A desktop OCR read, as the helper is to be asked it: the eye kept open,
/// so the helper reads again only what repainted since its last reading of
/// the display, and what ZeroCode's own windows show as they stand now
/// (`own_region`) handed over — the reading leaves it out, the way the
/// hands never press it. A read that names an app is asked as it came.
#[must_use]
pub fn reading(params: &Value) -> Value {
    reading_with(&MEMORY, params, &mut super::call)
}

pub(super) fn reading_with(memory: &Memory, params: &Value, call: Call<'_>) -> Value {
    let reads_the_desktop =
        params.get("ocr").and_then(Value::as_bool) == Some(true) && params.get("app").is_none();
    if !reads_the_desktop {
        return params.clone();
    }
    if !memory.refused_lately() {
        let display = params.get("display").and_then(Value::as_u64);
        let _ = changes(memory, display, None, None, call);
    }
    let mut asked = params.clone();
    if let (Some(fields), Ok(windows)) =
        (asked.as_object_mut(), super::marks::desktop_windows(call))
    {
        let region = own_region(&windows)
            .iter()
            .map(|piece| json!([piece.x, piece.y, piece.width, piece.height]))
            .collect();
        fields.insert(OWN_REGION_KEY.into(), Value::Array(region));
    }
    asked
}

/// ZeroCode's own windows as a wait judges its repaints: listed when the
/// wait begins, and again before a poll that brought repaints is judged,
/// once the list is `EYE_QUIET_MS` old on the helper's clock — a window
/// that came forward over ZeroCode's is not taken for ZeroCode's own for
/// longer than the quiet a look waits for.
struct Listed {
    windows: Vec<DesktopWindow>,
    at_ms: i64,
}

impl Listed {
    const fn new(windows: Vec<DesktopWindow>, at_ms: i64) -> Self {
        Self { windows, at_ms }
    }

    /// The windows to judge a poll's repaints by; a list that cannot be had
    /// again leaves the last one.
    fn judging(&mut self, brought: bool, now_ms: i64, call: Call<'_>) -> &[DesktopWindow] {
        let quiet = i64::try_from(EYE_QUIET_MS).unwrap_or(i64::MAX);
        if brought
            && now_ms - self.at_ms >= quiet
            && let Ok(windows) = super::marks::desktop_windows(call)
        {
            *self = Self::new(windows, now_ms);
        }
        &self.windows
    }
}

/// `watch`, through the helper, waiting on the thread.
pub fn watch_desktop(params: &Value) -> Result<Value, ComputerUseError> {
    watch(&MEMORY, params, &mut super::call, &mut std::thread::sleep)
}

/// Where the eye stands, and the repaints after `after` or from `from_ms`.
fn changes(
    memory: &Memory,
    display: Option<u64>,
    after: Option<u64>,
    from_ms: Option<i64>,
    call: Call<'_>,
) -> Result<Changes, ComputerUseError> {
    let mut extra = Map::new();
    if let Some(after) = after {
        extra.insert(AFTER_KEY.into(), after.into());
    }
    if let Some(from_ms) = from_ms {
        extra.insert(FROM_KEY.into(), from_ms.into());
    }
    let answer = ask(memory, CHANGES_METHOD, display, extra, call)?;
    Changes::from_answer(&answer).ok_or_else(|| {
        ComputerUseError::new(
            error_code::PROVIDER_INCOMPATIBLE,
            "the eye answered without its stream's place",
        )
    })
}

/// The repaints a wait reads, gathered across its asks: the first reaches
/// back `EYE_BACKGROUND_MS` before the wait's mark (what was already moving),
/// the rest ask past the newest seen.
struct Gathered {
    from: Mark,
    cursor: Cursor,
    changes: Vec<Change>,
    newest: Option<u64>,
}

impl Gathered {
    fn new(from: Mark, cursor: Cursor) -> Self {
        Self {
            from,
            cursor,
            changes: Vec::new(),
            newest: None,
        }
    }

    /// One ask; the helper's clock when it answered.
    fn poll(
        &mut self,
        memory: &Memory,
        display: Option<u64>,
        call: Call<'_>,
    ) -> Result<i64, ComputerUseError> {
        let back = i64::try_from(EYE_BACKGROUND_MS).unwrap_or(i64::MAX);
        let got = match self.newest {
            None => changes(memory, display, None, Some(self.from.at_ms - back), call)?,
            Some(newest) => changes(memory, display, Some(newest), None, call)?,
        };
        if !got.streaming {
            return Err(ComputerUseError::new(
                NOT_WATCHING,
                "the eye's stream stopped while it was read",
            ));
        }
        if !got.continues(&self.cursor) {
            return Err(ComputerUseError::new(
                HISTORY_LOST,
                "the screen stream changed or lost repaint history; observe again before continuing",
            ));
        }
        self.cursor = got.cursor();
        self.newest = Some(got.seq);
        self.changes.extend(got.changes);
        Ok(got.now_ms)
    }
}

/// How the look after an act waited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Settled {
    pub settled: bool,
    pub waited_ms: i64,
    pub polls: u32,
}

impl Settled {
    pub(super) fn answer(self) -> Value {
        json!({ "settled": self.settled, "waitedMs": self.waited_ms, "polls": self.polls })
    }
}

/// Wait until what the last act did has finished painting
/// (`computer_use_protocol::eye::settle`): `None` when the eye cannot be
/// had, and the caller settles by looking.
pub(super) fn settle(
    memory: &Memory,
    display: Option<u64>,
    call: Call<'_>,
    pause: &mut dyn FnMut(Duration),
) -> Option<Settled> {
    if memory.refused_lately() {
        return None;
    }
    let status = changes(memory, display, None, None, call).ok()?;
    let act = status.act.unwrap_or(Mark {
        seq: status.seq,
        at_ms: status.now_ms,
    });
    let mut listed = Listed::new(
        super::marks::desktop_windows(call).unwrap_or_default(),
        status.now_ms,
    );
    let mut gathered = Gathered::new(act, status.cursor());
    // The helper's clock decides; this bound only keeps a clock that never
    // moves from holding the look forever.
    let most = EYE_SETTLE_MAX_MS / EYE_POLL_MS + 2;
    let mut polls = 0_u32;
    loop {
        polls += 1;
        let before = gathered.changes.len();
        let now = gathered.poll(memory, display, call).ok()?;
        let windows = listed.judging(gathered.changes.len() > before, now, call);
        match zerocode_core::computer_use_protocol::eye::settle(
            &gathered.changes,
            act,
            windows,
            now,
        ) {
            Settle::Look { settled } => {
                return Some(Settled {
                    settled,
                    waited_ms: now - status.now_ms,
                    polls,
                });
            }
            Settle::Wait if u64::from(polls) >= most => {
                return Some(Settled {
                    settled: false,
                    waited_ms: now - status.now_ms,
                    polls,
                });
            }
            Settle::Wait => pause(Duration::from_millis(EYE_POLL_MS)),
        }
    }
}

/// `watch`: wait for the screen to change, or to go still, from the
/// display's repaints — no pictures taken. Answers where it changed, in
/// screen points; ZeroCode's own windows and what was already moving when
/// the watch began do not count.
pub(super) fn watch(
    memory: &Memory,
    params: &Value,
    call: Call<'_>,
    pause: &mut dyn FnMut(Duration),
) -> Result<Value, ComputerUseError> {
    let word = params.get("until").and_then(Value::as_str);
    let until = Until::from_word(word).ok_or_else(|| {
        ComputerUseError::invalid_argument("--until is one of change, quiet".to_string())
    })?;
    let display = params.get("display").and_then(Value::as_u64);
    let (budget_ms, capped) = desktop_wait_for_ms(params.get("timeoutMs").and_then(Value::as_u64));
    let status = changes(memory, display, None, None, call)?;
    // A stalled provider clock must not turn a bounded watch into an
    // unbounded loop. Repaint semantics still use the provider's clock.
    let began = Instant::now();
    let start = Mark {
        seq: status.seq,
        at_ms: status.now_ms,
    };
    let mut listed = Listed::new(super::marks::desktop_windows(call)?, status.now_ms);
    let mut gathered = Gathered::new(start, status.cursor());
    let mut polls = 0_u32;
    loop {
        polls += 1;
        let before = gathered.changes.len();
        let now = gathered.poll(memory, display, call)?;
        let elapsed = now - start.at_ms;
        let windows = listed.judging(gathered.changes.len() > before, now, call);
        let (at, rects) = match zerocode_core::computer_use_protocol::eye::watched(
            until,
            &gathered.changes,
            start,
            windows,
            now,
        ) {
            Watched::Changed { first_ms, rects } => (first_ms, rects),
            Watched::Quiet { last_ms, rects } => (last_ms, rects),
            Watched::Wait => {
                let budget = i64::try_from(budget_ms).unwrap_or(i64::MAX);
                let wall_ms = i64::try_from(began.elapsed().as_millis()).unwrap_or(i64::MAX);
                if elapsed + i64::try_from(EYE_POLL_MS).unwrap_or(0) >= budget || wall_ms >= budget
                {
                    let elapsed = elapsed.max(wall_ms);
                    return Err(ComputerUseError::new(
                        error_code::TIMEOUT,
                        format!(
                            "the screen did not {} in {elapsed} ms ({polls} asks)",
                            if until == Until::Change {
                                "change"
                            } else {
                                "go still"
                            }
                        ),
                    ));
                }
                pause(Duration::from_millis(EYE_POLL_MS));
                continue;
            }
        };
        return Ok(json!({
            "satisfied": true,
            "until": word.unwrap_or("change"),
            "atMs": at - start.at_ms,
            "regions": super::compare::rect_regions(&rects),
            "elapsedMs": elapsed,
            "polls": polls,
            "budgetMs": budget_ms,
            "budgetCapped": capped,
        }));
    }
}

/// An OCR wait's gate: the screen is read again only after a repaint that
/// is not ZeroCode's own touched what is read. Whatever was already moving
/// still counts here — a progress line that updates is the text waited for.
pub struct Gate<'m> {
    memory: &'m Memory,
    display: Option<u64>,
    area: Option<Rect>,
    listed: Listed,
    read_at: Option<Cursor>,
    pub reads: u32,
}

impl<'m> Gate<'m> {
    /// A gate over what an OCR wait reads: `None` without the eye, and the
    /// wait reads every time as before.
    pub fn new(memory: &'m Memory, params: &Value, call: Call<'_>) -> Option<Self> {
        if memory.refused_lately() {
            return None;
        }
        let display = params.get("display").and_then(Value::as_u64);
        let area = params.get("region").and_then(|region| {
            let edge = |key: &str| region.get(key).and_then(Value::as_f64);
            Some(Rect::new(
                edge("x")?,
                edge("y")?,
                edge("width")?,
                edge("height")?,
            ))
        });
        let standing = ask(memory, CHANGES_METHOD, display, Map::new(), call).ok()?;
        // A region the eye's display does not wholly show is read every time:
        // the eye would never see the text there change.
        let shows = |area: &Rect| {
            standing.pointer("/display/bounds").is_some_and(|bounds| {
                let edge = |key: &str| bounds.get(key).and_then(Value::as_f64);
                match (edge("x"), edge("y"), edge("width"), edge("height")) {
                    (Some(x), Some(y), Some(width), Some(height)) => {
                        Rect::new(x, y, width, height).contains_rect(area)
                    }
                    _ => false,
                }
            })
        };
        if area.as_ref().is_some_and(|area| !shows(area)) {
            return None;
        }
        let now_ms = standing.get(NOW_KEY).and_then(Value::as_i64).unwrap_or(0);
        Some(Self {
            memory,
            display,
            area,
            listed: Listed::new(
                super::marks::desktop_windows(call).unwrap_or_default(),
                now_ms,
            ),
            read_at: None,
            reads: 0,
        })
    }

    /// Before a reading: whether to read — the first time, after a counted
    /// repaint, and whenever the eye cannot say.
    pub fn read(&mut self, call: Call<'_>) -> bool {
        let Some(read_at) = self.read_at.as_ref() else {
            // Where the stream stands before this reading: a repaint from
            // here on is one the reading may have missed.
            self.read_at = changes(self.memory, self.display, None, None, call)
                .ok()
                .map(|got| got.cursor());
            self.reads += 1;
            return true;
        };
        match changes(
            self.memory,
            self.display,
            Some(read_at.mark.seq),
            None,
            call,
        ) {
            Ok(got) if got.continues(read_at) => {
                let windows = self
                    .listed
                    .judging(!got.changes.is_empty(), got.now_ms, call);
                let ignore = Ignore::new(&[], read_at.mark, windows);
                let ignore = match self.area {
                    Some(area) => ignore.within(area),
                    None => ignore,
                };
                let counted = !ignore.after(&got.changes, read_at.mark).is_empty();
                if counted {
                    self.read_at = Some(got.cursor());
                    self.reads += 1;
                }
                counted
            }
            Ok(got) if got.streaming => {
                // The next OCR read covers the current stream. Never wait
                // for an old sequence to catch up after reconnecting.
                self.read_at = Some(got.cursor());
                self.reads += 1;
                true
            }
            _ => {
                self.read_at = None;
                self.reads += 1;
                true
            }
        }
    }
}

#[cfg(test)]
mod tests;
