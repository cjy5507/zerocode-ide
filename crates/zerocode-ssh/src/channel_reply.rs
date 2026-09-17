//! What one channel message says about a request that asked for a reply.
//!
//! A server answers `subsystem`, `pty-req`, `env` and `exec` with
//! `CHANNEL_SUCCESS` or `CHANNEL_FAILURE`, but OpenSSH also sends a
//! `CHANNEL_WINDOW_ADJUST` the moment the subsystem or command gets its file
//! descriptors — and that packet leaves before the reply does. Reading one
//! message and calling anything else a protocol error therefore rejects every
//! working OpenSSH server; the flow-control notice has to be stepped over.
use russh::{Channel, ChannelMsg, client};

use crate::ConnectError;

/// One step of waiting for a request reply.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// `CHANNEL_SUCCESS`: the request was honoured.
    Accepted,
    /// `CHANNEL_FAILURE`: the server declined the request.
    Rejected,
    /// Flow control or another notice that says nothing about the request.
    Informational,
    /// Data, EOF, close or a dropped channel before any reply.
    Protocol,
}

pub(crate) fn classify(message: Option<&ChannelMsg>) -> Step {
    match message {
        Some(ChannelMsg::Success) => Step::Accepted,
        Some(ChannelMsg::Failure) => Step::Rejected,
        Some(ChannelMsg::WindowAdjusted { .. }) => Step::Informational,
        Some(_) | None => Step::Protocol,
    }
}

/// Wait for the reply to the request just sent on `channel`, stepping over
/// informational messages. `rejected` is the error a `CHANNEL_FAILURE` becomes.
pub(crate) async fn await_request_reply(
    channel: &mut Channel<client::Msg>,
    rejected: ConnectError,
) -> Result<(), ConnectError> {
    loop {
        match classify(channel.wait().await.as_ref()) {
            Step::Accepted => return Ok(()),
            Step::Rejected => return Err(rejected),
            Step::Informational => continue,
            Step::Protocol => return Err(ConnectError::ChannelProtocol),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_adjust_before_the_reply_is_informational() {
        // OpenSSH_8.0 observed 2026-09-09: `rcvd adjust 2097152` arrives two
        // lines before `subsystem request accepted`.
        let adjust = ChannelMsg::WindowAdjusted {
            new_size: 2_097_152,
        };
        assert_eq!(classify(Some(&adjust)), Step::Informational);
    }

    #[test]
    fn success_and_failure_are_the_two_replies() {
        assert_eq!(classify(Some(&ChannelMsg::Success)), Step::Accepted);
        assert_eq!(classify(Some(&ChannelMsg::Failure)), Step::Rejected);
    }

    #[test]
    fn an_end_or_a_closed_channel_before_the_reply_stays_a_protocol_error() {
        assert_eq!(classify(Some(&ChannelMsg::Eof)), Step::Protocol);
        assert_eq!(classify(Some(&ChannelMsg::Close)), Step::Protocol);
        assert_eq!(classify(None), Step::Protocol);
    }
}
