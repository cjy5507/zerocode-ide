use std::time::Duration;

pub(crate) const CONNECT: Duration = Duration::from_secs(10);
pub(crate) const KEEPALIVE: Duration = Duration::from_secs(15);
pub(crate) const AUTHENTICATE: Duration = Duration::from_secs(10);
pub(crate) const CHANNEL_OPEN: Duration = Duration::from_secs(10);
pub(crate) const CHANNEL_REQUEST: Duration = Duration::from_secs(10);
pub(crate) const PTY_SETUP: Duration = Duration::from_secs(30);
pub(crate) const CHANNEL_OPERATION: Duration = Duration::from_secs(10);
pub(crate) const DISCONNECT: Duration = Duration::from_secs(5);
