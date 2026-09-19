//! The report an evidence folder renders of itself, and the check that it
//! did (docs/design/flow-engine-operator-and-qa.md §3.5, plan D5·D12).
//!
//! Pure: `render` reads the folder's files — the step log, every walk's
//! record, the verdict, the frames — and answers the same bytes for the
//! same folder, with no clock of its own. The frames are not touched: what
//! the run pressed is drawn over them as SVG from the rectangles the walk
//! recorded, and a step that has no frame says so and why (review 19). A
//! manifest of hashes seals what the report was rendered from, and
//! `verify` renders again from the folder alone — a swapped frame, another
//! run's step log or a rewritten verdict fails it.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zerocode_core::artifact_publish::skeleton;
use zerocode_core::computer_flow::{EvidenceLevel, FlowRecord, FlowVerdict};
use zerocode_core::computer_use_protocol::frame::ShotFrame;

use crate::cmd::review::{
    EVIDENCE_FILES_SHOWN, EVIDENCE_PREVIEW_MAX_BYTES, EVIDENCE_TEXT_MAX_BYTES, image_data_url,
};
use crate::run_evidence::{self, FrameMeta, STEPS_FILE, Step};

/// The report a folder renders of itself, and the seal of what it was
/// rendered from — beside the steps.
pub(crate) const REPORT_FILE: &str = "report.html";
pub(crate) const MANIFEST_FILE: &str = "manifest.json";

/// The report, and the manifest it was rendered from.
pub(crate) struct Rendered {
    pub html: String,
    pub manifest: Manifest,
}

/// What the report was rendered from: each file's SHA-256, and the walk's
/// stored verdict — what `verify` compares the folder against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub files: BTreeMap<String, String>,
    pub verdict: Option<FlowVerdict>,
}

/// What `verify` found.
#[derive(Debug, Serialize)]
pub(crate) struct Verified {
    pub reproduced: bool,
    pub mismatches: Vec<Mismatch>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Mismatch {
    pub file: String,
    pub kind: MismatchKind,
}

/// How a file failed to reproduce: its hash differs, it is gone, the report
/// rendered from the folder is not the one on disk, or the verdict judged
/// again is not the stored one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MismatchKind {
    Hash,
    Missing,
    Render,
    Verdict,
}

/// The space a recorded target rectangle is in: screen points (the desktop),
/// CSS pixels (a browser page), or device pixels (the emulator, whose picture
/// is the device itself, so its rectangle is the picture's, 1:1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TargetSpace {
    Points,
    Css,
    Px,
}

/// A walk's record as the report reads it (plan D4's shape, written by the
/// walk): every field optional, so a record from another wave still reads.
#[derive(Debug, Default, Deserialize)]
struct WalkRecord {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    at_epoch_ms: Option<i64>,
    #[serde(default, rename = "elapsedMs")]
    elapsed_ms: Option<u64>,
    #[serde(default)]
    ran: Vec<Ran>,
    #[serde(default)]
    evidence_ids: bool,
    #[serde(default)]
    flow: Option<Value>,
}

impl WalkRecord {
    /// The walk's Flow record, when it wrote a whole one.
    fn flow_record(&self) -> Option<FlowRecord> {
        serde_json::from_value(self.flow.clone()?).ok()
    }

    /// The evidence level the walk ran at, when its Flow said.
    fn evidence_level(&self) -> Option<EvidenceLevel> {
        serde_json::from_value(
            self.flow
                .as_ref()?
                .get(zerocode_core::computer_flow::FLOW_KEY_EVIDENCE)?
                .clone(),
        )
        .ok()
    }
}

/// One walked step as the walk recorded it.
#[derive(Debug, Default, Deserialize)]
struct Ran {
    #[serde(default)]
    ms: Option<u64>,
    /// In the order written — `act`, `settle`, `verify`, `pointer`.
    #[serde(default)]
    phases: serde_json::Map<String, Value>,
    #[serde(default)]
    evidence_id: Option<String>,
    #[serde(default)]
    evidence_n: Option<usize>,
    #[serde(default)]
    target: Option<Target>,
    #[serde(default)]
    marks: Vec<Mark>,
    #[serde(default)]
    check: Option<bool>,
}

/// What the step pressed: its rectangle in its space, and how the walk
/// named it.
#[derive(Debug, Deserialize)]
struct Target {
    space: TargetSpace,
    rect: [f64; 4],
    #[serde(default)]
    dpr: Option<f64>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    mark: Option<u64>,
}

/// A numbered control as the step's look placed it, in picture pixels.
#[derive(Debug, Deserialize)]
struct Mark {
    mark: u64,
    #[serde(default)]
    element_px: Option<[f64; 4]>,
    #[serde(default)]
    badge_px: Option<[f64; 4]>,
    #[serde(default)]
    label: Option<String>,
}

/// A frame read once: its file, its bytes, and its size in pixels.
struct FrameFile {
    name: String,
    bytes: Vec<u8>,
    size: Option<(u32, u32)>,
}

/// The folder, read once.
struct Folder {
    steps: Vec<Step>,
    walks: Vec<(String, WalkRecord)>,
    frames: BTreeMap<usize, FrameFile>,
    /// `(pass, reason)` from `qa-verdict.json`, when one was left.
    verdict: Option<(bool, Option<String>)>,
    /// Every file's digest, by name.
    files: BTreeMap<String, String>,
}

impl Folder {
    fn read(dir: &Path) -> Result<Self, String> {
        if !dir.is_dir() {
            return Err(format!("{} is not a folder", dir.display()));
        }
        let steps = run_evidence::steps_in(dir);
        if steps.is_empty() {
            return Err(format!("no steps in {}", dir.display()));
        }
        let walks: Vec<(String, WalkRecord)> = run_evidence::walks_in(dir)
            .into_iter()
            .filter_map(|(name, record)| Some((name, serde_json::from_value(record).ok()?)))
            .collect();
        let mut files = BTreeMap::new();
        let mut seal = |name: &str| {
            if let Some(digest) = crate::artifact_runtime::file_sha256(&dir.join(name), u64::MAX) {
                files.insert(name.to_string(), digest);
            }
        };
        seal(STEPS_FILE);
        seal(super::evidence::VERDICT_FILE);
        for (name, _) in &walks {
            seal(name);
        }
        let mut frames = BTreeMap::new();
        for step in &steps {
            let Some(name) = &step.shot else {
                continue;
            };
            let Ok(bytes) = std::fs::read(dir.join(name)) else {
                continue;
            };
            files.insert(name.clone(), crate::artifact_runtime::sha256_hex(&bytes));
            let size = step
                .frame
                .as_ref()
                .map(|meta| (meta.width, meta.height))
                .or_else(|| super::compare::png_size(&bytes));
            frames.insert(
                step.n,
                FrameFile {
                    name: name.clone(),
                    bytes,
                    size,
                },
            );
        }
        Ok(Self {
            steps,
            walks,
            frames,
            verdict: super::evidence::read_verdict(dir),
            files,
        })
    }

    /// The level the folder's last walk ran at — `full` when no walk said.
    fn level(&self) -> EvidenceLevel {
        self.walks
            .last()
            .and_then(|(_, walk)| walk.evidence_level())
            .unwrap_or_default()
    }

    /// The last walk that recorded a whole Flow.
    fn flow(&self) -> Option<FlowRecord> {
        self.walks
            .iter()
            .rev()
            .find_map(|(_, walk)| walk.flow_record())
    }

    /// New walks attach metadata only to the actual call's recorded identity.
    /// The legacy branch reproduces old sealed HTML, including its old display
    /// associations. It supplies no retry/arena authority; those require origin.
    fn ran_by_step(&self) -> BTreeMap<usize, (&WalkRecord, &Ran)> {
        let mut by_step = BTreeMap::new();
        let legacy = self.walks.iter().all(|(_, walk)| !walk.evidence_ids);
        for (_, walk) in &self.walks {
            let mut after_start = self
                .steps
                .iter()
                .filter(|step| walk.at_epoch_ms.is_some_and(|at| step.at_epoch_ms >= at))
                .map(|step| step.n);
            for ran in &walk.ran {
                let n = if !legacy {
                    crate::run_evidence::step_with_id(&self.steps, ran.evidence_id.as_deref())
                        .map(|step| step.n)
                } else {
                    ran.evidence_n.or_else(|| after_start.next())
                };
                if let Some(n) = n {
                    by_step.insert(n, (walk, ran));
                }
            }
        }
        by_step
    }
}

/// Render the folder's report: the same bytes for the same folder, from
/// its files alone.
pub(crate) fn render(dir: &Path) -> Result<Rendered, String> {
    Ok(render_folder(&Folder::read(dir)?))
}

/// Write the folder's `report.html` and `manifest.json` (each beside, then
/// renamed over). `None` when the folder's walk ran at `off`: the verdict
/// and the walk's record stand, no report is owed.
pub(crate) fn write(dir: &Path) -> Result<Option<PathBuf>, String> {
    let folder = Folder::read(dir)?;
    if folder.level() == EvidenceLevel::Off {
        return Ok(None);
    }
    let rendered = render_folder(&folder);
    let manifest =
        serde_json::to_vec_pretty(&rendered.manifest).map_err(|error| error.to_string())?;
    let report = dir.join(REPORT_FILE);
    let seal = dir.join(MANIFEST_FILE);
    for (path, bytes) in [
        (&report, rendered.html.as_bytes()),
        (&seal, manifest.as_slice()),
    ] {
        crate::update_store::write_then_rename(path, bytes)
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    }
    run_evidence::emit(
        "flow:report",
        serde_json::json!({"dir": dir, "report": report}),
    );
    Ok(Some(report))
}

/// Whether the folder's report and verdict reproduce from its files alone:
/// the folder rendered again — every file read and digested once on the
/// way — against the sealed digests and the report on disk, and the walk's
/// verdict judged again from its baseline and observations.
pub(crate) fn verify(dir: &Path) -> Verified {
    let mut mismatches = Vec::new();
    let mut note = |file: &str, kind: MismatchKind| {
        mismatches.push(Mismatch {
            file: file.to_string(),
            kind,
        });
    };
    let manifest: Option<Manifest> = std::fs::read(dir.join(MANIFEST_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let Some(manifest) = manifest else {
        note(MANIFEST_FILE, MismatchKind::Missing);
        return Verified {
            reproduced: false,
            mismatches,
        };
    };
    match render(dir) {
        Ok(rendered) => {
            for (file, sealed) in &manifest.files {
                match rendered.manifest.files.get(file) {
                    None => note(file, MismatchKind::Missing),
                    Some(now) if now != sealed => note(file, MismatchKind::Hash),
                    Some(_) => {}
                }
            }
            match std::fs::read(dir.join(REPORT_FILE)) {
                Ok(bytes) if bytes == rendered.html.as_bytes() => {}
                Ok(_) => note(REPORT_FILE, MismatchKind::Render),
                Err(_) => note(REPORT_FILE, MismatchKind::Missing),
            }
            if rendered.manifest.verdict != manifest.verdict {
                note(MANIFEST_FILE, MismatchKind::Verdict);
            }
        }
        Err(_) => {
            for file in manifest.files.keys() {
                if !dir.join(file).is_file() {
                    note(file, MismatchKind::Missing);
                }
            }
            note(REPORT_FILE, MismatchKind::Render);
        }
    }
    // The verdict is not taken from the record: it is judged again from the
    // record's own baseline and observations.
    for (name, record) in run_evidence::walks_in(dir) {
        if let Some(flow) = serde_json::from_value::<WalkRecord>(record)
            .ok()
            .and_then(|walk| walk.flow_record())
            && flow.rejudged() != flow.verdict
        {
            note(&name, MismatchKind::Verdict);
        }
    }
    Verified {
        reproduced: mismatches.is_empty(),
        mismatches,
    }
}

/// Where a recorded rectangle falls in the picture: screen points through
/// the frame's origin and scale (the one `ShotFrame` arithmetic), CSS pixels
/// through the device pixel ratio. A points rectangle with no placement is
/// read as the unit frame.
pub(crate) fn to_picture_px(
    rect: [f64; 4],
    space: TargetSpace,
    frame: Option<&FrameMeta>,
    dpr: Option<f64>,
) -> Option<[f64; 4]> {
    match space {
        TargetSpace::Points => {
            let placed = frame.map_or(Some(ShotFrame::UNIT), |meta| {
                ShotFrame::new((meta.origin[0], meta.origin[1]), meta.scale)
            })?;
            let [x, y] = placed.to_pixel([rect[0], rect[1]]);
            Some([
                x,
                y,
                placed.length_to_pixel(rect[2]),
                placed.length_to_pixel(rect[3]),
            ])
        }
        TargetSpace::Css => {
            // A CSS pixel is a picture pixel unless the page said otherwise.
            let dpr = dpr.unwrap_or(ShotFrame::UNIT.scale());
            Some(rect.map(|side| side * dpr))
        }
        // A device pixel is the picture's pixel: the emulator's frame is the
        // device's own screen, so the rectangle needs no placement.
        TargetSpace::Px => Some(rect),
    }
}

const STYLE: &str = "body{font:14px/1.45 system-ui,sans-serif;margin:0;padding:16px 20px;color:#1c1c1c;background:#fafafa}\
h1{font-size:20px;margin:0 0 8px}h2{font-size:16px;margin:24px 0 8px}\
dl.facts{display:grid;grid-template-columns:max-content 1fr;gap:2px 12px;margin:0}dl.facts dt{color:#666}dl.facts dd{margin:0}\
.pass{color:#1a7f37}.fail{color:#b42318}.none{color:#666;font-style:italic}\
article.step{border:1px solid #ddd;border-radius:6px;padding:10px 12px;margin:10px 0;background:#fff}\
article.step header{display:flex;flex-wrap:wrap;gap:8px;align-items:baseline}article.step .n{font-weight:600}\
article.step .mark{color:#0550ae;font-weight:600}article.step code{font-size:12px;background:#f2f2f2;padding:1px 4px;border-radius:3px}\
.badge{border-radius:10px;padding:0 8px;font-size:12px;color:#fff;background:#1a7f37}.badge.fail{background:#b42318}\
p.phases{margin:4px 0;color:#444;font-size:13px}p.error{color:#b42318;font-size:13px}\
div.frames{display:flex;gap:12px;flex-wrap:wrap}div.frames figure{margin:0;flex:1 1 280px;max-width:640px}\
div.frames figcaption{font-size:12px;color:#666}div.frames svg{width:100%;height:auto;border:1px solid #ccc;background:#eee}\
svg .ring{fill:none;stroke:#ff3b30;stroke-width:3}svg .element{fill:none;stroke:#0550ae;stroke-width:1.5}\
svg .badge{fill:#0550ae;stroke:#fff;stroke-width:1}svg text{font:bold 12px system-ui,sans-serif;fill:#fff;paint-order:stroke;stroke:#0550ae;stroke-width:3}\
svg text.label{fill:#fff;stroke:#ff3b30}table{border-collapse:collapse;font-size:13px}td,th{border:1px solid #ddd;padding:3px 8px;text-align:left}\
td.hash{font-family:ui-monospace,monospace;font-size:12px;word-break:break-all}";

/// A number as an attribute value: `20`, not `20.0`.
/// How a walk's kind reads in the header when it has a word of its own; any
/// other kind is shown as the walk wrote it.
const WALK_KINDS_SHOWN: &[(&str, &str)] = &[(super::arena::WALK_KIND, "경기장")];

fn kind_shown(kind: &str) -> &str {
    WALK_KINDS_SHOWN
        .iter()
        .find(|(word, _)| *word == kind)
        .map_or(kind, |(_, shown)| shown)
}

fn px(value: f64) -> String {
    format!("{value}")
}

/// A frame's symbol id in the page: one per step number, at the width the
/// step log numbers by.
fn frame_id(n: usize) -> String {
    format!("frame-{n:0width$}", width = run_evidence::NUMBER_WIDTH)
}

fn render_folder(folder: &Folder) -> Rendered {
    let esc = skeleton::escape;
    let last_walk = folder.walks.last().map(|(_, walk)| walk);
    let flow = folder.flow();
    let name = last_walk
        .and_then(|walk| walk.name.clone())
        .unwrap_or_else(|| "computer-use".to_string());
    let title = format!("{name} · Flow 리포트");
    let mut body = String::new();
    let _ = write!(
        body,
        "<style>{STYLE}</style>\n<header class=\"run\"><h1>{}</h1><dl class=\"facts\">",
        esc(&title)
    );
    let kind = last_walk
        .and_then(|walk| walk.kind.as_deref())
        .unwrap_or("session");
    let _ = write!(body, "<dt>종류</dt><dd>{}</dd>", esc(kind_shown(kind)));
    if let Some(flow) = &flow {
        let print = &flow.spec.fingerprint;
        let _ = write!(
            body,
            "<dt>환경</dt><dd>apps: {} · hosts: {} · protocol {}</dd>",
            esc(&comma_joined(&print.apps)),
            esc(&comma_joined(&print.hosts)),
            print.protocol
        );
    }
    let started = last_walk
        .and_then(|walk| walk.at_epoch_ms)
        .unwrap_or(folder.steps[0].at_epoch_ms);
    let _ = write!(
        body,
        "<dt>시작</dt><dd>{} UTC</dd>",
        super::evidence::stamp(started)
    );
    if let Some(elapsed) = last_walk.and_then(|walk| walk.elapsed_ms) {
        let _ = write!(body, "<dt>소요</dt><dd>{elapsed} ms</dd>");
    }
    let (verdict_class, verdict_said) = match (&folder.verdict, &flow) {
        (Some((true, _)), _) => ("pass", "합격".to_string()),
        (Some((false, reason)), _) => (
            "fail",
            format!(
                "불합격{}",
                reason
                    .as_deref()
                    .map_or(String::new(), |why| format!(" — {why}"))
            ),
        ),
        (None, Some(flow)) if flow.verdict.pass => ("pass", "합격 (오라클)".to_string()),
        (None, Some(_)) => ("fail", "불합격 (오라클)".to_string()),
        (None, None) => ("none", "판정 없음".to_string()),
    };
    let _ = write!(
        body,
        "<dt>판정</dt><dd class=\"{verdict_class}\">{}</dd>",
        esc(&verdict_said)
    );
    let _ = write!(body, "<dt>걸음</dt><dd>{}</dd>", folder.steps.len());
    if let Some(flow) = &flow {
        let passed = flow
            .verdict
            .lines
            .iter()
            .filter(|line| line.status == zerocode_core::computer_flow::LineStatus::Pass)
            .count();
        let _ = write!(
            body,
            "<dt>오라클</dt><dd>{passed}/{} 통과</dd>",
            flow.verdict.lines.len()
        );
    }
    body.push_str("</dl></header>\n");

    // Each frame once, as a symbol the cards use — inlined under the tables,
    // named by file past them, and never at a level that keeps no frames.
    let inline = folder.level().frames();
    let mut inlined = 0usize;
    body.push_str("<svg class=\"frames\" width=\"0\" height=\"0\" aria-hidden=\"true\"><defs>");
    for (n, frame) in &folder.frames {
        let Some((width, height)) = frame.size else {
            continue;
        };
        let href = if inline
            && inlined < EVIDENCE_FILES_SHOWN
            && frame.bytes.len() as u64 <= EVIDENCE_PREVIEW_MAX_BYTES
        {
            inlined += 1;
            image_data_url(&frame.name, &frame.bytes)
        } else {
            esc(&frame.name)
        };
        let _ = write!(
            body,
            "<image id=\"{}\" href=\"{href}\" width=\"{width}\" height=\"{height}\"/>",
            frame_id(*n)
        );
    }
    body.push_str("</defs></svg>\n");

    body.push_str("<section class=\"timeline\"><h2>타임라인</h2>\n");
    let ran_by_step = folder.ran_by_step();
    let mut last_framed: Option<usize> = None;
    let mut previous_at: Option<i64> = None;
    for step in &folder.steps {
        let ran = ran_by_step.get(&step.n).map(|(_, ran)| *ran);
        let _ = write!(
            body,
            "<article class=\"step\" id=\"step-{}\"><header><span class=\"n\">#{}</span>",
            step.n, step.n
        );
        let target = ran.and_then(|ran| ran.target.as_ref());
        if let Some(mark) = target.and_then(|target| target.mark) {
            let _ = write!(body, "<span class=\"mark\">#{mark}</span>");
        }
        if let Some(label) = target.and_then(|target| target.label.as_deref()) {
            let _ = write!(body, "<span class=\"label\">{}</span>", esc(label));
        }
        let _ = write!(
            body,
            "<code>{} {}</code>",
            esc(&step.tool),
            esc(&step.argv.join(" "))
        );
        if ran.and_then(|ran| ran.check) == Some(true) {
            body.push_str("<span class=\"check\">점검</span>");
        }
        if step.ok {
            body.push_str("<span class=\"badge\">통과</span>");
        } else {
            let _ = write!(
                body,
                "<span class=\"badge fail\">실패{}</span>",
                step.code
                    .as_deref()
                    .map_or(String::new(), |code| format!(" · {}", esc(code)))
            );
        }
        body.push_str("</header>");
        let phases: Vec<String> = ran
            .map(|ran| {
                ran.phases
                    .iter()
                    .filter_map(|(phase, ms)| Some(format!("{} {} ms", esc(phase), ms.as_u64()?)))
                    .chain(ran.ms.map(|ms| format!("계 {ms} ms")))
                    .collect()
            })
            .unwrap_or_default();
        if !phases.is_empty() {
            let _ = write!(body, "<p class=\"phases\">{}</p>", phases.join(" · "));
        } else if let Some(previous) = previous_at {
            let _ = write!(
                body,
                "<p class=\"phases\">이전 걸음에서 +{} ms</p>",
                step.at_epoch_ms - previous
            );
        }
        if let Some(error) = &step.error {
            let _ = write!(body, "<p class=\"error\">{}</p>", esc(error));
        }
        body.push_str("<div class=\"frames\">");
        body.push_str("<figure class=\"before\"><figcaption>전</figcaption>");
        match last_framed.and_then(|n| folder.frames.get(&n).map(|frame| (n, frame))) {
            Some((n, frame)) => frame_figure(&mut body, n, frame, None),
            None => body.push_str("<p class=\"none\">이전 프레임 없음</p>"),
        }
        body.push_str("</figure><figure class=\"after\"><figcaption>후</figcaption>");
        match folder.frames.get(&step.n) {
            Some(frame) => {
                let overlay = ran.map(|ran| (ran, step.frame.as_ref()));
                frame_figure(&mut body, step.n, frame, overlay);
                last_framed = Some(step.n);
            }
            None => {
                let why = step
                    .frame_skipped
                    .and_then(|why| serde_json::to_value(why).ok())
                    .and_then(|why| why.as_str().map(str::to_string));
                match why {
                    Some(why) => {
                        let _ = write!(body, "<p class=\"none\">프레임 없음: {}</p>", esc(&why));
                    }
                    None => body.push_str("<p class=\"none\">프레임 없음</p>"),
                }
            }
        }
        body.push_str("</figure></div></article>\n");
        previous_at = Some(step.at_epoch_ms);
    }
    body.push_str("</section>\n");

    if let Some(flow) = &flow {
        body.push_str("<section class=\"oracle\"><h2>오라클</h2><table><tr><th>#</th><th>상태</th><th>줄</th></tr>");
        for line in &flow.verdict.lines {
            let status = serde_json::to_value(line.status)
                .ok()
                .and_then(|status| status.as_str().map(str::to_string))
                .unwrap_or_default();
            let _ = write!(
                body,
                "<tr class=\"{status}\"><td>{}</td><td>{status}</td><td>{}</td></tr>",
                line.id,
                esc(&line.said)
            );
        }
        body.push_str("</table></section>\n");
    }

    body.push_str("<section class=\"sources\"><h2>출처</h2><dl class=\"facts\">");
    if let Some(walk) = last_walk {
        let _ = write!(
            body,
            "<dt>레시피</dt><dd>{}{}</dd>",
            esc(walk.name.as_deref().unwrap_or("—")),
            walk.file
                .as_deref()
                .map_or(String::new(), |file| format!(" ({})", esc(file)))
        );
    }
    if let Some(flow) = &flow {
        let print = &flow.spec.fingerprint;
        let _ = write!(
            body,
            "<dt>지문</dt><dd>apps: {} · hosts: {} · protocol {}</dd><dt>정책 · 증거</dt><dd>{} · {}</dd>",
            esc(&comma_joined(&print.apps)),
            esc(&comma_joined(&print.hosts)),
            print.protocol,
            flow.spec.policy.as_str(),
            flow.spec.evidence.as_str()
        );
    }
    body.push_str("</dl><table class=\"manifest\"><tr><th>파일</th><th>sha256</th></tr>");
    for (file, digest) in &folder.files {
        let _ = write!(
            body,
            "<tr><td>{}</td><td class=\"hash\">{digest}</td></tr>",
            esc(file)
        );
    }
    body.push_str("</table>");
    if let Some((pass, reason)) = &folder.verdict {
        let text = serde_json::json!({ "pass": pass, "reason": reason }).to_string();
        if text.len() as u64 <= EVIDENCE_TEXT_MAX_BYTES {
            let _ = write!(
                body,
                "<details><summary>{}</summary><pre>{}</pre></details>",
                super::evidence::VERDICT_FILE,
                esc(&text)
            );
        }
    }
    body.push_str("</section>\n");

    Rendered {
        html: skeleton::wrap(&body, &title),
        manifest: Manifest {
            files: folder.files.clone(),
            verdict: flow.map(|flow| flow.verdict),
        },
    }
}

/// A set's words, comma-joined.
fn comma_joined(words: &std::collections::BTreeSet<String>) -> String {
    words
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// One frame in a card: the symbol, and — for the step's own frame — the
/// ring around what it pressed and the badges of the controls its look
/// numbered, all from the rectangles the walk recorded.
fn frame_figure(
    body: &mut String,
    n: usize,
    frame: &FrameFile,
    overlay: Option<(&Ran, Option<&FrameMeta>)>,
) {
    let esc = skeleton::escape;
    let Some((width, height)) = frame.size else {
        let _ = write!(
            body,
            "<a href=\"{}\">{}</a>",
            esc(&frame.name),
            esc(&frame.name)
        );
        return;
    };
    let _ = write!(
        body,
        "<svg viewBox=\"0 0 {width} {height}\" role=\"img\"><use href=\"#{}\"/>",
        frame_id(n)
    );
    if let Some((ran, meta)) = overlay {
        for mark in &ran.marks {
            if let Some([x, y, w, h]) = mark.element_px {
                let _ = write!(
                    body,
                    "<rect class=\"element\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/>",
                    px(x),
                    px(y),
                    px(w),
                    px(h)
                );
            }
            if let Some([x, y, w, h]) = mark.badge_px {
                let _ = write!(
                    body,
                    "<rect class=\"badge\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\"/><text x=\"{}\" y=\"{}\">#{}{}</text>",
                    px(x),
                    px(y),
                    px(w),
                    px(h),
                    // A pill: the corner is half the height.
                    px(h / 2.0),
                    px(x),
                    px(y + h),
                    mark.mark,
                    mark.label
                        .as_deref()
                        .map_or(String::new(), |label| format!(" {}", esc(label)))
                );
            }
        }
        if let Some(target) = &ran.target
            && let Some([x, y, w, h]) = to_picture_px(target.rect, target.space, meta, target.dpr)
        {
            let _ = write!(
                body,
                "<rect class=\"ring\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/>",
                px(x),
                px(y),
                px(w),
                px(h)
            );
            if let Some(label) = &target.label {
                let _ = write!(
                    body,
                    "<text class=\"label\" x=\"{}\" y=\"{}\">{}</text>",
                    px(x),
                    px(y),
                    esc(label)
                );
            }
        }
    }
    body.push_str("</svg>");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_evidence::{FrameSkipped, Framing, record, record_walk, steps_in};
    use serde_json::json;
    use std::collections::BTreeMap;
    use zerocode_core::computer_flow::{
        Check, CheckKind, EvidenceLevel, Fingerprint, FlowRecord, FlowSpec, Observed, Policy,
        Presence, judge,
    };
    use zerocode_core::computer_recipe::RecipeTool;
    use zerocode_core::computer_use_protocol::frame::ShotFrame;

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    /// A real PNG of one shade, so the frame's size reads back from it.
    fn png(width: u32, height: u32, shade: u8) -> Vec<u8> {
        super::super::screenshot_png::RgbaImage::new(
            width,
            height,
            vec![shade; (width * height * 4) as usize],
        )
        .expect("an image")
        .encode()
        .expect("encodes")
    }

    fn framed(dir: &std::path::Path, at: i64, argv: &[&str], shot: &[u8], placed: ShotFrame) {
        record(
            dir,
            at,
            "computer",
            &words(argv),
            Ok(()),
            Framing::Picture { png: shot, placed },
        );
    }

    /// A walk's record of its Flow: one required state line, seen at the end.
    fn flow_record() -> FlowRecord {
        let checks = vec![Check {
            id: 1,
            kind: CheckKind::State,
            required: true,
            tool: RecipeTool::Computer,
            argv: words(&["wait-for", "--app", "Wallet", "--text", "완료"]),
        }];
        let baseline = BTreeMap::new();
        let observed = BTreeMap::from([(
            1,
            Observed::Seen(Presence {
                present: true,
                count: None,
            }),
        )]);
        let verdict = judge(&checks, &baseline, &observed);
        FlowRecord {
            spec: FlowSpec {
                policy: Policy::Dry,
                evidence: EvidenceLevel::Full,
                fingerprint: Fingerprint {
                    apps: ["com.example.admin".to_string()].into_iter().collect(),
                    hosts: ["stg-admin.example.internal".to_string()]
                        .into_iter()
                        .collect(),
                    protocol: 3,
                    assets: Default::default(),
                },
                checks,
                money: None,
                confirm: Default::default(),
                trigger: None,
            },
            baseline,
            observed,
            verdict,
        }
    }

    fn walk_of(steps: usize, flow: Option<&FlowRecord>) -> serde_json::Value {
        let mut walk = json!({
            "kind": "recipe-run", "name": "song-geum", "file": "/r/song-geum.md",
            "start": 1, "steps": steps, "at_epoch_ms": 900, "elapsedMs": 40, "budgetMs": 60_000,
            "phases": { "resolve": 2, "report": 1 },
            "ran": (1..=steps).map(|n| json!({
                "step": n, "shown": n, "tool": "computer", "verb": "click", "ok": true, "ms": 12,
                "evidence_n": n, "phases": { "act": 8, "settle": 3, "verify": 1 },
            })).collect::<Vec<_>>(),
            "done": true, "stoppedAt": null, "stop": null, "next": null,
        });
        if let Some(flow) = flow {
            walk["flow"] = serde_json::to_value(flow).unwrap();
        }
        walk
    }

    #[test]
    fn flow_retry_reports_link_only_recorded_ids_without_changing_legacy_display() {
        let dir = tempfile::tempdir().unwrap();
        record(
            dir.path(),
            1_000,
            "computer",
            &words(&["key", "--key", "tab"]),
            Ok(()),
            Framing::None,
        );
        let legacy = walk_of(1, None);
        record_walk(dir.path(), &legacy).unwrap();
        let old = Folder::read(dir.path()).unwrap();
        assert_eq!(
            old.ran_by_step().len(),
            1,
            "legacy HTML keeps its historic association"
        );
        let before = render(dir.path()).unwrap();
        write(dir.path()).unwrap();
        assert!(verify(dir.path()).reproduced);
        assert_eq!(before.html, render(dir.path()).unwrap().html);
        let mut strict = legacy;
        strict["evidence_ids"] = json!(true);
        strict["ran"][0]["evidence_id"] = json!("unwritten");
        record_walk(dir.path(), &strict).unwrap();
        assert!(
            Folder::read(dir.path()).unwrap().ran_by_step().is_empty(),
            "a guessed number and timestamp cannot link a new-format row"
        );
    }

    /// Every step is a card; a framed step shows its frame after (and the
    /// last frame before it), a step without one says why; each frame is
    /// inlined once; the same folder renders the same bytes.
    #[test]
    fn a_report_page_shows_each_step_with_its_frames_or_says_there_is_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        framed(
            dir.path(),
            1_000,
            &["click"],
            &png(4, 3, 0x20),
            ShotFrame::UNIT,
        );
        record(
            dir.path(),
            1_100,
            "computer",
            &words(&["key", "--key", "a"]),
            Ok(()),
            Framing::Skipped(FrameSkipped::Backlog),
        );
        framed(
            dir.path(),
            1_200,
            &["type", "--text", "hi"],
            &png(4, 3, 0x80),
            ShotFrame::UNIT,
        );
        let rendered = render(dir.path()).expect("renders");
        let html = &rendered.html;
        assert_eq!(html.matches("<article class=\"step\"").count(), 3, "{html}");
        assert!(html.contains("프레임 없음: backlog"), "{html}");
        assert_eq!(
            html.matches("data:image/png;base64,").count(),
            2,
            "each frame is inlined once, as a symbol the cards use"
        );
        assert_eq!(
            html.matches("href=\"#frame-001\"").count(),
            3,
            "the first frame is the first card's after and the next two cards' before"
        );
        assert_eq!(html.matches("href=\"#frame-003\"").count(), 1);
        assert!(html.starts_with("<!doctype html>"), "a whole document");
        assert!(!html.contains("<script"), "a report runs nothing");
        assert_eq!(
            rendered.manifest.files.keys().collect::<Vec<_>>(),
            [
                "001-computer-click.png",
                "003-computer-type.png",
                "steps.jsonl"
            ],
            "the manifest seals the log and the frames it shows"
        );
        assert_eq!(
            render(dir.path()).expect("renders again").html,
            rendered.html,
            "the same folder renders the same bytes"
        );
    }

    /// The pressed target is ringed where the recorded rectangle falls in
    /// the picture — points through the frame's origin and scale, CSS
    /// pixels through the device ratio — and a mark's badge is drawn from
    /// its recorded pixels, never re-derived.
    #[test]
    fn a_report_rings_the_acted_target_from_the_recorded_rect() {
        let dir = tempfile::tempdir().expect("tempdir");
        let placed = ShotFrame::new((100.0, 50.0), 2.0).unwrap();
        framed(
            dir.path(),
            1_000,
            &["click", "--mark", "9"],
            &png(8, 6, 0),
            placed,
        );
        let mut walk = walk_of(1, None);
        walk["ran"][0]["target"] = json!({
            "space": "points", "rect": [110.0, 60.0, 20.0, 10.0], "label": "송금 확인", "mark": 9
        });
        walk["ran"][0]["marks"] = json!([
            { "mark": 9, "element_px": [20, 20, 40, 20], "badge_px": [12, 12, 8, 8], "label": "송금 확인" }
        ]);
        record_walk(dir.path(), &walk).expect("the walk's record");
        let steps = steps_in(dir.path());
        let meta = steps[0]
            .frame
            .as_ref()
            .expect("a framed step keeps its placement");
        assert_eq!(
            (meta.scale, meta.origin, meta.width, meta.height),
            (2.0, [100.0, 50.0], 8, 6)
        );
        assert_eq!(
            to_picture_px(
                [110.0, 60.0, 20.0, 10.0],
                TargetSpace::Points,
                Some(meta),
                None
            ),
            Some([20.0, 20.0, 40.0, 20.0])
        );
        assert_eq!(
            to_picture_px(
                [10.0, 20.0, 30.0, 40.0],
                TargetSpace::Css,
                Some(meta),
                Some(2.0)
            ),
            Some([20.0, 40.0, 60.0, 80.0])
        );
        let html = render(dir.path()).expect("renders").html;
        assert!(
            html.contains(r#"class="ring" x="20" y="20" width="40" height="20""#),
            "{html}"
        );
        assert!(html.contains("#9") && html.contains("송금 확인"), "{html}");
        assert!(html.contains(r#"class="badge" x="12" y="12""#), "{html}");
        assert!(
            html.contains("act 8"),
            "the step's phases are shown: {html}"
        );
    }

    /// `write` leaves the report and its manifest; `verify` renders the
    /// folder again and re-judges the walk's Flow — and a frame swapped
    /// for another picture, or a verdict rewritten in the record, fails it.
    #[test]
    fn verify_reproduces_the_verdict_from_the_folder_and_fails_on_a_swapped_frame() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = png(4, 3, 0x10);
        framed(dir.path(), 1_000, &["click"], &first, ShotFrame::UNIT);
        framed(
            dir.path(),
            1_100,
            &["click"],
            &png(4, 3, 0x90),
            ShotFrame::UNIT,
        );
        let flow = flow_record();
        record_walk(dir.path(), &walk_of(2, Some(&flow))).expect("the walk's record");
        super::super::evidence::write_verdict(
            dir.path(),
            &words(&["verdict", "--pass"]),
            true,
            None,
            2_000,
        )
        .expect("a verdict");
        let path = write(dir.path())
            .expect("written")
            .expect("a report at this level");
        assert!(path.ends_with(REPORT_FILE) && dir.path().join(MANIFEST_FILE).is_file());
        let verified = verify(dir.path());
        assert!(verified.reproduced, "{:?}", verified.mismatches);

        std::fs::write(dir.path().join("001-computer-click.png"), png(4, 3, 0x11)).unwrap();
        let verified = verify(dir.path());
        assert!(!verified.reproduced);
        assert!(
            verified.mismatches.iter().any(|mismatch| {
                mismatch.file == "001-computer-click.png" && mismatch.kind == MismatchKind::Hash
            }),
            "{:?}",
            verified.mismatches
        );
        std::fs::write(dir.path().join("001-computer-click.png"), &first).unwrap();
        assert!(
            verify(dir.path()).reproduced,
            "the frame put back verifies again"
        );

        let walk = dir.path().join("walk-001.json");
        let mut record: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&walk).unwrap()).unwrap();
        record["flow"]["verdict"]["pass"] = json!(false);
        std::fs::write(&walk, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
        let verified = verify(dir.path());
        assert!(
            verified.mismatches.iter().any(|mismatch| {
                mismatch.file == "walk-001.json" && mismatch.kind == MismatchKind::Verdict
            }),
            "the verdict is judged again from the baseline and the observations: {:?}",
            verified.mismatches
        );
    }

    /// Another run's step log put into the folder is not this run's
    /// evidence: its hash differs, and the report renders differently.
    #[test]
    fn verify_fails_when_another_runs_steps_are_substituted() {
        let mine = tempfile::tempdir().expect("tempdir");
        let theirs = tempfile::tempdir().expect("tempdir");
        framed(
            mine.path(),
            1_000,
            &["click"],
            &png(4, 3, 0x10),
            ShotFrame::UNIT,
        );
        framed(
            mine.path(),
            1_100,
            &["key", "--key", "a"],
            &png(4, 3, 0x20),
            ShotFrame::UNIT,
        );
        framed(
            theirs.path(),
            5_000,
            &["click"],
            &png(4, 3, 0x10),
            ShotFrame::UNIT,
        );
        framed(
            theirs.path(),
            5_100,
            &["type", "--text", "x"],
            &png(4, 3, 0x30),
            ShotFrame::UNIT,
        );
        write(mine.path()).expect("written").expect("a report");
        assert!(verify(mine.path()).reproduced);
        std::fs::copy(
            theirs.path().join(crate::run_evidence::STEPS_FILE),
            mine.path().join(crate::run_evidence::STEPS_FILE),
        )
        .unwrap();
        let verified = verify(mine.path());
        assert!(!verified.reproduced);
        let kinds: Vec<(&str, MismatchKind)> = verified
            .mismatches
            .iter()
            .map(|mismatch| (mismatch.file.as_str(), mismatch.kind))
            .collect();
        assert!(
            kinds.contains(&(crate::run_evidence::STEPS_FILE, MismatchKind::Hash))
                && kinds.contains(&(REPORT_FILE, MismatchKind::Render)),
            "{kinds:?}"
        );
    }

    /// The plan's numbers (§5, F4): a folder of twenty framed steps with
    /// 1280×800 frames — `render`, `write` and `verify` in ms (median of
    /// three) and the report's bytes. Run by hand:
    /// `cargo test -p zerocode-shell --release --bin zerocode-shell -- --ignored --nocapture report::tests::measure`.
    /// An arena's walk is shown as the arena it was — the one kind with a
    /// word of its own; a recipe walk keeps its own name.
    #[test]
    fn an_arena_walk_is_shown_as_the_arena_it_was() {
        let dir = tempfile::tempdir().expect("tempdir");
        framed(
            dir.path(),
            1_000,
            &["click"],
            &png(4, 4, 9),
            ShotFrame::UNIT,
        );
        let mut walk = walk_of(1, None);
        walk["kind"] = json!(super::super::arena::WALK_KIND);
        record_walk(dir.path(), &walk).unwrap();
        let page = render(dir.path()).unwrap().html;
        assert!(page.contains("<dt>종류</dt><dd>경기장</dd>"), "{page}");
        assert_eq!(kind_shown("recipe-run"), "recipe-run");
    }

    #[test]
    #[ignore = "a measurement, printed; not a check"]
    fn measure_render_write_and_verify_on_twenty_framed_steps() {
        const STEPS: usize = 20;
        const RUNS: usize = 3;
        let dir = tempfile::tempdir().expect("tempdir");
        let (width, height) = (1280u32, 800u32);
        let mut frame_bytes = 0usize;
        for n in 0..STEPS {
            // A gradient with a noisy band, so the PNG is the size of a
            // screenshot rather than of a solid colour.
            let mut seed = 0x9E37_79B9u32.wrapping_add(n as u32);
            let mut pixels = Vec::with_capacity((width * height * 4) as usize);
            for y in 0..height {
                for x in 0..width {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    let noisy = (height / 4..height / 2).contains(&y);
                    let shade = if noisy {
                        seed as u8
                    } else {
                        ((x + y + n as u32) % 256) as u8
                    };
                    pixels.extend_from_slice(&[shade, shade.wrapping_add(40), shade / 2, 255]);
                }
            }
            let shot = super::super::screenshot_png::RgbaImage::new(width, height, pixels)
                .unwrap()
                .encode()
                .unwrap();
            frame_bytes += shot.len();
            framed(
                dir.path(),
                1_000 + n as i64 * 100,
                &["click"],
                &shot,
                ShotFrame::UNIT,
            );
        }
        let flow = flow_record();
        record_walk(dir.path(), &walk_of(STEPS, Some(&flow))).unwrap();
        super::super::evidence::write_verdict(
            dir.path(),
            &words(&["verdict", "--pass"]),
            true,
            None,
            9_000,
        )
        .unwrap();
        let median = |mut runs: Vec<u128>| {
            runs.sort_unstable();
            runs[runs.len() / 2]
        };
        let timed = |work: &dyn Fn()| {
            median(
                (0..RUNS)
                    .map(|_| {
                        let started = std::time::Instant::now();
                        work();
                        started.elapsed().as_millis()
                    })
                    .collect(),
            )
        };
        let render_ms = timed(&|| {
            render(dir.path()).unwrap();
        });
        let write_ms = timed(&|| {
            write(dir.path()).unwrap();
        });
        let verify_ms = timed(&|| {
            assert!(verify(dir.path()).reproduced);
        });
        let report_bytes = std::fs::metadata(dir.path().join(REPORT_FILE))
            .unwrap()
            .len();
        println!(
            "measure: steps={STEPS} frames={STEPS}×{width}×{height} frame_bytes_total={frame_bytes} \
             render_ms={render_ms} write_ms={write_ms} verify_ms={verify_ms} report_bytes={report_bytes} \
             (median of {RUNS}, {})",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
    }
}
