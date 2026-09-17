//! Hermetic tests for the second brain.
//!
//! Every test runs against a vault and a Zo home it created in a fresh
//! temporary directory, with `HOME`, `ZO_CONFIG_HOME` and `ZO_HOME` all
//! redirected there. Redirecting only `ZO_CONFIG_HOME` would not be enough:
//! [`core_types::paths::zo_global_config_roots`] *appends* `$HOME/.zo` and
//! `$HOME/.forge`, so a test that left `HOME` alone would read the developer's
//! own machine.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use core_types::MemoryRetriever;
use decision_core::dreamer::{CurationPlan, LessonKind, PromotedLesson};

use super::{corpus, promote, SecondBrain, VAULT_ENV};
use crate::config::ConfigLoader;
use crate::memory::{AppliedPromotion, DreamReport, WriteOutcome};

/// A temporary world: a vault, a Zo home, and a workspace outside both.
struct World {
    root: PathBuf,
    vault: PathBuf,
    workspace: PathBuf,
    _guard: EnvGuard,
}

impl World {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "zo-second-brain-{label}-{}-{:?}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let vault = root.join("vault");
        let workspace = root.join("workspace");
        fs::create_dir_all(vault.join(super::WIKI_DIR)).expect("wiki dir");
        fs::create_dir_all(vault.join(super::RAW_DIR)).expect("raw dir");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::create_dir_all(root.join("home")).expect("home");
        let guard = EnvGuard::redirect(&root);
        Self {
            root,
            vault,
            workspace,
            _guard: guard,
        }
    }

    fn page(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.vault.join(super::WIKI_DIR).join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("page parent");
        }
        fs::write(&path, contents).expect("write page");
        path
    }

    fn brain(&self) -> SecondBrain {
        SecondBrain::at(&self.vault)
    }

    /// Point `ZEROCODE_SECOND_BRAIN` at this world's vault.
    fn export_vault(&self) {
        std::env::set_var(VAULT_ENV, &self.vault);
    }

    fn write_settings(&self, json: &str) {
        let dir = self.root.join("zo-global");
        fs::create_dir_all(&dir).expect("zo home");
        fs::write(dir.join("settings.json"), json).expect("settings");
    }

    fn config(&self) -> crate::config::RuntimeConfig {
        ConfigLoader::default_for(&self.workspace)
            .load()
            .expect("settings should load")
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Holds the process env lock for the test and restores every variable it
/// touched, so a panicking test cannot leak a redirected `HOME` into the next.
struct EnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    fn redirect(root: &Path) -> Self {
        let lock = crate::test_env_lock();
        let keys = ["HOME", "ZO_CONFIG_HOME", "ZO_HOME", VAULT_ENV];
        let previous = keys
            .iter()
            .map(|key| (*key, std::env::var_os(key)))
            .collect();
        std::env::set_var("HOME", root.join("home"));
        std::env::set_var("ZO_CONFIG_HOME", root.join("zo-global"));
        std::env::set_var("ZO_HOME", root.join("absent-zo-home"));
        std::env::remove_var(VAULT_ENV);
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

#[test]
fn no_vault_configured_leaves_the_feature_entirely_off() {
    let world = World::new("off");
    assert!(SecondBrain::from_env().is_none());
    assert!(SecondBrain::resolve(&world.config()).is_none());
    assert!(super::status(&world.config()).is_none());
}

#[test]
fn the_settings_vault_is_read_and_the_environment_overrides_it() {
    let world = World::new("resolve");
    let settings_vault = world.root.join("from-settings");
    world.write_settings(&format!(
        r#"{{"secondBrain":{{"vault":"{}"}}}}"#,
        settings_vault.display()
    ));
    assert_eq!(
        SecondBrain::resolve(&world.config()),
        Some(SecondBrain::at(&settings_vault)),
        "settings supply the vault when the environment is silent"
    );

    world.export_vault();
    assert_eq!(
        SecondBrain::resolve(&world.config()),
        Some(world.brain()),
        "an exported vault wins over the settings one"
    );
}

#[test]
fn off_in_the_environment_turns_off_even_the_settings_vault() {
    let world = World::new("off-switch");
    world.write_settings(&format!(
        r#"{{"secondBrain":{{"vault":"{}"}}}}"#,
        world.vault.display()
    ));
    std::env::set_var(VAULT_ENV, "");
    assert!(
        SecondBrain::resolve(&world.config()).is_some(),
        "a blank value is unset: settings fill it"
    );
    for word in [super::VAULT_OFF, " OFF "] {
        std::env::set_var(VAULT_ENV, word);
        assert!(SecondBrain::from_env().is_none());
        assert!(SecondBrain::resolve(&world.config()).is_none(), "{word:?}");
        super::publish_vault_from_config(&world.config());
        assert_eq!(
            std::env::var(VAULT_ENV).ok().as_deref(),
            Some(word),
            "the startup publish keeps the word"
        );
    }
}

#[test]
fn a_relative_vault_path_is_refused_rather_than_joined_onto_the_cwd() {
    let world = World::new("relative");
    std::env::set_var(VAULT_ENV, "vault");
    assert!(SecondBrain::from_env().is_none());
    world.write_settings(r#"{"secondBrain":{"vault":"./vault"}}"#);
    std::env::remove_var(VAULT_ENV);
    assert!(SecondBrain::resolve(&world.config()).is_none());
}

#[test]
fn publishing_the_settings_vault_never_rewrites_an_operator_export() {
    let world = World::new("publish");
    world.write_settings(&format!(
        r#"{{"secondBrain":{{"vault":"{}"}}}}"#,
        world.vault.display()
    ));
    super::publish_vault_from_config(&world.config());
    assert_eq!(
        std::env::var(VAULT_ENV).ok(),
        Some(world.vault.display().to_string())
    );

    let operator = world.root.join("operator-vault");
    std::env::set_var(VAULT_ENV, &operator);
    super::publish_vault_from_config(&world.config());
    assert_eq!(
        std::env::var(VAULT_ENV).ok(),
        Some(operator.display().to_string()),
        "an export the operator made is a decision, not a suggestion"
    );
}

#[test]
fn write_back_defaults_on_and_a_boolean_turns_it_off() {
    let world = World::new("write-back-setting");
    assert!(world.config().second_brain().write_back());
    world.write_settings(r#"{"secondBrain":{"writeBack":false}}"#);
    assert!(!world.config().second_brain().write_back());
    world.write_settings(r#"{"secondBrain":{"writeBack":"no"}}"#);
    let error = ConfigLoader::default_for(&world.workspace)
        .load()
        .expect_err("a non-boolean writeBack is reported, not read as false");
    assert!(format!("{error:?}").contains("writeBack"), "{error:?}");
}

// ---------------------------------------------------------------------------
// Corpus
// ---------------------------------------------------------------------------

#[test]
fn a_page_is_indexed_by_its_title_tags_and_body_head() {
    let world = World::new("corpus-shape");
    world.page(
        "zo/pty-grid.md",
        "---\ntitle: pty grid footprint\ntags: [measurement, terminal]\n---\n\nHeap histograms are the honest instrument for grid size.\n",
    );
    world.page("attachments/diagram.png", "not markdown");

    let scan = corpus::scan(&world.brain());
    assert_eq!(scan.pages.len(), 1, "only Markdown pages are indexed");
    assert!(!scan.capped);
    let entry = scan.pages[0].entry();
    assert_eq!(entry.slug, "wiki/zo/pty-grid");
    assert_eq!(
        entry.summary,
        "[[wiki/zo/pty-grid]] — pty grid footprint · tags: measurement, terminal",
        "the pointer names the wikilink a reader would follow"
    );
    assert_eq!(entry.path, world.vault.join("wiki/zo/pty-grid.md").display().to_string());
}

#[test]
fn a_page_without_frontmatter_falls_back_to_its_file_name() {
    let world = World::new("corpus-fallback");
    world.page("obsidian_sync.md", "Sync is a folder, not a service.\n");
    let scan = corpus::scan(&world.brain());
    assert_eq!(
        scan.pages[0].entry().summary,
        "[[wiki/obsidian_sync]] — obsidian sync"
    );
}

#[test]
fn an_unchanged_page_is_reused_and_a_touched_one_is_reread() {
    let world = World::new("corpus-cache");
    let path = world.page("cache.md", "---\ntitle: first\n---\n\nbody\n");
    let first = corpus::scan(&world.brain()).pages;
    let second = corpus::scan(&world.brain()).pages;
    assert!(
        Arc::ptr_eq(&first[0], &second[0]),
        "an unchanged page costs one stat, not a re-read"
    );

    // A rewrite that also changes the length, so the (mtime, len) stamp moves
    // even on a filesystem with coarse timestamps.
    fs::write(&path, "---\ntitle: second reading\n---\n\nbody\n").expect("rewrite");
    let third = corpus::scan(&world.brain()).pages;
    assert!(!Arc::ptr_eq(&first[0], &third[0]));
    assert_eq!(
        third[0].entry().summary,
        "[[wiki/cache]] — second reading"
    );
}

#[test]
fn the_walk_stops_at_the_page_cap_and_says_so() {
    let world = World::new("corpus-cap");
    for index in 0..(corpus::MAX_INDEXED_PAGES + 5) {
        world.page(&format!("page-{index:05}.md"), "body\n");
    }
    let scan = corpus::scan(&world.brain());
    assert_eq!(scan.pages.len(), corpus::MAX_INDEXED_PAGES);
    assert!(scan.capped, "a capped scan reports the cap rather than lying");
}

#[test]
fn only_the_capped_head_of_a_page_reaches_the_index() {
    let world = World::new("corpus-byte-cap");
    let mut body = String::from("---\ntitle: long\n---\n\n");
    body.push_str(&"filler ".repeat(corpus::MAX_PAGE_INDEX_BYTES / 7 + 64));
    body.push_str(" tailmarker\n");
    world.page("long.md", &body);
    world.export_vault();

    let retriever = crate::memory::load_lexical_memory_retriever(&world.workspace, None)
        .expect("a vault alone is enough to build a retriever");
    assert!(
        retriever.recall("filler", 5).iter().any(|hit| hit.entry.slug == "wiki/long"),
        "the head of the page is indexed"
    );
    assert!(
        retriever.recall("tailmarker", 5).is_empty(),
        "the tail past the byte cap is not read, so it cannot be recalled"
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_directory_is_not_followed_out_of_the_vault() {
    let world = World::new("corpus-symlink");
    let outside = world.root.join("outside");
    fs::create_dir_all(&outside).expect("outside dir");
    fs::write(outside.join("secret.md"), "secret body\n").expect("outside page");
    std::os::unix::fs::symlink(&outside, world.vault.join("wiki").join("linked"))
        .expect("symlink");
    world.page("real.md", "real body\n");

    let scan = corpus::scan(&world.brain());
    let slugs: Vec<&str> = scan.pages.iter().map(|page| page.entry().slug.as_str()).collect();
    assert_eq!(slugs, vec!["wiki/real"]);
}

// ---------------------------------------------------------------------------
// Relations
// ---------------------------------------------------------------------------

/// Every relation of one page as `kind target[?]`, `?` marking an unresolved
/// target — the whole outgoing half of the graph in one readable line.
fn relations_of(scan: &corpus::CorpusScan, slug: &str) -> Vec<String> {
    scan.pages
        .iter()
        .find(|page| page.entry().slug == slug)
        .expect("page")
        .relations()
        .iter()
        .map(|relation| {
            format!(
                "{} {}{}",
                relation.kind.as_str(),
                relation.target,
                if relation.resolved { "" } else { "?" }
            )
        })
        .collect()
}

#[test]
fn every_frontmatter_relation_form_and_body_link_is_read() {
    let world = World::new("relations-forms");
    world.page("adr/003.md", "an accepted decision\n");
    world.page("old-decision.md", "what came before\n");
    world.page("a.md", "a\n");
    world.page("b.md", "b\n");
    world.page("x.md", "x\n");
    world.page("y.md", "y\n");
    world.page(
        "hub.md",
        "---\n\
         title: hub\n\
         implements: [[wiki/adr/003]]\n\
         supersedes: \"[[wiki/old-decision|옛 결정]]\"\n\
         depends_on: [ [[wiki/a]], b ]\n\
         related:\n\
         \x20 - [[wiki/x#section]]\n\
         \x20 - y\n\
         ---\n\
         \n\
         The body mentions [[wiki/a]] and embeds ![[b]].\n\
         \n\
         ```\n\
         A fenced [[wiki/x]] is documentation, not a link.\n\
         ```\n\
         \n\
         So is an inline `[[wiki/y]]` span.\n",
    );

    let scan = corpus::scan(&world.brain());
    assert_eq!(
        relations_of(&scan, "wiki/hub"),
        vec![
            // Body links first, in written order; a fenced or inline-code link
            // is documentation and contributes nothing.
            "mentions wiki/a",
            "mentions wiki/b",
            // Then the typed keys, in the shared contract's order.
            "related wiki/x",
            "related wiki/y",
            "implements wiki/adr/003",
            "depends_on wiki/a",
            "depends_on wiki/b",
            "supersedes wiki/old-decision",
        ],
        "each accepted form parses to its kind and its bare target"
    );

    assert_eq!(
        scan.incoming.get("wiki/adr/003").map(Vec::as_slice),
        Some(&[(corpus::RelationKind::Implements, "wiki/hub".to_string())][..]),
        "the reverse index names who points at a page, and how"
    );
}

#[test]
fn a_target_resolves_by_stem_and_an_unresolved_one_is_still_shown() {
    let world = World::new("relations-resolution");
    world.page("deep/nested/Design.md", "the real page\n");
    world.page(
        "pointer.md",
        "By stem [[Design]], by case [[design]], and [[never-written]].\n",
    );

    let scan = corpus::scan(&world.brain());
    assert_eq!(
        relations_of(&scan, "wiki/pointer"),
        vec![
            "mentions wiki/deep/nested/Design",
            "mentions wiki/deep/nested/Design",
            "mentions never-written?",
        ],
        "a stem and its lower-cased form both find the page; a target nothing \
         answers to is kept as written and marked unresolved"
    );
    assert!(
        !scan.incoming.contains_key("never-written"),
        "an unresolved target is displayed, never ranked: {:?}",
        scan.incoming
    );
}

#[test]
fn the_pointer_line_names_the_pages_company_within_the_summary_cap() {
    let world = World::new("relations-summary");
    world.page("one.md", "one\n");
    world.page("two.md", "two\n");
    world.page("three.md", "three\n");
    world.page("four.md", "four\n");
    world.page(
        "company.md",
        "---\ntitle: company\nrelated: [[[wiki/one]], [[wiki/two]]]\n---\n\nAnd [[wiki/three]], [[wiki/four]].\n",
    );
    // A title long enough that the base summary leaves no room for a suffix,
    // while still fitting inside the renderer's own cap.
    let long_title = "가".repeat(190);
    world.page(
        "verbose.md",
        &format!("---\ntitle: {long_title}\nrelated: [[wiki/one]]\n---\n\n[[wiki/two]]\n"),
    );

    let scan = corpus::scan(&world.brain());
    let summary = |slug: &str| {
        scan.pages
            .iter()
            .find(|page| page.entry().slug == slug)
            .expect("page")
            .entry()
            .summary
            .clone()
    };

    assert_eq!(
        summary("wiki/company"),
        "[[wiki/company]] — company · related: [[wiki/one]] · related: [[wiki/two]] \
         · mentions: [[wiki/three]]",
        "typed relations take the room first, and only three suffixes are shown"
    );
    assert!(
        summary("wiki/company").len() <= crate::memory::recall::CORPUS_SUMMARY_MAX_BYTES,
        "a suffixed summary stays under the cap that keeps the renderer's \
         truncation away from a `[[…]]`"
    );

    let verbose = summary("wiki/verbose");
    assert!(
        !verbose.contains(" · related:") && !verbose.contains(" · mentions:"),
        "a suffix that would not fit is dropped whole rather than half-written: {verbose}"
    );
    assert!(
        verbose.len() + " · related: [[wiki/one]]".len()
            > crate::memory::recall::CORPUS_SUMMARY_MAX_BYTES,
        "and the check is not vacuous — the suffix really was unaffordable ({} bytes)",
        verbose.len()
    );
}

#[test]
fn an_unchanged_vault_is_relinked_without_being_reread() {
    let world = World::new("relations-cache");
    world.page("target.md", "---\ntitle: target\n---\n\nbody\n");
    world.page("source.md", "---\ntitle: source\n---\n\nSee [[target]].\n");

    let first = corpus::scan(&world.brain());
    let second = corpus::scan(&world.brain());
    for (before, after) in first.pages.iter().zip(&second.pages) {
        assert!(
            Arc::ptr_eq(before, after),
            "an unchanged page keeps its tokens AND its links, so nothing is re-read"
        );
    }
    assert_eq!(
        second.incoming, first.incoming,
        "the reverse index is rebuilt from the cached pages rather than lost with them"
    );
    assert_eq!(
        second.incoming.get("wiki/target").map(Vec::as_slice),
        Some(&[(corpus::RelationKind::Mentions, "wiki/source".to_string())][..])
    );
}

#[test]
fn a_page_written_later_relinks_the_page_that_already_pointed_at_it() {
    let world = World::new("relations-late");
    world.page("early.md", "---\ntitle: early\n---\n\nSee [[late]].\n");
    let first = corpus::scan(&world.brain());
    assert_eq!(relations_of(&first, "wiki/early"), vec!["mentions late?"]);

    world.page("late.md", "---\ntitle: late\n---\n\nwritten afterwards\n");
    let second = corpus::scan(&world.brain());
    assert_eq!(
        relations_of(&second, "wiki/early"),
        vec!["mentions wiki/late"],
        "resolution is a whole-vault fact, so a cached page is re-linked when a \
         neighbour appears"
    );
    assert!(second.incoming.contains_key("wiki/late"));
}

// ---------------------------------------------------------------------------
// Recall
// ---------------------------------------------------------------------------

#[test]
fn a_vault_page_is_recalled_and_rendered_with_its_wikilink_and_snippet() {
    let world = World::new("recall");
    world.page(
        "zo/webkit-leak.md",
        "---\ntitle: WebKit spinner leak\ntags: [gotcha]\n---\n\nRotateTransformFunction objects outlive the spinner that made them.\n",
    );
    world.export_vault();

    let retriever = crate::memory::load_lexical_memory_retriever(&world.workspace, None)
        .expect("retriever");
    let hits = retriever.recall("spinner leak", 5);
    assert_eq!(hits.len(), 1, "the vault page answers a vault question");

    let section = crate::memory::render_recalled_memory_section(&hits).expect("section");
    assert!(section.contains("[[wiki/zo/webkit-leak]]"), "{section}");
    assert!(
        section.contains("RotateTransformFunction"),
        "the snippet reads the authorized page: {section}"
    );
    assert!(
        u64::try_from(section.chars().count() / 4 + 1).unwrap_or(u64::MAX)
            <= crate::memory::recall::recall_section_reserve_tokens(),
        "the rendered section must stay inside the reserve the preflight holds"
    );
}

#[test]
fn a_page_outside_the_configured_vault_is_never_opened_for_a_snippet() {
    let world = World::new("recall-authz");
    let outside = world.root.join("outside");
    fs::create_dir_all(&outside).expect("outside");
    let stray = outside.join("stray.md");
    fs::write(&stray, "SECRET STRAY BODY\n").expect("stray");
    world.export_vault();

    let hits = vec![core_types::MemoryHit {
        entry: core_types::MemoryEntry {
            slug: "wiki/stray".to_string(),
            path: stray.display().to_string(),
            summary: "[[wiki/stray]] — stray".to_string(),
        },
        score: 9,
    }];
    let section = crate::memory::render_recalled_memory_section(&hits).expect("section");
    assert!(
        !section.contains("SECRET STRAY BODY"),
        "authorization is by location, not by what the pointer claims: {section}"
    );
}

#[test]
fn recall_without_a_vault_is_byte_identical_to_what_it_was() {
    let world = World::new("recall-off");
    world.page("zo/unused.md", "---\ntitle: unused\n---\n\nnobody asked\n");
    // No export: the vault exists on disk but nothing points zo at it.
    assert!(
        crate::memory::load_lexical_memory_retriever(&world.workspace, None).is_none(),
        "an unconfigured vault costs nothing and contributes nothing"
    );
}

/// The slugs recalled for `query`, in rank order.
fn recalled(world: &World, query: &str) -> Vec<String> {
    crate::memory::load_lexical_memory_retriever(&world.workspace, None)
        .expect("retriever")
        .recall(query, 5)
        .into_iter()
        .map(|hit| hit.entry.slug)
        .collect()
}

#[test]
fn a_neighbour_of_the_best_hit_outranks_a_stranger_that_scored_the_same() {
    let world = World::new("recall-neighbour");
    // Three pages carrying the same query word, so lexical scoring ties them
    // and only the graph can order them. `anchor` sorts first, so it is the
    // best hit; `neighbour` is one hop from it and `stranger` is not.
    world.page("anchor.md", "---\ntitle: anchor\n---\n\nquartzbeam telemetry.\n");
    world.page(
        "neighbour.md",
        "---\ntitle: neighbour\n---\n\nquartzbeam telemetry, from [[anchor]].\n",
    );
    world.page(
        "stranger.md",
        "---\ntitle: stranger\n---\n\nquartzbeam telemetry, alone.\n",
    );
    world.export_vault();

    // The boost lands on the best hit's COMPANY, never on the best hit itself,
    // so a neighbour tied on words rises past the page it was measured from —
    // which is the point: the two together are the answer, and the one that
    // also says why is worth reading first. A body link buys exactly this and
    // nothing more; see the two tests below for what a typed relation buys.
    assert_eq!(
        recalled(&world, "quartzbeam"),
        vec!["wiki/neighbour", "wiki/anchor", "wiki/stranger"],
        "a link to the best hit breaks the tie the words could not"
    );
}

#[test]
fn a_page_only_mentioned_in_prose_is_never_admitted_by_the_link() {
    let world = World::new("recall-no-admit");
    world.page(
        "anchor.md",
        "---\ntitle: anchor\n---\n\nquartzbeam telemetry, see [[silent]].\n",
    );
    world.page(
        "silent.md",
        "---\ntitle: silent\n---\n\nnothing about the question at all.\n",
    );
    world.export_vault();

    assert_eq!(
        recalled(&world, "quartzbeam"),
        vec!["wiki/anchor"],
        "a body link says two pages are about each other, never that either is \
         about the query"
    );
}

#[test]
fn a_typed_relation_does_admit_the_page_the_words_could_never_reach() {
    let world = World::new("recall-typed-admit");
    // The same shape as the test above, with one word changed: the link is now
    // a frontmatter `depends_on` rather than a sentence. A person wrote that on
    // purpose, so it carries the answer across a query the answer shares no
    // word with — which is the whole point of the vault having a graph.
    world.page(
        "anchor.md",
        "---\ntitle: anchor\ndepends_on: [[wiki/silent]]\n---\n\nquartzbeam telemetry.\n",
    );
    world.page(
        "silent.md",
        "---\ntitle: silent\n---\n\nnothing about the question at all.\n",
    );
    world.export_vault();

    assert_eq!(
        recalled(&world, "quartzbeam"),
        vec!["wiki/anchor", "wiki/silent"],
        "the dependency of the best answer arrives with it"
    );
    let summary = crate::memory::load_lexical_memory_retriever(&world.workspace, None)
        .expect("retriever")
        .recall("quartzbeam", 5)
        .into_iter()
        .find(|hit| hit.entry.slug == "wiki/silent")
        .expect("the dependency")
        .entry
        .summary;
    assert!(
        summary.ends_with("· ← depends_on of [[wiki/anchor]]"),
        "and says which edge carried it: {summary}"
    );
}

#[test]
fn a_superseded_page_ranks_below_the_page_that_replaced_it() {
    let world = World::new("recall-superseded");
    // `new-decision` sorts after `old-decision`, so without the demotion the
    // slug tie-break would put the stale page first.
    world.page(
        "old-decision.md",
        "---\ntitle: old decision\n---\n\nzephyrline routing is manual.\n",
    );
    world.page(
        "new-decision.md",
        "---\ntitle: new decision\nsupersedes: [[wiki/old-decision]]\n---\n\nzephyrline routing is automatic.\n",
    );
    world.export_vault();

    assert_eq!(
        recalled(&world, "zephyrline routing"),
        vec!["wiki/new-decision", "wiki/old-decision"],
        "the page another page replaced is still recalled, just second"
    );
}

// ---------------------------------------------------------------------------
// Prompt
// ---------------------------------------------------------------------------

#[test]
fn the_prompt_names_the_vault_from_outside_it_and_from_inside_it() {
    let world = World::new("prompt");
    world.export_vault();
    let outside = crate::prompt::load_system_prompt_for_main(
        &world.workspace,
        "2026-09-03",
        "macos",
        "26.0",
        None,
    )
    .expect("prompt")
    .join("\n\n");
    assert!(outside.contains("# Second brain"), "{outside}");
    assert!(outside.contains(&world.vault.display().to_string()));
    assert!(outside.contains("Never modify or delete anything under `raw/`"));

    // The session sitting INSIDE the vault gets it too. Skipping there is the
    // tempting move — the vault's own `AGENTS.md` states the same rules — but
    // zo never reads that file: `discover_instruction_files` collects
    // `context.md`/`CONTEXT.md`/`.zo/context.md` and nothing else. Dropping the
    // section would leave the one session standing in the second brain as the
    // only one never told so.
    std::fs::write(world.vault.join(super::AGENTS_FILE), "vault house rules\n")
        .expect("vault AGENTS.md");
    let inside = crate::prompt::load_system_prompt_for_main(
        &world.vault,
        "2026-09-03",
        "macos",
        "26.0",
        None,
    )
    .expect("prompt")
    .join("\n\n");
    assert!(
        inside.contains("# Second brain"),
        "a session inside the vault is told where it is: {inside}"
    );
    assert!(
        !inside.contains("vault house rules"),
        "and the check above is not vacuous — zo really does not load AGENTS.md, \
         which is why the section has to name it"
    );
}

#[test]
fn a_session_with_no_vault_gets_no_second_brain_section() {
    let world = World::new("prompt-off");
    let prompt = crate::prompt::load_system_prompt_for_main(
        &world.workspace,
        "2026-09-03",
        "macos",
        "26.0",
        None,
    )
    .expect("prompt")
    .join("\n\n");
    assert!(!prompt.contains("# Second brain"));
}

// ---------------------------------------------------------------------------
// Write-back
// ---------------------------------------------------------------------------

fn lesson(slug: &str, summary: &str, body: &str) -> PromotedLesson {
    PromotedLesson {
        slug: slug.to_string(),
        summary: summary.to_string(),
        lesson: body.to_string(),
        kind: LessonKind::Gotcha,
        distinct_sessions: 2,
        verified: true,
        confidence: 0.9,
        expiry_days: 0,
    }
}

fn dream_report(lessons: Vec<(PromotedLesson, WriteOutcome)>) -> DreamReport {
    DreamReport {
        applied: lessons
            .iter()
            .map(|(lesson, outcome)| AppliedPromotion {
                slug: lesson.slug.clone(),
                outcome: *outcome,
            })
            .collect(),
        plan: CurationPlan {
            promote: lessons.into_iter().map(|(lesson, _)| lesson).collect(),
            skipped: Vec::new(),
        },
    }
}

#[test]
fn a_promotion_becomes_an_atomic_page_and_one_log_line() {
    let world = World::new("promote");
    let report = dream_report(vec![
        (
            lesson(
                "gotcha-zsh-pipe",
                "zsh 파이프가 게이트 실패를 초록으로 숨긴다",
                "Check `$?` without a pipe; clippy needs `--no-deps`.",
            ),
            WriteOutcome::Created,
        ),
        (
            lesson("gotcha-already-known", "already current", "nothing new"),
            WriteOutcome::Unchanged,
        ),
    ]);

    let promotion = promote::record_lessons(
        &world.brain(),
        "sess-abc",
        &report,
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_772_000_000),
    )
    .expect("write-back");

    assert_eq!(promotion.pages.len(), 1, "an unchanged lesson writes nothing");
    assert_eq!(promotion.pages[0].link, "wiki/zo/gotcha-zsh-pipe");

    let page = fs::read_to_string(world.vault.join("wiki/zo/gotcha-zsh-pipe.md")).expect("page");
    assert!(page.starts_with("---\n"), "{page}");
    assert!(page.contains(r#"source: "zo:session:sess-abc""#), "{page}");
    assert!(
        page.contains(r#"title: "zsh 파이프가 게이트 실패를 초록으로 숨긴다""#),
        "a free-form summary is quoted so a colon cannot break the frontmatter: {page}"
    );
    assert!(page.contains("ingested_at: 2026-02-25T"), "{page}");
    assert!(page.contains("tags: [zo, gotcha]"), "{page}");
    assert!(page.contains("Check `$?` without a pipe"), "{page}");

    let log = fs::read_to_string(world.vault.join("wiki/log.md")).expect("log");
    assert!(
        log.contains("— zo:session:sess-abc → [[wiki/zo/gotcha-zsh-pipe]]"),
        "{log}"
    );
    assert!(log.starts_with("- 2026-02-25 "), "{log}");
}

#[test]
fn write_back_never_rewrites_a_page_or_touches_raw() {
    let world = World::new("promote-append-only");
    let raw = world.vault.join("raw/source.md");
    fs::write(&raw, "ORIGINAL SOURCE\n").expect("raw source");
    let report = dream_report(vec![(
        lesson("gotcha-once", "once", "first body"),
        WriteOutcome::Created,
    )]);
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_772_000_000);

    promote::record_lessons(&world.brain(), "sess-1", &report, now).expect("first pass");
    let after_first = fs::read_to_string(world.vault.join("wiki/zo/gotcha-once.md")).expect("page");

    let second_report = dream_report(vec![(
        lesson("gotcha-once", "once", "a DIFFERENT body"),
        WriteOutcome::Updated,
    )]);
    let second = promote::record_lessons(&world.brain(), "sess-2", &second_report, now)
        .expect("second pass");

    assert!(second.is_noop());
    assert_eq!(second.already_present, 1);
    assert_eq!(
        fs::read_to_string(world.vault.join("wiki/zo/gotcha-once.md")).expect("page"),
        after_first,
        "an existing page is never rewritten"
    );
    assert_eq!(
        fs::read_to_string(&raw).expect("raw"),
        "ORIGINAL SOURCE\n",
        "raw/ is never written"
    );
    let log = fs::read_to_string(world.vault.join("wiki/log.md")).expect("log");
    assert_eq!(log.lines().count(), 1, "a skipped page logs nothing: {log}");
}

#[test]
fn write_back_turned_off_leaves_the_vault_untouched() {
    let world = World::new("promote-off");
    world.export_vault();
    world.write_settings(r#"{"secondBrain":{"writeBack":false}}"#);
    let report = dream_report(vec![(
        lesson("gotcha-quiet", "quiet", "body"),
        WriteOutcome::Created,
    )]);

    assert!(promote::promote_session_lessons(&world.workspace, "sess-off", &report).is_none());
    assert!(!world.vault.join("wiki/zo").exists());
    assert!(!world.vault.join("wiki/log.md").exists());
}

#[test]
fn write_back_through_the_configured_path_creates_the_page() {
    let world = World::new("promote-configured");
    world.export_vault();
    let report = dream_report(vec![(
        lesson("gotcha-configured", "configured", "body"),
        WriteOutcome::Created,
    )]);
    let promotion = promote::promote_session_lessons(&world.workspace, "sess-on", &report)
        .expect("write-back runs when a vault is configured");
    assert_eq!(promotion.pages.len(), 1);
    assert!(world.vault.join("wiki/zo/gotcha-configured.md").is_file());
}

#[test]
fn a_promoted_page_links_the_vault_pages_its_lesson_overlaps() {
    let world = World::new("promote-related");
    world.page(
        "hook-bridge.md",
        "---\ntitle: Hook bridge\ntags: [timeout]\n---\n\nThe bridge.\n",
    );
    world.page(
        "coffee.md",
        "---\ntitle: Coffee grinders\ntags: [kitchen]\n---\n\nUnrelated.\n",
    );
    // A page a previous session promoted is not a neighbour: promoted pages
    // link into vault knowledge, never into each other.
    world.page(
        "zo/gotcha-earlier.md",
        "---\ntitle: Hook bridge timeout, earlier\ntags: [zo]\n---\n\nEarlier.\n",
    );
    let report = dream_report(vec![(
        lesson(
            "gotcha-hook-timeout",
            "hook bridge timeout",
            "The hook bridge raises a timeout under load.",
        ),
        WriteOutcome::Created,
    )]);

    promote::record_lessons(
        &world.brain(),
        "sess-rel",
        &report,
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_772_000_000),
    )
    .expect("write-back");

    let page =
        fs::read_to_string(world.vault.join("wiki/zo/gotcha-hook-timeout.md")).expect("page");
    assert!(
        page.contains("related: [ [[wiki/hook-bridge]] ]"),
        "the overlapping page is named in the frontmatter: {page}"
    );
    assert!(
        page.contains("관련: [[wiki/index]] · [[wiki/hook-bridge]]"),
        "and on the trailing line: {page}"
    );
    assert!(
        !page.contains("coffee"),
        "an unrelated page is not a neighbour: {page}"
    );
    assert!(
        !page.contains("gotcha-earlier"),
        "a page under wiki/zo/ is never chosen: {page}"
    );
}

#[test]
fn a_promoted_page_with_no_overlap_carries_no_related_key() {
    let world = World::new("promote-unrelated");
    world.page(
        "coffee.md",
        "---\ntitle: Coffee grinders\ntags: [kitchen]\n---\n\nUnrelated.\n",
    );
    let report = dream_report(vec![(
        lesson(
            "gotcha-sqlite-busy",
            "sqlite busy",
            "Retry the write; the handle is contended.",
        ),
        WriteOutcome::Created,
    )]);

    promote::record_lessons(
        &world.brain(),
        "sess-none",
        &report,
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_772_000_000),
    )
    .expect("write-back");

    let page = fs::read_to_string(world.vault.join("wiki/zo/gotcha-sqlite-busy.md")).expect("page");
    assert!(
        !page.contains("related:"),
        "`related: []` would claim the page has no neighbours: {page}"
    );
    assert!(page.contains("관련: [[wiki/index]]\n"), "{page}");
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

#[test]
fn status_counts_pages_and_names_a_vault_that_was_never_set_up() {
    let world = World::new("status");
    world.page("one.md", "one\n");
    world.page("two.md", "two\n");
    world.export_vault();
    let status = super::status(&world.config()).expect("status");
    assert!(status.ready);
    assert_eq!(status.pages, 2);
    assert!(!status.capped);

    let absent = world.root.join("never-created");
    std::env::set_var(VAULT_ENV, &absent);
    let status = super::status(&world.config()).expect("status");
    assert!(!status.ready);
    assert_eq!(status.pages, 0);
}

// ---------------------------------------------------------------------------
// Cost
// ---------------------------------------------------------------------------

/// A page of mixed Korean/English prose whose vocabulary varies per page.
///
/// Repeating one paragraph a thousand times would measure a corpus with almost
/// no distinct tokens and flatter both the clock and the footprint. This draws
/// each word from a wide vocabulary with a per-page seed, so the number of
/// distinct tokens per page is in the range real prose produces.
fn synthetic_page(seed: usize) -> String {
    const STEMS: &[&str] = &[
        "recall", "vault", "index", "session", "promotion", "budget", "snippet", "token",
        "lattice", "harness", "cursor", "gradient", "manifest", "checkpoint", "provenance",
    ];
    const HANGUL: &[&str] = &[
        "기억", "회수", "색인", "페이지", "세션", "승격", "예산", "발췌", "토큰", "실측",
        "경계", "지식", "연결", "기록", "관측",
    ];
    let mut state = (seed as u64).wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    let mut page = String::with_capacity(corpus::MAX_PAGE_INDEX_BYTES);
    while page.len() < corpus::MAX_PAGE_INDEX_BYTES {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let pick = (state >> 33) as usize;
        if pick.is_multiple_of(3) {
            page.push_str(HANGUL[pick % HANGUL.len()]);
            page.push_str(HANGUL[(pick / 7) % HANGUL.len()]);
        } else {
            page.push_str(STEMS[pick % STEMS.len()]);
            page.push('-');
            let _ = write!(page, "{}", pick % 4_096);
        }
        page.push(' ');
    }
    page
}

/// The design point: a first session over a thousand-page vault must not cost a
/// visible pause at startup. Measured cold (an empty cache) so the number is
/// the one a real first session pays.
///
/// The printed number is the measurement; the assertion is a regression
/// tripwire sitting well above it. The target is 300ms and a quiet machine
/// indexes this corpus in about 220ms, but this test shares a host with a dozen
/// parallel test binaries and a release build, and a bound that tight would
/// report the host's load as a second-brain regression. [`CEILING`] still
/// catches anything that changes the cost by an order of magnitude — which is
/// what a real regression here (a second read per page, a body retained, a cap
/// removed) would do.
#[test]
fn indexing_a_thousand_pages_stays_under_the_startup_budget() {
    const PAGES: usize = 1_000;
    const TARGET: Duration = Duration::from_millis(300);
    const CEILING: Duration = Duration::from_millis(1_500);

    let world = World::new("cost");
    for index in 0..PAGES {
        world.page(
            &format!("topic-{index:04}.md"),
            &format!(
                "---\ntitle: topic {index}\ntags: [bench]\n---\n\n{}\n",
                synthetic_page(index)
            ),
        );
    }

    let started = std::time::Instant::now();
    let scan = corpus::scan(&world.brain());
    let cold = started.elapsed();
    assert_eq!(scan.pages.len(), PAGES);

    let started = std::time::Instant::now();
    let warm_scan = corpus::scan(&world.brain());
    let warm = started.elapsed();
    assert_eq!(warm_scan.pages.len(), PAGES);

    // What the index retains, measured rather than asserted: tokens, not the
    // bytes they came from. `MAX_PAGE_INDEX_BYTES` is read per page and dropped.
    let (tokens, key_bytes) = scan.pages.iter().fold((0, 0), |(tokens, bytes), page| {
        let (page_tokens, page_bytes) = page.token_footprint();
        (tokens + page_tokens, bytes + page_bytes)
    });
    let read_bytes = PAGES * corpus::MAX_PAGE_INDEX_BYTES;

    eprintln!(
        "[second-brain] {PAGES} pages: cold {}ms (target {}ms), warm {}ms, {tokens} tokens, \
         {key_bytes}B of keys against {read_bytes}B read",
        cold.as_millis(),
        TARGET.as_millis(),
        warm.as_millis()
    );
    assert!(
        cold < CEILING,
        "cold index of {PAGES} pages took {cold:?}, past the {CEILING:?} regression ceiling \
         (the target is {TARGET:?})"
    );
    assert!(
        key_bytes * 4 < read_bytes,
        "the index must stay far smaller than the text it read \
         ({key_bytes}B of keys against {read_bytes}B read) — a regression here means \
         page bodies started living in the index"
    );
    assert!(
        warm < cold,
        "the mtime cache must make a rebuild cheaper than the first read (cold {cold:?}, warm {warm:?})"
    );
}
