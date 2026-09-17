//! Report / card generation for [`LiveCli`].
//!
//! A behaviour-preserving SRP split of the report-rendering responsibility out
//! of `live_cli_commands.rs`: these `&self` methods project live `LiveCli`
//! state (model, runtime, session path, permission mode) onto the
//! [`report_services`](super::report_services) formatters — plain-text reports
//! and structured TUI cards. Stateless reports (config/agents/mcp/skills/diff/
//! version) call `report_services` directly at their dispatch site.

use super::live_cli::LiveCli;
use super::report_services;

impl LiveCli {
    pub(crate) fn status_report(&self) -> String {
        report_services::status_report(
            &self.model,
            &self.runtime,
            &self.session.path,
            self.permission_mode,
        )
    }

    /// Structured `/status` card for the persistent TUI.
    pub(crate) fn status_card(&self) -> core_types::CardModel {
        report_services::status_card(
            &self.model,
            &self.runtime,
            &self.session.path,
            self.permission_mode,
        )
    }

    /// Structured `/cost` card for the persistent TUI.
    pub(crate) fn cost_card(&self) -> core_types::CardModel {
        report_services::cost_card(&self.model, &self.runtime)
    }

    pub(crate) fn cost_report(&self) -> String {
        report_services::cost_report(&self.runtime)
    }
}
