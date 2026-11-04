//! t-94842: a released pane's tree is the window's own child and what it
//! started; only those still standing as themselves are ended, by pid.

use super::*;

/// The window (100), a released worker's pane child (200) with its CLI's
/// command (201) and that command's own child (202), a person's terminal (300)
/// with what they run in it (301), and a program nobody here started (400).
fn the_machine() -> Vec<Row> {
    [
        (100, 1, "window"),
        (200, 100, "pane-child"),
        (201, 200, "tool"),
        (202, 201, "tool-child"),
        (300, 100, "persons-shell"),
        (301, 300, "persons-command"),
        (400, 1, "someone-elses"),
    ]
    .into_iter()
    .map(|(pid, ppid, started)| (pid, ppid, started.to_string()))
    .collect()
}

fn pids(tree: &[Named]) -> Vec<u32> {
    tree.iter().map(|one| one.pid).collect()
}

#[test]
fn a_released_panes_tree_is_its_child_and_what_it_started_and_nothing_else() {
    let tree = tree_under(&the_machine(), 200);
    assert_eq!(pids(&tree), vec![200, 201, 202]);
    assert_eq!(tree[1].started, "tool");
    assert!(
        tree_under(&the_machine(), 999).is_empty(),
        "a child already gone has no tree"
    );
    // A table whose rows loop on each other ends.
    let looped: Vec<Row> = vec![(5, 6, "a".into()), (6, 5, "b".into())];
    assert_eq!(pids(&tree_under(&looped, 5)), vec![5, 6]);
}

/// After the close the pane child is gone; its command outlived it under
/// launchd, the command's child died and its pid went to another program. Only
/// the command is ended — politely after the linger, hard after the wait — and
/// the person's terminal, their command, the other program and the reused pid
/// are never signalled.
#[test]
fn only_what_stands_as_itself_is_signalled_politely_then_hard() {
    let tree = tree_under(&the_machine(), 200);
    let after: Vec<Row> = vec![
        (100, 1, "window".into()),
        (201, 1, "tool".into()),
        (202, 1, "another-program".into()),
        (300, 100, "persons-shell".into()),
        (301, 300, "persons-command".into()),
        (400, 1, "someone-elses".into()),
    ];
    let closed = 1_000_000;
    let mut reaper = Reaper::default();
    reaper.watch(Watch::new("w-1", 7, tree, closed));
    let mut sent = Vec::new();
    let mut signal = |pid: u32, hard: bool| {
        sent.push((pid, hard));
        true
    };

    assert!(reaper.beat(&after, closed + 1_000, &mut signal).is_empty());
    let lines = reaper.beat(&after, closed + LINGER_MS, &mut signal);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].contains("SIGTERM sent by pid to [201]"),
        "{lines:?}"
    );
    assert!(
        reaper
            .beat(&after, closed + LINGER_MS + 1_000, &mut signal)
            .is_empty()
    );
    let lines = reaper.beat(&after, closed + LINGER_MS + SIGNAL_WAIT_MS, &mut signal);
    assert!(
        lines[0].contains("SIGKILL sent by pid to [201]"),
        "{lines:?}"
    );
    let gone: Vec<Row> = after.into_iter().filter(|row| row.0 != 201).collect();
    let lines = reaper.beat(
        &gone,
        closed + LINGER_MS + SIGNAL_WAIT_MS + 1_000,
        &mut signal,
    );
    assert!(
        lines[0].contains("ended after the window signalled"),
        "{lines:?}"
    );
    assert!(reaper.is_idle());

    assert_eq!(sent, vec![(201, false), (201, true)]);
    for never in [100, 202, 300, 301, 400] {
        assert!(
            !sent.iter().any(|(pid, _)| *pid == never),
            "pid {never} was signalled: {sent:?}"
        );
    }
}

#[test]
fn a_tree_the_close_ended_is_signalled_nothing() {
    let tree = tree_under(&the_machine(), 200);
    let after: Vec<Row> = the_machine()
        .into_iter()
        .filter(|row| ![200, 201, 202].contains(&row.0))
        .collect();
    let mut reaper = Reaper::default();
    reaper.watch(Watch::new("w-2", 8, tree, 0));
    let mut sent = Vec::new();
    let lines = reaper.beat(&after, 10, &mut |pid, hard| {
        sent.push((pid, hard));
        true
    });
    assert!(lines[0].contains("ended with the close"), "{lines:?}");
    assert!(sent.is_empty());
    assert!(reaper.is_idle());
}

#[test]
fn a_tree_that_outlives_the_hard_signal_is_said_once_and_let_go() {
    let tree = vec![Named {
        pid: 201,
        started: "tool".into(),
    }];
    let rows: Vec<Row> = vec![(201, 1, "tool".into())];
    let watch = Watch::new("w-3", 9, tree, 0);
    let mut reaper = Reaper::default();
    reaper.watch(watch);
    let mut never = |_: u32, _: bool| false;
    reaper.beat(&rows, LINGER_MS, &mut never);
    reaper.beat(&rows, LINGER_MS + SIGNAL_WAIT_MS, &mut never);
    let lines = reaper.beat(&rows, LINGER_MS + 2 * SIGNAL_WAIT_MS, &mut never);
    assert!(lines[0].contains("still stand after SIGKILL"), "{lines:?}");
    assert!(reaper.is_idle());
    // An identity nobody could read matches nothing, so nothing is watched.
    let unread = vec![Named {
        pid: 201,
        started: String::new(),
    }];
    assert!(standing(&unread, &rows).is_empty());
}

/// Wait up to five seconds for `done`, looking every 50 ms.
#[cfg(unix)]
fn within_five_seconds(mut done: impl FnMut() -> bool) -> bool {
    let began = std::time::Instant::now();
    while began.elapsed() < std::time::Duration::from_secs(5) {
        if done() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 asks only whether this test's own child still stands.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Real processes this test started: a pane child whose command ignores the
/// hang-up and leaves its group, as a tool's command or a detached server
/// does, and a bystander outside the tree. Killing the child (what the close
/// does) leaves the command standing; one beat after the linger ends it by its
/// pid, and the bystander is untouched.
#[cfg(unix)]
#[test]
fn a_command_the_close_left_is_ended_by_its_pid_and_a_bystander_is_not() {
    use std::os::unix::process::CommandExt as _;
    let mut child = crate::proc::quiet_command("/bin/sh")
        .args([
            "-c",
            "perl -e '$SIG{HUP}=\"IGNORE\"; setpgrp(0,0); sleep 600' & exec sleep 600",
        ])
        .process_group(0)
        .spawn()
        .expect("the pane child");
    let mut bystander = crate::proc::quiet_command("/bin/sleep")
        .arg("600")
        .spawn()
        .expect("the bystander");
    let root = child.id();
    let mut tree = Vec::new();
    assert!(
        within_five_seconds(|| {
            tree = table()
                .map(|rows| tree_under(&rows, root))
                .unwrap_or_default();
            tree.len() >= 2
        }),
        "the pane's tree never showed its command: {tree:?}"
    );
    let command = tree
        .iter()
        .map(|one| one.pid)
        .find(|pid| *pid != root)
        .expect("the command");

    // SAFETY: the pane child this test started, by its literal pid.
    unsafe { libc::kill(root as libc::pid_t, libc::SIGKILL) };
    let _ = child.wait();
    let rows = table().expect("the table");
    assert_eq!(standing(&tree, &rows), vec![command], "{tree:?}");

    let mut reaper = Reaper::default();
    reaper.watch(Watch::new("w-real", 1, tree.clone(), 0));
    let lines = reaper.beat(&rows, LINGER_MS, &mut signal_pid);
    assert!(lines[0].contains("SIGTERM"), "{lines:?}");
    assert!(
        within_five_seconds(|| !alive(command)),
        "the command the close left still stands"
    );
    assert!(
        alive(bystander.id()),
        "a process outside the pane's tree was ended"
    );
    let lines = reaper.beat(
        &table().expect("the table"),
        LINGER_MS + 100,
        &mut signal_pid,
    );
    assert!(
        lines[0].contains("ended after the window signalled"),
        "{lines:?}"
    );
    // SAFETY: the bystander this test started, by its literal pid.
    unsafe { libc::kill(bystander.id() as libc::pid_t, libc::SIGKILL) };
    let _ = bystander.wait();
}

/// The resident memory of `pids`, in KiB, as `ps` reads it.
#[cfg(target_os = "macos")]
fn resident_kib(pids: &[u32]) -> u64 {
    if pids.is_empty() {
        return 0;
    }
    let listed = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let out = crate::proc::quiet_command("/bin/ps")
        .args(["-o", "rss=", "-p", &listed])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|word| word.parse::<u64>().ok())
        .sum()
}

/// The numbers for the report: one synthetic worker pane through the window's
/// own pty layer — a CLI holding 160 MiB that printed 20,000 lines, and a
/// command of its own holding 96 MiB that ignores the hang-up and left its
/// group — closed the way a release closes it, then one reaper pass. Prints the
/// window side (this process) and the pane's tree, before and after. Run by
/// name: `--ignored --nocapture`, once as is and once under `taskpolicy -b`.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "prints memory numbers; run by name with --ignored --nocapture"]
fn release_memory_numbers() {
    let me = std::process::id();
    let window_before = resident_kib(&[me]);
    let script = "perl -e '$SIG{HUP}=\"IGNORE\"; setpgrp(0,0); my $x = \"t\" x (96*1024*1024); sleep 600' & \
         exec perl -e '$| = 1; my $x = \"c\" x (160*1024*1024); print \"line $_ \" . (\"x\" x 100) . \"\\n\" for 1..20000; sleep 600'";
    let mut lane = zerocode_pty::PtyLane::spawn(
        "/bin/sh",
        &["-c".to_string(), script.to_string()],
        None,
        &[],
        40,
        120,
    )
    .expect("the synthetic pane");
    let root = lane.pid().expect("the pane child");
    let mut tree = Vec::new();
    let mut held_kib = 0;
    let began = std::time::Instant::now();
    while began.elapsed() < std::time::Duration::from_secs(20) {
        lane.pump();
        tree = table()
            .map(|rows| tree_under(&rows, root))
            .unwrap_or_default();
        held_kib = resident_kib(&pids(&tree));
        if tree.len() >= 2 && held_kib > 250 * 1024 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    for _ in 0..50 {
        lane.pump();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let window_with = resident_kib(&[me]);
    let began_close = std::time::Instant::now();
    lane.kill().expect("the close");
    drop(lane);
    std::thread::sleep(std::time::Duration::from_secs(1));
    let rows = table().expect("the table");
    let left = standing(&tree, &rows);
    let left_kib = resident_kib(&left);
    let window_closed = resident_kib(&[me]);
    let mut reaper = Reaper::default();
    reaper.watch(Watch::new("w-numbers", 0, tree.clone(), 0));
    let began_reap = std::time::Instant::now();
    let mut lines = reaper.beat(&rows, LINGER_MS, &mut signal_pid);
    let ended = within_five_seconds(|| standing(&tree, &table().unwrap_or_default()).is_empty());
    if !ended {
        lines.extend(reaper.beat(
            &table().unwrap_or_default(),
            LINGER_MS + SIGNAL_WAIT_MS,
            &mut signal_pid,
        ));
    }
    let reaped_in = began_reap.elapsed();
    let after = standing(&tree, &table().unwrap_or_default());
    let after_kib = resident_kib(&after);
    let window_after = resident_kib(&[me]);
    println!(
        "NUMBERS t-94842 window_kib before={window_before} with_pane={window_with} \
         after_close={window_closed} after_reap={window_after}"
    );
    println!(
        "NUMBERS t-94842 pane_tree processes={} kib={held_kib} | after_close processes={} \
         kib={left_kib} | after_reap processes={} kib={after_kib} | close_to_reap_ms={} \
         reap_ms={}",
        tree.len(),
        left.len(),
        after.len(),
        began_close.elapsed().as_millis(),
        reaped_in.as_millis(),
    );
    for line in &lines {
        println!("NUMBERS t-94842 line {line}");
    }
    assert!(tree.len() >= 2, "the synthetic pane never stood: {tree:?}");
    assert!(after.is_empty(), "the reaper left {after:?}");
}
