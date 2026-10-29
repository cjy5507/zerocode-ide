//! What a claimed port promises. Run from `zerocode-lane`'s `quiet_ports` test
//! binary, the lightest crate that includes the source.

use std::collections::HashSet;
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::time::Duration;

use super::quiet_ports::{
    QuietPort, apart_slot, band_for, lock_dir, lock_path_in, lowest_port, netsh_start,
    os_dynamic_start,
};

fn bind_loopback(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(("127.0.0.1", port))
}

/// Whether some other holder of the lock file in `dir` (another process, here
/// another handle) could take the lock now.
fn lock_is_free(dir: &Path, port: u16) -> bool {
    let path = lock_path_in(dir, port);
    std::fs::create_dir_all(path.parent().expect("a folder")).expect("the lock folder");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .expect("the lock file opens");
    file.try_lock().is_ok()
}

#[test]
fn the_band_lies_under_wherever_the_os_starts_its_dynamic_range() {
    for reported in [None, Some(24_000), Some(32_768), Some(49_152)] {
        let band = band_for(reported);
        assert!(
            band.end <= reported.unwrap_or(32_768),
            "{band:?} reaches the dynamic range the OS reported ({reported:?})"
        );
        assert!(band.start >= 1_024, "{band:?} reaches the privileged ports");
        assert!(band.len() >= 256, "{band:?} is too narrow to pick from");
    }
}

#[test]
fn a_real_claim_is_under_this_machines_dynamic_range() {
    let (lease, _held) = QuietPort::claim(bind_loopback);
    assert!(lease.port() < os_dynamic_start().unwrap_or(32_768));
}

#[test]
fn the_os_answers_are_read_by_their_numbers_not_their_words() {
    assert_eq!(lowest_port("32768\t60999\n"), Some(32_768));
    assert_eq!(lowest_port("49152\n49152\n"), Some(49_152));
    assert_eq!(lowest_port("not a number\n"), None);
    let english = "Protocol tcp Dynamic Port Range\n---------------------------------\nStart Port      : 49152\nNumber of Ports : 16384\n";
    let localised =
        "TCP 동적 포트 범위\n---------------\n시작 포트        : 49152\n포트 수          : 16384\n";
    assert_eq!(netsh_start(english), Some(49_152));
    assert_eq!(netsh_start(localised), Some(49_152));
    assert_eq!(netsh_start("nothing here\n"), None);
}

/// Sixteen claims, held, then let go; `parallel` runs between the letting go
/// and the check, standing for another test that asks for ports in that
/// moment and holds what it was given until the check is over.
///
/// The claims are made in a lock folder of this call's own and in a slice of
/// the band under the shared one that no other call has at the same time: no
/// other test can be given these ports, so the lease's release is the one thing
/// judged, whoever else is claiming (t-75231, t-75388).
fn release_is_seen<R>(parallel: impl FnOnce(&HashSet<u16>) -> R) {
    let apart = tempfile::tempdir().expect("a lock folder of its own");
    let slot = apart_slot(os_dynamic_start());
    let held: Vec<_> = (0..16)
        .map(|_| QuietPort::claim_apart(apart.path(), slot.band(), bind_loopback))
        .collect();
    let ports: HashSet<u16> = held.iter().map(|(lease, _)| lease.port()).collect();
    assert_eq!(ports.len(), held.len(), "two holders were given one port");
    for port in &ports {
        assert!(
            !lock_is_free(apart.path(), *port),
            "{port} is held by nothing"
        );
    }
    drop(held);
    let _other_test = parallel(&ports);
    for port in &ports {
        assert!(
            lock_is_free(apart.path(), *port),
            "{port} stayed locked after its holder"
        );
    }
}

/// Two tests judging a release in the same moment must not draw from one
/// band: the one lets go of a port and asks for it again, and the other, in a
/// lock folder of its own, may be given it in between (t-75388).
#[test]
fn tests_judging_a_release_at_once_draw_from_bands_that_do_not_meet() {
    let (first, second) = (
        apart_slot(os_dynamic_start()),
        apart_slot(os_dynamic_start()),
    );
    let (first, second) = (first.band(), second.band());
    assert!(
        first.end <= second.start || second.end <= first.start,
        "{first:?} and {second:?} are one band for two tests"
    );
    assert!(
        first.end <= band_for(os_dynamic_start()).start,
        "{first:?} reaches the band every ordinary claim draws from"
    );
}

/// A claim that finds no port says why when it was the lock files that could
/// not be taken, not their holders (R10 P2, t-75669).
#[test]
fn a_claim_whose_locks_cannot_be_opened_says_the_os_words() {
    let apart = tempfile::tempdir().expect("a lock folder of its own");
    let dir = apart.path().join("file");
    std::fs::write(&dir, "not a folder").expect("a file where the folder goes");
    let cause = std::fs::create_dir_all(&dir)
        .expect_err("a file is no folder")
        .to_string();
    let band = band_for(os_dynamic_start());
    let band = band.start..band.start + 2;
    let panic = std::panic::catch_unwind(|| QuietPort::claim_apart(&dir, &band, bind_loopback))
        .err()
        .expect("no port can be locked in a file");
    let message = panic.downcast_ref::<String>().expect("a panic message");
    assert!(
        message.contains("2 of its lock files could not be taken"),
        "{message}"
    );
    assert!(message.contains(&cause), "{message}");
    assert!(message.contains(&dir.display().to_string()), "{message}");
}

#[test]
fn a_port_stays_taken_until_its_holder_lets_go() {
    release_is_seen(|_| {});
}

/// R3 (t-75231): another test, in the same process or beside it, may be given
/// a port the moment it is let go — a claim is that test's right. The check
/// must still see that the lease's own hold ended.
#[test]
fn a_release_is_judged_though_another_test_is_given_the_port_at_once() {
    release_is_seen(|ports| {
        ports
            .iter()
            .map(|port| QuietPort::claim_in(&(*port..*port + 1), bind_loopback))
            .collect::<Vec<_>>()
    });
}

#[test]
fn threads_claiming_together_never_meet() {
    let claimed: Vec<Vec<(QuietPort, TcpListener)>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    (0..8)
                        .map(|_| QuietPort::claim(bind_loopback))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a claiming thread"))
            .collect()
    });
    let all: Vec<u16> = claimed
        .iter()
        .flatten()
        .map(|(lease, _)| lease.port())
        .collect();
    assert_eq!(all.len(), 64);
    assert_eq!(all.iter().collect::<HashSet<_>>().len(), 64);
}

#[test]
fn a_silent_port_refuses_and_is_not_given_to_anyone_else_meanwhile() {
    let silent = QuietPort::silent();
    assert!(
        TcpStream::connect_timeout(&silent.addr(), Duration::from_secs(5)).is_err(),
        "something answered on {}",
        silent.addr()
    );
    assert!(!lock_is_free(&lock_dir(), silent.port()));
    for _ in 0..64 {
        let (other, _held) = QuietPort::claim(bind_loopback);
        assert_ne!(other.port(), silent.port());
    }
}
