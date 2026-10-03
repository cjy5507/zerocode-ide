//! What each of a worker's model calls cost, read off its CLI's own record of
//! them (t-26583).
//!
//! The gate's one fact that is not the same envelope for every CLI: a hook says
//! a tool was called, but only the transcript says what the model's call cost.
//! Each format is therefore one small reader behind one interface
//! ([`CostReader`]), fed the file's new whole lines as it grows — never the
//! file again — and answering the model calls those lines finish.
//!
//! **Wired:** Claude Code's format ([`ClaudeFormat`]) — `usage` on each
//! assistant record, priced at the table the usage statistics already keep
//! ([`crate::usage_stats::estimate_cost_usd`]). **Not yet:** every other CLI.
//! [`reader_for`] answers `None` for them, and the window then says on the board
//! that the cost is unread and judges that worker on the facts it does have. A
//! reader is added here, as a row, the day its format has been measured; a
//! format guessed at is a cost nobody can check.

use crate::agent::AgentKind;
use crate::usage_stats::{SourceTurn, estimate_cost_usd, parse_record};

/// What an assistant record without a session of its own is filed under: the
/// reader judges one file and never looks at the id.
const NO_SESSION: &str = "-";

/// One model call's cost.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CallCost {
    /// Dollars at the API's price for the model that answered.
    Usd(f64),
    /// The model has no price on file: the call is counted, not priced.
    Unpriced,
}

impl CallCost {
    /// The dollars, when there are any.
    #[must_use]
    pub const fn usd(self) -> Option<f64> {
        match self {
            Self::Usd(usd) => Some(usd),
            Self::Unpriced => None,
        }
    }
}

/// One CLI's transcript format, read as its file grows.
pub trait CostReader: Send {
    /// The model calls that these new whole lines finish. A call is finished
    /// when the next one begins, or when [`CostReader::flush`] says its stream
    /// is over: a vendor writes one call as several records, each carrying more
    /// of its usage than the last.
    fn feed(&mut self, lines: &str) -> Vec<CallCost>;

    /// The call still open, once nothing more has come for long enough that its
    /// stream is over. It is not counted again if a late record of it arrives.
    fn flush(&mut self) -> Option<CallCost>;
}

/// The reader for `agent`'s transcript format, or `None` for a CLI whose format
/// no reader here reads.
#[must_use]
pub fn reader_for(agent: &str) -> Option<Box<dyn CostReader>> {
    match AgentKind::from_slug(agent)? {
        AgentKind::Claude => Some(Box::new(ClaudeFormat::default())),
        _ => None,
    }
}

/// Claude Code's transcript: one JSON record a line, an assistant record's
/// `message.usage` carrying the call's tokens, the same `message.id` and
/// `requestId` on every record one call is written as — the identity
/// [`crate::usage_stats::dedupe`] folds them by, the fuller usage winning.
#[derive(Debug, Default)]
pub struct ClaudeFormat {
    /// The call being written: its fullest usage so far.
    open: Option<SourceTurn>,
    /// The identity of the call last closed, so a record of it that arrives
    /// late is not a second call.
    closed: Option<String>,
}

impl ClaudeFormat {
    fn close(&mut self) -> Option<CallCost> {
        let turn = self.open.take()?;
        self.closed = turn.dedupe_key.clone();
        Some(cost_of(&turn))
    }
}

/// What a call cost at its model's price.
fn cost_of(turn: &SourceTurn) -> CallCost {
    estimate_cost_usd(
        turn.model.as_deref(),
        turn.input_tokens,
        turn.output_tokens,
        turn.cache_read_tokens,
        turn.cache_write_tokens,
    )
    .map_or(CallCost::Unpriced, CallCost::Usd)
}

impl CostReader for ClaudeFormat {
    fn feed(&mut self, lines: &str) -> Vec<CallCost> {
        let mut finished = Vec::new();
        for line in lines.lines() {
            let Some(turn) = parse_record(line, Some(NO_SESSION)) else {
                continue;
            };
            if let Some(open) = self.open.as_mut()
                && open.dedupe_key.is_some()
                && open.dedupe_key == turn.dedupe_key
            {
                open.input_tokens = open.input_tokens.max(turn.input_tokens);
                open.output_tokens = open.output_tokens.max(turn.output_tokens);
                open.cache_read_tokens = open.cache_read_tokens.max(turn.cache_read_tokens);
                open.cache_write_tokens = open.cache_write_tokens.max(turn.cache_write_tokens);
                if turn.model.is_some() {
                    open.model = turn.model;
                }
                continue;
            }
            finished.extend(self.close());
            if turn.dedupe_key.is_some() && turn.dedupe_key == self.closed {
                continue;
            }
            self.open = Some(turn);
        }
        finished
    }

    fn flush(&mut self) -> Option<CallCost> {
        self.close()
    }
}

#[cfg(test)]
mod tests;
