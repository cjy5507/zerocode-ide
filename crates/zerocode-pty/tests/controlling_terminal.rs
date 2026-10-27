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

    pub(super) fn main() {
        // The helper's side, first, as the mirror's `main` does.
        zerocode_pty::serve_ctty_exec();
        let args: Vec<OsString> = std::env::args_os().skip(1).collect();
        match args.first().and_then(|first| first.to_str()) {
            Some("program") => program(&args[1..]),
            Some("window") => window(&args[1..]),
            _ => run_cases(),
        }
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
            assert!(zerocode_pty::launch_through(exe(), None));
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

    // ---- the cases ----

    /// One case: its own folder in, why it failed out.
    type Case = fn(&Path) -> Result<(), String>;

    fn run_cases() {
        let scratch = tempfile::tempdir().expect("a scratch folder");
        // Before: the same lanes started straight, as every start was before
        // t-77414. Counted, not asserted — they say what the platform does.
        let before = census(scratch.path(), "direct");
        assert!(zerocode_pty::launch_through(exe(), None));
        let cases: [(&str, Case); 5] = [
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
