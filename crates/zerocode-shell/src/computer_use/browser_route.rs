//! The door every request passes on the way in. A desktop browser used to be
//! refused here in favour of the window's work browser. The person wants the whole
//! computer usable (2026-11-01), so a browser is an app like any other: the work
//! browser stays the faster road for a page with no login, and the skill says so.

use zerocode_core::computer_use::ComputerCommand;

use super::ComputerUseError;

pub(crate) fn command(command: &ComputerCommand) -> Result<(), ComputerUseError> {
    if command.method.acts()
        && let Some(refused) = crate::agent_tools_runtime::door_refusal(command)
    {
        return Err(refused);
    }
    Ok(())
}
