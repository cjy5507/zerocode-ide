//! Registered `ToolSpec` table for every tool in this crate.
//!
//! Each entry declares the wire-level schema (`input_schema`) plus the
//! permission level the tool runs under. The dispatcher in `super`
//! looks the spec up by name when a model invokes the tool.
//!
//! Kept separate from the dispatchers (`run_*` and `execute_*`) so
//! reading the catalogue is independent from reading the executor
//! plumbing.

use runtime::PermissionMode;
use serde_json::json;

use super::{
    ToolSpec, MAX_COUNCIL_CANDIDATES, MAX_COUNCIL_CANDIDATE_CHARS, MAX_SEND_TO_USER_CHARS,
    MAX_SPAWN_MULTI_AGENT_AGENTS,
};

// --- tool_specs and dispatch ---

#[allow(clippy::too_many_lines)] // a flat spec table, clearer unsplit
pub(crate) fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "Skill",
            description: "Load and follow a discovered Zo skill when the user names it or the task clearly requires it.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "skill": { "type": "string" },
                    "args": { "type": "string" }
                },
                "required": ["skill"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "skill_search",
            description: "Rank every installed skill against the work at hand and return the best ones' full SKILL.md. Use it when the system prompt lists no skills, or when more are installed than it lists.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "A sentence or two describing the work at hand, not keywords."
                    },
                    "maxSkills": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": zerocode_core::jev::SKILL_TOP_CAP,
                        "description": "How many to return whole; defaults to 3."
                    }
                },
                "required": ["task"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "skill_load",
            description: "Load installed skills by name and return each one's full SKILL.md. Case and separators are ignored; a name nothing answers to comes back with the closest ones.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "names": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1
                    }
                },
                "required": ["names"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "SkillDistill",
            description: "Write a reusable skill draft as `.zo/skills/<slug>/SKILL.md` with `state: proposed`. Use only after a task reveals repeatable procedure knowledge worth saving; proposed drafts are not auto-activated until approved. To evolve an existing draft (or augment one a duplicate check pointed you to), pass `update: true` to bump its version and rewrite the body.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "slug": {
                        "type": "string",
                        "description": "Lowercase kebab-case skill slug, used as the directory name under .zo/skills."
                    },
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "body": { "type": "string" },
                    "update": {
                        "type": "boolean",
                        "description": "Re-distill an existing same-slug draft: bump its version and rewrite the body (stays proposed). Defaults to false, which refuses to overwrite."
                    }
                },
                "required": ["slug", "description", "body"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
        ToolSpec {
            name: "SkillReview",
            description: "Approve or discard a proposed skill draft created under `.zo/skills/<slug>/SKILL.md`. Approval changes `state: proposed` to `state: active`; discard removes the draft cleanly.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "slug": {
                        "type": "string",
                        "description": "Lowercase kebab-case skill slug under .zo/skills."
                    },
                    "action": {
                        "type": "string",
                        "enum": ["approve", "discard"]
                    }
                },
                "required": ["slug", "action"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
        ToolSpec {
            name: "Agent",
            // Only what a CALL needs. Routing — when to delegate at all, when a
            // fan-out or a pipeline fits better — is taught once, in the
            // `# Delegation and workflow routing` prompt section; delivery of
            // a detached result is stated per call by the receipt
            // (`BACKGROUND_AGENT_NOTE`); the r49 text repeated all of it here
            // at 1,200 tokens on every request, which is what kept this schema
            // off the wire (t-2903). Every property below is used by the local
            // corpus (name 277 · background 226 · model 97 ·
            // allow_cross_provider 80 · sees 9 of 291 calls). The
            // `subagent_type` list names the profiles that corpus reaches for
            // (code-reviewer 87 · Explore 70 · general-purpose 70 ·
            // deep-research 17 · debugger 12 · Plan 12 · data-analyst 6 ·
            // custom 12); `Verification` and `refactor` (1 call each) stay
            // callable by name and are not advertised on every request.
            description: "Launch one sub-agent for one bounded task. Detached by default in the interactive main session; blocking in sub-agent and headless runs. Relay its result; the user never sees it.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "description": { "type": "string" },
                    "prompt": { "type": "string" },
                    "subagent_type": { "type": "string", "description": "general-purpose, Explore, Plan, deep-research, code-reviewer, debugger, data-analyst, fork (inherits chat), .zo/agents/<name>" },
                    "name": { "type": "string", "description": "SendMessage address." },
                    "model": { "type": "string", "description": "Omit to inherit the session model; else same provider family." },
                    "allow_cross_provider": { "type": "boolean", "description": "For user-requested read-only delegation only." },
                    "background": { "type": "boolean" },
                    "sees": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Finished agents whose results it reads."
                    }
                },
                "required": ["description", "prompt"],
                "additionalProperties": false
            }),
            // Spawning is not itself a privileged act: the child's enforcer is
            // clamped to the parent's active mode (`clamped_spawn_mode`), so a
            // read-only session can delegate read-only research/analysis.
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "CapabilityInvoke",
            description: "Run a deferred registered tool by exact name, with input shaped by its ToolSearch schema. The inner tool's permissions apply.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "input": { "type": "object", "description": "Arguments matching the searched schema." }
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            // The wrapper itself grants nothing: the inner name re-enters the
            // same executor and is gated as if the model had called it
            // directly. A stricter requirement here would deny a read-only
            // session tools it is allowed to run.
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "ToolSearch",
            description: "Find deferred tools by `select:Name1,Name2` or keywords. Returns schemas without changing the wire list; batch expected names, then call them via CapabilityInvoke.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "max_results": { "type": "integer", "minimum": 1 }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "Audit",
            description: "Summarize this session's tool-invocation ledger: how many tools ran, how many the policy allowed / denied / failed, per-family counts, and the reason for every denial. Read-only; takes no arguments.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "NotebookEdit",
            description: "Replace, insert, or delete a cell in a Jupyter notebook.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "notebook_path": { "type": "string" },
                    "cell_id": { "type": "string" },
                    "new_source": { "type": "string" },
                    "cell_type": { "type": "string", "enum": ["code", "markdown"] },
                    "edit_mode": { "type": "string", "enum": ["replace", "insert", "delete"] }
                },
                "required": ["notebook_path"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
        ToolSpec {
            name: "Sleep",
            description: "Wait for a specified duration without holding a shell process.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "duration_ms": { "type": "integer", "minimum": 0 }
                },
                "required": ["duration_ms"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "send_to_user",
            description: "During a long autonomous run, push important verbatim content straight to the user without ending the turn — a finding, a diff, a URL, a config block, the exact text they must see now. This is NOT a status report or progress ping: reserve it for content that is worth showing verbatim, and do not narrate every step with it (spamming the user each turn defeats the purpose). In headless / sub-agent runs with no interactive surface the message is returned inline instead.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "message": {
                        "type": "string",
                        "maxLength": MAX_SEND_TO_USER_CHARS
                    }
                },
                "required": ["message"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "Brief",
            description: "Legacy alias for `send_to_user`: push a message to the user mid-run. Prefer `send_to_user`.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "SyntheticOutput",
            description: "Inject synthetic tool output for testing or scripted flows.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tool_name": { "type": "string" },
                    "output": { "type": "string" }
                },
                "required": ["tool_name", "output"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "StopAgent",
            description: "Stop one sub-agent this session spawned, by its exact agent id. Use it when an agent you launched is no longer worth its remaining cost — it is repeating itself, chasing a path the task no longer needs, or waiting on something that will not arrive — and you would otherwise leave it running with nobody watching. It refuses anything it cannot prove you own, so it can only ever end your own work. Give `reason` in one plain sentence; it is recorded on the agent and shown to the user. Do NOT use this to \"clean up\" agents that are simply slow: a running agent still holds everything it has learned, and stopping it throws that away.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["agent_id", "reason"],
                "additionalProperties": false
            }),
            // Ending your own sub-agent needs no more authority than starting
            // one: the ownership proof below is what makes it safe, not the
            // permission tier.
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "PushNotification",
            // Claude Code's tool of the same name, with zo's roads in place of
            // Remote Control: in the ZeroCode window an OS notification whose
            // click focuses this pane, in a bare terminal a bell plus OSC 9 /
            // OSC 777. The attendance rule is Claude Code's own — a person at
            // the keyboard already sees the answer, so the tool skips and says
            // so (`docs/design/zo-push-notification.md`).
            description: "Send a desktop notification to the person — in the ZeroCode window an OS notification whose click focuses this pane; in a bare terminal a bell plus OSC 9/777. Only for something they would act on (a finished task, a blocker), never routine progress. One line, under 200 characters, no markdown; lead with the action. When they are at the keyboard the tool skips and says so — a `skipped` road is expected, not an error.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "message": {
                        "type": "string",
                        "minLength": 1,
                        "description": "One line, under 200 characters; mobile OSes truncate."
                    },
                    // Claude Code spells this `const: "proactive"`; a one-value
                    // enum says the same and every provider reads it.
                    "status": { "type": "string", "enum": ["proactive"] }
                },
                "required": ["message", "status"],
                "additionalProperties": false
            }),
            // Calling the person is not a privilege: the attendance rule and
            // the window's own bell ladder bound it, not the permission tier.
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "Jev",
            // The agent tool seat (t-6040): TypeSafe's System One asked by the
            // model itself, through the one Jev door, on the person's own
            // switch (`smart.agentTool`). Deferred, like every tool a plain
            // coding turn does not touch; the manifest hook is its whole
            // advertisement.
            description: "Ask Jev, TypeSafe's fast typed judge, instead of reasoning it out: `ask` a yes/no question, `choose` one of your options, or `score` items on your own ordered levels (low to high, 2-10). Give it the facts in `context`; it returns probabilities, no prose. Off unless smart.agentTool is on — an `off` or `shadow` verdict means decide it yourself.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "shape": { "type": "string", "enum": ["ask", "choose", "score"] },
                    "question": { "type": "string", "minLength": 1 },
                    "context": { "type": "string", "description": "ask/choose: the facts to read first." },
                    "options": { "type": "array", "items": { "type": "string" }, "description": "choose: 2-255 options." },
                    "levels": { "type": "array", "items": { "type": "string" }, "description": "score: 2-10 level descriptions, low to high." },
                    "items": { "type": "array", "items": { "type": "string" }, "description": "score: the items to grade." }
                },
                "required": ["shape", "question"],
                "additionalProperties": false
            }),
            // Asking a judge is not a privilege: the person's switch and the
            // Jev door bound what leaves, not the permission tier.
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "ListAgents",
            description: "List the sub-agents and teammates THIS session owns, with what each one is and where it is. One row per agent: `id` (what `SendMessage`/`StopAgent` take), `name` (its addressable label), `status`, `execution` — `inline` for a thread of this process, `pane` for a teammate running in a zo of its own — its `pane` when it has one, `lastReceipt` (what the last message to it came to: consumed/queued/rejected), `run_generation`, and `completion_waiting`, which says a finished agent's result is sitting there for `GetAgentCompletion` to collect. Reach for it when you have lost track of what you launched, before spawning something you may already have running, or when a result you expected has not arrived and you want to know whether the agent is still going. It reads the session's own registry, so another session's agents never appear; `format: \"table\"` returns the same rows as compact text instead of JSON.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "format": { "type": "string", "enum": ["json", "table"] }
                },
                "additionalProperties": false
            }),
            // Reading your own roster is not a privilege: the session filter
            // below is what bounds it, not the permission tier.
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "SpawnMultiAgent",
            description: "Spawn multiple sub-agents in one flat parallel fan-out and collect their results once all finish. Use this for independent research, review, verification, or implementation slices that should run as a real swarm; use `Workflow` instead when phases depend on earlier results. Size the fan-out to the ask, not the session mode: never spawn a swarm or a multi-perspective verification panel for a simple question, a lookup, or a bounded single-file fix — those take zero agents, or at most one. Each entry's `subagent_type` selects that agent's harness (see Agent). Provider requests flow through an adaptive per-provider rate governor that opens with genuine parallelism when headroom is healthy, stays serial during recent rate-limit pressure, ramps while quota has headroom, and backs off on rate limits; optional `concurrency` can only tighten this. Sub-agents inherit the active parent/session model by default; a per-agent `model` is honored inside the same provider family, and read-only cross-family delegation requires `allow_cross_provider: true` only when the user named that model. Implementation stays in the parent's provider and reserved models require the person's launch/settings pin; model-authored flags cannot authorize exceptions. Never substitute a different model when the user named one. The user-level ZO_AGENT_MODEL override still forces one model for every sub-agent. Task difficulty tunes the inherited model's reasoning budget/effort rather than silently switching to a different model.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "agents": {
                        "type": "array",
                        "maxItems": MAX_SPAWN_MULTI_AGENT_AGENTS,
                        "items": {
                            "type": "object",
                            "properties": {
                                "subagent_type": { "type": "string" },
                                "prompt": { "type": "string" },
                                "description": { "type": "string" },
                                "name": { "type": "string" },
                                "model": { "type": "string", "description": "Semantic model alias or explicit provider/model pin. Prefer versionless aliases and never infer a concrete release id from stale knowledge." },
                                "allow_cross_provider": { "type": "boolean", "description": "Read-only delegation only, when the user named a cross-family model. Implementation exceptions require a launch/settings pin." },
                                "sees": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Names/ids of ALREADY-FINISHED agents (from an earlier spawn — members of THIS call run concurrently and cannot see each other) whose results this member should read. The harness injects each deliverable verbatim as a labeled shared-context section ahead of the prompt, so a second wave never needs its inputs re-typed into its prompts: declare the edge instead. Errors if a referenced agent is unknown or still running."
                                }
                            },
                            "required": ["prompt"]
                        }
                    },
                    "concurrency": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_SPAWN_MULTI_AGENT_AGENTS,
                        "description": "Live sub-agent window; unset defaults to min(16, cores-2) and later members queue for freed slots. Provider requests still pass through the adaptive per-provider rate governor, so widening this never raises real API concurrency."
                    }
                },
                "required": ["agents"],
                "additionalProperties": false
            }),
            // Like `Agent`: spawning is unprivileged, the members' enforcers
            // are clamped to the parent's active mode (`clamped_spawn_mode`).
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "Council",
            description: "Compare multiple candidate answers for the same task without exposing candidate source/model identity. Use after fan-out (for example SpawnMultiAgent) to select a self-consistent majority when one exists, or return an honest tie when candidates fail or disagree. Pass each candidate's answer verbatim: when EVERY successful candidate ends its answer with a final `VERDICT: <one line>` line, agreement is judged on those declared verdicts, and otherwise on the full text, which independently written answers almost never match. The verdict is read out of the answer itself — there is no separate field to supply — and the returned `voted_on` reports which basis decided the outcome.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "candidates": {
                        "type": "array",
                        "maxItems": MAX_COUNCIL_CANDIDATES,
                        "items": {
                            "type": "object",
                            "properties": {
                                "text": {
                                    "type": "string",
                                    "maxLength": MAX_COUNCIL_CANDIDATE_CHARS
                                },
                                "status": { "type": "string" }
                            },
                            "required": ["text"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["candidates"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "SendUserMessage",
            description: "Legacy alias for `send_to_user`: push a message to the user mid-run. Prefer `send_to_user`.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "Config",
            description: "Get or set Zo settings.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "setting": { "type": "string" },
                    "value": {
                        "type": ["string", "boolean", "number"]
                    }
                },
                "required": ["setting"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
        ToolSpec {
            name: "EnterPlanMode",
            description: "Enable a worktree-local planning mode override and remember the previous local setting for ExitPlanMode.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
        ToolSpec {
            name: "ExitPlanMode",
            description: "Clear the worktree-local settings override created by EnterPlanMode (settings-file management; needs write access). NOT the plan-submission tool: while in read-only plan mode, present your plan with ExitPlanModeV2 (deferred; call it by name or via CapabilityInvoke) instead — plan mode is lifted only by the user.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
        ToolSpec {
            name: "StructuredOutput",
            description: "Return structured output in the requested format.",
            input_schema: json!({
                "type": "object",
                "additionalProperties": true
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "AskUserQuestion",
            description: "Ask the user a question and wait for their response. Reserve this for decisions where the answer changes what you do next — a missing target, success criterion, or safety boundary with more than one viable path. Do not use it for choices with a conventional default or facts you can verify in the codebase yourself: pick the obvious option, say so, and proceed. Offer 2-4 options with one-line descriptions of their tradeoffs; the user can always type a free-form answer instead, so the returned answer may not match any option. Options are mutually exclusive by default; set `multiSelect: true` when the user may pick several, in which case `answer` comes back as an array. When the options are concrete artifacts the user should compare by eye — UI layouts, diagram shapes, code or config alternatives — give each one a `preview` holding an ASCII mockup or snippet; the modal then shows the option list beside a preview pane. Skip previews for plain preference questions where the labels already say everything.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "The complete question to ask, ending with a question mark."
                    },
                    "header": {
                        "type": "string",
                        "description": "Very short topic chip shown beside the prompt title (max ~12 chars), e.g. \"Auth method\"."
                    },
                    "options": {
                        "type": "array",
                        "description": "Fixed choices in display order. Plain strings or {label, description, preview} objects; the description explains the tradeoff in one line.",
                        "items": {
                            "oneOf": [
                                { "type": "string" },
                                {
                                    "type": "object",
                                    "properties": {
                                        "label": { "type": "string" },
                                        "description": { "type": "string" },
                                        "preview": {
                                            "type": "string",
                                            "description": "Monospace preview of the concrete artifact this option produces — an ASCII layout mockup, a code snippet, a diagram, a config sample. Rendered in a pane beside the option list, so use it when the options are things the user should SEE and compare, not for plain preference questions where the labels already say everything. Multi-line text is fine. Single-select only."
                                        }
                                    },
                                    "required": ["label"],
                                    "additionalProperties": false
                                }
                            ]
                        }
                    },
                    "multiSelect": {
                        "type": "boolean",
                        "description": "When true, the user may check several options and the `answer` is returned as an array of the selected values. Defaults to false (a single mutually-exclusive choice)."
                    }
                },
                "required": ["question"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "MemoryWrite",
            description: "Atomically record a persistent project memory entry. By default it writes the machine-local project overlay (`memory.local`), which survives compaction and a restart on this machine but cannot modify global memory. Set `local: false` only to write durable global project knowledge in `memory/`. Both live under Zo's config home, never the repository, and recall merges both.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "summary": { "type": "string" },
                    "body": { "type": "string" },
                    "local": { "type": "boolean", "description": "Write the machine-local `memory.local` overlay. Defaults to true; set false only for explicit durable global memory." }
                },
                "required": ["slug", "summary", "body"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
        ToolSpec {
            name: "RemoteTrigger",
            description: "Trigger a remote action or webhook endpoint.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "method": { "type": "string", "enum": ["GET", "POST", "PUT", "DELETE"] },
                    "headers": { "type": "object" },
                    "body": { "type": "string" }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "Monitor",
            description: "Stream stdout lines from a background process as notifications.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "process_id": { "type": "string" },
                    "command": { "type": "string" },
                    "lines": { "type": "integer", "minimum": 1 }
                },
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "SendMessage",
            description: "Send a message to a previously spawned agent by name or id. A RUNNING \
                          agent receives it mid-turn (injected at its next tool boundary) and \
                          keeps working with the new guidance. A COMPLETED/failed/stopped agent \
                          is RESUMED in the background with its full prior context intact and \
                          your message as its next turn — its reply is delivered to you in a \
                          later message, so do NOT poll. Use this to steer, follow up, or ask \
                          an agent to go deeper instead of spawning a fresh agent and \
                          re-explaining the task.\n\n\
                          | `to` | |\n\
                          | `\"researcher\"` | Teammate by name — the `name` you passed to \
                          `Agent`. Names keep working after the agent finishes (a send resumes \
                          it from its transcript). |\n\
                          | `\"<agentId>\"` | The raw agent id from the spawn result or the task \
                          notification. Use it only when the agent has no name, or when a newer \
                          agent took the name (latest wins). |\n\
                          | `\"main\"` | The conversation that spawned you; not a spawnable agent \
                          name. If YOU are a sub-agent this is your ONLY allowed target: send \
                          there to report a blocking question or a finding that cannot wait for \
                          your final result, then keep working — do not wait for a reply. |",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "to": { "type": "string", "description": "Teammate name (exact, latest wins on duplicates) or agent id (prefix match allowed)" },
                    "message": { "type": "string" },
                    "attach_results": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Names/ids of FINISHED agents whose final results travel with this message as a labeled shared-context section — the harness relays each deliverable verbatim, so never re-type one agent's output to send it to another. Perfect for feedback loops: SendMessage(to: builder, attach_results: [reviewer]) hands the reviewer's findings to the builder with its own context intact. Errors if a referenced agent is unknown or still running."
                    },
                    "refresh_harness": {
                        "type": "boolean",
                        "description": "When resuming a finished agent, re-resolve its harness (role prompt, tools, permission rules) from the current agent definition instead of reloading the one it was spawned with. Default false: a follow-up keeps the SAME agent, and the response says harness: \"stale\" if its definition changed underneath."
                    }
                },
                "required": ["to", "message"],
                "additionalProperties": false
            }),
            // Same grade as `Agent`/`SpawnMultiAgent`: the resume path re-spawns
            // a worker with its original permission envelope, so a lower grade
            // here would let a read-only context relaunch a full-access agent.
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "ScheduleWakeup",
            description: "Schedule this session's next self-paced turn: once the session is idle and delaySeconds have passed, a new turn opens with `prompt` (the delay is bounded by the session's autonomy limits). Set noop: true when this tick found nothing to report; call with stop: true alone to end the loop so no further wakeup fires. A wakeup turn that schedules nothing ends the loop.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "delaySeconds": { "type": "number", "minimum": 0 },
                    "reason": { "type": "string" },
                    "prompt": { "type": "string" },
                    "noop": { "type": "boolean", "default": false },
                    "stop": { "type": "boolean", "default": false }
                },
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "retrieve_tool_output",
            description: "Recover the FULL original of a tool output that was truncated. When a result ends with a notice naming a sha256 artifact id, call this with that id to read the untruncated output from the local artifact store; narrow large outputs with `offset`/`limit` (a 0-based line window, same semantics as read_file).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "sha256": { "type": "string", "description": "64-char hex id from the truncation notice" },
                    "offset": { "type": "integer", "minimum": 0, "description": "0-based first line of the window" },
                    "limit": { "type": "integer", "minimum": 1, "description": "maximum lines to return" }
                },
                "required": ["sha256"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "session_recall",
            description: "Selectively read a PAST conversation (read-only) without resuming it. Two modes: (1) RECALL — pass `session_ref` (a session id, \"latest\"/\"last\"/\"recent\", or \"current\" for THIS session) to read that session, optionally narrowed by a substring `query`, a `role`, a `last_n` tail, and/or a `seq_from`/`seq_to` window; (2) SEARCH — omit `session_ref` and pass `query` to find which prior sessions discussed something, with per-session match counts, optionally restricted to a `since_days`/`before_days` time window. Use \"current\" with a `seq_from`/`seq_to` range to pull back the EXACT raw originals a compaction round sealed to this session's vault (the continuation message names the spans). Returns only the matched excerpt as text and never touches the live transcript.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session_ref": { "type": "string", "description": "Session id, \"latest\"/\"last\"/\"recent\", or \"current\" (THIS session, e.g. to recover its just-compacted originals). Omit (or \"all\") + query to search across all sessions. Paths are not accepted; discover ids via search mode." },
                    "query": { "type": "string", "description": "Case-insensitive substring to match in message text. Required in search mode." },
                    "role": { "type": "string", "enum": ["user", "assistant", "tool", "system"] },
                    "last_n": { "type": "integer", "minimum": 1, "description": "Return only the last N matching messages." },
                    "seq_from": { "type": "integer", "minimum": 0, "description": "Inclusive lower bound of the seq window. Seqs are one monotonic domain per session: an evicted vault record's vault_seq and a live message's absolute index share it, so this window addresses both. Combined (AND) with query/role." },
                    "seq_to": { "type": "integer", "minimum": 0, "description": "Inclusive upper bound of the seq window. Must be >= seq_from." },
                    "include_tool_results": { "type": "boolean", "description": "Include tool-result blocks in the output (default true). Set false to omit them; a message with no other content is then skipped." },
                    "since_days": { "type": "number", "minimum": 0, "description": "SEARCH only: keep sessions modified within the last N days (recent edge; fractional ok). Rejected in recall mode." },
                    "before_days": { "type": "number", "minimum": 0, "description": "SEARCH only: keep sessions modified more than N days ago (older edge). Pair with since_days to bracket a span (since_days >= before_days); since_days < before_days is an empty window and is rejected." }
                },
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
    ]
}
