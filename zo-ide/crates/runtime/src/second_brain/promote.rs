//! Session-end write-back: what a session learned, left in the vault.
//!
//! The Dreamer already decides what is durable — it mines the session's own
//! evidence, gates it, and promotes the survivors into the project's memory
//! store. This module does not repeat that judgement. It takes the pass's
//! report and mirrors each promotion into the vault as one atomic page plus one
//! line in the ingest log, so knowledge a session paid for is legible outside
//! zo's own state directory.
//!
//! # Append-only, on purpose
//!
//! Nothing here rewrites a file. A page is created with [`fs::File::create_new`]
//! and skipped if the name is taken; the log is opened for append. That is what
//! makes an automatic writer safe to point at a directory somebody edits by
//! hand: the worst case of a bug here is an unwanted new file, never a lost
//! paragraph. `raw/` is never touched at all.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use decision_core::dreamer::{
    cited_targets, judge_relation, LessonEvidence, LessonRelation, SkipReason, VaultPage,
    RELATED_RELATION_KEY,
};

use crate::memory::recall::tokenize;
use crate::memory::{DreamReport, WriteOutcome};

use super::{corpus, wiki_index_link, SecondBrain, SESSION_SOURCE_PREFIX, WIKI_DIR, ZO_PAGE_DIR};

/// Tag stamped on every page zo writes, so a vault query can separate what a
/// session left from what a person ingested.
const ZO_PAGE_TAG: &str = "zo";

/// Existing pages one promoted page may name as neighbours. A promoted lesson
/// is one claim, and a page that points at everything vaguely adjacent is a
/// hub the graph learns nothing from.
pub const MAX_PROMOTED_RELATED: usize = 3;

/// Overlapping tokens a page needs before it is called a neighbour. One shared
/// word is a coincidence — `timeout` alone joins every unrelated gotcha.
const MIN_RELATED_OVERLAP: usize = 2;

/// Pages one promoted page may name under a single typed relation. Same bound
/// as [`MAX_PROMOTED_RELATED`] and for the same reason: a page that supersedes
/// nine others is a claim nobody can check.
const MAX_PROMOTED_TYPED: usize = 3;

/// One page this pass created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotedPage {
    /// The memory slug the page was named after.
    pub slug: String,
    /// Vault-relative wikilink target, e.g. `wiki/zo/gotcha-sqlite-busy`.
    pub link: String,
    /// The typed edges this page declares, strongest first. Carried out of the
    /// write so the ingest log can say what the promotion *did* to the graph —
    /// `→ [[new]] supersedes [[old]]` — rather than only that a file appeared.
    pub typed: Vec<(LessonRelation, String)>,
    /// Absolute path of the file that was written.
    pub path: PathBuf,
}

/// What one session-end write-back did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WikiPromotion {
    /// Pages created by this pass, in promotion order.
    pub pages: Vec<PromotedPage>,
    /// Lessons whose page already existed. Counted rather than rewritten —
    /// a re-promoted lesson is the steady state, not an event.
    pub already_present: usize,
}

impl WikiPromotion {
    /// True when the vault was left exactly as it was found.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.pages.is_empty()
    }

    /// One line for a log or an exit summary.
    #[must_use]
    pub fn summary_line(&self) -> String {
        format!(
            "second brain: wrote {} page(s), {} already present",
            self.pages.len(),
            self.already_present
        )
    }
}

/// Mirror `report`'s promotions into `vault`.
///
/// Takes the vault, the clock and the report explicitly so the whole write-back
/// is testable against a temporary directory with no environment at all.
/// Lessons the store left [`WriteOutcome::Unchanged`] are skipped: memory
/// already held those bytes, so the vault already holds their page.
pub fn record_lessons(
    vault: &SecondBrain,
    session_id: &str,
    report: &DreamReport,
    now: SystemTime,
) -> io::Result<WikiPromotion> {
    let mut promotion = WikiPromotion::default();
    let written: Vec<&str> = report
        .applied
        .iter()
        .filter(|applied| applied.outcome != WriteOutcome::Unchanged)
        .map(|applied| applied.slug.as_str())
        .collect();
    if written.is_empty() {
        return Ok(promotion);
    }
    if !vault.is_set_up() {
        // A configured path that was never set up is not this pass's problem to
        // fix: creating `wiki/` here would scatter a half-vault across whatever
        // the setting happens to point at.
        return Ok(promotion);
    }

    let pages_dir = vault.zo_pages_dir();
    fs::create_dir_all(&pages_dir)?;
    let stamp = Stamp::at(now);
    // Once per pass, not once per lesson: one walk of `wiki/` answers every
    // page this pass writes.
    let candidates = related_candidates(vault);

    for lesson in &report.plan.promote {
        if !written.contains(&lesson.slug.as_str())
            || !crate::memory::curation::is_safe_memory_slug(&lesson.slug)
        {
            continue;
        }
        let path = pages_dir.join(format!("{}.md", lesson.slug));
        let link = format!("{WIKI_DIR}/{ZO_PAGE_DIR}/{}", lesson.slug);
        match fs::File::create_new(&path) {
            Ok(mut file) => {
                let relations = page_relations(&candidates, &evidence_for(lesson, report));
                let page = render_page(
                    session_id,
                    &stamp,
                    &lesson.slug,
                    &lesson.summary,
                    &lesson.lesson,
                    lesson.kind.as_str(),
                    &relations,
                );
                file.write_all(page.as_bytes())?;
                file.sync_all()?;
                promotion.pages.push(PromotedPage {
                    slug: lesson.slug.clone(),
                    link,
                    typed: relations.typed,
                    path,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                promotion.already_present += 1;
            }
            Err(error) => return Err(error),
        }
    }

    if !promotion.pages.is_empty() {
        append_log(&vault.log_path(), session_id, &stamp, &promotion.pages)?;
    }
    Ok(promotion)
}

/// Resolve the vault and the write-back switch, then run [`record_lessons`].
///
/// Returns `None` when the feature is off — no vault, or `secondBrain.writeBack`
/// turned off — and when the write fails, which must never sink the session
/// shutdown that called it. A failure is reported on stderr instead, where the
/// rest of the session-end diagnostics already land.
#[must_use]
pub fn promote_session_lessons(
    cwd: &Path,
    session_id: &str,
    report: &DreamReport,
) -> Option<WikiPromotion> {
    let config = crate::config::ConfigLoader::default_for(cwd).load().ok()?;
    if !config.second_brain().write_back() {
        return None;
    }
    let vault = SecondBrain::resolve(&config)?;
    match record_lessons(&vault, session_id, report, SystemTime::now()) {
        Ok(promotion) => Some(promotion),
        Err(error) => {
            eprintln!(
                "zo: second brain write-back failed at {}: {error}",
                vault.root().display()
            );
            None
        }
    }
}

/// The two renderings of one instant this module needs: `YYYY-MM-DD HH:MM` for
/// the vault's log-line convention and RFC 3339 for a page's `ingested_at`.
/// Both are UTC, which is what makes two machines' log lines sortable together.
struct Stamp {
    date: String,
    minute: String,
    rfc3339: String,
}

impl Stamp {
    fn at(now: SystemTime) -> Self {
        let secs = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        let days = i64::try_from(secs / 86_400).unwrap_or(0);
        let (year, month, day) = crate::team_cron_registry::civil_from_days(days);
        let seconds_of_day = secs % 86_400;
        let (hour, minute, second) = (
            seconds_of_day / 3_600,
            (seconds_of_day % 3_600) / 60,
            seconds_of_day % 60,
        );
        Self {
            date: format!("{year:04}-{month:02}-{day:02}"),
            minute: format!("{hour:02}:{minute:02}"),
            rfc3339: format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"),
        }
    }
}

/// One atomic page in the vault's own shape: YAML frontmatter carrying
/// `source`, `ingested_at`, `tags` and this page's relations, then the lesson,
/// then the links back to the index and to every page named above, which a
/// reader follows to see what this lesson sits next to.
///
/// Relation keys are written in [`corpus::RelationKind::TYPED`] order —
/// `related` first, then the typed three — so a reader and a `git diff` see
/// them in the same order the scanner reads them in.
fn render_page(
    session_id: &str,
    stamp: &Stamp,
    slug: &str,
    summary: &str,
    lesson: &str,
    kind: &str,
    relations: &PageRelations,
) -> String {
    let title = single_line(summary, slug);
    let mut keys = String::new();
    // Omitted rather than written empty: `related: []` is a claim that this
    // page has no neighbours, which is not what "none overlapped" means.
    if !relations.related.is_empty() {
        let _ = writeln!(
            keys,
            "{RELATED_RELATION_KEY}: [ {} ]",
            wikilinks(&relations.related)
        );
    }
    for relation in TYPED_KEY_ORDER {
        let targets: Vec<String> = relations
            .typed
            .iter()
            .filter(|(kind, _)| *kind == relation)
            .map(|(_, target)| target.clone())
            .collect();
        if !targets.is_empty() {
            let _ = writeln!(keys, "{}: [ {} ]", relation.as_str(), wikilinks(&targets));
        }
    }
    let mut neighbours = format!("[[{}]]", wiki_index_link());
    for (_, target) in &relations.typed {
        let _ = write!(neighbours, " · [[{target}]]");
    }
    for slug in &relations.related {
        let _ = write!(neighbours, " · [[{slug}]]");
    }
    format!(
        "---\ntitle: {quoted_title}\nsource: {quoted_source}\ningested_at: {ingested}\ntags: [{ZO_PAGE_TAG}, {kind}]\n{keys}---\n\n# {title}\n\n{body}\n\n관련: {neighbours}\n",
        quoted_title = yaml_scalar(&title),
        quoted_source = yaml_scalar(&format!(
            "{SESSION_SOURCE_PREFIX}{}",
            single_line(session_id, "unknown")
        )),
        ingested = stamp.rfc3339,
        body = lesson.trim(),
    )
}

/// The typed keys, in the order [`corpus::RelationKind::TYPED`] reads them.
/// `implements` has no writer here — nothing in a curation pass decides that a
/// lesson implements a design — so it is absent rather than guessed at.
const TYPED_KEY_ORDER: [LessonRelation; 3] = [
    LessonRelation::DependsOn,
    LessonRelation::Supersedes,
    LessonRelation::Contradicts,
];

fn wikilinks(slugs: &[String]) -> String {
    slugs
        .iter()
        .map(|slug| format!("[[{slug}]]"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One page a promoted lesson may be filed against.
struct Candidate {
    /// Slug and claim, in the shape the pure judgement reads.
    page: VaultPage,
    /// Tokens of the claim, for the untyped overlap fallback.
    tokens: BTreeSet<String>,
    /// Written by zo itself. Such a page may still carry a *typed* edge — a
    /// re-promotion supersedes the page it refines — but never the untyped
    /// `related` fallback, whose job is to file a lesson against vault
    /// knowledge rather than against zo's own back-catalogue.
    zo_written: bool,
}

/// Every page a promoted lesson may be filed against: its slug, its pointer
/// claim, and that claim's tokens.
///
/// Pointer summaries only — no page body is opened, because this runs while a
/// session shuts down. The index is skipped because every promoted page already
/// links it.
fn related_candidates(vault: &SecondBrain) -> Vec<Candidate> {
    let index = wiki_index_link();
    let zo_pages = format!("{WIKI_DIR}/{ZO_PAGE_DIR}/");
    corpus::scan(vault)
        .pages
        .iter()
        .filter_map(|page| {
            let entry = page.entry();
            if entry.slug == index {
                return None;
            }
            let claim = pointer_text(&entry.summary);
            Some(Candidate {
                tokens: tokenize(claim),
                zo_written: entry.slug.starts_with(&zo_pages),
                page: VaultPage {
                    slug: entry.slug.clone(),
                    claim: claim.to_string(),
                },
            })
        })
        .collect()
}

/// The relations one promoted page declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PageRelations {
    /// Typed edges, strongest relation first and slug-ordered within a kind.
    typed: Vec<(LessonRelation, String)>,
    /// The untyped overlap fallback, minus every page already typed above.
    related: Vec<String>,
}

/// Reduce one promoted lesson to the evidence the relation judgement reads.
///
/// `replaces` comes from the pass's own audit trail: a candidate skipped as
/// [`SkipReason::SameSummaryTemplate`] naming this lesson is exactly the plan
/// saying "this promotion is the representative — it swaps that one's index
/// summary out".
fn evidence_for(
    lesson: &decision_core::dreamer::PromotedLesson,
    report: &DreamReport,
) -> LessonEvidence {
    let claim = format!("{} {}", lesson.summary, lesson.lesson);
    LessonEvidence {
        slug: lesson.slug.clone(),
        cites: cited_targets(&claim),
        replaces: report
            .plan
            .skipped
            .iter()
            .filter(|skipped| match &skipped.reason {
                SkipReason::SameSummaryTemplate { promoted_slug } => *promoted_slug == lesson.slug,
                _ => false,
            })
            .map(|skipped| skipped.slug.clone())
            .collect(),
        verified: lesson.verified,
        claim,
    }
}

/// Judge `lesson` against every candidate, then fall back to token overlap for
/// the pages no typed relation claimed.
///
/// A page appears under exactly one key. That is the whole point of typing the
/// edge: `related` means "these two share vocabulary", and leaving a superseded
/// page in it as well would say both "this replaces that" and "these merely sit
/// near each other" about the same pair.
fn page_relations(candidates: &[Candidate], lesson: &LessonEvidence) -> PageRelations {
    let mut typed: Vec<(LessonRelation, String)> = Vec::new();
    for candidate in candidates {
        if let Some(relation) = judge_relation(lesson, &candidate.page) {
            typed.push((relation, candidate.page.slug.clone()));
        }
    }
    // Strongest relation first, then by slug, so two passes over one vault
    // write the same frontmatter.
    typed.sort_by(|left, right| {
        relation_rank(left.0)
            .cmp(&relation_rank(right.0))
            .then_with(|| left.1.cmp(&right.1))
    });
    let mut kept: Vec<(LessonRelation, String)> = Vec::new();
    for (relation, slug) in typed {
        if kept.iter().filter(|(kind, _)| *kind == relation).count() < MAX_PROMOTED_TYPED {
            kept.push((relation, slug));
        }
    }
    let related = related_pages(candidates, &kept, lesson);
    PageRelations {
        typed: kept,
        related,
    }
}

fn relation_rank(relation: LessonRelation) -> usize {
    LessonRelation::RANKED
        .iter()
        .position(|ranked| *ranked == relation)
        .unwrap_or(LessonRelation::RANKED.len())
}

/// The title and tags of a pointer summary, without the leading `[[slug]] — `
/// the recall section renders it with. The slug's own words reach the tokens
/// through the title anyway; keeping the link would give every page in the
/// vault the token `wiki`.
fn pointer_text(summary: &str) -> &str {
    summary.split_once("— ").map_or(summary, |(_, rest)| rest)
}

/// Up to [`MAX_PROMOTED_RELATED`] candidates sharing at least
/// [`MIN_RELATED_OVERLAP`] tokens with the lesson, most overlap first and ties
/// by slug so two passes over one vault choose the same neighbours.
///
/// Pages already carrying a typed edge are excluded, and so are zo's own
/// promotions — see [`Candidate::zo_written`].
fn related_pages(
    candidates: &[Candidate],
    typed: &[(LessonRelation, String)],
    lesson: &LessonEvidence,
) -> Vec<String> {
    let words = tokenize(&lesson.claim);
    let mut scored: Vec<(usize, &str)> = candidates
        .iter()
        .filter(|candidate| {
            !candidate.zo_written
                && !typed
                    .iter()
                    .any(|(_, slug)| *slug == candidate.page.slug)
        })
        .filter_map(|candidate| {
            let overlap = candidate.tokens.intersection(&words).count();
            (overlap >= MIN_RELATED_OVERLAP).then_some((overlap, candidate.page.slug.as_str()))
        })
        .collect();
    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    scored
        .into_iter()
        .take(MAX_PROMOTED_RELATED)
        .map(|(_, slug)| slug.to_string())
        .collect()
}

/// A double-quoted YAML scalar. A promoted summary is free-form prose that may
/// carry a colon, a `#`, or a leading `-`, any of which turns a plain scalar
/// into a different value — or into invalid YAML that makes Obsidian show the
/// frontmatter as body text.
fn yaml_scalar(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Append one line per created page, in the vault's documented log shape:
/// `- YYYY-MM-DD HH:MM — <source> → [[page]]` (UTC), followed by the typed
/// edges the promotion drew: `→ [[new]] supersedes [[old]]`.
///
/// The relations belong on the line because the log is what a person reads to
/// see what a session did to the graph. "A page appeared" and "a page appeared
/// that says the page you wrote last month is wrong" are not the same event,
/// and only one of them is worth opening.
fn append_log(
    log_path: &Path,
    session_id: &str,
    stamp: &Stamp,
    pages: &[PromotedPage],
) -> io::Result<()> {
    // Refuse anything that is not already a regular file. The log lives in a
    // directory a person edits, and appending through a symlink is how an
    // automatic writer ends up writing somewhere nobody pointed it at.
    if let Ok(metadata) = fs::symlink_metadata(log_path) {
        if !metadata.file_type().is_file() {
            return Ok(());
        }
    }
    let mut lines = String::new();
    for page in pages {
        let mut relations = String::new();
        for (relation, target) in page.typed.iter().take(MAX_PROMOTED_TYPED) {
            let _ = write!(relations, " {} [[{target}]]", relation.as_str());
        }
        let _ = writeln!(
            lines,
            "- {} {} — {SESSION_SOURCE_PREFIX}{} → [[{}]]{relations}",
            stamp.date,
            stamp.minute,
            single_line(session_id, "unknown"),
            page.link,
        );
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    file.write_all(lines.as_bytes())?;
    file.sync_all()
}

/// Collapse a value to one printable line, so nothing written here can inject a
/// second frontmatter field or a second log entry.
fn single_line(value: &str, fallback: &str) -> String {
    let collapsed: String = value
        .chars()
        .map(|character| if character.is_control() { ' ' } else { character })
        .collect();
    let collapsed = collapsed.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        fallback.to_string()
    } else {
        collapsed
    }
}

#[cfg(test)]
mod relation_tests {
    //! The typed-relation half of the write-back. Hermetic: every test builds
    //! its own vault in a fresh temp directory and reads nothing else — no
    //! `HOME`, no config, no clock.

    use std::fmt::Write as _;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    use decision_core::dreamer::{
        CurationPlan, LessonKind, PromotedLesson, SkipReason, SkippedLesson,
    };

    use super::{corpus, record_lessons, SecondBrain, MAX_PROMOTED_TYPED};
    use crate::memory::{AppliedPromotion, DreamReport, WriteOutcome};

    /// A vault with nothing in it but `wiki/`.
    struct Vault {
        root: PathBuf,
    }

    impl Vault {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "zo-promote-relations-{label}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            fs::create_dir_all(root.join(super::WIKI_DIR)).expect("wiki dir");
            Self { root }
        }

        fn page(&self, relative: &str, summary: &str) {
            let path = self.root.join(super::WIKI_DIR).join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("page parent");
            }
            fs::write(&path, format!("---\ntitle: \"{summary}\"\n---\n\n{summary}\n"))
                .expect("write page");
        }

        fn brain(&self) -> SecondBrain {
            SecondBrain::at(&self.root)
        }

        fn promoted(&self, slug: &str) -> String {
            fs::read_to_string(self.root.join(super::WIKI_DIR).join("zo").join(format!("{slug}.md")))
                .expect("promoted page")
        }

        fn log(&self) -> String {
            fs::read_to_string(self.root.join(super::WIKI_DIR).join("log.md")).expect("log")
        }

        fn record(&self, report: &DreamReport) -> super::WikiPromotion {
            record_lessons(
                &self.brain(),
                "sess-rel",
                report,
                SystemTime::UNIX_EPOCH + Duration::from_secs(1_772_000_000),
            )
            .expect("write-back")
        }
    }

    impl Drop for Vault {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn lesson(slug: &str, summary: &str, body: &str, verified: bool) -> PromotedLesson {
        PromotedLesson {
            slug: slug.to_string(),
            summary: summary.to_string(),
            lesson: body.to_string(),
            kind: LessonKind::Workflow,
            distinct_sessions: 2,
            verified,
            confidence: 0.9,
            expiry_days: 0,
        }
    }

    fn report(promote: Vec<PromotedLesson>, skipped: Vec<SkippedLesson>) -> DreamReport {
        DreamReport {
            applied: promote
                .iter()
                .map(|lesson| AppliedPromotion {
                    slug: lesson.slug.clone(),
                    outcome: WriteOutcome::Created,
                })
                .collect(),
            plan: CurationPlan { promote, skipped },
        }
    }

    #[test]
    fn a_typed_relation_replaces_the_untyped_one_for_the_same_page() {
        let vault = Vault::new("typed");
        // Shares vocabulary with the lesson, so the old code would have filed
        // it as `related` — and cites it, so the new code types the edge.
        vault.page(
            "gotcha-locale-grep.md",
            "grep on a Korean bundle exits 1 and prints nothing",
        );

        vault.record(&report(
            vec![lesson(
                "workflow-locale-grep",
                "Search bundles with `LC_ALL=C grep -a`",
                "The plain grep on a Korean bundle exits 1 and prints nothing; \
                 see [[wiki/gotcha-locale-grep]].",
                true,
            )],
            Vec::new(),
        ));

        let page = vault.promoted("workflow-locale-grep");
        assert!(
            page.contains("depends_on: [ [[wiki/gotcha-locale-grep]] ]"),
            "the cited page is a typed edge: {page}"
        );
        assert!(
            !page.contains("related:"),
            "a page under a typed key must not also be a vague neighbour: {page}"
        );
        assert!(
            page.contains("관련: [[wiki/index]] · [[wiki/gotcha-locale-grep]]"),
            "the reader's link line still names it once: {page}"
        );
    }

    #[test]
    fn an_untyped_neighbour_is_still_filed_as_related() {
        let vault = Vault::new("untyped");
        vault.page(
            "gotcha-locale-grep.md",
            "grep on a Korean bundle exits 1 and prints nothing",
        );

        vault.record(&report(
            vec![lesson(
                "workflow-locale-grep",
                "grep on a bundle",
                "The plain grep on a Korean bundle prints nothing.",
                false,
            )],
            Vec::new(),
        ));

        let page = vault.promoted("workflow-locale-grep");
        assert!(
            page.contains("related: [ [[wiki/gotcha-locale-grep]] ]"),
            "token overlap with no typed relation is still a neighbour: {page}"
        );
    }

    #[test]
    fn the_curation_plan_names_the_page_a_promotion_supersedes() {
        let vault = Vault::new("supersede");
        vault.page("zo/workflow-check-old.md", "the earlier phrasing");

        vault.record(&report(
            vec![lesson(
                "workflow-check-new",
                "Verify with `just verify`",
                "Run the workspace gate before calling a change done.",
                true,
            )],
            vec![SkippedLesson {
                slug: "workflow-check-old".to_string(),
                summary: "the earlier phrasing".to_string(),
                reason: SkipReason::SameSummaryTemplate {
                    promoted_slug: "workflow-check-new".to_string(),
                },
            }],
        ));

        let page = vault.promoted("workflow-check-new");
        assert!(
            page.contains("supersedes: [ [[wiki/zo/workflow-check-old]] ]"),
            "{page}"
        );
        assert!(
            vault
                .log()
                .contains("→ [[wiki/zo/workflow-check-new]] supersedes [[wiki/zo/workflow-check-old]]"),
            "the log line says what the promotion did to the graph: {}",
            vault.log()
        );
        assert!(
            fs::metadata(vault.root.join("wiki/zo/workflow-check-old.md")).is_ok(),
            "the superseded page is pointed at, never deleted"
        );
    }

    #[test]
    fn verified_evidence_contradicts_a_page_it_refutes() {
        let vault = Vault::new("contradict");
        vault.page(
            "gotcha-worktree-target.md",
            "cargo test on runtime fails without its own target dir",
        );

        vault.record(&report(
            vec![lesson(
                "workflow-worktree-target",
                "cargo test on runtime passes with a shared target dir",
                "Two worktrees now share one target dir and the runtime suite passes.",
                true,
            )],
            Vec::new(),
        ));

        let page = vault.promoted("workflow-worktree-target");
        assert!(
            page.contains("contradicts: [ [[wiki/gotcha-worktree-target]] ]"),
            "{page}"
        );
        assert!(
            vault.log().contains("contradicts [[wiki/gotcha-worktree-target]]"),
            "{}",
            vault.log()
        );
    }

    #[test]
    fn zo_pages_are_never_untyped_neighbours() {
        let vault = Vault::new("zo-pages");
        vault.page(
            "zo/gotcha-locale-grep.md",
            "grep on a Korean bundle exits 1 and prints nothing",
        );

        vault.record(&report(
            vec![lesson(
                "workflow-something-else",
                "grep on a Korean bundle prints nothing",
                "Unrelated body.",
                false,
            )],
            Vec::new(),
        ));

        let page = vault.promoted("workflow-something-else");
        assert!(
            !page.contains("related:"),
            "promoted pages link vault knowledge, not each other: {page}"
        );
    }

    #[test]
    fn typed_targets_are_bounded_and_ordered() {
        let vault = Vault::new("bounded");
        let mut skipped = Vec::new();
        for index in 0..5 {
            vault.page(&format!("zo/workflow-old-{index}.md"), "an earlier phrasing");
            skipped.push(SkippedLesson {
                slug: format!("workflow-old-{index}"),
                summary: "an earlier phrasing".to_string(),
                reason: SkipReason::SameSummaryTemplate {
                    promoted_slug: "workflow-new".to_string(),
                },
            });
        }

        vault.record(&report(
            vec![lesson("workflow-new", "the new phrasing", "body", true)],
            skipped,
        ));

        let page = vault.promoted("workflow-new");
        let line = page
            .lines()
            .find(|line| line.starts_with("supersedes:"))
            .expect("supersedes key");
        assert_eq!(
            line.matches("[[").count(),
            MAX_PROMOTED_TYPED,
            "a page superseding nine others is a claim nobody can check: {line}"
        );
        assert!(
            line.contains("workflow-old-0") && line.contains("workflow-old-2"),
            "ties break by slug so two passes write the same bytes: {line}"
        );
    }

    #[test]
    fn a_pass_with_no_relations_writes_exactly_what_it_wrote_before() {
        let vault = Vault::new("noop");
        vault.record(&report(
            vec![lesson("workflow-alone", "a lonely lesson", "body", true)],
            Vec::new(),
        ));

        let page = vault.promoted("workflow-alone");
        assert!(
            !page.contains("related:") && !page.contains("depends_on:"),
            "an empty key claims the page has no neighbours, which is not what \
             `none matched` means: {page}"
        );
        assert!(page.contains("관련: [[wiki/index]]\n"), "{page}");
        assert!(
            vault.log().trim_end().ends_with("→ [[wiki/zo/workflow-alone]]"),
            "no relations, no suffix: {}",
            vault.log()
        );
    }

    #[test]
    fn the_typed_spellings_are_the_scanners_own_keys() {
        // The only place both vocabularies are in scope. A graph that reads
        // `depends_on` and a writer that emits `dependsOn` fail silently.
        for (relation, kind) in [
            (
                super::LessonRelation::DependsOn,
                corpus::RelationKind::DependsOn,
            ),
            (
                super::LessonRelation::Supersedes,
                corpus::RelationKind::Supersedes,
            ),
            (
                super::LessonRelation::Contradicts,
                corpus::RelationKind::Contradicts,
            ),
        ] {
            assert_eq!(relation.as_str(), kind.as_str());
        }
        assert_eq!(
            super::RELATED_RELATION_KEY,
            corpus::RelationKind::Related.as_str()
        );
    }

    #[test]
    fn a_written_relation_is_read_back_by_the_scanner() {
        let vault = Vault::new("roundtrip");
        vault.page("zo/workflow-check-old.md", "the earlier phrasing");
        vault.record(&report(
            vec![lesson(
                "workflow-check-new",
                "Verify with `just verify`",
                "Run the workspace gate before calling a change done.",
                true,
            )],
            vec![SkippedLesson {
                slug: "workflow-check-old".to_string(),
                summary: "the earlier phrasing".to_string(),
                reason: SkipReason::SameSummaryTemplate {
                    promoted_slug: "workflow-check-new".to_string(),
                },
            }],
        ));

        let scan = corpus::scan(&vault.brain());
        let page = scan
            .pages
            .iter()
            .find(|page| page.entry().slug == "wiki/zo/workflow-check-new")
            .expect("the promoted page is in the scan");
        assert!(
            page.relations().iter().any(|relation| {
                relation.kind == corpus::RelationKind::Supersedes
                    && relation.target == "wiki/zo/workflow-check-old"
                    && relation.resolved
            }),
            "the edge this pass wrote must be the edge the graph reads: {:?}",
            page.relations()
        );
    }

    /// The measurement §4 asks for: of every page a promotion pass names, how
    /// many carry a typed edge and how many are still the untyped fallback.
    ///
    /// "Before" needs no separate run — before this change every named page was
    /// `related`, so the untyped column IS the old behaviour and the typed
    /// column is what the graph gained. Printed as a table (`--nocapture`) and
    /// asserted, so the number in the report and the number in the gate are the
    /// same number.
    #[test]
    // 픽스처 볼트 하나를 세우고 표까지 찍는 한 편의 측정이라 쪼개면 숫자가
    // 어디서 나왔는지 읽을 수 없다.
    #[allow(clippy::too_many_lines)]
    fn relation_ratio_over_a_fixture_vault() {
        let vault = Vault::new("ratio");
        // Twelve pages a person wrote, six of them about things the lessons
        // below actually touch.
        for (slug, summary) in [
            ("gotcha-locale-grep", "grep on a Korean bundle exits 1 and prints nothing"),
            ("gotcha-worktree-target", "cargo test on runtime fails without its own target dir"),
            ("shell-pipelines", "a pipeline reports its last command's status"),
            ("cargo-workspaces", "one target dir per worktree, never shared"),
            ("obsidian-frontmatter", "typed keys are read by the graph view"),
            ("terminal-copy-paste", "shift drag over a tracking TUI selects locally"),
            ("prompt-cache", "the static prefix is cached for an hour"),
            ("memory-recall", "token overlap ranks the recalled entries"),
            ("pty-grid", "the alternate screen keeps no scrollback"),
            ("model-catalog", "the registry gates its list by client version"),
            ("ledger-mail", "a dead lease strands the mail behind it"),
            ("second-brain", "one atomic page per concept, linked both ways"),
        ] {
            vault.page(&format!("{slug}.md"), summary);
        }
        vault.page("zo/workflow-check-old.md", "the earlier phrasing of the gate");

        let lessons = vec![
            // depends_on — the evidence cites a page.
            lesson(
                "gotcha-bundle-search",
                "Search bundles with `LC_ALL=C grep -a`",
                "The plain grep on a Korean bundle exits 1 and prints nothing; see \
                 [[wiki/gotcha-locale-grep]].",
                true,
            ),
            // contradicts — verified evidence against a page's claim.
            lesson(
                "workflow-shared-target",
                "cargo test on runtime passes with a shared target dir",
                "Two worktrees now share one target dir and the runtime suite passes.",
                true,
            ),
            // supersedes — named by the pass's own audit trail.
            lesson(
                "workflow-check-new",
                "Verify with `just verify`",
                "Run the workspace gate before calling a change done.",
                true,
            ),
            // related — overlap and nothing more.
            lesson(
                "gotcha-alternate-screen",
                "the alternate screen keeps no scrollback",
                "An agent pane runs on the alternate screen, so its scrollback never fills.",
                false,
            ),
            lesson(
                "preference-typed-keys",
                "typed keys are read by the graph view",
                "Write the relation as a frontmatter key; the graph view reads those keys.",
                false,
            ),
            // Nothing in the vault to file against.
            lesson("constraint-nothing-nearby", "an unshared claim", "Nothing here overlaps.", true),
        ];
        let skipped = vec![SkippedLesson {
            slug: "workflow-check-old".to_string(),
            summary: "the earlier phrasing of the gate".to_string(),
            reason: SkipReason::SameSummaryTemplate {
                promoted_slug: "workflow-check-new".to_string(),
            },
        }];

        let promotion = vault.record(&report(lessons, skipped));
        assert_eq!(promotion.pages.len(), 6);

        let mut typed = 0usize;
        let mut untyped = 0usize;
        let mut by_kind: Vec<(String, usize)> = Vec::new();
        let mut rows = String::new();
        for page in &promotion.pages {
            let body = vault.promoted(&page.slug);
            let count = |key: &str| {
                body.lines()
                    .find(|line| line.starts_with(&format!("{key}: ")))
                    .map_or(0, |line| line.matches("[[").count())
            };
            let related = count(super::RELATED_RELATION_KEY);
            let page_typed: usize = super::TYPED_KEY_ORDER
                .iter()
                .map(|relation| {
                    let n = count(relation.as_str());
                    if n > 0 {
                        by_kind.push((relation.as_str().to_string(), n));
                    }
                    n
                })
                .sum();
            typed += page_typed;
            untyped += related;
            let _ = writeln!(
                rows,
                "  {:<28} typed {page_typed}  related {related}",
                page.slug
            );
        }

        let named = typed + untyped;
        eprintln!(
            "\n[relation ratio] {} pages promoted against a 13-page vault\n{rows}  \
             ----\n  typed {typed} / {named} named pages ({}%), untyped {untyped}\n  by kind: {by_kind:?}\n",
            promotion.pages.len(),
            typed * 100 / named.max(1)
        );

        assert!(named > 0, "the fixture must name something");
        assert!(
            typed >= 3,
            "all three typed relations must appear: {by_kind:?}"
        );
        assert!(
            untyped > 0,
            "and the untyped fallback must survive for the pages nothing else claims"
        );
    }
}
