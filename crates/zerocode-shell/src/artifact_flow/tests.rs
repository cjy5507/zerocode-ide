use std::time::Instant;

use zerocode_core::orchestration::{
    Closure, Draft, Ledger, MessageKind, Priority, ResultAuthor, TaskStatus, Text, worker_address,
};

use super::*;

/// A day in 2026, in epoch milliseconds; the fixtures' moments are hours after it.
const T0: i64 = 1_790_000_000_000;
const HOUR_MS: i64 = 60 * 60 * 1000;
/// The project the fixture's runs belong to, as the ledger keys one, and another.
const PROJECT: &str = "/acme/app/.git";
const ELSEWHERE: &str = "/acme/docs/.git";
const SEAT: &str = "team-acme/%1";

fn at(hours: i64) -> i64 {
    T0 + hours * HOUR_MS
}

/// When the fixture is read: a little after its newest moment (the carried
/// task's attempt, at hour 131), so every task of its seated run is somebody's.
const READ_AT: i64 = 140;

/// The flow's own table with two releases kept, so a third folds.
fn table() -> FlowTable {
    FlowTable {
        releases_kept: 2,
        ..FLOW
    }
}

/// A commit's 40 hex digits, one byte repeated.
fn sha(byte: u8) -> String {
    format!("{byte:02x}").repeat(20)
}

/// One line of the walk, as git prints it in [`LOG_FORMAT`].
fn walked(commit: &str, parents: &[&str], hours: i64, subject: &str, refs: &str) -> String {
    format!(
        "{commit}\u{1f}{}\u{1f}{}\u{1f}{refs}\u{1f}{subject}",
        parents.join(" "),
        at(hours) / 1000
    )
}

/// A merge on the first-parent line: two parents.
fn merge_line(byte: u8, hours: i64, subject: &str) -> String {
    walked(
        &sha(byte),
        &[&sha(byte + 100), &sha(byte + 50)],
        hours,
        subject,
        "",
    )
}

fn coordinator(attempt: Option<String>, source: Option<&str>) -> ResultAuthor {
    ResultAuthor::Coordinator {
        seat: SEAT.to_string(),
        generation: Some(1),
        attempt,
        source: source.map(str::to_string),
        completed_ms: None,
        verified_ms: None,
    }
}

/// A worker summoned on `task` at `hours`, reporting `body` at `reported`: its attempt.
fn handed(
    ledger: &mut Ledger,
    run: &str,
    task: &str,
    pane: &str,
    (hours, reported): (i64, i64),
    body: &str,
) -> String {
    handed_at(ledger, run, task, pane, (at(hours), at(reported)), body)
}

/// [`handed`] at two moments in epoch milliseconds.
fn handed_at(
    ledger: &mut Ledger,
    run: &str,
    task: &str,
    pane: &str,
    (started_ms, reported_ms): (i64, i64),
    body: &str,
) -> String {
    let started = ledger
        .start_worker(run, "claude", ("team-acme", pane), Some(task), started_ms)
        .expect("a worker carries it");
    ledger
        .post(
            run,
            Draft {
                from: worker_address(&started.worker),
                to: ledger.run(run).expect("the run").address(),
                kind: MessageKind::WorkerDone,
                body: Text::from(body.to_string()),
                subject: Text::default(),
                priority: Priority::Normal,
                payload: Text::default(),
                thread: None,
                task: Some(task.to_string()),
                dispatch: started.dispatch.clone(),
            },
            reported_ms,
        )
        .expect("the report");
    started.dispatch.expect("an attempt")
}

/// The fixture: acme-app's run with a task at every stage, another project's run
/// whose task a merge on acme-app's base carries, and acme-app's base walked —
/// two merges after the newest release, three releases.
struct Fixture {
    ledger: Ledger,
    git: GitFacts,
    ids: BTreeMap<&'static str, String>,
    other_run: String,
}

const D_HEAD: &str = "d4d4d4d";
const E_HEAD: &str = "e5e5e5e";
const F_HEAD: &str = "f6f6f6f";

fn fixture() -> Fixture {
    let mut ledger = Ledger::new();
    let run = ledger.create_run("acme-app", at(0));
    assert!(ledger.state_run_project(&run, PROJECT, SEAT, at(0)));
    ledger
        .seat_coordinator(&run, SEAT, None, at(0))
        .expect("acme-app's coordinator sits");
    let other_run = ledger.create_run("acme-docs", at(0));
    assert!(ledger.state_run_project(&other_run, ELSEWHERE, SEAT, at(0)));
    let mut ids = BTreeMap::new();
    let mut task = |ledger: &mut Ledger, name: &'static str, run: &str, hours: i64| {
        let id = ledger
            .create_task(
                run,
                format!("do {name}"),
                format!("the {name} work"),
                vec![],
                None,
                at(hours),
            )
            .expect("a task");
        ids.insert(name, id.clone());
        id
    };
    // Carried: an attempt open.
    let a = task(&mut ledger, "carried", &run, 130);
    ledger
        .start_worker(&run, "claude", ("team-acme", "%2"), Some(&a), at(131))
        .expect("a worker carries it");
    // Reported, and nobody reviewed it.
    let b = task(&mut ledger, "reported", &run, 120);
    handed(
        &mut ledger,
        &run,
        &b,
        "%3",
        (121, 125),
        r#"{"ok":true,"head":"b2b2b2b"}"#,
    );
    // Verified, not landed.
    let c = task(&mut ledger, "verified", &run, 116);
    let c_try = handed(
        &mut ledger,
        &run,
        &c,
        "%4",
        (116, 117),
        r#"{"ok":true,"head":"c3c3c3c"}"#,
    );
    ledger
        .update_task(
            &run,
            &c,
            None,
            Some(r#"{"verified":true}"#.into()),
            coordinator(Some(c_try), Some("c3c3c3c")),
            at(118),
        )
        .expect("a review");
    // Landed after the newest release: the merge's subject names it.
    let d = task(&mut ledger, "landed", &run, 111);
    let d_try = handed(
        &mut ledger,
        &run,
        &d,
        "%5",
        (111, 112),
        &format!(r#"{{"ok":true,"head":"{D_HEAD}"}}"#),
    );
    ledger
        .update_task(
            &run,
            &d,
            None,
            Some(r#"{"verified":true,"merged":true}"#.into()),
            coordinator(Some(d_try), Some(D_HEAD)),
            at(113),
        )
        .expect("a review");
    // Landed after the newest release by a merge whose subject names no task:
    // the coordinator's mergeHead names the merge.
    let e = task(&mut ledger, "named-by-head", &run, 105);
    let e_try = handed(
        &mut ledger,
        &run,
        &e,
        "%6",
        (105, 106),
        &format!(r#"{{"ok":true,"head":"{E_HEAD}"}}"#),
    );
    ledger
        .update_task(
            &run,
            &e,
            None,
            Some(format!(r#"{{"verified":true,"mergeHead":"{}"}}"#, sha(2))),
            coordinator(Some(e_try), Some(E_HEAD)),
            at(107),
        )
        .expect("a review");
    // Shipped in the newest release — the ledger says merged and no more.
    let f = task(&mut ledger, "shipped", &run, 60);
    let f_body = format!(
        r#"{{"ok":true,"head":"{F_HEAD}","summary":"t-1 at {F_HEAD}: 검색 결과에 찾은 줄과 앞뒤 한 줄이 보이게 했다. 시험 31개가 통과했다.","numbers":"검색 42 → 38 ms"}}"#
    );
    let f_try = handed(&mut ledger, &run, &f, "%7", (61, 70), &f_body);
    ledger
        .update_task(
            &run,
            &f,
            None,
            Some(r#"{"verified":true,"merged":true}"#.into()),
            coordinator(Some(f_try), Some(F_HEAD)),
            at(75),
        )
        .expect("a review");
    // Folded into another task: over, and no line.
    let g = task(&mut ledger, "folded", &run, 119);
    ledger
        .close_task(&run, &g, Closure::Folded { into: a.clone() })
        .expect("a closing");
    // Reported long before the release past the kept ones, named by no merge.
    let h = task(&mut ledger, "old", &run, 1);
    handed(
        &mut ledger,
        &run,
        &h,
        "%8",
        (1, 2),
        r#"{"ok":true,"head":"a8a8a8a"}"#,
    );
    // Failed after the newest release.
    let i = task(&mut ledger, "failed", &run, 115);
    ledger
        .update_task(
            &run,
            &i,
            Some(TaskStatus::Failed),
            Some(String::new()),
            coordinator(None, None),
            at(115),
        )
        .expect("a failure");
    // Another project's task, carried by a merge on acme-app's base.
    let k = task(&mut ledger, "carried-in", &other_run, 30);

    let log = [
        merge_line(
            1,
            120,
            &format!("Merge wt/{d}/queue at {D_HEAD} ({d}) — the queue empties when it closes"),
        ),
        merge_line(
            2,
            110,
            "Merge fix/close-budget at e5e5e5e — the close budget is a setting",
        ),
        walked(&sha(3), &[&sha(4)], 100, "release: v2.4.0", "tag: v2.4.0"),
        merge_line(
            5,
            90,
            &format!("Merge wt/{f}/search at {F_HEAD} ({f}) — search shows the line it found"),
        ),
        merge_line(6, 80, "Merge wt/t-9001 at 9001900 (t-9001)"),
        walked(&sha(7), &[&sha(8)], 70, "docs: a note on the queue", ""),
        walked(&sha(9), &[&sha(10)], 50, "release: v2.3.0", ""),
        merge_line(
            11,
            40,
            &format!("Merge wt/{k} at 5555555 ({k}) — the docs build waits less"),
        ),
        walked(&sha(12), &[&sha(13)], 10, "chore: bump", "tag: v2.2.0"),
        merge_line(14, 5, "Merge wt/t-9002 at 9002900 (t-9002) — an old change"),
    ]
    .join("\n");
    let (unreleased, releases) = parse_log(&log);
    let notes = parse_notes(
        &format!(
            "# Changelog\n\n## [2.4.0] — 2026-10-25\n\n\
             - 검색 결과에 찾은 줄이 보입니다 — 결과마다 앞뒤 한 줄 ({f})\n\
             - 빌드 줄이 덜 기다립니다 (t-9001)\n\n\
             ## [2.3.0] — 2026-10-23\n\n- 문서 빌드가 덜 기다립니다 ({k}, ZO)\n\n\
             ## [2.2.0] — 2026-10-21\n\n- 오래된 고침 (t-9002)\n"
        ),
        FLOW.releases_kept + 1,
    );
    Fixture {
        ledger,
        git: GitFacts {
            base: Some("main".to_string()),
            unreleased,
            releases,
            notes,
            stale: false,
        },
        ids,
        other_run,
    }
}

fn flow_of(fixture: &Fixture, table: &FlowTable) -> Flow {
    assemble(&Reading {
        root: "/acme/app",
        project: Some(PROJECT),
        ledger: Some(&fixture.ledger),
        git: &fixture.git,
        table,
        now_ms: at(READ_AT),
    })
}

fn ids(group: &FlowGroup) -> Vec<&str> {
    group.tasks.iter().map(|line| line.task.as_str()).collect()
}

fn line<'a>(flow: &'a Flow, task: &str) -> &'a FlowTask {
    lines_of(flow)
        .find(|line| line.task == task)
        .unwrap_or_else(|| panic!("{task} has no line: {flow:#?}"))
}

/// The walk is read newest first: the merges after the newest release, then
/// every release with the merges between it and the release before it. A
/// release is a version tag on a first-parent commit or a `release:` subject;
/// a commit with one parent is no merge.
#[test]
fn the_walk_reads_merges_and_releases_newest_first() {
    let fixture = fixture();
    let git = &fixture.git;
    let d = &fixture.ids["landed"];
    assert_eq!(git.unreleased.len(), 2, "{git:#?}");
    assert_eq!(git.unreleased[0].tasks, [d.as_str()]);
    assert_eq!(git.unreleased[0].head.as_deref(), Some(D_HEAD));
    assert_eq!(git.unreleased[0].at_ms, at(120));
    assert_eq!(
        git.unreleased[0].line.as_deref(),
        Some("the queue empties when it closes")
    );
    assert!(
        git.unreleased[1].tasks.is_empty(),
        "a merge whose subject names no task"
    );
    let versions: Vec<&str> = git
        .releases
        .iter()
        .map(|release| release.version.as_str())
        .collect();
    assert_eq!(versions, ["v2.4.0", "v2.3.0", "v2.2.0"]);
    assert_eq!(git.releases[0].at_ms, at(100));
    assert_eq!(
        git.releases[0].merges.len(),
        2,
        "the plain commit between them is no merge: {:#?}",
        git.releases[0]
    );
    assert_eq!(git.releases[0].merges[1].tasks, ["t-9001"]);
    assert_eq!(git.releases[0].merges[1].line, None);
    assert_eq!(git.releases[2].merges[0].tasks, ["t-9002"]);
}

/// A merge's tasks are the one its branch names first and the ones its
/// parentheses name; a task id later in a branch's name is a name.
#[test]
fn a_merge_names_its_branchs_task_and_its_parentheses_and_nothing_else() {
    let parents = ["a", "b"];
    let merge = |subject: &str| merge_of(&sha(1), &parents, 0, subject).expect("a merge");
    assert_eq!(
        merge("Merge wt/t-9003/2-zo-t-9004-plan at 9003900 (t-9003) — a plan").tasks,
        ["t-9003"]
    );
    assert_eq!(
        merge("Merge t-9005 at 9005900 (folding t-9006 and t-9007) — three as one").tasks,
        ["t-9005", "t-9006", "t-9007"]
    );
    assert_eq!(
        merge("Merge the candidate at 9008900 (t-9008, t-9009; carrying t-9010 at 9010900) — many")
            .tasks,
        ["t-9008", "t-9009", "t-9010"]
    );
    let plain = merge("Merge fix/lane at 9011900 — the lane stops twice");
    assert!(plain.tasks.is_empty());
    assert_eq!(plain.head.as_deref(), Some("9011900"));
    assert_eq!(
        merge("Merge fix/cookie (9012900) — no at").head.as_deref(),
        Some("b"),
        "without `at`, the second parent is what was merged"
    );
    assert!(
        merge_of(&sha(1), &["a"], 0, "Merge t-1 at 1234567").is_none(),
        "one parent is no merge"
    );
    assert_eq!(task_ids("art-12 t-34, (t-56) t-7x st-8"), ["t-34", "t-56"]);
}

/// The release notes' line for a task is its bullet's words before 「 — 」, and
/// the newest section that names it keeps it.
#[test]
fn the_release_notes_give_each_task_its_newest_line() {
    let notes = parse_notes(
        "# Changelog\n\n## [1.2.0] — 2026-10-21\n\n\
         - 설정의 표에 전체 선택이 생겼습니다 — 머리칸 하나로 모두 켭니다 (t-11)\n\
         - 두 과업의 줄 (t-12, t-13, ZO)\n\n\
         ## [1.1.9] — 2026-10-20\n\n- 예전 줄 (t-11)\n- 과업 없는 줄\n\n\
         ## [1.1.8] — 2026-10-19\n\n- 읽지 않는 절 (t-14)\n",
        2,
    );
    assert_eq!(
        notes.get("t-11"),
        Some(&Note {
            version: "1.2.0".to_string(),
            line: "설정의 표에 전체 선택이 생겼습니다".to_string(),
        })
    );
    assert_eq!(notes["t-13"].line, "두 과업의 줄");
    assert!(
        !notes.contains_key("t-14"),
        "a section past the bound is not read"
    );
}

/// What was done, in one sentence: without the head a worker's summary opens
/// with, up to its first sentence end, bounded.
#[test]
fn a_line_says_the_hand_ins_first_sentence() {
    assert_eq!(
        first_sentence(
            "t-12 at abcdef1 (red 1234567): 큐를 닫을 때 남은 일을 비웠다. 시험 12개 통과.",
            140
        )
        .as_deref(),
        Some("큐를 닫을 때 남은 일을 비웠다")
    );
    assert_eq!(
        first_sentence("t-12 상황판을 B 언어로 그렸습니다.\n둘째 줄", 140).as_deref(),
        Some("상황판을 B 언어로 그렸습니다")
    );
    assert_eq!(
        first_sentence("v1.2.3 ships 4.5 s faster; the rest waits", 140).as_deref(),
        Some("v1.2.3 ships 4.5 s faster"),
        "a full stop inside a number ends nothing"
    );
    let long = "가".repeat(30) + " " + &"나".repeat(30);
    let cut = first_sentence(&long, 40).expect("a sentence");
    assert!(cut.ends_with('…') && cut.chars().count() <= 41, "{cut}");
    assert_eq!(first_sentence("  ", 140), None);
}

/// Before → after: the hand-in's own numbers first, then its summary.
#[test]
fn the_before_and_after_is_the_hand_ins_own_number() {
    assert_eq!(
        gain_of(
            Some(&Value::String("거절 11→0, 시연 16 → 0".into())),
            None,
            48
        )
        .as_deref(),
        Some("거절 11 → 0")
    );
    let table: Value =
        serde_json::json!({ "draws": 13, "read": "longest list read 5496 ms -> 1 ms" });
    assert_eq!(
        gain_of(Some(&table), None, 48).as_deref(),
        Some("5496 ms → 1 ms")
    );
    assert_eq!(
        gain_of(None, Some("고쳤다. 검색 42 → 38 ms, 시험 31개 통과"), 48).as_deref(),
        Some("검색 42 → 38 ms")
    );
    assert_eq!(gain_of(None, Some("숫자 없는 요약"), 48), None);
}

/// A report's three lines, Korean or English; all three or none.
#[test]
fn a_reports_three_lines_are_what_why_and_cost() {
    let korean = "# 제목\n\n## 세 줄\n\n- **무엇**: 닫기가 남은 일을 비운다.\n- **왜**: 일 7건이 사라졌다.\n- **대가**: 450 ms를 넘으면 미룬다.\n";
    assert_eq!(
        three_of(korean, 200),
        Some(Three {
            what: "닫기가 남은 일을 비운다.".to_string(),
            why: "일 7건이 사라졌다.".to_string(),
            cost: "450 ms를 넘으면 미룬다.".to_string(),
        })
    );
    let english = "- **What**: it drains\n* **Why**: work vanished\n- **Cost** — a budget\n";
    assert_eq!(
        three_of(english, 200).map(|three| three.cost),
        Some("a budget".to_string())
    );
    assert_eq!(three_of("- **무엇**: 하나뿐\n", 200), None);
}

/// Every line stands where the ledger and git put it: carried, reported and
/// verified work now; landed work and work that ended since the newest release
/// before the next one; shipped work under its release, newest first; the
/// release past the kept ones folded; a closed task and old work nowhere.
#[test]
fn the_flow_places_every_task_where_the_ledger_and_git_put_it() {
    let fixture = fixture();
    let flow = flow_of(&fixture, &table());
    let id = |name: &str| fixture.ids[name].as_str();
    let mut now = ids(&flow.now);
    now.sort_unstable();
    let mut expected = vec![id("carried"), id("reported"), id("verified")];
    expected.sort_unstable();
    assert_eq!(now, expected, "{flow:#?}");
    assert_eq!(
        ids(&flow.next),
        [id("landed"), id("failed"), id("named-by-head")],
        "newest first by the moment each landed or ended"
    );
    let versions: Vec<&str> = flow
        .releases
        .iter()
        .map(|release| release.version.as_str())
        .collect();
    assert_eq!(versions, ["v2.4.0", "v2.3.0"]);
    assert_eq!(ids(&flow.releases[0].group), [id("shipped"), "t-9001"]);
    assert_eq!(ids(&flow.releases[1].group), [id("carried-in")]);
    assert_eq!(
        flow.older,
        Some(FlowOlder {
            releases: 1,
            newest: "v2.2.0".to_string(),
            oldest: "v2.2.0".to_string(),
            tasks: 1,
            until_ms: at(10),
        })
    );
    let shown = flow.task_ids();
    assert!(
        !shown.contains(id("folded")) && !shown.contains(id("old")),
        "{shown:?}"
    );
    assert_eq!(flow.base.as_deref(), Some("main"));
    assert!(flow.ledger);

    let stage = |name: &str| line(&flow, id(name)).stage;
    assert_eq!(
        [
            stage("carried"),
            stage("reported"),
            stage("verified"),
            stage("failed")
        ],
        ["dispatched", "reported", "verified", "failed"]
    );
    assert_eq!(
        [
            stage("landed"),
            stage("named-by-head"),
            stage("shipped"),
            stage("carried-in")
        ],
        ["merged", "merged", "deployed", "deployed"],
        "git moves a stage on: a merge on the base landed it, a release after it shipped it"
    );
    assert_eq!(line(&flow, id("shipped")).ledger_stage, Some("merged"));
    assert_eq!(line(&flow, "t-9001").ledger_stage, None);
}

/// Work nobody is coming back to stands in its piles, oldest first, and not in
/// 「지금」 (t-71220, the person's 10-22 picture): in a run whose coordinator seat
/// is live, a task touched within the quiet week is that coordinator's turn and
/// one untouched for longer is left; in a run nobody sits in, every open task is
/// left — closed by a settle when nothing is on record, kept for a look when
/// something was handed in — and finished work no review was written for is
/// left apart. A task a current one waits on is current with it, and a press
/// names only what it would act on.
#[test]
fn work_nobody_is_coming_back_to_stands_in_its_piles_and_not_in_now() {
    let mut ledger = Ledger::new();
    let seated = ledger.create_run("acme-seated", at(-400));
    assert!(ledger.state_run_project(&seated, PROJECT, SEAT, at(-400)));
    ledger
        .seat_coordinator(&seated, SEAT, None, at(-400))
        .expect("the coordinator sits");
    let empty = ledger.create_run("acme-left", at(-400));
    assert!(ledger.state_run_project(&empty, PROJECT, SEAT, at(-400)));
    let task = |ledger: &mut Ledger, run: &str, name: &str, deps: Vec<String>, hours: i64| {
        ledger
            .create_task(
                run,
                format!("do {name}"),
                name.to_string(),
                deps,
                None,
                at(hours),
            )
            .expect("a task")
    };
    // The seated run: a fresh turn, one a week and more old, and an old one a
    // fresh task waits on.
    let fresh = task(&mut ledger, &seated, "fresh", vec![], 100);
    let quiet = task(&mut ledger, &seated, "quiet", vec![], -100);
    let waited = task(&mut ledger, &seated, "waited on", vec![], -90);
    let waiting = task(&mut ledger, &seated, "waiting", vec![waited.clone()], 110);
    // The run nobody sits in: written yesterday and never started, handed in
    // and put back, and finished with no review.
    let never = task(&mut ledger, &empty, "never started", vec![], 120);
    let back = task(&mut ledger, &empty, "handed and put back", vec![], -50);
    handed(
        &mut ledger,
        &empty,
        &back,
        "%2",
        (-49, -48),
        r#"{"ok":true,"head":"b1b1b1b"}"#,
    );
    ledger
        .update_task(
            &empty,
            &back,
            Some(TaskStatus::Ready),
            None,
            coordinator(None, None),
            at(-47),
        )
        .expect("back to ready");
    let unreviewed = task(&mut ledger, &empty, "finished, never reviewed", vec![], -30);
    handed(
        &mut ledger,
        &empty,
        &unreviewed,
        "%3",
        (-29, -28),
        r#"{"ok":true,"head":"c1c1c1c"}"#,
    );
    let git = GitFacts::default();
    let flow = assemble(&Reading {
        root: "/acme/app",
        project: Some(PROJECT),
        ledger: Some(&ledger),
        git: &git,
        table: &table(),
        now_ms: at(READ_AT),
    });
    let mut now = ids(&flow.now);
    now.sort_unstable();
    let mut expected = vec![fresh.as_str(), waited.as_str(), waiting.as_str()];
    expected.sort_unstable();
    assert_eq!(now, expected, "{flow:#?}");
    let pile = |pile: &FlowPile| -> Vec<(String, Option<Left>)> {
        pile.tasks
            .iter()
            .map(|line| (line.task.clone(), line.left))
            .collect()
    };
    assert_eq!(
        pile(&flow.left),
        [
            (quiet.clone(), Some(Left::Settle)),
            (back.clone(), Some(Left::Look)),
            (never.clone(), Some(Left::Settle)),
        ],
        "oldest first"
    );
    assert_eq!(
        pile(&flow.unreviewed),
        [(unreviewed.clone(), Some(Left::Unreviewed))]
    );
    let named = |pile: &FlowPile| -> Vec<(String, String)> {
        pile.named
            .iter()
            .map(|one| (one.run.clone(), one.task.clone()))
            .collect()
    };
    assert_eq!(
        named(&flow.left),
        [
            (seated.clone(), quiet.clone()),
            (empty.clone(), never.clone())
        ],
        "a settle names what it closes, never what it keeps for a look"
    );
    assert_eq!(
        named(&flow.unreviewed),
        [(empty.clone(), unreviewed.clone())]
    );
    assert!(
        lines_of(&flow).all(|line| line.left.is_none()),
        "no line of the groups carries a pile"
    );
    assert_eq!(flow.press_max, PRESS_TASKS_MAX);
    let json = serde_json::to_value(&flow).expect("the flow serializes");
    assert_eq!(json["left"]["tasks"][0]["left"], "settle");
    assert_eq!(json["left"]["named"][0]["task"], quiet.as_str());
    assert_eq!(json["unreviewed"]["tasks"][0]["left"], "unreviewed");
}

/// The ledger's landing moment is when the coordinator wrote `merged`
/// (`AfterReport::merged_ms`), and it stamps none for a release
/// (`deployed_ms` is always empty): git's merge commit and release say when,
/// and the line says git told it.
#[test]
fn an_empty_landing_time_is_read_off_git() {
    let fixture = fixture();
    let flow = flow_of(&fixture, &table());
    let shipped = line(&flow, &fixture.ids["shipped"]);
    let after = {
        let run = fixture
            .ledger
            .runs()
            .iter()
            .find(|run| run.tasks.iter().any(|task| task.id == shipped.task))
            .expect("the run");
        let task = run
            .tasks
            .iter()
            .find(|task| task.id == shipped.task)
            .expect("the task");
        task_time(run, task).after_report
    };
    assert_eq!(
        (after.merged_ms, after.deployed_ms),
        (Some(at(75)), None),
        "the ledger holds the moment its merged was written, and no release moment"
    );
    assert_eq!(
        shipped.land,
        Some(FlowLand {
            at_ms: Some(at(90)),
            commit: Some(sha(5)),
            line: Some("search shows the line it found".to_string()),
            told: Told::Git,
        })
    );
    assert_eq!(
        shipped.ship,
        Some(FlowShip {
            version: "v2.4.0".to_string(),
            at_ms: Some(at(100)),
            told: Told::Git,
        })
    );
    let by_head = line(&flow, &fixture.ids["named-by-head"]);
    assert_eq!(
        by_head.land.as_ref().map(|land| (land.at_ms, land.told)),
        Some((Some(at(110)), Told::Git))
    );
    // Landed by git's word: the two after the newest release, the two of v2.4.0
    // and the one of v2.3.0.
    assert_eq!(flow.sources.land.get(&Told::Git), Some(&5));
    assert_eq!(flow.sources.ship.get(&Told::Git), Some(&3));
}

/// The board's schedule ends a finished task's bar where git says it landed
/// (t-71840), found the way the flow finds a line's landing: the merge that
/// names the task, else the merge its `mergeHead` names, with the release that
/// merge falls under; with no merge, the release the coordinator wrote it
/// shipped in, when the walk holds it. A task git says nothing of is left out.
#[test]
fn the_schedule_reads_each_tasks_landing_and_release_off_git() {
    let fixture = fixture();
    let (landed, by_head, shipped) = (
        fixture.ids["landed"].as_str(),
        fixture.ids["named-by-head"].as_str(),
        fixture.ids["shipped"].as_str(),
    );
    let head = sha(2);
    let ask = |task, merge_head, release| LandingAsk {
        task,
        merge_head,
        release,
    };
    let found = landings(
        &fixture.git,
        &[
            ask(landed, None, None),
            ask(by_head, Some(head.as_str()), None),
            ask(shipped, None, Some("v2.4.0")),
            ask("t-7001", None, Some("v2.3.0")),
            ask("t-7002", None, Some("v9.9.9")),
            ask("t-7003", None, None),
        ],
    );
    let told = |landed_ms, released_ms| Landed {
        landed_ms,
        released_ms,
    };
    let expected: BTreeMap<String, Landed> = [
        (landed, told(Some(at(120)), None)),
        (by_head, told(Some(at(110)), None)),
        (shipped, told(Some(at(90)), Some(at(100)))),
        ("t-7001", told(None, Some(at(50)))),
    ]
    .into_iter()
    .map(|(task, landing)| (task.to_string(), landing))
    .collect();
    assert_eq!(found, expected);
    assert_eq!(
        serde_json::to_value(found[shipped]).expect("a landing serializes"),
        serde_json::json!({"landed_ms": at(90), "released_ms": at(100)}),
        "the window reads these keys"
    );
}

/// Each step's moment comes after the one before it: the task, its attempt,
/// the hand-in, the review, the landing and the release.
#[test]
fn a_lines_steps_stand_in_their_order() {
    let fixture = fixture();
    let flow = flow_of(&fixture, &table());
    let shipped = line(&flow, &fixture.ids["shipped"]);
    assert_eq!(shipped.attempts.len(), 1);
    let moments = [
        shipped.opened_ms,
        Some(shipped.attempts[0].started_ms),
        shipped.hand_in.as_ref().and_then(|hand_in| hand_in.at_ms),
        shipped.review.as_ref().and_then(|review| review.at_ms),
        shipped.land.as_ref().and_then(|land| land.at_ms),
        shipped.ship.as_ref().and_then(|ship| ship.at_ms),
    ];
    assert!(moments.iter().all(Option::is_some), "{moments:?}");
    assert!(
        moments.windows(2).all(|pair| pair[0] <= pair[1]),
        "{moments:?}"
    );
    assert_eq!(
        moments,
        [
            Some(at(60)),
            Some(at(61)),
            Some(at(70)),
            Some(at(75)),
            Some(at(90)),
            Some(at(100))
        ]
    );
    let attempt = &shipped.attempts[0];
    assert_eq!(attempt.agent.as_deref(), Some("claude"));
    assert_eq!(attempt.ended_ms, Some(at(70)));
    assert_eq!(attempt.source.as_deref(), Some(F_HEAD));
    assert_eq!(
        attempt.said.as_deref(),
        Some("검색 결과에 찾은 줄과 앞뒤 한 줄이 보이게 했다")
    );
}

/// What a line says was done: the hand-in's first sentence, else the merge's
/// line, else the release notes', else the task's title — and it says which.
#[test]
fn a_line_says_what_was_done_and_where_it_read_it() {
    let fixture = fixture();
    let flow = flow_of(&fixture, &table());
    let shipped = line(&flow, &fixture.ids["shipped"]);
    assert_eq!(
        (
            shipped.said.as_str(),
            shipped.said_from,
            shipped.gain.as_deref()
        ),
        (
            "검색 결과에 찾은 줄과 앞뒤 한 줄이 보이게 했다",
            Said::Summary,
            Some("검색 42 → 38 ms")
        )
    );
    let carried_in = line(&flow, &fixture.ids["carried-in"]);
    assert_eq!(
        (carried_in.said.as_str(), carried_in.said_from),
        ("the docs build waits less", Said::Merge)
    );
    // Another project's run holds it, and that run is not this project's to show (t-69107 ①).
    assert_eq!(carried_in.run, None);
    let noted = line(&flow, "t-9001");
    assert_eq!(
        (noted.said.as_str(), noted.said_from),
        ("빌드 줄이 덜 기다립니다", Said::Notes)
    );
    let carried = line(&flow, &fixture.ids["carried"]);
    assert_eq!(
        (carried.said.as_str(), carried.said_from),
        ("the carried work", Said::Title)
    );
    let total: usize = flow.sources.said.values().sum();
    assert_eq!(
        total,
        flow.task_ids().len(),
        "every line counted once: {:?}",
        flow.sources
    );
}

/// Without this window's ledger the flow is git's alone: what landed and
/// shipped, in the merges' and the notes' words.
#[test]
fn a_flow_without_the_ledger_is_gits_alone() {
    let fixture = fixture();
    let flow = assemble(&Reading {
        root: "/acme/app",
        project: Some(PROJECT),
        ledger: None,
        git: &fixture.git,
        table: &table(),
        now_ms: at(READ_AT),
    });
    assert!(!flow.ledger);
    assert!(flow.now.tasks.is_empty());
    assert_eq!(ids(&flow.next), [fixture.ids["landed"].as_str()]);
    assert!(lines_of(&flow).all(|line| line.run.is_none() && line.attempts.is_empty()));
}

/// The catalog dresses a flow: a line's chips and its report's three lines, and
/// the rows no task is linked to in the group their time falls in, at most the
/// table's count a group.
#[test]
fn the_catalog_dresses_the_lines_and_places_the_records_by_their_time() {
    let fixture = fixture();
    let table = FlowTable {
        records_max: 2,
        ..table()
    };
    let mut flow = flow_of(&fixture, &table);
    let shipped = fixture.ids["shipped"].clone();
    let chips = BTreeMap::from([(
        shipped.clone(),
        Chips {
            report: Some("r-shipped".to_string()),
            pairs: 1,
            lead_text: Some("- **무엇**: 찾은 줄이 보인다\n- **왜**: 제목만 보였다\n- **대가**: 결과가 높아진다\n".to_string()),
            ..Chips::default()
        },
    )]);
    let record = |id: &str, hours: i64| -> Artifact {
        serde_json::from_value(serde_json::json!({
            "id": id, "kind": "page", "title": id, "path": format!("/acme/app/{id}.html"),
            "bytes": 1, "created_ms": at(hours), "modified_ms": at(hours),
            "origin": { "project": "/acme/app" }
        }))
        .expect("a row")
    };
    assert_eq!(
        flow.records_since(&fixture.git),
        at(10),
        "the release past the kept ones bounds them"
    );
    let records = vec![
        record("p-new", 128),
        record("p-new-2", 127),
        record("p-new-3", 126),
        record("p-in-2-4", 95),
        record("p-in-2-3", 45),
    ];
    dress(&mut flow, &chips, records, &table);
    let dressed = line(&flow, &shipped);
    assert_eq!(dressed.chips.report.as_deref(), Some("r-shipped"));
    assert_eq!(
        dressed.three.as_ref().map(|three| three.why.as_str()),
        Some("제목만 보였다")
    );
    fn titles(group: &FlowGroup) -> Vec<&str> {
        group.records.iter().map(|row| row.id.as_str()).collect()
    }
    assert_eq!(
        titles(&flow.next),
        ["p-new", "p-new-2"],
        "the newest, at most two"
    );
    assert_eq!(titles(&flow.releases[0].group), ["p-in-2-4"]);
    assert_eq!(titles(&flow.releases[1].group), ["p-in-2-3"]);
    assert!(flow.now.records.is_empty());
}

/// The answer the window reads: the stage words are the desk's, the sources are
/// words, and a line carries its chips.
#[test]
fn the_flow_serializes_the_words_the_window_reads() {
    let fixture = fixture();
    let flow = flow_of(&fixture, &table());
    let json = serde_json::to_value(&flow).expect("the flow serializes");
    let first = &json["releases"][0];
    assert_eq!(first["version"], "v2.4.0");
    assert_eq!(first["tasks"][0]["stage"], "deployed");
    assert_eq!(first["tasks"][0]["said_from"], "summary");
    assert_eq!(first["tasks"][0]["land"]["told"], "git");
    assert!(first["tasks"][0]["chips"]["parts"].is_object());
    assert_eq!(json["sources"]["said"]["notes"], 1);
    for stage in lines_of(&flow).map(|line| line.stage) {
        assert!(
            crate::orchestration::desk::STAGES.contains(&stage),
            "`{stage}` is no stage word of the desk's"
        );
    }
}

/// git in `at`, for a test's own repository; a failure is the test's.
fn git_in(at: &Path, args: &[&str]) {
    Host::for_workspace(at)
        .vcs()
        .text(at, args)
        .unwrap_or_else(|why| panic!("git {args:?}: {why}"));
}

/// A repository of its own — `main` with one commit — in a temporary folder
/// that holds it while the test runs.
fn repository() -> (tempfile::TempDir, PathBuf) {
    let held = tempfile::tempdir().expect("a folder");
    let repo = held.path().join("repo");
    std::fs::create_dir_all(&repo).expect("the repository's folder");
    git_in(&repo, &["init", "-q", "-b", "main"]);
    git_in(
        &repo,
        &[
            "-c",
            "user.name=ZeroCode Test",
            "-c",
            "user.email=test@zerocode",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "first",
        ],
    );
    (held, repo)
}

fn ask_for(root: &Path, roots: &[&Path]) -> FlowAsk {
    FlowAsk {
        root: root.to_string_lossy().into_owned(),
        roots: roots
            .iter()
            .map(|one| one.to_string_lossy().into_owned())
            .collect(),
    }
}

/// Whether git's answer is held for any of `roots`, as given or made canonical.
fn git_held_for(roots: &[&Path]) -> bool {
    let held = git_held().lock().unwrap_or_else(PoisonError::into_inner);
    roots.iter().any(|root| {
        let canonical = root.canonicalize().ok();
        held.iter()
            .any(|one| one.root == *root || Some(&one.root) == canonical.as_ref())
    })
}

/// A folder outside every repository this window lists is a request git is
/// never run for (t-69070 P2), though git could read it: the read answers from
/// the ledger and the catalog alone and says so, and no listed repository is
/// asked on its behalf either.
#[test]
fn a_folder_outside_the_listed_repositories_asks_git_nothing() {
    let (_listed_folder, listed) = repository();
    let (_outside_folder, outside) = repository();
    let ask = ask_for(&outside, &[&outside]);
    let flow = read(&ask, std::slice::from_ref(&listed), at(0));
    assert!(
        !git_held_for(&[&outside, &listed]),
        "git was asked for the read"
    );
    assert_eq!(flow.base, None, "git answered for the folder outside");
    assert_eq!(flow.git, GitAsked::Outside);
    let json = serde_json::to_value(&flow).expect("the flow serializes");
    assert_eq!(json["git"], "outside");
    assert_eq!(
        vouch(&ask, std::slice::from_ref(&listed)).repo,
        None,
        "git would be asked for a folder no listed repository holds"
    );
}

/// A listed repository answers for its own folders — its root, a folder
/// inside it, a linked worktree — and git is asked at the root the window
/// listed, never in the folder the webview named. A listed folder no checkout
/// holds answers for none, and the roots a read's records may lie under are
/// the project's own folders and nothing wider.
#[test]
fn a_listed_repository_is_asked_at_its_listed_root_for_its_own_folders() {
    let (held, listed) = repository();
    let worktree = held.path().join("wt-feature");
    git_in(
        &listed,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            &worktree.to_string_lossy(),
        ],
    );
    let inside = listed.join("crates");
    std::fs::create_dir_all(&inside).expect("a folder inside");
    let (_other_folder, other) = repository();
    let plain = tempfile::tempdir().expect("a plain folder");
    let projects = [plain.path().to_path_buf(), listed.clone()];
    for asked in [&listed, &inside, &worktree] {
        assert_eq!(
            vouch(&ask_for(asked, &[]), &projects).repo.as_ref(),
            Some(&listed),
            "{asked:?} is not asked at the root the window listed"
        );
    }
    assert_eq!(
        vouch(&ask_for(plain.path(), &[plain.path()]), &projects).repo,
        None,
        "a listed folder no checkout holds has git asked for it"
    );
    let ask = ask_for(
        &worktree,
        &[&listed, &worktree, &other, plain.path(), Path::new("/")],
    );
    assert_eq!(
        vouch(&ask, &projects).roots,
        [listed.clone(), worktree.clone()],
        "the records may lie under folders that are not the project's"
    );
    let flow = read(&ask, &projects, at(0));
    assert_eq!(flow.git, GitAsked::Kept);
    assert_eq!(flow.base.as_deref(), Some("main"));
    assert!(
        git_held_for(&[&listed]) && !git_held_for(&[&worktree]),
        "git was asked in the folder the webview named, not at the listed root"
    );
}

/// A read's vouch is kept for the same folder, roots and list for as long as
/// git's answer stands — the window asks again on every ledger change — and
/// asked again when that time is up or the list moves: a repository whose
/// `.git` went away is vouched from the kept answer until then, and a kept
/// answer is the one the next read of the same ask gets.
#[test]
fn a_vouch_is_kept_for_the_same_ask_until_its_time_is_up_or_its_list_moves() {
    let (_held, listed) = repository();
    let ask = ask_for(&listed, &[&listed]);
    let projects = [listed.clone()];
    let mut kept = None;
    let repo = |kept: &mut Option<VouchHeld>, projects: &[PathBuf], now_ms: i64| {
        vouch_kept(kept, &ask, projects, now_ms).repo
    };
    assert_eq!(repo(&mut kept, &projects, at(0)), Some(listed.clone()));
    let gone = listed.join("git-gone");
    std::fs::rename(listed.join(".git"), &gone).expect("the .git goes away");
    assert_eq!(
        repo(&mut kept, &projects, at(0) + FLOW.git_fresh_ms - 1),
        Some(listed.clone()),
        "the same ask was vouched again within the time git's answer stands"
    );
    assert_eq!(
        repo(&mut kept, &projects, at(0) + FLOW.git_fresh_ms),
        None,
        "the time was up and the answer was kept"
    );
    std::fs::rename(&gone, listed.join(".git")).expect("the .git comes back");
    let later = at(0) + FLOW.git_fresh_ms + 1;
    assert_eq!(
        repo(&mut kept, &projects, later),
        None,
        "the kept answer is not the next read's"
    );
    let more = [listed.clone(), PathBuf::from("/a/project/listed/since")];
    assert_eq!(
        repo(&mut kept, &more, later),
        Some(listed.clone()),
        "the list moved and the answer was kept"
    );
}

/// What a read costs on this repository's own history with thirty tasks in the
/// ledger — the briefing's budget is 50 ms: git asked cold and the flow
/// assembled, five times over and judged by the middle one (one busy moment of
/// the machine is not the read's cost), then the answer kept by the base's
/// commit. The measuring job of t-68458 runs it: `cargo test -p zerocode-shell
/// --bin zerocode-shell -- --ignored --nocapture artifact_flow::tests::a_read_of`.
#[test]
#[ignore = "measures a read of this repository's own history (t-68458, job c)"]
fn a_read_of_thirty_tasks_and_twelve_releases_costs_at_most_fifty_ms() {
    const BUDGET_MS: f64 = 50.0;
    const TASKS: u8 = 30;
    const MINUTE_MS: i64 = 60 * 1000;
    const ROUNDS: usize = 5;
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository");
    let project = zerocode_core::git_dir::project_root(&repo)
        .map(|project| project.to_string_lossy().into_owned())
        .expect("a git repository");
    let now = crate::project_runtime::now_epoch_ms();
    let mut ledger = Ledger::new();
    let run = ledger.create_run("acme-measure", now - HOUR_MS);
    assert!(ledger.state_run_project(&run, &project, SEAT, now - HOUR_MS));
    ledger
        .seat_coordinator(&run, SEAT, None, now - HOUR_MS)
        .expect("the coordinator sits");
    for n in 0..TASKS {
        let moment = now - i64::from(TASKS - n) * MINUTE_MS;
        let task = ledger
            .create_task(
                &run,
                format!("task {n}"),
                format!("the {n} work"),
                vec![],
                None,
                moment,
            )
            .expect("a task");
        let body = format!(
            r#"{{"ok":true,"head":"{}","summary":"{task}: 일 {n}을 끝냈다. 시험이 통과했다.","numbers":"{n} → 0"}}"#,
            sha(n)
        );
        handed_at(
            &mut ledger,
            &run,
            &task,
            &format!("%{n}"),
            (moment, moment + 1),
            &body,
        );
    }
    let ms = |spent: Duration| spent.as_secs_f64() * 1000.0;
    let middle = |values: &[f64]| {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        sorted[sorted.len() / 2]
    };
    let said = |values: &[f64]| {
        values
            .iter()
            .map(|one| format!("{one:.1}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let cold_read = || {
        git_held()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        let started = Instant::now();
        let git = git_facts(&repo, now);
        let cold = ms(started.elapsed());
        let started = Instant::now();
        let flow = assemble(&Reading {
            root: repo.to_str().expect("a path"),
            project: Some(&project),
            ledger: Some(&ledger),
            git: &git,
            table: &FLOW,
            now_ms: now,
        });
        (cold, ms(started.elapsed()), git, flow)
    };
    let rounds: Vec<_> = (0..ROUNDS).map(|_| cold_read()).collect();
    let colds: Vec<f64> = rounds.iter().map(|(cold, ..)| *cold).collect();
    let assembles: Vec<f64> = rounds.iter().map(|(_, assembled, ..)| *assembled).collect();
    let (cold, assembled) = (middle(&colds), middle(&assembles));
    let (_, _, git, flow) = rounds.last().expect("the rounds");
    let started = Instant::now();
    let kept = git_facts(&repo, now + 1);
    let held = ms(started.elapsed());
    let started = Instant::now();
    let checked = git_facts(&repo, now + FLOW.git_fresh_ms + 1);
    let rechecked = ms(started.elapsed());
    // The vouch every read now begins with (t-69070 P2): this repository
    // listed, and as roots the repository and its worktrees, as the window
    // sends them.
    let listing = Host::for_workspace(&repo)
        .vcs()
        .text(&repo, &["worktree", "list", "--porcelain"])
        .expect("the worktree list");
    let ask = FlowAsk {
        root: repo.to_string_lossy().into_owned(),
        roots: listing
            .lines()
            .filter_map(|line| line.strip_prefix("worktree "))
            .map(str::to_string)
            .collect(),
    };
    let listed = [repo.clone()];
    let vouches: Vec<f64> = (0..ROUNDS * 3)
        .map(|_| {
            let started = Instant::now();
            let vouched = vouch(&ask, &listed);
            let spent = ms(started.elapsed());
            assert_eq!(vouched.repo.as_ref(), Some(&repo), "this repository");
            spent
        })
        .collect();
    // And the vouch kept for the next read of the same ask.
    let mut kept_vouch = None;
    vouch_kept(&mut kept_vouch, &ask, &listed, now);
    let kept_vouches: Vec<f64> = (0..ROUNDS * 3)
        .map(|_| {
            let started = Instant::now();
            let vouched = vouch_kept(&mut kept_vouch, &ask, &listed, now + 1);
            let spent = ms(started.elapsed());
            assert_eq!(vouched.repo.as_ref(), Some(&repo), "this repository");
            spent
        })
        .collect();
    println!(
        "FLOW-COST git cold p50 {cold:.1} ms ({}) · assemble p50 {assembled:.2} ms ({}) · \
         kept {held:.3} ms · base checked {rechecked:.1} ms · vouch p50 {:.3} ms ({} roots), \
         kept {:.4} ms · releases {} · lines {}",
        said(&colds),
        said(&assembles),
        middle(&vouches),
        ask.roots.len(),
        middle(&kept_vouches),
        git.releases.len(),
        flow.task_ids().len()
    );
    assert!(
        git.releases.len() >= FLOW.releases_kept,
        "the walk saw {} releases",
        git.releases.len()
    );
    assert!(
        flow.task_ids().len() >= usize::from(TASKS),
        "{} lines",
        flow.task_ids().len()
    );
    assert!(
        Arc::ptr_eq(git, &kept) && Arc::ptr_eq(git, &checked),
        "the answer was not kept by the base's commit"
    );
    assert!(
        cold + assembled <= BUDGET_MS,
        "a cold read took {:.1} ms (the middle of {ROUNDS})",
        cold + assembled
    );
}

/// The person's own ledger, read from a read-only copy (t-71220, the person's
/// 10-22 picture of the work project's 「지금 · 36」): what each project's
/// 「지금」 held under the rule before the piles and what it holds now, what
/// the piles hold and what a press over them names, and the board's desk over
/// the same ledger — its 「지금 도는 일」 rows and its 「답할 일」 before and
/// after. The copy is made with `sqlite3 "file:<store>?mode=ro" ".backup
/// <copy>"` and named by `ZEROCODE_DESK_REPLAY_STORE`; it is read through the
/// desk's own reader ([`crate::orchestration::desk::projection_at_rest`]), which
/// snapshots it again into a scratch file and writes nothing to it.
///
/// ```text
/// ZEROCODE_DESK_REPLAY_STORE=<copy> cargo test -p zerocode-shell --bin zerocode-shell \
///   -- --ignored --nocapture artifact_flow::tests::the_persons_ledger
/// ```
#[test]
#[ignore = "a measurement over a copy of the person's own ledger, printed (t-71220)"]
fn the_persons_ledger_counts_now_before_and_after_the_piles() {
    use zerocode_core::orchestration::settle::left_behind;
    use zerocode_core::orchestration::task_cost::TaskCost;
    let store = std::env::var("ZEROCODE_DESK_REPLAY_STORE")
        .expect("ZEROCODE_DESK_REPLAY_STORE names the copy of the store");
    let ledger = Ledger::rebuild(crate::orchestration::desk::projection_at_rest(&store))
        .expect("the ledger as it stands");
    let now = crate::project_runtime::now_epoch_ms();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository");
    let zerocode = zerocode_core::git_dir::project_root(&repo)
        .map(|project| project.to_string_lossy().into_owned())
        .expect("a git repository");
    // Every line drawn: the before is counted off the piles' own lines.
    let table = FlowTable {
        group_tasks_max: usize::MAX,
        ..FLOW
    };
    for (name, project, git) in [
        (
            "work",
            // The person's work root comes from the environment; no machine path is committed.
            std::env::var("ZEROCODE_LEDGER_COPY_WORK_ROOT")
                .unwrap_or_else(|_| "/Users/dev/work".to_string()),
            Arc::new(GitFacts::default()),
        ),
        ("zerocode", zerocode, git_facts(&repo, now)),
    ] {
        let started = Instant::now();
        let flow = assemble(&Reading {
            root: &project,
            project: Some(&project),
            ledger: Some(&ledger),
            git: &git,
            table: &table,
            now_ms: now,
        });
        let spent = started.elapsed().as_secs_f64() * 1000.0;
        // Before the piles, a line stood in 「지금」 when it was in view: touched
        // since the oldest kept release, or every line where git kept none.
        let horizon = git
            .releases
            .get(FLOW.releases_kept)
            .map(|release| release.at_ms);
        let seen = |pile: &FlowPile| {
            pile.tasks
                .iter()
                .filter(|line| horizon.is_none_or(|horizon| line.last_ms >= horizon))
                .count()
        };
        let looks = flow
            .left
            .tasks
            .iter()
            .filter(|line| line.left == Some(Left::Look))
            .count();
        println!(
            "LEDGER-COPY {name}: 지금 {} → {} · 정리 필요 {} (a settle closes {}, keeps {looks} for a look) · \
             검토 없이 끝남 {} (a waiver names {}) · assembled in {spent:.1} ms",
            flow.now.tasks.len() + seen(&flow.left) + seen(&flow.unreviewed),
            flow.now.tasks.len(),
            flow.left.tasks.len(),
            flow.left.named.len(),
            flow.unreviewed.tasks.len(),
            flow.unreviewed.named.len(),
        );
        for line in flow.left.tasks.iter().chain(&flow.unreviewed.tasks) {
            println!(
                "LEDGER-COPY {name} pile {:?} {} {} {:.0} days — {}",
                line.left,
                line.run.as_deref().unwrap_or("-"),
                line.task,
                now.saturating_sub(line.last_ms) as f64 / 86_400_000.0,
                line.title
            );
        }
    }
    // What the reading itself costs over every run the ledger holds: the desk's beat and every
    // flow read now take it beside the stages they read already.
    let rounds: Vec<f64> = (0..5)
        .map(|_| {
            let started = Instant::now();
            let left: usize = ledger
                .runs()
                .iter()
                .map(|run| left_behind(run, now).len())
                .sum();
            assert!(left > 0, "the copy holds work nobody is coming back to");
            started.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    let mut sorted = rounds.clone();
    sorted.sort_by(f64::total_cmp);
    println!(
        "LEDGER-COPY left_behind over {} runs and {} tasks: p50 {:.2} ms ({})",
        ledger.runs().len(),
        ledger
            .runs()
            .iter()
            .map(|run| run.tasks.len())
            .sum::<usize>(),
        sorted[sorted.len() / 2],
        rounds
            .iter()
            .map(|one| format!("{one:.2}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let started = Instant::now();
    let desk =
        crate::orchestration::desk::desk_snapshot(&ledger, |_| None, |_, _| TaskCost::default());
    let spent = started.elapsed().as_secs_f64() * 1000.0;
    let stage = |name: &str| {
        desk.stages
            .iter()
            .find(|one| one.stage == name)
            .map_or((0, 0), |one| (one.count, one.left))
    };
    let running = [
        "pending",
        "ready",
        "dispatched",
        "gate",
        "blocked",
        "failed",
    ];
    let (rows, left_rows) = running.iter().fold((0, 0), |(all, left), name| {
        let (count, behind) = stage(name);
        (all + count, left + behind)
    });
    let (reported, reported_left) = stage("reported");
    let owed = desk.counts.mail + stage("gate").0 + reported;
    println!(
        "LEDGER-COPY desk: 지금 도는 일 {rows} → {} · 답할 일 {owed} → {} · 정리 필요 {} (closes {}) · \
             검토 없이 끝남 {} · desk read in {spent:.1} ms",
        rows - left_rows,
        owed - reported_left,
        desk.left.settle.len() + desk.left.look.len(),
        desk.left.settle.len(),
        desk.left.unreviewed.len(),
    );
}

/* ---- the review t-69070's P3 (t-69107) ------------------------------------- */

/// ① A merge on this project's base that names a task another project's run
/// holds is drawn from git alone — its attempts, model and hand-in summary are
/// that other project's — while a run that states no project lends its tasks.
#[test]
fn a_task_another_projects_run_holds_is_drawn_from_git_alone() {
    let fixture = fixture();
    let flow = flow_of(&fixture, &table());
    let carried_in = line(&flow, &fixture.ids["carried-in"]);
    assert_ne!(
        carried_in.run.as_deref(),
        Some(fixture.other_run.as_str()),
        "the line names another project's run"
    );
    assert!(
        carried_in.run.is_none()
            && carried_in.attempts.is_empty()
            && carried_in.summary.is_none()
            && carried_in.hand_in.is_none()
            && carried_in.ledger_stage.is_none(),
        "{carried_in:#?}"
    );
    // A run with no project of its own is nobody else's: its task is shown.
    let mut ledger = Ledger::new();
    let loose = ledger.create_run("loose", at(0));
    let id = ledger
        .create_task(
            &loose,
            "do it".into(),
            "the loose work".into(),
            vec![],
            None,
            at(1),
        )
        .expect("a task");
    let (unreleased, releases) = parse_log(&merge_line(
        1,
        10,
        &format!("Merge wt/{id} at 1111111 ({id}) — the loose work lands"),
    ));
    let flow = assemble(&Reading {
        root: "/acme/app",
        project: Some(PROJECT),
        ledger: Some(&ledger),
        git: &GitFacts {
            base: Some("main".to_string()),
            unreleased,
            releases,
            ..GitFacts::default()
        },
        table: &table(),
        now_ms: at(READ_AT),
    });
    assert_eq!(line(&flow, &id).run.as_deref(), Some(loose.as_str()));
}

fn commit_in(repo: &Path, message: &str) {
    let file = repo
        .parent()
        .expect("the folder holding it")
        .join("message.txt");
    std::fs::write(&file, message).expect("a message");
    git_in(
        repo,
        &[
            "-c",
            "user.name=ZeroCode Test",
            "-c",
            "user.email=test@zerocode",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--allow-empty",
            "-F",
            &file.to_string_lossy(),
        ],
    );
}

fn no_held() -> Mutex<Vec<GitHeld>> {
    Mutex::new(Vec::new())
}

/// ② What git answers is read to a bound: a changelog past it gives no notes
/// and does not stop the rest, and a log past it is no answer at all — a repository
/// somebody else wrote is not read whole into this window's memory.
#[test]
fn what_git_answers_is_read_to_a_bound() {
    const BIG: usize = 5 * 1024 * 1024;
    let (_held, repo) = repository();
    std::fs::write(
        repo.join(FLOW.notes_file),
        format!(
            "# Changelog\n\n## [1.0.0] — 2026-10-25\n\n- the note (t-1)\n\n{}",
            "- padding\n".repeat(BIG / 10)
        ),
    )
    .expect("a heavy changelog");
    git_in(&repo, &["add", FLOW.notes_file]);
    commit_in(&repo, "release: v1.0.0\n");
    git_in(&repo, &["tag", "v1.0.0"]);
    let facts = git_facts_in(&no_held(), 8, &repo, at(0));
    assert_eq!(
        facts.base.as_deref(),
        Some("main"),
        "the rest of the answer stands"
    );
    assert_eq!(facts.releases.len(), 1);
    assert!(
        facts.notes.is_empty(),
        "a changelog of {} MiB was read whole",
        BIG / (1024 * 1024)
    );
    // A subject past any bound: the walk is not answered.
    commit_in(&repo, &format!("{}\n", "x".repeat(BIG)));
    let facts = git_facts_in(&no_held(), 8, &repo, at(0));
    assert!(
        facts.base.is_none() && facts.releases.is_empty(),
        "a log of {} MiB was read whole",
        BIG / (1024 * 1024)
    );
}

/// The ref the base stands on, made unreadable and given back.
fn break_main(repo: &Path) -> impl FnOnce() {
    let path = repo.join(".git/refs/heads/main");
    let was = std::fs::read_to_string(&path).expect("main is a loose ref");
    std::fs::write(&path, "not a commit\n").expect("a broken ref");
    move || std::fs::write(&path, was).expect("the ref back")
}

/// ③ git failing once does not blank a flow the window already draws: the last
/// answer stands, says it is being read again, and git is not asked again for
/// as long as a failure is remembered.
#[test]
fn a_git_that_fails_leaves_the_last_answer_standing_and_is_not_asked_again_at_once() {
    let (_held, repo) = repository();
    let held = no_held();
    let good = git_facts_in(&held, 8, &repo, at(0));
    assert_eq!((good.base.as_deref(), good.stale), (Some("main"), false));
    let restore = break_main(&repo);
    let failed_at = at(0) + FLOW.git_fresh_ms + 1;
    let stale = git_facts_in(&held, 8, &repo, failed_at);
    assert_eq!(
        (stale.base.as_deref(), stale.stale),
        (Some("main"), true),
        "one failed read emptied the flow"
    );
    restore();
    // git would answer now, but a failure is remembered for a while.
    let within = git_facts_in(&held, 8, &repo, failed_at + FLOW.git_fresh_ms + 1);
    assert!(within.stale, "git was asked again at once after a failure");
    let later = git_facts_in(&held, 8, &repo, failed_at + FLOW.git_retry_ms + 1);
    assert_eq!((later.base.as_deref(), later.stale), (Some("main"), false));
}

/// ③ The read says so: a flow whose git answer is the last one says `stale`.
#[test]
fn a_flow_read_over_a_stale_answer_says_so() {
    let (_held, repo) = repository();
    let ask = ask_for(&repo, &[&repo]);
    let projects = [repo.clone()];
    let fresh_at = at(1_000);
    let first = read(&ask, &projects, fresh_at);
    assert_eq!(first.git, GitAsked::Kept);
    let restore = break_main(&repo);
    let flow = read(&ask, &projects, fresh_at + FLOW.git_fresh_ms + 1);
    restore();
    assert_eq!(
        serde_json::to_value(&flow).expect("the flow serializes")["git"],
        "stale"
    );
    assert_eq!(flow.base.as_deref(), Some("main"));
}

/// ④ A subject that holds the walk's own field mark does not move the tags
/// into the subject's place: no release a subject names is a release.
#[test]
fn a_subject_with_the_field_mark_makes_no_release() {
    let (_held, repo) = repository();
    commit_in(&repo, "evil\u{1f}tag: v9.9.9\u{1f}x\n");
    let facts = git_facts_in(&no_held(), 8, &repo, at(0));
    let versions: Vec<&str> = facts
        .releases
        .iter()
        .map(|release| release.version.as_str())
        .collect();
    assert!(
        versions.is_empty(),
        "a subject made a release: {versions:?}"
    );
}

/// ⑤ The walk names its revision and then ends the revisions with `--`: a file
/// in the work tree called `main` does not make `main` ambiguous.
#[test]
fn a_file_named_like_the_branch_does_not_make_the_walk_ambiguous() {
    let (_held, repo) = repository();
    std::fs::write(repo.join("main"), "a file\n").expect("a file named main");
    let walked = format!("{:?}", walk_at(&repo, "main"));
    assert!(walked.contains("first"), "the walk was refused: {walked}");
}

/// ⑥ The eight repositories kept are the last used, not the last read whole:
/// a repository asked again keeps its place, and the one not asked for longest
/// goes.
#[test]
fn the_repositories_kept_are_the_last_used() {
    let held = no_held();
    let made: Vec<_> = (0..3).map(|_| repository()).collect();
    let [(_a, one), (_b, two), (_c, three)] = [&made[0], &made[1], &made[2]];
    let now = at(0);
    git_facts_in(&held, 2, one, now);
    git_facts_in(&held, 2, two, now);
    // `one` is asked again within the time its answer stands, so it is the last used.
    git_facts_in(&held, 2, one, now + 1);
    git_facts_in(&held, 2, three, now + 2);
    let roots: Vec<PathBuf> = held
        .lock()
        .expect("held")
        .iter()
        .map(|kept| kept.root.clone())
        .collect();
    assert!(
        roots.contains(one) && roots.contains(three) && !roots.contains(two),
        "the one asked longest ago was kept: {roots:?}"
    );
}

/// ⑥ A trailing slash is the same repository.
#[test]
fn a_path_with_a_trailing_slash_is_the_same_repository() {
    let (_held, repo) = repository();
    let held = no_held();
    git_facts_in(&held, 8, &repo, at(0));
    let slashed = PathBuf::from(format!("{}/", repo.display()));
    git_facts_in(&held, 8, &slashed, at(0) + 1);
    assert_eq!(held.lock().expect("held").len(), 1);
}
