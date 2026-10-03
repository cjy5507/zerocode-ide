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
pub fn reader_for(_agent: &str) -> Option<Box<dyn CostReader>> {
    None
}

/// Claude Code's transcript: one JSON record a line.
#[derive(Debug, Default)]
pub struct ClaudeFormat;

impl CostReader for ClaudeFormat {
    fn feed(&mut self, _lines: &str) -> Vec<CallCost> {
        Vec::new()
    }

    fn flush(&mut self) -> Option<CallCost> {
        None
    }
}

#[cfg(test)]
mod tests;
