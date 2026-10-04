//! Where a person's code goes (t-40807): the window types it, once, into the
//! field the agent named, and the agent is told how many characters went in.
//!
//! The code is the person's, typed on a card the agent cannot see or answer.
//! What it does there is the agent's request to say where it belongs
//! (`handoff --ask-code --into '[…]'`); what comes back to the agent is never
//! the code — not as the answer, and not as the echo of a field it was written
//! to, which the helper's own answers carry (`verification.expected`,
//! `actualPreview`, the tree's new value, a picture). This is the one place
//! the code is read out, and what its input answers is read for whether it
//! worked and thrown away.

use serde_json::Value;
use zerocode_core::computer_use::ComputerCommand;
use zerocode_core::computer_use_protocol::error_code;
use zerocode_core::handoff_code::{EnterInto, OneTimeCode};

use super::ComputerUseError;

/// The three doors an input comes in by, as the window types through them:
/// the live ones are the window's own roads (`agent_tools_runtime`), a test's
/// are a form that remembers what it was given.
pub trait Typer {
    /// A desktop input — the command as the lone-command road runs it, the
    /// code already its value. Answers the helper's answer.
    fn computer(&mut self, command: &ComputerCommand) -> Result<Value, ComputerUseError>;
    /// A tab's input — the door's words, the code the last of them.
    fn browser(&mut self, words: &[String]) -> Result<(), ComputerUseError>;
    /// A phone's input — the door's words, the code the value of `--text`.
    fn emulator(&mut self, words: &[String]) -> Result<(), ComputerUseError>;
}

/// A window with no door to type through (a process without the window's
/// roads): every input is refused, so a code is never taken for nothing.
pub struct Unseated;

impl Typer for Unseated {
    fn computer(&mut self, _command: &ComputerCommand) -> Result<Value, ComputerUseError> {
        Err(unseated())
    }

    fn browser(&mut self, _words: &[String]) -> Result<(), ComputerUseError> {
        Err(unseated())
    }

    fn emulator(&mut self, _words: &[String]) -> Result<(), ComputerUseError> {
        Err(unseated())
    }
}

fn unseated() -> ComputerUseError {
    ComputerUseError::new(
        error_code::INVALID_ARGUMENT,
        "no door to type the code through",
    )
}

/// Put the person's code where the agent said, and answer what happened —
/// never the code. Not yet: the code is not typed anywhere.
pub fn enter(
    reason: &str,
    code: OneTimeCode,
    into: &EnterInto,
    typer: &mut dyn Typer,
) -> Result<Value, ComputerUseError> {
    let _ = (reason, &code, into, typer);
    Err(ComputerUseError::new(
        error_code::INVALID_ARGUMENT,
        "a code is not typed yet",
    ))
}
