//! The assign moment's request (t-15554): the difficulty seat and the model
//! seat asked for one summons. The questions, and which answer runs, are
//! the core's (`zerocode_core::summon_assign`, the worker-start plan); this
//! is the wire.
use crate::agent_teams::Host;
use crate::systemone::Wire;
use std::path::Path;
use zerocode_core::orchestration::PreparedWorkerStart;
use zerocode_core::summon_assign::{AssignAsk, Receipts};

/// Every question `asked` carries, each seat's row.
pub(super) fn ask(wire: &Wire, asked: &AssignAsk, checkout: Option<&Path>) -> Receipts {
    Receipts {
        difficulty: asked
            .difficulty
            .as_ref()
            .map(|look| super::summon_difficulty::ask(wire, look, checkout)),
        model: asked
            .model
            .as_ref()
            .map(|model| super::summon_model::ask(wire, model, checkout)),
    }
}

/// The receipts of a fresh summons under `origin`, asked on the launch's own
/// path.
pub(super) fn choose(asked: &AssignAsk, origin: [&str; 3]) -> Receipts {
    choose_with(&Wire::of_this_machine(), asked, origin)
}

pub(super) fn choose_with(wire: &Wire, asked: &AssignAsk, origin: [&str; 3]) -> Receipts {
    Receipts {
        difficulty: asked
            .difficulty
            .as_ref()
            .and_then(|look| super::summon_difficulty::choose_with(wire, look, origin)),
        model: asked
            .model
            .as_ref()
            .and_then(|model| super::summon_model::choose_with(wire, model, origin)),
    }
}

/// Each seat's row beside what the summons launched — off the beat.
pub(super) fn record(
    host: &dyn Host,
    prepared: &PreparedWorkerStart,
    checkout: Option<&str>,
    now_ms: i64,
) {
    super::summon_difficulty::record(host, prepared, checkout, now_ms);
    super::summon_model::record(host, prepared, checkout, now_ms);
}

#[cfg(test)]
mod tests;
