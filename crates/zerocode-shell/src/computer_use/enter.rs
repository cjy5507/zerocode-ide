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

use serde_json::{Value, json};
use zerocode_core::computer_use::{ComputerCommand, parse_command};
use zerocode_core::handoff_code::{EnterInto, EnterTool, OneTimeCode};

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

/// What the answer says of how the field took the code: what the helper said
/// when the field was read back (`verified` or `unverified`), or `none` for a
/// door whose answer is a done and nothing more.
const VERIFIED: &str = "verified";
const UNVERIFIED: &str = "unverified";
const UNREAD: &str = "none";

/// Put the person's code where the agent said, and answer what happened —
/// never the code. The helper's answer to a desktop input echoes what was
/// written (`action.verification.expected`, the read-back, the tree, a
/// picture): it is read for the one word that says whether the field read
/// back what was written, and dropped. A code that was not entered is dropped
/// with the error that says why — the code taken out of the words if the
/// helper quoted it — and the person is asked again by a new handoff, not by a
/// second try on the same code.
pub fn enter(
    reason: &str,
    code: OneTimeCode,
    into: &EnterInto,
    typer: &mut dyn Typer,
) -> Result<Value, ComputerUseError> {
    let length = code.chars();
    let typed = into.words_with(code.reveal());
    let verification = match into.tool() {
        EnterTool::Computer => {
            let mut command = parse_command(&typed)
                .map_err(|why| dropped(&code, ComputerUseError::invalid_argument(why)))?;
            // No picture of a field that shows the code, from the helper or
            // from the look after an act.
            if let Some(params) = command.params.as_object_mut() {
                params.insert("noScreenshot".into(), Value::Bool(true));
            }
            let answer = typer
                .computer(&command)
                .map_err(|error| dropped(&code, error))?;
            verification_of(&answer)
        }
        EnterTool::Browser => {
            typer
                .browser(&typed)
                .map_err(|error| dropped(&code, error))?;
            UNREAD
        }
        EnterTool::Emulator => {
            typer
                .emulator(&typed)
                .map_err(|error| dropped(&code, error))?;
            UNREAD
        }
    };
    Ok(json!({
        "resumed": true,
        "reason": reason,
        "codeLength": length,
        "entered": true,
        "into": { "tool": into.tool().name(), "verb": into.verb() },
        "verification": verification,
    }))
}

/// The one thing of a desktop input's answer that the agent is told: whether
/// the helper read the field back as written. A field that is a secret is not
/// read back at all; that is `unverified`, and the answer says no more.
fn verification_of(answer: &Value) -> &'static str {
    match answer
        .pointer("/action/verification/state")
        .and_then(Value::as_str)
    {
        Some("verified") => VERIFIED,
        _ => UNVERIFIED,
    }
}

/// An input that did not go: the code is dropped, the refusal's own code
/// stands, and its words say so — with the code taken out of them if the
/// helper quoted what it was given. The words of a refusal that holds no code
/// are the helper's, whole.
fn dropped(code: &OneTimeCode, error: ComputerUseError) -> ComputerUseError {
    let message = error.message.replace(code.reveal(), "[code]");
    ComputerUseError::new(
        error.code,
        format!("the person's code was dropped, not kept: it was not entered — {message}"),
    )
}
