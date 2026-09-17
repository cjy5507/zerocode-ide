//! Verified SSH connections for ZeroCode.
//!
//! This crate owns the network boundary only. Persisted host identity stays in
//! `zerocode-core`; ZeroCode never persists a password; lanes decide what to do
//! with an authenticated connection.
//!
//! [`SshPassword`] zeroizes its caller-supplied buffer after the connection
//! attempt. `russh` 0.62.6 also makes a protocol-internal `String` copy for
//! password authentication; that copy lives until the SSH session terminates
//! and upstream deallocates rather than explicitly zeroizes it. Agent
//! authentication keeps private-key operations inside the system agent.

mod channel_reply;
mod command;
mod connection;
mod deadline;
mod error;
pub mod files;
mod host_key;
mod password;
mod process_sftp;
mod pty;
mod spawner;

pub use connection::{SshConnection, SshConnector};
pub use error::ConnectError;
pub use host_key::host_key_fingerprint;
pub use password::SshPassword;
pub use process_sftp::ProcessSftp;
pub use pty::PTY_OUTPUT_BYTE_BUDGET;
pub use spawner::SshPtySpawner;
