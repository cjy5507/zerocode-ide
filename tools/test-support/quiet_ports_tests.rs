//! What a claimed port promises. Run from `zerocode-lane`'s `quiet_ports` test
//! binary, the lightest crate that includes the source.

use std::collections::HashSet;
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use super::quiet_ports::{
    QuietPort, band_for, lock_path, lowest_port, netsh_start, os_dynamic_start,
};

fn bind_loopback(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(("127.0.0.1", port))
}

/// Whether some other holder of the lock file (another process, here another
/// handle) could take the lock now.
fn lock_is_free(port: u16) -> bool {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path(port))
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
    let localised = "TCP 동적 포트 범위\n---------------\n시작 포트        : 49152\n포트 수          : 16384\n";
    assert_eq!(netsh_start(english), Some(49_152));
    assert_eq!(netsh_start(localised), Some(49_152));
    assert_eq!(netsh_start("nothing here\n"), None);
}

#[test]
fn a_port_stays_taken_until_its_holder_lets_go() {
    let held: Vec<_> = (0..16)
        .map(|_| QuietPort::claim(bind_loopback))
        .collect();
    let ports: HashSet<u16> = held.iter().map(|(lease, _)| lease.port()).collect();
    assert_eq!(ports.len(), held.len(), "two holders were given one port");
    for port in &ports {
        assert!(!lock_is_free(*port), "{port} is held by nothing");
    }
    drop(held);
    for port in &ports {
        assert!(lock_is_free(*port), "{port} stayed locked after its holder");
    }
}

#[test]
fn threads_claiming_together_never_meet() {
    let claimed: Vec<Vec<(QuietPort, TcpListener)>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| (0..8).map(|_| QuietPort::claim(bind_loopback)).collect()))
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
    assert!(!lock_is_free(silent.port()));
    for _ in 0..64 {
        let (other, _held) = QuietPort::claim(bind_loopback);
        assert_ne!(other.port(), silent.port());
    }
}
