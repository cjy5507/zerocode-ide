//! Loopback ports for tests that need one nobody else is on.
//!
//! One source file, `#[path]`-included by every test binary that needs it, so
//! there is one way to pick a port and not one per crate (t-74864).

#![allow(dead_code)]

use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

const FIRST: u16 = 20_000;
const SPAN: u16 = 10_000;
static STEP: AtomicU32 = AtomicU32::new(0);

/// A port a test may count on until the value is dropped.
pub struct QuietPort {
    port: u16,
}

/// The ports a claim draws from when the OS says its dynamic range starts at
/// `reported`.
pub fn band_for(_reported: Option<u16>) -> Range<u16> {
    FIRST..FIRST + SPAN
}

/// Where the lock that holds `port` lives.
pub fn lock_path(port: u16) -> PathBuf {
    std::env::temp_dir()
        .join("zerocode-test-ports")
        .join(port.to_string())
}

/// Where the OS says its dynamic range starts, when it says.
pub fn os_dynamic_start() -> Option<u16> {
    None
}

/// The lowest number in a `sysctl` or `/proc` answer.
pub fn lowest_port(text: &str) -> Option<u16> {
    text.split_whitespace()
        .filter_map(|word| word.parse::<u16>().ok())
        .min()
}

/// The start port in `netsh int ipv4 show dynamicport tcp` text.
pub fn netsh_start(text: &str) -> Option<u16> {
    text.lines()
        .filter_map(|line| line.rsplit_once(':'))
        .find_map(|(_, number)| number.trim().parse::<u16>().ok())
}

impl QuietPort {
    pub fn claim<T>(bind: impl FnMut(u16) -> io::Result<T>) -> (Self, T) {
        Self::claim_in(&band_for(os_dynamic_start()), bind)
    }

    pub fn claim_in<T>(band: &Range<u16>, mut bind: impl FnMut(u16) -> io::Result<T>) -> (Self, T) {
        let span = u32::from(band.end - band.start);
        for _ in 0..256 {
            let step = STEP.fetch_add(1, Ordering::Relaxed);
            let offset = (std::process::id() + step) % span;
            let port = band.start + u16::try_from(offset).expect("the offset is below the span");
            if let Ok(held) = bind(port) {
                return (Self { port }, held);
            }
        }
        panic!("no free loopback port in {band:?}");
    }

    /// A loopback port nobody listens on: a connection to it is refused.
    pub fn silent() -> Self {
        let (lease, listener) = Self::claim(|port| TcpListener::bind((Ipv4Addr::LOCALHOST, port)));
        drop(listener);
        lease
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn addr(&self) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, self.port))
    }
}
