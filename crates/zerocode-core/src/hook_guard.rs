//! What the Jev seats a pane's work is put to — the two tool guards
//! (`crate::jev::tool_guard`, t-10916), the completion claim seat
//! (`crate::jev::claim`) and the file pick seat (`crate::jev::file_pick`,
//! t-11349) — can see of an agent's work through the hooks this window
//! installs for it.
//!
//! One layer over the events [`crate::hook`] already reads, in the moments the
//! seats ask and grade at ([`Moment`], [`moments`]): a shell command about to
//! run and one that ran, a tool's text handed back, another call starting —
//! the step after a text, and the files an edit writes — a call coming back
//! with what a claim may cite of it, a call waiting on the person's
//! permission, the person's prompt, and a turn's end with the agent's
//! answer. The events and the tools arrive here normalized
//! ([`crate::hook::activity_of_parsed`], [`crate::hook::Tool`]), so nothing
//! here spells a vendor's event name. Only the seats being asked are read
//! for ([`Asking`]): with every seat off a pane's event is read for nothing.
//!
//! What differs by agent is one row each ([`sight`]): which of those moments
//! its installed hooks carry — and, where one is missing, the word that says
//! why ([`Unseen`]) — and where its payloads keep a finished tool's text when
//! the common readers do not find it. The guards' code names no agent: an
//! agent is a row, and a row that cannot see a moment says so, so a count of
//! zero reads as "cannot see" and not as quiet — the dashboard reads the same
//! rows seat by seat ([`seat_sight`]).

#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

use serde_json::Value;

use crate::agent::{ALL_AGENTS, AgentKind};
use crate::hook::{self, HookState, Phase, Tool};
use crate::jev::claim::{self, Evidence};
use crate::jev::tool_guard::{MCP_TOOL_PREFIX, TextSource, WRITTEN_WORDS_KEYS};
use crate::jev::{CLAIM, COMMAND_GUARD, FILE_PICK, JevUse, TOOL_TEXT_GUARD};
use crate::payload::HookPayload;
use crate::transcript::SaidAt;

/// Why an agent's hooks do not carry a moment the guards read — the word a
/// row names it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unseen {
    /// The agent's own runtime asks the two seats itself (zo): the window
    /// asks nothing a second time.
    OwnRuntime,
    /// The window installs no hook of this agent's.
    NoHooks,
    /// Its hooks carry no event before a tool runs — Antigravity since
    /// t-10461, whose hook there decides the tool and so cannot only watch.
    /// The command guard asks after the command ran, with nothing stamped
    /// before it: its label has no evidence of what the run changed.
    NoEventBefore,
    /// Its hooks carry no prompt event this window reads: a command is judged
    /// with no task line, and a text's order is told from the person's words
    /// by nothing.
    NoPromptEvent,
    /// Neither a call's nor a turn's end says the person stopped it.
    NoStopFlag,
    /// Its hooks come, but no answer of theirs was measured to carry context
    /// to the model at a turn's start (t-14869): a seat that speaks then is
    /// recorded for its panes, never said.
    NoContextRoad,
}

impl Unseen {
    /// Every reason, in the order a reader lists them.
    pub const ALL: [Self; 6] = [
        Self::OwnRuntime,
        Self::NoHooks,
        Self::NoEventBefore,
        Self::NoPromptEvent,
        Self::NoStopFlag,
        Self::NoContextRoad,
    ];

    /// The word a row names this reason by.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::OwnRuntime => "own_runtime",
            Self::NoHooks => "no_hooks",
            Self::NoEventBefore => "no_event_before",
            Self::NoPromptEvent => "no_prompt_event",
            Self::NoStopFlag => "no_stop_flag",
            Self::NoContextRoad => "no_context_road",
        }
    }

    /// Whether the seats are still asked of the agent's work, only not by
    /// the window — its own runtime asks them: a count of its requests is
    /// its own, not a moment nobody sees.
    #[must_use]
    pub const fn asked_elsewhere(self) -> bool {
        matches!(self, Self::OwnRuntime)
    }
}

/// Whether an agent's hooks carry one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sees {
    Yes,
    No(Unseen),
}

impl Sees {
    #[must_use]
    pub const fn yes(self) -> bool {
        matches!(self, Self::Yes)
    }
}

/// What one agent's hooks let the two guards see — its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sight {
    /// A shell command about to run: where the command guard asks, stamping
    /// what the command names before it runs.
    pub before: Sees,
    /// A shell command that ran: its facts for the label — and where
    /// `before` is unseen, where the command guard asks.
    pub after: Sees,
    /// A tool's text handed back: where the text guard asks.
    pub text: Sees,
    /// The person's prompt: the task line a command is judged for, and the
    /// words a text's order must not be.
    pub prompt: Sees,
    /// A turn's end: where hindsight is settled.
    pub turn_end: Sees,
    /// The person stopping a call while it ran — the call's own flag, or a
    /// turn end that says it was stopped while the call had no result yet.
    pub stopped_call: Sees,
    /// A turn's end with the agent's answer — its words in the payload, or
    /// the transcript that holds them: where the claim seat asks.
    pub turn_answer: Sees,
    /// The files an edit or a write is about to change: what a file pick is
    /// graded on.
    pub edited_path: Sees,
    /// Where a finished tool's text lives when the common readers do not find
    /// it: JSON pointers into the payload, tried first.
    pub result_at: &'static [&'static str],
    /// How a skill's load shows in its hooks: only the turn's ruler reads it
    /// ([`tally`]).
    pub skill_load: SkillLoad,
}

/// How an agent's hooks show it loading a skill (t-14869).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillLoad {
    /// A tool of its own, by its name as the agent spells it.
    Tool(&'static str),
    /// Its shell reads the skill's file ([`tally::SKILL_FILE`]).
    ReadsSkillFile,
    /// Nothing of it reaches the window.
    No(Unseen),
}

/// Claude's family, as the hooks this window installs report it: every
/// moment, its Read tool's text under `file.content` (measured on the
/// installed 2.1.283, whose `PostToolUseFailure` also carries `is_interrupt`
/// and whose `Stop` carries `last_assistant_message`).
const CLAUDE: Sight = Sight {
    before: Sees::Yes,
    after: Sees::Yes,
    text: Sees::Yes,
    prompt: Sees::Yes,
    turn_end: Sees::Yes,
    stopped_call: Sees::Yes,
    turn_answer: Sees::Yes,
    edited_path: Sees::Yes,
    result_at: &["/tool_response/file/content"],
    skill_load: SkillLoad::Tool("Skill"),
};

/// An agent whose installed hooks carry every moment in the Claude family's
/// shape — `tool_name`, `tool_input`, `tool_response` — and whose turn's end
/// says when the person stopped it.
const STOPS_ITS_TURN: Sight = Sight {
    stopped_call: Sees::Yes,
    result_at: &[],
    skill_load: SkillLoad::ReadsSkillFile,
    ..CLAUDE
};

/// The same, with no word for a stopped call or turn.
const NO_STOP: Sight = Sight {
    stopped_call: Sees::No(Unseen::NoStopFlag),
    ..STOPS_ITS_TURN
};

/// A row for an agent the window asks nothing of, and why.
const fn none(why: Unseen) -> Sight {
    Sight {
        before: Sees::No(why),
        after: Sees::No(why),
        text: Sees::No(why),
        prompt: Sees::No(why),
        turn_end: Sees::No(why),
        stopped_call: Sees::No(why),
        turn_answer: Sees::No(why),
        edited_path: Sees::No(why),
        result_at: &[],
        skill_load: SkillLoad::No(why),
    }
}

/// The row of `agent`. The events each row counts on are the ones its
/// installer puts on disk (`zerocode-hookd`'s event tables, held to these
/// rows by that crate's tests).
#[must_use]
pub const fn sight(agent: AgentKind) -> Sight {
    match agent {
        AgentKind::Zo => none(Unseen::OwnRuntime),
        AgentKind::Opencode => none(Unseen::NoHooks),
        AgentKind::Claude => CLAUDE,
        // Devin, Kimi and Cursor say at their turn's end that the person
        // stopped it (`hook::interrupt_declared`); Amp's plugin says it on
        // `agent.end`, and carries no prompt event this window reads.
        AgentKind::Devin | AgentKind::Kimi | AgentKind::Cursor => STOPS_ITS_TURN,
        AgentKind::Amp => Sight {
            prompt: Sees::No(Unseen::NoPromptEvent),
            ..STOPS_ITS_TURN
        },
        AgentKind::Codex | AgentKind::Droid | AgentKind::Copilot | AgentKind::Grok => NO_STOP,
        AgentKind::CommandCode => Sight {
            prompt: Sees::No(Unseen::NoPromptEvent),
            ..NO_STOP
        },
        // Nothing before a tool runs: neither a command's nor an edit's.
        AgentKind::Antigravity => Sight {
            before: Sees::No(Unseen::NoEventBefore),
            edited_path: Sees::No(Unseen::NoEventBefore),
            prompt: Sees::No(Unseen::NoPromptEvent),
            ..NO_STOP
        },
    }
}

/// Whether a seat that speaks at a turn's start reaches `agent`'s model
/// (t-14869): where the catalog measured its prompt hook to take context
/// ([`AgentKind::hook_additional_context`]) — else why not: its row's own
/// reason where the window asks it nothing, [`Unseen::NoContextRoad`] where
/// its hooks come but carry no context there.
#[must_use]
pub const fn turn_start_road(agent: AgentKind) -> Sees {
    if agent.hook_additional_context().is_some() {
        return Sees::Yes;
    }
    match sight(agent).turn_end {
        Sees::No(why) => Sees::No(why),
        Sees::Yes => Sees::No(Unseen::NoContextRoad),
    }
}

/// Every agent with its row, in the catalog's order — what a dashboard lists.
#[must_use]
pub fn sights() -> Vec<(AgentKind, Sight)> {
    ALL_AGENTS
        .into_iter()
        .map(|agent| (agent, sight(agent)))
        .collect()
}

/// What one of the seats a pane's moments serve reads of an agent's panes:
/// whether its question is asked there at all, and — when it is — each
/// reason part of what the seat reads is missing, once, in [`Unseen::ALL`]'s
/// order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatSight {
    pub asked: Sees,
    pub misses: Vec<Unseen>,
}

/// `seat`'s sight of `agent`'s panes, off the agent's row. The command guard
/// is asked before a shell command runs — after it ran, where nothing comes
/// before — and reads the prompt for its task line, the turn's end and a
/// stopped call for its hindsight; the text guard is asked on a tool's text
/// and reads the prompt and the turn's end; the claim seat is asked at a
/// turn's end over its answer, cites the turn's finished calls and is graded
/// on the person's next prompt; the file pick seat is asked on the person's
/// prompt and graded on the files the turn's edits wrote once it ends.
/// `None` for a seat no moment of a pane asks.
#[must_use]
pub fn seat_sight(seat: &JevUse, agent: AgentKind) -> Option<SeatSight> {
    let row = sight(agent);
    let (asked, read) = if seat.id == COMMAND_GUARD.id {
        let asked = if row.before.yes() || row.after.yes() {
            Sees::Yes
        } else {
            row.before
        };
        (
            asked,
            vec![row.before, row.prompt, row.turn_end, row.stopped_call],
        )
    } else if seat.id == TOOL_TEXT_GUARD.id {
        (row.text, vec![row.prompt, row.turn_end])
    } else if seat.id == CLAIM.id {
        (row.turn_answer, vec![row.prompt, row.after])
    } else if seat.id == FILE_PICK.id {
        (
            row.prompt,
            vec![row.edited_path, row.turn_end, turn_start_road(agent)],
        )
    } else {
        return None;
    };
    let misses = if asked.yes() {
        Unseen::ALL
            .into_iter()
            .filter(|why| read.contains(&Sees::No(*why)))
            .collect()
    } else {
        Vec::new()
    };
    Some(SeatSight { asked, misses })
}

/// The seats a pane's moments serve, each one the window asks or leaves
/// off by its own switch.
pub const PANE_SEATS: [&JevUse; 4] = [&COMMAND_GUARD, &TOOL_TEXT_GUARD, &CLAIM, &FILE_PICK];

/// Which of the seats a pane's moments serve ([`PANE_SEATS`]) are asked now
/// (t-11349): the one gate before anything of a pane's work is read — a seat
/// left off is read nothing for, and with every seat off nothing is read at
/// all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)] // one switch per seat, not a state machine
pub struct Asking {
    pub command: bool,
    pub text: bool,
    pub claim: bool,
    pub file_pick: bool,
}

impl Asking {
    /// Every seat asked — a reader no switch gates.
    pub const ALL: Self = Self {
        command: true,
        text: true,
        claim: true,
        file_pick: true,
    };

    /// The seats `asked` names, each by its row.
    #[must_use]
    pub fn of<'a>(asked: impl IntoIterator<Item = &'a JevUse>) -> Self {
        asked.into_iter().fold(Self::default(), |mut asking, seat| {
            if seat.id == COMMAND_GUARD.id {
                asking.command = true;
            } else if seat.id == TOOL_TEXT_GUARD.id {
                asking.text = true;
            } else if seat.id == CLAIM.id {
                asking.claim = true;
            } else if seat.id == FILE_PICK.id {
                asking.file_pick = true;
            }
            asking
        })
    }

    /// Whether `seat` is asked.
    #[must_use]
    pub fn asks(self, seat: &JevUse) -> bool {
        (seat.id == COMMAND_GUARD.id && self.command)
            || (seat.id == TOOL_TEXT_GUARD.id && self.text)
            || (seat.id == CLAIM.id && self.claim)
            || (seat.id == FILE_PICK.id && self.file_pick)
    }

    /// Whether no seat is asked.
    #[must_use]
    pub const fn nothing(self) -> bool {
        !(self.command || self.text || self.claim || self.file_pick)
    }
}

/* ---- the moments -------------------------------------------------------------- */

/// A tool call as the guards know it: its id when the payload names one, the
/// command it runs when it is a shell's, and the folder the agent reported.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Call {
    pub id: Option<String>,
    /// The whole command, credentials scrubbed ([`crate::clone::scrub_credentials`]).
    pub command: Option<String>,
    pub cwd: Option<PathBuf>,
}

/// One moment of an agent's work, as the two guards read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Moment {
    /// A shell command is about to run.
    CommandAbout(Call),
    /// A shell command ran: `failed` when its host said so, `stopped` when
    /// the call itself says the person stopped it while it ran.
    CommandRan {
        call: Call,
        failed: bool,
        stopped: bool,
    },
    /// A tool of the text guard's kinds handed text back.
    Text {
        call_id: Option<String>,
        /// The tool's own name, as the agent spelled it.
        tool: String,
        source: TextSource,
        /// Its text, credentials scrubbed and capped as the viewer caps a
        /// result ([`crate::hook::WORKER_OUTPUT_CHARS`]).
        text: String,
    },
    /// A call started — any tool: what it carries out, a shell command or the
    /// words an edit writes, for the step after a text; and, for an edit or a
    /// write while the file pick seat is asked, the files it writes.
    Started {
        call_id: Option<String>,
        tool: String,
        words: Option<String>,
        paths: Vec<String>,
    },
    /// A shell call waits on the person's permission.
    Asked { command: Option<String> },
    /// The person's words that began a turn, whole.
    Prompt(String),
    /// A call came back — any tool, finished or failed: the calls after a
    /// text have begun to answer, and the step after it is over; while the
    /// claim seat is asked, what a claim may cite of it.
    Finished {
        call_id: Option<String>,
        evidence: Option<Evidence<String>>,
    },
    /// The turn ended: `stopped` when its end says the person stopped it;
    /// while the claim seat is asked, where its answer is — the payload's own
    /// words, credentials scrubbed, or the transcript to read them from.
    TurnEnded { stopped: bool, said: Option<SaidAt> },
    /// What the turn's ruler counts of a call ([`tally::tallied_parsed`]) —
    /// no seat's question, filed beside the moments of the same event.
    Tally(tally::Tallied),
}

/// The kind of text a finished tool handed back, told from its normalized
/// verb and — for a shell's answer — from the window's fence in it: a file
/// read, a web tool, an MCP tool, or an answer the window fenced before a
/// shell handed it back. `None` for every other tool, and for a shell's own
/// output: that is the agent's command speaking, not a page.
#[must_use]
pub fn text_source(verb: &Tool, text: &str) -> Option<TextSource> {
    match verb {
        Tool::Read => Some(TextSource::File),
        Tool::Web | Tool::WebSearch => Some(TextSource::Web),
        Tool::Bash => text
            .contains(crate::untrusted::PHRASE)
            .then_some(TextSource::Browser),
        Tool::Other(name) if name.starts_with(MCP_TOOL_PREFIX) => Some(TextSource::Mcp),
        _ => None,
    }
}

/// The moments one hook event of `agent`'s holds for every seat, in the order
/// the seats read them — none for an event no seat reads, and none that the
/// agent's row cannot see ([`sight`]).
#[must_use]
pub fn moments(agent: AgentKind, event: &str, payload: &str) -> Vec<Moment> {
    moments_parsed(agent, event, &HookPayload::of(payload), Asking::ALL)
}

/// [`moments`], for the seats `asking` asks and a caller that already holds
/// the payload's parse: every field is read off that one tree, and only for
/// a seat that reads it — nothing at all while no seat is asked.
#[must_use]
pub fn moments_parsed(
    agent: AgentKind,
    event: &str,
    parsed: &HookPayload<'_>,
    asking: Asking,
) -> Vec<Moment> {
    let mut found = Vec::new();
    if asking.nothing() {
        return found;
    }
    let row = sight(agent);
    let tree = parsed.tree_or_null();
    // A permission request is attention, not a tool's phase: the one fact the
    // guards take from it is that a shell call waits on the person.
    if hook::hook_state_parsed(event, parsed) == Some(HookState::NeedsAttention) {
        let tool = hook::tool_name_in_parsed(parsed).and_then(|name| Tool::named(&name));
        if asking.command && tool == Some(Tool::Bash) && row.before.yes() {
            found.push(Moment::Asked {
                command: command_in(tree),
            });
        }
        return found;
    }
    let Some(activity) = hook::activity_of_parsed(event, parsed) else {
        return found;
    };
    let call_id = hook::worker_call_id_in(tree);
    let cwd = tree
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|cwd| !cwd.trim().is_empty())
        .map(PathBuf::from);
    // The step after a text reads what every call carries out.
    let steps = asking.text && row.text.yes();
    match activity.phase {
        Phase::Started => {
            let words = match activity.verb {
                Tool::Bash => command_in(tree),
                Tool::Edit | Tool::Write => written_in(tree),
                _ => None,
            };
            if asking.command && activity.verb == Tool::Bash && row.before.yes() && words.is_some()
            {
                found.push(Moment::CommandAbout(Call {
                    id: call_id.clone(),
                    command: words.clone(),
                    cwd,
                }));
            }
            let paths = if asking.file_pick
                && row.edited_path.yes()
                && matches!(activity.verb, Tool::Edit | Tool::Write)
            {
                edited_paths_in(tree)
            } else {
                Vec::new()
            };
            if steps || !paths.is_empty() {
                found.push(Moment::Started {
                    call_id,
                    tool: tool_name(parsed, &activity.verb),
                    words,
                    paths,
                });
            }
        }
        Phase::Finished | Phase::Failed => {
            let failed = activity.phase == Phase::Failed;
            if asking.command && activity.verb == Tool::Bash && row.after.yes() {
                found.push(Moment::CommandRan {
                    call: Call {
                        id: call_id.clone(),
                        command: command_in(tree),
                        cwd,
                    },
                    failed,
                    stopped: failed && row.stopped_call.yes() && hook::call_says_interrupted(tree),
                });
            }
            if steps
                && !failed
                && let Some(text) = result_text(&row, tree)
                && let Some(source) = text_source(&activity.verb, &text)
            {
                found.push(Moment::Text {
                    call_id: call_id.clone(),
                    tool: tool_name(parsed, &activity.verb),
                    source,
                    text,
                });
            }
            let evidence = if asking.claim && row.after.yes() {
                evidence_in(&row, &activity.verb, tree, failed)
            } else {
                None
            };
            if steps || evidence.is_some() {
                found.push(Moment::Finished { call_id, evidence });
            }
        }
        Phase::Prompted => {
            if row.prompt.yes()
                && let Some(words) = crate::transcript::prompt_words_in_parsed(parsed)
            {
                found.push(Moment::Prompt(crate::clone::scrub_credentials(&words)));
            }
        }
        Phase::Stopped => {
            if row.turn_end.yes() {
                let said = if asking.claim && row.turn_answer.yes() {
                    crate::transcript::said_at_in_parsed(parsed).map(|said| match said {
                        SaidAt::Words(words) => {
                            SaidAt::Words(crate::clone::scrub_credentials(&words))
                        }
                        transcript @ SaidAt::Transcript(_) => transcript,
                    })
                } else {
                    None
                };
                found.push(Moment::TurnEnded {
                    stopped: row.stopped_call.yes()
                        && hook::interrupt_declared(agent, event, parsed.text(), false),
                    said,
                });
            }
        }
    }
    found
}

/// The tool's own name, as the payload spells it, or its verb's word.
fn tool_name(parsed: &HookPayload<'_>, verb: &Tool) -> String {
    hook::tool_name_in_parsed(parsed).unwrap_or_else(|| verb.as_str().to_string())
}

/// The whole command a shell call runs: where the common reader finds it
/// ([`crate::hook::worker_command`]), or at the payload's top level — the
/// shell event that names no tool carries it there.
fn command_in(tree: &Value) -> Option<String> {
    hook::worker_command_in(tree).or_else(|| {
        tree.get("command")
            .and_then(Value::as_str)
            .map(|command| crate::clone::scrub_credentials(command.trim()))
            .filter(|command| !command.is_empty())
    })
}

/// The words an edit or a write carries out.
fn written_in(tree: &Value) -> Option<String> {
    let input = hook::INPUT_KEYS.iter().find_map(|key| tree.get(*key))?;
    WRITTEN_WORDS_KEYS
        .iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))
        .map(crate::clone::scrub_credentials)
}

/// The files an edit or a write changes, as its call names them: the path it
/// targets ([`hook::target_in`]), or — for a patch — every file the patch
/// updates, adds or deletes ([`crate::transcript::patch_edits`]).
fn edited_paths_in(tree: &Value) -> Vec<String> {
    let Some(target) = hook::target_in(tree) else {
        return Vec::new();
    };
    let patched: Vec<String> = crate::transcript::patch_edits(&target)
        .into_iter()
        .map(|edit| edit.path)
        .collect();
    if patched.is_empty() {
        vec![target]
    } else {
        patched
    }
}

/// What a claim may cite of a finished call ([`claim::Evidence`]): the shell
/// command that made it, and its output as the claim seat reads zo's — a
/// shell's two streams under their own keys, any other tool's text, a
/// failure's own words — scrubbed of credentials as the person's words are,
/// each kept to its end ([`claim::tail`]). A shell that failed exited
/// non-zero, unless the person stopped it; any other failure is the tool's
/// error.
fn evidence_in(row: &Sight, verb: &Tool, tree: &Value, failed: bool) -> Option<Evidence<String>> {
    let shell = *verb == Tool::Bash;
    let streams = hook::RESULT_KEYS
        .iter()
        .find_map(|key| tree.get(*key))
        .and_then(Value::as_object)
        .filter(|streams| {
            shell
                && claim::STREAM_KEYS
                    .iter()
                    .any(|key| streams.get(*key).is_some_and(Value::is_string))
        });
    let output = if let Some(streams) = streams {
        Value::Object(
            claim::STREAM_KEYS
                .iter()
                .filter_map(|key| {
                    let text = crate::clone::scrub_credentials(streams.get(*key)?.as_str()?);
                    Some(((*key).to_string(), Value::from(claim::tail(&text))))
                })
                .collect(),
        )
        .to_string()
    } else {
        let failure = failed
            .then(|| crate::transcript::tool_failure_words(tree))
            .flatten()
            .map(|words| crate::clone::scrub_credentials(&words));
        let text = failure.or_else(|| result_text(row, tree))?;
        claim::tail(&text).to_string()
    };
    let nonzero = shell && failed && !hook::call_says_interrupted(tree);
    Some(Evidence {
        command: if shell { command_in(tree) } else { None },
        output,
        is_error: failed && !nonzero,
        nonzero,
    })
}

/// A finished tool's text: under the agent's own pointers first, then the
/// common reader ([`crate::hook::worker_output`]), then the text parts of a
/// content list (an MCP tool's answer), then an answer's own text field as
/// the card's line reads it ([`crate::transcript::tool_response_text`] —
/// `text_result_for_llm` and its spellings). Scrubbed, and capped as the
/// viewer caps a result.
fn result_text(row: &Sight, tree: &Value) -> Option<String> {
    let held = hook::RESULT_KEYS.iter().find_map(|key| tree.get(*key));
    let text = row
        .result_at
        .iter()
        .find_map(|at| tree.pointer(at).and_then(Value::as_str))
        .map(str::to_string)
        .or_else(|| hook::worker_output_in(tree).map(|(text, _)| text))
        .or_else(|| {
            // An MCP answer is a list of content parts, bare or under
            // `content`.
            let held = held?;
            let parts: Vec<&str> = held
                .as_array()
                .or_else(|| held.get("content")?.as_array())?
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect();
            (!parts.is_empty()).then(|| parts.join("\n"))
        })
        .or_else(|| crate::transcript::tool_response_text(held?))?;
    let scrubbed = crate::clone::scrub_credentials(text.trim());
    (!scrubbed.is_empty()).then(|| scrubbed.chars().take(hook::WORKER_OUTPUT_CHARS).collect())
}

pub mod tally;

#[cfg(test)]
mod tests;
