//! Every lane's child holds the pane's terminal as its controlling terminal
//! (t-77414), on macOS, where the child starts by `posix_spawn`.
//!
//! On 2026-10-27 a restart left five zo processes spinning at 97 % CPU, each
//! holding its conversation's writer lease, so the new window's resumes were
//! refused. A spawn's open of the slave gives the child no controlling
//! terminal, and zo never opens its terminal again, so the window going sent
//! it no hangup. The earlier shape tests in `lane.rs` run `/bin/sh`, which
//! opens its terminal again and takes it itself — they passed over the gap.
//!
//! So the programs here never open their terminal: this binary itself
//! (`program`, which only reads what the kernel says of it and records the
//! signals it gets) and `/bin/sleep`. The binary is also the helper the lanes
//! start through, as the window's mirror is ([`zerocode_pty::launch_through`]),
//! and a stand-in for the window (`window`), which leaves without closing
//! its lane so the kernel closes the master, as when the window goes.
//! Hence its own `main` (`harness = false`). No real agent runs.
//!
//! As the helper it also stands in for the deployed mirror's two sides the
//! window cannot see from here (t-77495): dyld takes every `DYLD_*` out of a
//! helper signed with the hardened runtime before its `main` runs
//! (`forget_dyld` does the same), and a mirror built before the
//! road (`OLD_MIRROR`) starts its first argument and knows no marker.
//!
//! And for the window's question to its helper (t-77530, Codex t-77525):
//! asked with [`PROBE_GATE`] in its environment, this binary answers only
//! once that file exists — a mirror that is slow to answer, or one that
//! never does.

fn main() {
    #[cfg(target_os = "macos")]
    macos::main();
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{CString, OsString};
    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use zerocode_pty::PtyLane;

    /// How long a case waits for what it is sure will come.
    const PATIENCE: Duration = Duration::from_secs(20);
    /// How long a synthetic program stands at most, so a case that failed
    /// leaves nothing running for long.
    const PROGRAM_LIFETIME: Duration = Duration::from_secs(60);

    /// The signals a synthetic program records, by name.
    const RECORDED: [(libc::c_int, &str); 4] = [
        (libc::SIGHUP, "HUP"),
        (libc::SIGWINCH, "WINCH"),
        (libc::SIGINT, "INT"),
        (libc::SIGTSTP, "TSTP"),
    ];
    static SEEN: [AtomicBool; 4] = [const { AtomicBool::new(false) }; 4];

    /// Asked the window's question with this set, the binary answers only
    /// once the file it names exists.
    const PROBE_GATE: &str = "ZEROCODE_TEST_PROBE_GATE";

    /// A helper whose answer is not the road's.
    const WRONG_ANSWER: &str = "#!/bin/sh\nprintf 'zerocode-ctty-exec 2'\n";

    /// A mirror built before the road (t-77414), in its own words: its first
    /// argument is the program, and the environment is not its business.
    const OLD_MIRROR: &str = "#!/bin/sh\n\
        [ \"$#\" -gt 0 ] || { echo 'zerocode-mirror: real binary path missing' >&2; exit 127; }\n\
        real=$1\n\
        shift\n\
        exec \"$real\" \"$@\"\n";

    pub(super) fn main() {
        // Called as a helper, this binary is a hardened one: dyld has taken
        // every `DYLD_*` out before `main`.
        if std::env::var_os(zerocode_pty::CTTY_EXEC_ENV).is_some() {
            forget_dyld();
        }
        if std::env::var_os(zerocode_pty::CTTY_PROBE_ENV).is_some()
            && let Some(gate) = std::env::var_os(PROBE_GATE)
        {
            let gate = PathBuf::from(gate);
            wait_for_within(|| gate.exists(), PROGRAM_LIFETIME);
        }
        // The helper's side, first, as the mirror's `main` does.
        zerocode_pty::serve_ctty_exec();
        let args: Vec<OsString> = std::env::args_os().skip(1).collect();
        match args.first().and_then(|first| first.to_str()) {
            Some("program") => program(&args[1..]),
            Some("window") => window(&args[1..]),
            Some("old-window") => old_window(&args[1..]),
            Some("asking-window") => asking_window(&args[1..]),
            Some("env-window") => env_window(&args[1..]),
            Some("leftover") => leftover(&args[1..]),
            _ => run_cases(),
        }
    }

    /// What dyld does to a hardened binary's environment before its `main`.
    fn forget_dyld() {
        let names: Vec<OsString> = std::env::vars_os()
            .map(|(name, _)| name)
            .filter(|name| name.as_encoded_bytes().starts_with(b"DYLD_"))
            .collect();
        for name in names {
            // SAFETY: first thing in `main`, before this process has a
            // second thread.
            unsafe { std::env::remove_var(name) };
        }
    }

    /// The environment's helper names and `DYLD_*` names, as `NAME=value`.
    fn launch_env(vars: impl Iterator<Item = (OsString, OsString)>) -> Vec<String> {
        vars.map(|(name, value)| format!("{}={}", name.to_string_lossy(), value.to_string_lossy()))
            .filter(|pair| pair.starts_with("ZEROCODE_CTTY") || pair.starts_with("DYLD_"))
            .collect()
    }

    fn exe() -> PathBuf {
        std::env::current_exe().expect("this test's own path")
    }

    fn exe_str() -> String {
        exe().to_str().expect("a UTF-8 test path").to_string()
    }

    // ---- the synthetic program ----

    extern "C" fn on_signal(signal: libc::c_int) {
        for (at, (recorded, _)) in RECORDED.iter().enumerate() {
            if *recorded == signal {
                SEEN[at].store(true, Ordering::SeqCst);
            }
        }
    }

    /// `program <report> [grandchild]`: says what the kernel says of it —
    /// its pid, session, its terminal's session and front group, whether
    /// `/dev/tty` opens — then records each signal it gets, and leaves on a
    /// hangup. With `grandchild`, it first starts itself again as a child of
    /// its own, in its group, as a shell runs a command.
    fn program(args: &[OsString]) {
        let report = PathBuf::from(&args[0]);
        let grandchild = args.get(1).is_some_and(|word| word == "grandchild");
        for (signal, _) in RECORDED {
            let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            // SAFETY: the handler only stores to an atomic.
            unsafe { libc::signal(signal, handler) };
        }
        let mut out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&report)
            .expect("the program's report");
        let mut say = |line: String| {
            out.write_all(format!("{line}\n").as_bytes())
                .expect("a report line");
        };
        // SAFETY: plain questions of the kernel about this process and its
        // descriptor 0.
        let (pid, session, terminal_session, front) = unsafe {
            (
                libc::getpid(),
                libc::getsid(0),
                libc::tcgetsid(0),
                libc::tcgetpgrp(0),
            )
        };
        say(format!("pid {pid}"));
        say(format!("session {session}"));
        say(format!("terminal-session {terminal_session}"));
        say(format!("front {front}"));
        let tty = CString::new("/dev/tty").expect("a path");
        // SAFETY: an open that claims nothing (`O_NOCTTY`), closed at once.
        let fd = unsafe { libc::open(tty.as_ptr(), libc::O_RDWR | libc::O_NOCTTY) };
        if fd >= 0 {
            // SAFETY: opened above.
            unsafe { libc::close(fd) };
        }
        say(format!(
            "dev-tty {}",
            if fd >= 0 { "opens" } else { "none" }
        ));
        for pair in launch_env(std::env::vars_os()) {
            say(format!("env {pair}"));
        }
        let mut child = grandchild.then(|| {
            let mut below = report.clone().into_os_string();
            below.push(".grandchild");
            std::process::Command::new(exe())
                .arg("program")
                .arg(below)
                .spawn()
                .expect("the grandchild")
        });
        if let Some(child) = &child {
            say(format!("grandchild {}", child.id()));
        }
        say("ready".to_string());
        let mut told = [false; 4];
        let until = Instant::now() + PROGRAM_LIFETIME;
        while Instant::now() < until {
            for (at, (_, name)) in RECORDED.iter().enumerate() {
                if !told[at] && SEEN[at].load(Ordering::SeqCst) {
                    told[at] = true;
                    say(format!("signal {name}"));
                }
            }
            if told[0] {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if let Some(child) = child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        std::process::exit(0);
    }

    // ---- the stand-in window ----

    /// `window <report> <program|sleep> <helper|direct>`: starts one lane —
    /// through this binary as the helper, or straight as every start was
    /// before — says its child's pid on stdout once the child stands, and
    /// leaves without closing the lane, so the kernel closes the master.
    fn window(args: &[OsString]) {
        let report = PathBuf::from(&args[0]);
        let kind = args[1].to_str().expect("a kind");
        if args[2] == "helper" {
            assert!(zerocode_pty::launch_through(exe(), None, |_| {}));
        }
        let lane = match kind {
            "program" => spawn_program(&report, false),
            _ => {
                PtyLane::spawn("/bin/sleep", &["60".to_string()], None, &[], 24, 80).expect("sleep")
            }
        };
        let pid = lane.pid().expect("a child pid");
        if kind == "program" {
            wait_for_line(&report, "ready");
        } else {
            wait_until(
                || executable(pid).as_deref() == Some("/bin/sleep"),
                "sleep to start",
            );
        }
        println!("{pid}");
        std::io::stdout().flush().expect("stdout");
        // No drop: the lane's master closes with this process, as the
        // window's does when it goes.
        std::mem::forget(lane);
        std::process::exit(0);
    }

    /// `old-window <report> <helper>`: names `helper` and says whether it
    /// was taken, then starts a program on a lane, says whether it stood,
    /// and closes the lane.
    fn old_window(args: &[OsString]) {
        let report = PathBuf::from(&args[0]);
        let (told, unnamed) = std::sync::mpsc::channel();
        zerocode_pty::launch_through(
            PathBuf::from(&args[1]),
            Some(report.with_extension("log")),
            move |why| {
                let _ = told.send(why);
            },
        );
        let lane = spawn_program(&report, false);
        let stood = wait_for(|| lines_of(&report).iter().any(|line| line == "ready"));
        println!("named {}", unnamed.recv_timeout(PATIENCE).is_err());
        println!("stood {stood}");
        std::io::stdout().flush().expect("stdout");
        drop(lane);
        std::process::exit(0);
    }

    /// `asking-window <report> <helper> <limit in ms> [go]`: what a window's
    /// boot does — names `helper` with that limit for its question — and
    /// says it went on (`booted`); with `go`, only then lets the helper
    /// answer ([`PROBE_GATE`]). Then it starts a program on a lane at once,
    /// before any answer could be in, and says which road the program
    /// came by, why the helper was not named if it was not, and how many
    /// of this process's children are left besides the lane's — a
    /// question's that was not reaped is one. `done` last.
    fn asking_window(args: &[OsString]) {
        let report = PathBuf::from(&args[0]);
        let limit = Duration::from_millis(
            args[2]
                .to_str()
                .and_then(|ms| ms.parse().ok())
                .expect("a limit"),
        );
        let (told, unnamed) = std::sync::mpsc::channel();
        let asked = zerocode_pty::launch_through_within(
            PathBuf::from(&args[1]),
            Some(report.with_extension("log")),
            limit,
            move |why| {
                let _ = told.send(why);
            },
        );
        let say = |line: String| {
            let mut out = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&report)
                .expect("the window's report");
            out.write_all(format!("{line}\n").as_bytes())
                .expect("a report line");
        };
        say(format!("booted {asked}"));
        if args.get(3).is_some_and(|word| word == "go") {
            let gate = std::env::var_os(PROBE_GATE).expect("a gate");
            std::fs::write(gate, b"").expect("the gate");
        }
        let program = report.with_extension("program");
        let lane = spawn_program(&program, false);
        let pid = lane.pid().expect("a child pid");
        let lines = wait_for_line(&program, "ready");
        let through = field(&lines, "terminal-session") == Some(pid.to_string());
        say(format!(
            "road {}",
            if through { "helper" } else { "straight" }
        ));
        let why = if through {
            unnamed.try_recv().ok()
        } else {
            unnamed.recv_timeout(PATIENCE).ok()
        };
        say(why.map_or_else(|| "named".to_string(), |why| format!("unnamed {why}")));
        let left = children()
            .into_iter()
            .filter(|child| u32::try_from(*child).ok() != Some(pid))
            .count();
        say(format!("questions left {left}"));
        drop(lane);
        say("done".to_string());
        std::process::exit(0);
    }

    /// `env-window <report> <value>`: starts a program on a lane straight —
    /// no helper named — with [`user_names`] given to it, and leaves.
    fn env_window(args: &[OsString]) {
        let report = PathBuf::from(&args[0]);
        let value = args[1].to_str().expect("a UTF-8 value");
        let lane = PtyLane::spawn(
            &exe_str(),
            &[
                "program".to_string(),
                report.to_str().expect("UTF-8").to_string(),
            ],
            None,
            &user_names(value),
            24,
            80,
        )
        .expect("the program");
        wait_for_line(&report, "ready");
        drop(lane);
        std::process::exit(0);
    }

    /// What a person may give a pane in the settings' environment field
    /// that looks like the names the window wraps a `DYLD_*` value in
    /// (Codex t-77525): one of the same shape, one that wraps a name the
    /// pane is also given, one wrapped twice over, and one beside them.
    fn user_names(value: &str) -> Vec<(String, String)> {
        [
            ("ZEROCODE_CTTY_DYLD_REVIEW_CUSTOM", "user-setting"),
            ("DYLD_LIBRARY_PATH", value),
            ("ZEROCODE_CTTY_DYLD_LIBRARY_PATH", "user-own"),
            ("ZEROCODE_CTTY_ZEROCODE_CTTY_DYLD_X", "deep"),
            ("ZEROCODE_CTTY_NOT_DYLD", "keep-this"),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
    }

    /// `leftover <report>`: what a mirror's own road — its tee and its
    /// guard — reaches after the helper's side has had its look: one line
    /// to say so, then what a program this process starts holds.
    fn leftover(args: &[OsString]) {
        let report = PathBuf::from(&args[0]);
        let started = std::process::Command::new("/usr/bin/env")
            .output()
            .expect("env");
        let mut lines = vec!["own road".to_string()];
        lines.extend(
            String::from_utf8_lossy(&started.stdout)
                .lines()
                .filter_map(|line| line.split_once('='))
                .map(|(name, value)| (OsString::from(name), OsString::from(value)))
                .map(|pair| launch_env(std::iter::once(pair)))
                .flat_map(|pairs| pairs.into_iter().map(|pair| format!("env {pair}"))),
        );
        std::fs::write(&report, lines.join("\n") + "\n").expect("the report");
        std::process::exit(0);
    }

    // ---- the cases ----

    /// One case: its own folder in, why it failed out.
    type Case = fn(&Path) -> Result<(), String>;

    fn run_cases() {
        let scratch = tempfile::tempdir().expect("a scratch folder");
        // Before: the same lanes started straight, as every start was before
        // t-77414. Counted, not asserted — they say what the platform does.
        let before = census(scratch.path(), "direct");
        assert!(zerocode_pty::launch_through(exe(), None, |why| {
            eprintln!("this binary was not named a helper: {why}");
        }));
        let cases: [(&str, Case); 13] = [
            (
                "a_program_that_never_opens_its_terminal_holds_it_as_controlling",
                a_program_that_never_opens_its_terminal_holds_it_as_controlling,
            ),
            (
                "sleep_stands_on_the_pane_terminal_in_front",
                sleep_stands_on_the_pane_terminal_in_front,
            ),
            (
                "the_master_closing_hangs_up_the_child",
                the_master_closing_hangs_up_the_child,
            ),
            (
                "a_resize_reaches_a_grandchild_under_the_child",
                a_resize_reaches_a_grandchild_under_the_child,
            ),
            (
                "an_interrupt_and_a_suspend_reach_the_front_group",
                an_interrupt_and_a_suspend_reach_the_front_group,
            ),
            (
                "a_helper_from_before_the_road_is_not_named",
                a_helper_from_before_the_road_is_not_named,
            ),
            (
                "a_leftover_marker_does_not_make_a_call_a_pane_start",
                a_leftover_marker_does_not_make_a_call_a_pane_start,
            ),
            (
                "a_dyld_value_reaches_the_program_past_a_hardened_helper",
                a_dyld_value_reaches_the_program_past_a_hardened_helper,
            ),
            (
                "a_silent_helper_holds_neither_the_boot_nor_a_pane",
                a_silent_helper_holds_neither_the_boot_nor_a_pane,
            ),
            (
                "a_helper_that_answers_after_the_boot_went_on_is_named",
                a_helper_that_answers_after_the_boot_went_on_is_named,
            ),
            ("a_wrong_answer_is_not_named", a_wrong_answer_is_not_named),
            (
                "names_like_a_wrapped_one_reach_a_program_past_the_helper_as_given",
                names_like_a_wrapped_one_reach_a_program_past_the_helper_as_given,
            ),
            (
                "names_like_a_wrapped_one_reach_a_program_started_straight_as_given",
                names_like_a_wrapped_one_reach_a_program_started_straight_as_given,
            ),
        ];
        let mut failed = 0;
        for (name, case) in cases {
            let folder = scratch.path().join(name);
            std::fs::create_dir_all(&folder).expect("a case folder");
            match case(&folder) {
                Ok(()) => println!("test {name} ... ok"),
                Err(why) => {
                    failed += 1;
                    println!("test {name} ... FAILED\n    {why}");
                }
            }
        }
        let after = census(scratch.path(), "helper");
        println!("census (3 programs, 3 sleeps per road):");
        println!("  straight:      {before}");
        println!("  through helper: {after}");
        println!(
            "\ntest result: {}. {} passed; {failed} failed",
            if failed == 0 { "ok" } else { "FAILED" },
            cases.len() - failed
        );
        if failed > 0 {
            std::process::exit(1);
        }
    }

    /// A program, started by a lane through the helper, holds the pane's
    /// terminal as its controlling terminal with its group in front, and is
    /// the very process the lane started: the same pid and beginning, which
    /// is what the window writes down for its way out (t-76434).
    fn a_program_that_never_opens_its_terminal_holds_it_as_controlling(
        folder: &Path,
    ) -> Result<(), String> {
        let report = folder.join("report");
        let lane = spawn_program(&report, false);
        let pid = lane.pid().ok_or("no child pid")?;
        let began = started_us(pid).ok_or("the child's beginning was not read")?;
        let lines = wait_for_line(&report, "ready");
        let said = |key: &str| field(&lines, key);
        check(
            said("pid") == Some(pid.to_string()),
            format!("pid: {lines:?} vs {pid}"),
        )?;
        check(
            started_us(pid) == Some(began),
            "the beginning moved".to_string(),
        )?;
        for key in ["session", "terminal-session", "front"] {
            check(
                said(key) == Some(pid.to_string()),
                format!("{key}: {lines:?}"),
            )?;
        }
        check(
            said("dev-tty").as_deref() == Some("opens"),
            format!("{lines:?}"),
        )?;
        check(
            lane.foreground_is_child() == Some(true),
            "the window does not see its child in front".to_string(),
        )
    }

    /// `/bin/sleep`, which never touches its terminal, stands on the pane's
    /// terminal in front — `ps` says a tty and `+`.
    fn sleep_stands_on_the_pane_terminal_in_front(_folder: &Path) -> Result<(), String> {
        let lane = PtyLane::spawn("/bin/sleep", &["30".to_string()], None, &[], 24, 80)
            .map_err(|error| error.to_string())?;
        let pid = lane.pid().ok_or("no child pid")?;
        wait_until(
            || executable(pid).as_deref() == Some("/bin/sleep"),
            "sleep to start",
        );
        let (tty, stat) = ps_terminal(pid).ok_or("ps said nothing")?;
        check(
            tty != "??" && stat.contains('+'),
            format!("sleep stands on {tty} {stat}"),
        )?;
        check(
            lane.foreground_is_child() == Some(true),
            "the window does not see sleep in front".to_string(),
        )
    }

    /// The window going closes the master, and the child — a program, and
    /// `/bin/sleep` — gets the hangup and ends, without the window sending
    /// any signal of its own.
    fn the_master_closing_hangs_up_the_child(folder: &Path) -> Result<(), String> {
        let report = folder.join("report");
        let pid = window_leaves(&report, "program", "helper")?;
        let hung = wait_for(|| lines_of(&report).iter().any(|line| line == "signal HUP"));
        let ended = wait_for(|| gone(pid));
        if !ended {
            kill(pid);
        }
        check(
            hung,
            format!("the program got no hangup: {:?}", lines_of(&report)),
        )?;
        check(ended, format!("program {pid} outlived its master"))?;
        let pid = window_leaves(&folder.join("unused"), "sleep", "helper")?;
        let ended = wait_for(|| gone(pid));
        if !ended {
            kill(pid);
        }
        check(ended, format!("sleep {pid} outlived its master"))
    }

    /// A resize reaches what the child runs — its grandchild, in its group —
    /// as SIGWINCH.
    fn a_resize_reaches_a_grandchild_under_the_child(folder: &Path) -> Result<(), String> {
        let report = folder.join("report");
        let below = folder.join("report.grandchild");
        let mut lane = spawn_program(&report, true);
        wait_for_line(&report, "ready");
        wait_for_line(&below, "ready");
        lane.resize(30, 100).map_err(|error| error.to_string())?;
        check(
            wait_for(|| lines_of(&below).iter().any(|line| line == "signal WINCH")),
            format!("the grandchild got no SIGWINCH: {:?}", lines_of(&below)),
        )
    }

    /// ^C and ^Z typed into the pane reach its front group — the child and
    /// its grandchild — as SIGINT and SIGTSTP.
    fn an_interrupt_and_a_suspend_reach_the_front_group(folder: &Path) -> Result<(), String> {
        let report = folder.join("report");
        let below = folder.join("report.grandchild");
        let mut lane = spawn_program(&report, true);
        wait_for_line(&report, "ready");
        wait_for_line(&below, "ready");
        lane.write_input(b"\x03")
            .map_err(|error| error.to_string())?;
        for (who, file) in [("child", &report), ("grandchild", &below)] {
            check(
                wait_for(|| lines_of(file).iter().any(|line| line == "signal INT")),
                format!("the {who} got no SIGINT: {:?}", lines_of(file)),
            )?;
        }
        lane.write_input(b"\x1a")
            .map_err(|error| error.to_string())?;
        check(
            wait_for(|| lines_of(&below).iter().any(|line| line == "signal TSTP")),
            format!("the grandchild got no SIGTSTP: {:?}", lines_of(&below)),
        )
    }

    /// A mirror built before the road is not named the lanes' helper: it
    /// cannot answer the window's question, so the lanes start straight, as
    /// before the road. Named, it started the program but left the marker in
    /// its environment, and a nested launch under the program then took the
    /// mirror's helper side past its guard (t-77495, Codex t-77467:
    /// refused with 2 without the marker, run with 0 with it).
    fn a_helper_from_before_the_road_is_not_named(folder: &Path) -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt as _;
        let old = folder.join("zerocode-mirror");
        std::fs::write(&old, OLD_MIRROR).map_err(|error| error.to_string())?;
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| error.to_string())?;
        let report = folder.join("report");
        let out = std::process::Command::new(exe())
            .arg("old-window")
            .arg(&report)
            .arg(&old)
            .output()
            .map_err(|error| error.to_string())?;
        let said = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let lines = lines_of(&report);
        check(
            said.lines().next() == Some("named false"),
            format!(
                "the old mirror was {said:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            ),
        )?;
        check(
            said.lines().nth(1) == Some("stood true"),
            format!("the program did not start: {lines:?}"),
        )?;
        check(
            !lines
                .iter()
                .any(|line| line.starts_with("env ZEROCODE_CTTY")),
            format!("a marker reached the program: {lines:?}"),
        )
    }

    /// A marker that outlived its helper — every launch under the program
    /// inherits it — does not make the next call of a helper a pane's start:
    /// the helper side takes it out and returns, the binary goes its own
    /// road (the mirror's: its tee and its guard), and neither marker
    /// reaches what that road starts (t-77495). A name of the shape a
    /// wrapped `DYLD_*` travels under is not the window's there — the window
    /// wraps only on a start through a helper, which gives every one back —
    /// so it is the person's, and goes on as given (Codex t-77525).
    fn a_leftover_marker_does_not_make_a_call_a_pane_start(folder: &Path) -> Result<(), String> {
        let report = folder.join("report");
        let out = std::process::Command::new(exe())
            .arg("leftover")
            .arg(&report)
            .env(zerocode_pty::CTTY_EXEC_ENV, "/bin/sh")
            .env(zerocode_pty::CTTY_LOG_ENV, folder.join("log"))
            .env("ZEROCODE_CTTY_DYLD_LIBRARY_PATH", folder)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| error.to_string())?;
        let lines = lines_of(&report);
        check(
            out.status.success() && lines.first().is_some_and(|line| line == "own road"),
            format!(
                "the call became a start ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            ),
        )?;
        let folder = folder.to_str().ok_or("a UTF-8 folder")?;
        check(
            launch_names(&lines) == [format!("ZEROCODE_CTTY_DYLD_LIBRARY_PATH={folder}")],
            format!("a marker went on, or the person's name did not: {lines:?}"),
        )
    }

    /// A boot whose helper never answers its question goes on at once, and
    /// a pane started before any answer waits out no more than the
    /// question's limit, then starts straight; the question's process is
    /// ended and reaped, and the window hears why (Codex t-77525: the boot
    /// stood more than 5.8 s on a mirror that did not answer).
    fn a_silent_helper_holds_neither_the_boot_nor_a_pane(folder: &Path) -> Result<(), String> {
        let lines = ask_in_a_window(folder, &exe(), 300, false)?;
        let why = format!("unnamed {} did not answer within 300ms", exe().display());
        check(
            lines
                == [
                    "booted true",
                    "road straight",
                    why.as_str(),
                    "questions left 0",
                    "done",
                ],
            format!("{lines:?}"),
        )
    }

    /// A helper whose answer comes only after the boot went on — here it
    /// cannot come sooner, it waits on the boot's word — is named, and a
    /// pane started before the answer waits for it and comes through the
    /// helper.
    fn a_helper_that_answers_after_the_boot_went_on_is_named(folder: &Path) -> Result<(), String> {
        let lines = ask_in_a_window(folder, &exe(), 15_000, true)?;
        check(
            lines
                == [
                    "booted true",
                    "road helper",
                    "named",
                    "questions left 0",
                    "done",
                ],
            format!("{lines:?}"),
        )
    }

    /// A helper whose answer is not the road's is not named, and the pane
    /// starts straight.
    fn a_wrong_answer_is_not_named(folder: &Path) -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt as _;
        let wrong = folder.join("zerocode-mirror");
        std::fs::write(&wrong, WRONG_ANSWER).map_err(|error| error.to_string())?;
        std::fs::set_permissions(&wrong, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| error.to_string())?;
        let lines = ask_in_a_window(folder, &wrong, 15_000, false)?;
        let why = format!(
            "unnamed {} answered \"zerocode-ctty-exec 2\", not as a pane's helper",
            wrong.display()
        );
        check(
            lines
                == [
                    "booted true",
                    "road straight",
                    why.as_str(),
                    "questions left 0",
                    "done",
                ],
            format!("{lines:?}"),
        )
    }

    /// Names a person gave a pane that look like the window's wrapped ones
    /// reach the program as given past the helper — the window wraps them
    /// too, and gets back only what it wrapped (Codex t-77525: such a
    /// name was taken out on every road).
    fn names_like_a_wrapped_one_reach_a_program_past_the_helper_as_given(
        folder: &Path,
    ) -> Result<(), String> {
        let report = folder.join("report");
        let value = folder.to_str().ok_or("a UTF-8 folder")?;
        let lane = PtyLane::spawn(
            &exe_str(),
            &[
                "program".to_string(),
                report.to_str().ok_or("a UTF-8 report")?.to_string(),
            ],
            None,
            &user_names(value),
            24,
            80,
        )
        .map_err(|error| error.to_string())?;
        let lines = wait_for_line(&report, "ready");
        let pid = lane.pid().ok_or("no child pid")?;
        drop(lane);
        check(
            field(&lines, "terminal-session") == Some(pid.to_string()),
            format!("the program did not come through the helper: {lines:?}"),
        )?;
        check_given(&lines, value)
    }

    /// The same names reach a program started straight as given.
    fn names_like_a_wrapped_one_reach_a_program_started_straight_as_given(
        folder: &Path,
    ) -> Result<(), String> {
        let report = folder.join("report");
        let value = folder.to_str().ok_or("a UTF-8 folder")?;
        let out = std::process::Command::new(exe())
            .arg("env-window")
            .arg(&report)
            .arg(value)
            .output()
            .map_err(|error| error.to_string())?;
        check(
            out.status.success(),
            format!(
                "the window failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ),
        )?;
        check_given(&lines_of(&report), value)
    }

    fn check_given(lines: &[String], value: &str) -> Result<(), String> {
        let mut given: Vec<String> = user_names(value)
            .into_iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        given.sort();
        check(
            launch_names(lines) == given,
            format!("the program's names are not the ones given: {lines:?}"),
        )
    }

    /// The `env` lines a program said, sorted, but for the ones this test
    /// itself runs with (cargo gives it a `DYLD_*` of its own).
    fn launch_names(lines: &[String]) -> Vec<String> {
        let own = launch_env(std::env::vars_os());
        let mut names: Vec<String> = lines
            .iter()
            .filter_map(|line| line.strip_prefix("env "))
            .filter(|pair| !own.iter().any(|mine| mine == pair))
            .map(str::to_string)
            .collect();
        names.sort();
        names
    }

    /// A `DYLD_*` value given to a pane — the settings' environment field
    /// carries one — reaches its program, though a hardened helper holds
    /// none ([`forget_dyld`]); and the name it travelled under past the
    /// helper goes no further (t-77495).
    fn a_dyld_value_reaches_the_program_past_a_hardened_helper(
        folder: &Path,
    ) -> Result<(), String> {
        let report = folder.join("report");
        let libraries = folder.join("libraries");
        std::fs::create_dir_all(&libraries).map_err(|error| error.to_string())?;
        let value = libraries.to_str().ok_or("a UTF-8 folder")?.to_string();
        let lane = PtyLane::spawn(
            &exe_str(),
            &[
                "program".to_string(),
                report.to_str().ok_or("a UTF-8 report")?.to_string(),
            ],
            None,
            &[("DYLD_LIBRARY_PATH".to_string(), value.clone())],
            24,
            80,
        )
        .map_err(|error| error.to_string())?;
        let lines = wait_for_line(&report, "ready");
        drop(lane);
        check(
            lines.contains(&format!("env DYLD_LIBRARY_PATH={value}")),
            format!("the value did not reach the program: {lines:?}"),
        )?;
        check(
            !lines
                .iter()
                .any(|line| line.starts_with("env ZEROCODE_CTTY")),
            format!("a helper name went on: {lines:?}"),
        )
    }

    /// Three programs and three sleeps started on `road`: how many hold no
    /// controlling terminal, and how many outlive their master.
    fn census(scratch: &Path, road: &str) -> String {
        let folder = scratch.join(format!("census-{road}"));
        std::fs::create_dir_all(&folder).expect("a census folder");
        let (mut untied, mut outlived, mut total) = (0, 0, 0);
        for at in 0..6 {
            let kind = if at < 3 { "program" } else { "sleep" };
            let report = folder.join(format!("report-{at}"));
            let Ok(pid) = window_leaves(&report, kind, road) else {
                continue;
            };
            total += 1;
            if kind == "program"
                && field(&lines_of(&report), "terminal-session") != Some(pid.to_string())
            {
                untied += 1;
            }
            if !wait_for_within(|| gone(pid), Duration::from_secs(3)) {
                outlived += 1;
                kill(pid);
            }
        }
        format!(
            "{untied} of 3 programs with no controlling terminal; \
             {outlived} of {total} children still running 3 s after the master closed"
        )
    }

    // ---- helpers ----

    /// Run the stand-in window that asks `helper` with `limit_ms`, in a
    /// group of its own, and answer its report — or, when it does not say
    /// `done` in [`PATIENCE`], end the group and say so.
    fn ask_in_a_window(
        folder: &Path,
        helper: &Path,
        limit_ms: u64,
        go: bool,
    ) -> Result<Vec<String>, String> {
        use std::os::unix::process::CommandExt as _;
        let report = folder.join("window");
        let mut command = std::process::Command::new(exe());
        command
            .arg("asking-window")
            .arg(&report)
            .arg(helper)
            .arg(limit_ms.to_string())
            .env(PROBE_GATE, folder.join("go"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .process_group(0);
        if go {
            command.arg("go");
        }
        let mut window = command.spawn().map_err(|error| error.to_string())?;
        let done = wait_for(|| lines_of(&report).iter().any(|line| line == "done"));
        if !done && let Ok(group) = libc::pid_t::try_from(window.id()) {
            // SAFETY: the group this case's own window leads, unreaped.
            unsafe { libc::killpg(group, libc::SIGKILL) };
        }
        let _ = window.wait();
        let lines = lines_of(&report);
        check(done, format!("the window did not finish: {lines:?}"))?;
        Ok(lines)
    }

    /// This process's children, ended ones not yet reaped among them.
    fn children() -> Vec<libc::pid_t> {
        let mut pids: Vec<libc::pid_t> = vec![0; 256];
        let size = libc::c_int::try_from(std::mem::size_of_val(pids.as_slice())).expect("a size");
        // SAFETY: writes at most `size` bytes and answers how many pids.
        let count =
            unsafe { libc::proc_listchildpids(libc::getpid(), pids.as_mut_ptr().cast(), size) };
        pids.truncate(usize::try_from(count).unwrap_or(0));
        pids
    }

    fn spawn_program(report: &Path, grandchild: bool) -> PtyLane {
        let mut args = vec![
            "program".to_string(),
            report.to_str().expect("UTF-8").to_string(),
        ];
        if grandchild {
            args.push("grandchild".to_string());
        }
        PtyLane::spawn(&exe_str(), &args, None, &[], 24, 80).expect("the program")
    }

    /// Run the stand-in window and answer its child's pid.
    fn window_leaves(report: &Path, kind: &str, road: &str) -> Result<u32, String> {
        let out = std::process::Command::new(exe())
            .arg("window")
            .arg(report)
            .arg(kind)
            .arg(road)
            .output()
            .map_err(|error| error.to_string())?;
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .map_err(|_| {
                format!(
                    "the window said no pid: {}",
                    String::from_utf8_lossy(&out.stderr)
                )
            })
    }

    fn check(holds: bool, why: String) -> Result<(), String> {
        if holds { Ok(()) } else { Err(why) }
    }

    fn lines_of(report: &Path) -> Vec<String> {
        std::fs::read_to_string(report)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn field(lines: &[String], key: &str) -> Option<String> {
        lines
            .iter()
            .find_map(|line| line.strip_prefix(&format!("{key} ")).map(str::to_string))
    }

    fn wait_for_within(holds: impl Fn() -> bool, patience: Duration) -> bool {
        let until = Instant::now() + patience;
        loop {
            if holds() {
                return true;
            }
            if Instant::now() >= until {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_for(holds: impl Fn() -> bool) -> bool {
        wait_for_within(holds, PATIENCE)
    }

    fn wait_until(holds: impl Fn() -> bool, what: &str) {
        assert!(wait_for(holds), "waited {PATIENCE:?} for {what}");
    }

    fn wait_for_line(report: &Path, line: &str) -> Vec<String> {
        wait_until(
            || lines_of(report).iter().any(|said| said == line),
            &format!("{line:?} in {}", report.display()),
        );
        lines_of(report)
    }

    fn proc_info(pid: u32) -> Option<libc::proc_bsdinfo> {
        let pid = libc::c_int::try_from(pid).ok()?;
        let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
        // SAFETY: writes at most `size` bytes; read only when it wrote all.
        let wrote = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        // SAFETY: filled whole.
        (wrote == size).then(|| unsafe { info.assume_init() })
    }

    fn started_us(pid: u32) -> Option<u64> {
        proc_info(pid).map(|info| info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
    }

    fn executable(pid: u32) -> Option<String> {
        let pid = libc::c_int::try_from(pid).ok()?;
        let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: writes at most the buffer's length and answers how many.
        let len = unsafe {
            libc::proc_pidpath(
                pid,
                path.as_mut_ptr().cast(),
                u32::try_from(path.len()).ok()?,
            )
        };
        let len = usize::try_from(len).ok().filter(|len| *len > 0)?;
        Some(String::from_utf8_lossy(&path[..len]).into_owned())
    }

    fn ps_terminal(pid: u32) -> Option<(String, String)> {
        let out = std::process::Command::new("/bin/ps")
            .args(["-o", "tty=,stat=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let mut words = text.split_whitespace();
        Some((words.next()?.to_string(), words.next()?.to_string()))
    }

    fn gone(pid: u32) -> bool {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return true;
        };
        // SAFETY: signal 0 sends nothing.
        let refused = unsafe { libc::kill(pid, 0) } != 0;
        refused && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }

    fn kill(pid: u32) {
        if let Ok(pid) = libc::pid_t::try_from(pid) {
            // SAFETY: a pid this test's own window started, looked at just
            // before.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
}
