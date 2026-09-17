use russh::client;
use russh::keys::ssh_key::{self, HashAlg};
use std::sync::{Arc, Mutex};
use zerocode_core::PinnedHostKey;

use crate::ConnectError;

pub(crate) struct PinnedHostKeyHandler {
    expected: ssh_key::PublicKey,
    #[cfg(test)]
    stall_disconnect: bool,
}

impl PinnedHostKeyHandler {
    pub(crate) fn from_pin(pin: &PinnedHostKey) -> Result<Self, ConnectError> {
        let expected = parse_pinned_host_key(pin)?;

        Ok(Self {
            expected,
            #[cfg(test)]
            stall_disconnect: false,
        })
    }

    #[cfg(test)]
    pub(crate) fn stall_disconnect(&mut self) {
        self.stall_disconnect = true;
    }
}

/// Validate a persisted wire key and return its OpenSSH SHA-256 fingerprint.
///
/// Core validates the persistence shape; this network-boundary helper owns the
/// actual key parser so settings code cannot accidentally accept a plausible
/// looking but unusable pin.
pub fn host_key_fingerprint(pin: &PinnedHostKey) -> Result<String, ConnectError> {
    Ok(parse_pinned_host_key(pin)?
        .fingerprint(HashAlg::Sha256)
        .to_string())
}

fn parse_pinned_host_key(pin: &PinnedHostKey) -> Result<ssh_key::PublicKey, ConnectError> {
    let key = russh::keys::parse_public_key_base64(pin.encoded_key())
        .map_err(|_| ConnectError::InvalidPinnedHostKey)?;
    if key.algorithm().as_str() != pin.algorithm() {
        return Err(ConnectError::InvalidPinnedHostKey);
    }
    Ok(key)
}

pub(crate) struct HostKeyProbeHandler {
    observed: Arc<Mutex<Option<ssh_key::PublicKey>>>,
}

impl HostKeyProbeHandler {
    pub(crate) fn new() -> (Self, Arc<Mutex<Option<ssh_key::PublicKey>>>) {
        let observed = Arc::new(Mutex::new(None));
        (
            Self {
                observed: Arc::clone(&observed),
            },
            observed,
        )
    }
}

impl client::Handler for HostKeyProbeHandler {
    type Error = ConnectError;

    async fn check_server_key(
        &mut self,
        server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        *self
            .observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(server_public_key.clone());
        // Observation is not trust. End key exchange before authentication;
        // the caller must display and explicitly persist this exact key first.
        Ok(false)
    }
}

impl client::Handler for PinnedHostKeyHandler {
    type Error = ConnectError;

    async fn check_server_key(
        &mut self,
        server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        if self.expected.key_data() == server_public_key.key_data() {
            Ok(true)
        } else {
            Err(ConnectError::HostKeyMismatch)
        }
    }

    async fn disconnected(
        &mut self,
        reason: client::DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        #[cfg(test)]
        if self.stall_disconnect {
            std::future::pending().await
        }
        match reason {
            client::DisconnectReason::ReceivedDisconnect(_) => Ok(()),
            client::DisconnectReason::Error(error) => Err(error),
        }
    }
}
