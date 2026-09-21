//! `zo-ide` 명령줄 — 손으로 쓴 작은 파서(의존 추가 없음).
//!
//! 플래그는 계획서 keep-list 만큼만: 모델·권한·effort·resume·MCP 설정·cwd,
//! 그리고 표시 옵션 셋(`--render-markdown`, `--show-thinking`,
//! `--verbose-stderr`). 나머지 zo 플래그
//! 164종은 이 바이너리에 없다.

use std::path::PathBuf;
use std::time::Duration;

use runtime::PermissionMode;

use crate::effort::Effort;
use crate::permission_mode::{normalize_permission_mode, permission_mode_from_label};
use crate::session::plain_session::OpenOptions;

pub const USAGE: &str = "\
zo [--model <alias>] [--permission-mode <mode>] [--effort <level>]
       [--resume [<id|latest|path>]] [--continue] [--mcp-config <file>] [--cwd <dir>]
       [--no-spawn] [--allowed-tools <names>] [--render-markdown] [--show-thinking] [--verbose-stderr]
       [--teammate <dir> [--resume-transcript <path>]] [--launch-contract <version>]
       [--events-bind <addr>] [-p|--plain] [--json] [--last-message <file>]
       [--loop-every <duration>|--loop-until <command>] [--loop-max <runs>]
       [--status] [--prompt-input] [--orchestration-accuracy] [--version] [--help]
zo models [--refresh] [--json]
zo commands [--json]
zo mcp list|get|add|remove|login|logout [<name>] [--url <url>|-- <command>…] [--env K=V] [--header K=V]
       [--transport stdio|http|sse|ws] [--scopes <a,b>] [--project [--trust]] [--cwd <dir>] [--json]
zo cron ensure|show|remove --description <name> [--schedule <expr>] [--prompt <text>|--prompt-file <path|->] [--cwd <dir>]
zo decision-shadow eval --labels <file.jsonl> [--cwd <dir>] [--json]
zo decision-shadow check [--json]

  --permission-mode  read-only | workspace-write | danger-full-access
                     (Claude Code aliases plan/default/acceptEdits/bypassPermissions accepted)
  --effort           off | low | medium | high | xhigh | max | ultra | smart
  --resume           restore a session; with no id, list recent sessions and exit
  --continue, -c     restore the most recent session (same as --resume latest)
  --orchestration-accuracy
                     print the cwd project's orchestration accuracy report as
                     one JSON line (the window's board card execs this) and exit
  --no-spawn         turn off zo's own Agent/SpawnMultiAgent/Workflow/Task tools
                     (they are on by default: a sub-agent keeps its reading out
                     of this session's context, which is what a long session
                     needs; the window's ledger still owns pane-level workers)
  --allowed-tools    the only tools this session has, comma-separated (e.g.
                     Computer): nothing else is advertised, and a call to any
                     other tool is refused before it runs. A deferred tool
                     named here is advertised from the first request
  --launch-contract  exact launch (version 1): validate --model/--effort
                     against the published catalog before the session opens
                     and either launch exactly as asked or exit 4 with a named
                     reason (unsupported-effort, unsupported-model, …); never
                     clamp, demote or switch provider. Also ZO_LAUNCH_CONTRACT=1
                     or the settings key launchContract = 1; 0 closes the door
  --teammate         run the brief in <dir> as somebody's teammate: the first
                     turn from the brief, the answer written back beside it
                     (result.json), then WAIT for the parent's next words over
                     the events channel (result-<n>.json per turn) until the
                     parent closes this pane, its channel disappears, or the
                     idle budget runs out (result-final.json). This is what a
                     parent zo starts in a pane of its own when its `Agent`
                     tool runs in panes mode (`ZO_SUBAGENT_MODE`); it implies
                     --no-spawn, because a teammate does not start teammates
  --resume-transcript
                     with --teammate: continue this transcript instead of
                     starting fresh — a teammate re-cut after its pane closed
  --plain, -p        skip the interactive front-end and print the append-only
                     transcript even on a terminal (same bytes as a pipe)
  --json             emit one JSON object per line instead of prose — the
                     contract another program drives a long run with. Implies
                     --plain; nothing is folded, summarized or coloured
  --last-message     write the final assistant message to this file (also with
                     --json), so a caller reads one answer without parsing
  --loop-every       repeat the first piped prompt at this interval
  --loop-until       repeat it until this read-only command succeeds
  --loop-max         bound the run count (defaults to autonomy.maxLoopRuns)
  --verbose-stderr   keep stderr on the terminal. By default an interactive run
                     sends it to ~/.zo/logs/zo-ide.log so credential notes,
                     retention sweeps and panics stay out of the pane
  --events-bind      open the IDE events channel on a loopback address (e.g.
                     127.0.0.1:8788, or :0 to let the kernel pick). Interactive
                     TUI runs open :0 automatically; headless runs do not
  --status           open the session, print one status line, exit (engine smoke)
  --prompt-input     open the session, print the fixed harness the model is
                     shown (system core / tool schemas / skill index /
                     reminders, per piece, against the r15 budget), exit
  mcp                the configured MCP servers, without opening a session:
                     `list`/`get` render what ConfigLoader merged (a project
                     server the supply-chain gate skipped is shown as
                     untrusted, with the document that declared it),
                     `add`/`remove` write one server into ~/.zo/settings.json
                     or, with --project, <cwd>/.zo/settings.json — where the
                     server stays gated until --trust records that one name in
                     the project's trust record — and `login`/`logout` hold a
                     remote server's OAuth token
  decision-shadow    score the routing probe and its typed twin
                     (smart.decisionShadow) against a person's labels:
                     --labels names a JSONL file, one task per line with a
                     label per axis; the workspace's shadow ledger is joined
                     by task fingerprint and each reader's accuracy, answers
                     below the label, Brier score and calibration are printed.
                     `check` asks System One the twin's own question about a
                     fixed task with the key zo would use (TYPESAFE_API_KEY or
                     ZeroCode's settings) and prints the model and latency
  models             print the live model catalog: shipped rows, what the
                     connected providers serve today, and where each family
                     alias (fable, opus, sol, gemini-flash, …) points.
                     --refresh asks the providers now instead of trusting the
                     cache; settings `modelUpdatePolicy` (auto|notify|pinned)
                     decides whether aliases follow a new release";

/// 렌더러에 넘기는 표시 옵션.
///
/// 넷 다 독립된 on/off 플래그다 — enum 으로 묶으면 조합이 곱해진다.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderFlags {
    /// codex exec 실측은 마크다운을 원문 그대로 찍는다 — 렌더는 opt-in.
    pub render_markdown: bool,
    /// `--show-thinking`. The pipe front keeps thinking hidden without it
    /// (`codex exec` runs with reasoning summaries off); the interactive
    /// front shows thinking by default and reads the `showThinking` setting
    /// and `/thinking` on top of this flag (`tui::thinking`, t-5872).
    pub show_thinking: bool,
    /// stderr 를 터미널에 그대로 둘지. 기본(거짓)은 로그 파일로 돌린다 —
    /// `run_loop::install_stderr_redirect` 참고.
    pub verbose_stderr: bool,
    /// 참이면 터미널에서도 대화형 프런트(`tui`) 대신 append-only 경로를 쓴다.
    /// 파이프 골든과 같은 바이트를 사람이 눈으로 볼 때의 문이다.
    pub plain: bool,
    /// 산문 대신 이벤트마다 JSON 한 줄. 사람이 아니라 프로그램이 읽는 출력이라
    /// 대화형 프런트를 켤 수 없다 — 파서가 `--plain` 을 함께 세운다.
    pub json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Repl,
    Status,
    /// `--prompt-input`: open the session, print the fixed-harness accounting,
    /// exit. A diagnostic, so it costs no provider request.
    PromptInput,
    /// `--resume` with no id: print the recent-session list and exit.
    Sessions,
    /// `zo models`: print the live model catalog and exit. `refresh` asks the
    /// providers now instead of trusting the discovery cache.
    Models { refresh: bool },
    /// `zo commands`: print the slash commands this binary handles
    /// (`slash::Slash::CATALOG`, name and description) and exit. The window's
    /// composer palette reads it, so the list a person sees in the GUI is the
    /// one this very binary answers to — never a copy kept elsewhere.
    Commands,
    Version,
    /// Print the cwd project's orchestration accuracy report as one JSON line
    /// and exit. The window's board card execs this — the shell never links zo
    /// internals or re-derives the project state slug (P4 surface).
    OrchestrationAccuracy,
    Help,
}

pub struct Launch {
    /// Immutable spelling of explicit selection flags, before alias resolution.
    pub requested: super::channel::capabilities::Requested,
    pub action: Action,
    pub open: OpenOptions,
    pub render: RenderFlags,
    /// `--permission-mode` 가 명시됐는가 — 아니면 신뢰 게이트가 결정한다.
    pub permission_mode_explicit: bool,
    /// `--events-bind` 의 값. 없으면 `ZO_EVENTS_BIND` 가 결정한다
    /// ([`crate::ide::events::EventsConfig::from_launch`]). 둘 다 없는
    /// 대화형 TUI 는 앱 발견용 포트 0 채널을 자동으로 연다.
    pub events_bind: Option<String>,
    /// `--last-message` 의 목적지. 완성된 마지막 어시스턴트 답변 하나가
    /// 여기 앉는다 — 호출자가 출력 전체를 파싱하지 않고 답만 읽는 문이다.
    pub last_message: Option<PathBuf>,
    /// `--teammate <dir>`: the directory holding this child's `brief.json`,
    /// and where its `result.json` goes. `Some` means one turn and out.
    pub teammate: Option<PathBuf>,
    /// Bounded recurring execution for the first prompt read by the headless
    /// frontend. It deliberately carries only scheduling; the prompt itself
    /// still comes through the ordinary stdin pump.
    pub headless_loop: Option<HeadlessLoop>,
    /// `--launch-contract <version>`, raw. The one flag door of
    /// [`crate::launch_contract`]; the env and settings doors are read there.
    pub launch_contract: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlessLoop {
    pub trigger: HeadlessLoopTrigger,
    pub max_runs: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadlessLoopTrigger {
    Every { raw: String, duration: Duration },
    Until { command: String },
}

#[allow(clippy::too_many_lines)] // 평평한 플래그 match — 한 arm 씩
pub fn parse(args: &[String]) -> Result<Launch, String> {
    let mut requested = super::channel::capabilities::Requested::default();
    let mut model = crate::DEFAULT_MODEL.to_string();
    let mut permission_mode: Option<PermissionMode> = None;
    let mut effort: Option<Effort> = None;
    let mut resume: Option<String> = None;
    let mut mcp_config: Option<PathBuf> = None;
    let mut cwd: Option<PathBuf> = None;
    let mut no_spawn = false;
    let mut allowed_tools = None;
    let mut render = RenderFlags::default();
    let mut events_bind: Option<String> = None;
    let mut last_message: Option<PathBuf> = None;
    let mut teammate: Option<PathBuf> = None;
    let mut loop_trigger: Option<HeadlessLoopTrigger> = None;
    let mut loop_max: Option<u32> = None;
    let mut launch_contract: Option<String> = None;
    let mut action = Action::Repl;

    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        let value = |name: &str| -> Result<String, String> {
            args.get(index + 1)
                .cloned()
                .ok_or_else(|| format!("{name} requires a value"))
        };
        match flag {
            "models" if index == 0 => {
                action = Action::Models { refresh: false };
                index += 1;
            }
            "commands" if index == 0 => {
                action = Action::Commands;
                index += 1;
            }
            "--refresh" => {
                let Action::Models { .. } = action else {
                    return Err("--refresh belongs to `zo models`".to_string());
                };
                action = Action::Models { refresh: true };
                index += 1;
            }
            "--model" | "-m" => {
                let raw = value("--model")?;
                model = crate::cli_args::resolve_model_alias(&raw);
                requested.model = Some(raw);
                index += 2;
            }
            "--permission-mode" => {
                let raw = value("--permission-mode")?;
                let label = normalize_permission_mode(&raw).ok_or_else(|| {
                    format!(
                        "unsupported permission mode '{raw}'. Use read-only, workspace-write, \
                         danger-full-access, default, acceptEdits, plan, or bypassPermissions."
                    )
                })?;
                permission_mode = Some(permission_mode_from_label(label));
                index += 2;
            }
            "--effort" | "-e" => {
                let raw = value("--effort")?;
                requested.effort = Some(raw.clone());
                effort = Some(
                    Effort::from_token(&raw)
                        .ok_or_else(|| format!("unsupported effort '{raw}'"))?,
                );
                index += 2;
            }
            "--resume" | "-r" => {
                // Bare `--resume` (no value, or the next token is another flag)
                // asks for the list instead of restoring anything.
                if let Some(reference) = args.get(index + 1).filter(|next| !next.starts_with('-')) {
                    resume = Some(reference.clone());
                    index += 2;
                } else {
                    action = Action::Sessions;
                    index += 1;
                }
            }
            "--continue" | "-c" => {
                resume = Some(crate::LATEST_SESSION_REFERENCE.to_string());
                index += 1;
            }
            "--mcp-config" => {
                mcp_config = Some(PathBuf::from(value("--mcp-config")?));
                index += 2;
            }
            "--cwd" => {
                cwd = Some(PathBuf::from(value("--cwd")?));
                index += 2;
            }
            "--events-bind" => {
                events_bind = Some(value("--events-bind")?);
                index += 2;
            }
            "--no-spawn" => {
                no_spawn = true;
                index += 1;
            }
            "--allowed-tools" => {
                allowed_tools = allowed_tool_names(&value("--allowed-tools")?)?;
                index += 2;
            }
            "--teammate" => {
                teammate = Some(PathBuf::from(value("--teammate")?));
                // A teammate does not start teammates. Forced here rather than
                // trusted to the parent's command line: the flag is what stops
                // a grandchild pane from being cut out of a pane, and a parent
                // that forgot it would be one `Agent` call away from a screen
                // nobody can read.
                no_spawn = true;
                index += 2;
            }
            "--resume-transcript" => {
                // A teammate re-cut to continue an earlier pane's conversation
                // (t-2513 §2.4). The transcript is a path, and the resume road
                // already reads one; the brief names the same file, and a
                // person reading the pane's argv sees it is a continuation.
                resume = Some(value("--resume-transcript")?);
                index += 2;
            }
            "--render-markdown" => {
                render.render_markdown = true;
                index += 1;
            }
            "--show-thinking" => {
                render.show_thinking = true;
                index += 1;
            }
            "--verbose-stderr" => {
                render.verbose_stderr = true;
                index += 1;
            }
            "--plain" | "-p" => {
                render.plain = true;
                index += 1;
            }
            "--json" => {
                // JSON 은 프로그램이 읽는다. 대화형 프런트가 켜지면 그 위에
                // 화면 제어 바이트가 섞이므로 여기서 함께 눕힌다 — 사용자가
                // 두 플래그의 관계를 외우게 하지 않는다.
                render.json = true;
                render.plain = true;
                index += 1;
            }
            "--last-message" => {
                let Some(path) = args.get(index + 1) else {
                    return Err(format!("--last-message needs a file path\n\n{USAGE}"));
                };
                last_message = Some(PathBuf::from(path));
                index += 2;
            }
            "--loop-every" => {
                if loop_trigger.is_some() {
                    return Err("choose one of --loop-every and --loop-until".to_string());
                }
                let raw = value("--loop-every")?;
                let duration = parse_loop_duration(&raw)?;
                loop_trigger = Some(HeadlessLoopTrigger::Every { raw, duration });
                index += 2;
            }
            "--loop-until" => {
                if loop_trigger.is_some() {
                    return Err("choose one of --loop-every and --loop-until".to_string());
                }
                let command = value("--loop-until")?;
                if command.trim().is_empty() {
                    return Err("--loop-until requires a non-empty command".to_string());
                }
                loop_trigger = Some(HeadlessLoopTrigger::Until { command });
                index += 2;
            }
            "--loop-max" => {
                let raw = value("--loop-max")?;
                loop_max = Some(
                    raw.parse::<u32>()
                        .ok()
                        .filter(|runs| *runs > 0)
                        .ok_or_else(|| "--loop-max requires a positive integer".to_string())?,
                );
                index += 2;
            }
            "--launch-contract" => {
                launch_contract = Some(value("--launch-contract")?);
                index += 2;
            }
            "--status" => {
                action = Action::Status;
                index += 1;
            }
            "--prompt-input" => {
                action = Action::PromptInput;
                index += 1;
            }
            "--version" | "-V" => {
                action = Action::Version;
                index += 1;
            }
            "--orchestration-accuracy" => {
                action = Action::OrchestrationAccuracy;
                index += 1;
            }
            "--help" | "-h" => {
                action = Action::Help;
                index += 1;
            }
            other => return Err(format!("unknown argument '{other}'\n\n{USAGE}")),
        }
    }

    let cwd = match cwd {
        Some(cwd) => cwd,
        None => crate::current_cli_cwd().map_err(|error| error.to_string())?,
    };
    let permission_mode_explicit = permission_mode.is_some();
    if loop_max.is_some() && loop_trigger.is_none() {
        return Err("--loop-max requires --loop-every or --loop-until".to_string());
    }
    if loop_trigger.is_some() && action != Action::Repl {
        return Err("headless loop flags only apply to a prompt run".to_string());
    }
    if loop_trigger.is_some() && teammate.is_some() {
        return Err("headless loop flags cannot be combined with --teammate".to_string());
    }
    let headless_loop = loop_trigger.map(|trigger| HeadlessLoop {
        trigger,
        max_runs: loop_max,
    });
    Ok(Launch {
        requested,
        action,
        open: OpenOptions {
            person_model_pin: None,
            cwd,
            model,
            permission_mode: permission_mode.unwrap_or(PermissionMode::WorkspaceWrite),
            allowed_tools,
            disable_spawn_family: no_spawn,
            resume,
            mcp_config,
            effort,
            // The parser sees only the flag. Whether a terminal is actually
            // attached is main's to decide, and it overwrites this before the
            // session opens.
            headless: render.plain,
            // Only an accepted launch contract turns this on (main.rs).
            exact_selection: false,
            teammate_harness: None,
            parent_registry: None,
            remote_mcp: None,
        },
        render,
        permission_mode_explicit,
        events_bind,
        last_message,
        teammate,
        headless_loop,
        launch_contract,
    })
}

/// `--allowed-tools`: the builtin names as the registry spells them (its own
/// normalizer, so `computer` is `Computer` and a typo is refused here rather
/// than leaving a session with no tools), and MCP names as written — those
/// servers are not running yet to ask.
fn allowed_tool_names(raw: &str) -> Result<Option<crate::AllowedToolSet>, String> {
    let (mcp, builtin): (Vec<&str>, Vec<&str>) = raw
        .split(|ch: char| ch == ',' || ch.is_whitespace())
        .filter(|name| !name.is_empty())
        .partition(|name| name.starts_with("mcp__"));
    if mcp.is_empty() && builtin.is_empty() {
        return Err(format!("--allowed-tools needs at least one tool name\n\n{USAGE}"));
    }
    let mut allowed = tools::GlobalToolRegistry::builtin()
        .normalize_allowed_tools(&[builtin.join(",")])
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    allowed.extend(mcp.into_iter().map(str::to_string));
    Ok(Some(allowed))
}

fn parse_loop_duration(raw: &str) -> Result<Duration, String> {
    let input = format!("/loop every {raw} headless-loop-prompt");
    match commands::SlashCommand::parse(&input).map_err(|error| error.to_string())? {
        Some(commands::SlashCommand::Loop {
            command: commands::LoopCommand::StartInterval { every, .. },
        }) => Ok(every.duration),
        _ => Err(format!("invalid --loop-every duration '{raw}'")),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn bare_resume_lists_sessions_and_a_value_restores_one() {
        let bare = super::parse(&["--resume".to_string()]).expect("parse");
        assert_eq!(bare.action, super::Action::Sessions);
        assert!(bare.open.resume.is_none());
        let with_flag = super::parse(&["--resume".to_string(), "--plain".to_string()]).expect("parse");
        assert_eq!(with_flag.action, super::Action::Sessions);
        let restoring = super::parse(&["--resume".to_string(), "abc".to_string()]).expect("parse");
        assert_eq!(restoring.action, super::Action::Repl);
        assert_eq!(restoring.open.resume.as_deref(), Some("abc"));
    }

    /// A teammate pane is one turn under a brief, and it cannot start
    /// teammates of its own.
    #[test]
    fn a_teammate_names_its_brief_and_can_never_start_a_grandchild() {
        let launch = super::parse(&[
            "--teammate".to_string(),
            "/tmp/zo-agents/agent-1".to_string(),
        ])
        .expect("parse");
        assert_eq!(
            launch.teammate.as_deref(),
            Some(std::path::Path::new("/tmp/zo-agents/agent-1"))
        );
        assert!(
            launch.open.disable_spawn_family,
            "a teammate that can spawn cuts a pane out of a pane"
        );
        assert_eq!(launch.action, super::Action::Repl);
        // An ordinary run is untouched.
        let ordinary = super::parse(&[]).expect("parse");
        assert!(ordinary.teammate.is_none());
        assert!(!ordinary.open.disable_spawn_family);
        // And the flag needs its directory.
        assert!(super::parse(&["--teammate".to_string()]).is_err());
    }

    use super::{parse, Action};
    use crate::effort::Effort;
    use runtime::PermissionMode;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    /// 기본은 제 도구를 다 들고 시작한다. 하위 에이전트는 읽은 것을 제 문맥에
    /// 두고 결론만 돌려주므로, 긴 세션에서 **문맥을 아끼는** 쪽이 기본이라야 한다.
    /// `--json` is for a program, so it cannot leave the interactive front-end
    /// armed — the screen control bytes would sit on top of the stream. The
    /// parser lays `--plain` down with it so nobody has to know the pairing.
    #[test]
    fn json_lays_the_interactive_front_end_down_with_it() {
        let launch = super::parse(&["--json".to_string()]).expect("parse");
        assert!(launch.render.json, "the flag is read");
        assert!(
            launch.render.plain,
            "--json must imply --plain, or a tty run paints over the contract"
        );
        assert!(launch.open.headless, "and the session opens headless");
    }

    /// `--last-message` without a path is a mistake worth naming, not a silent
    /// no-op: the caller is going to read that file.
    #[test]
    fn last_message_needs_its_path() {
        let Err(error) = super::parse(&["--last-message".to_string()]) else {
            panic!("a path-less --last-message must fail");
        };
        assert!(error.contains("--last-message needs a file path"), "{error}");

        let launch = super::parse(&["--last-message".to_string(), "/tmp/answer.txt".to_string()])
            .expect("parse");
        assert_eq!(
            launch.last_message.as_deref(),
            Some(std::path::Path::new("/tmp/answer.txt"))
        );
    }

    /// The two are independent — a prose run may still want the answer in a
    /// file, and a JSON run may not need one.
    #[test]
    fn last_message_and_json_are_independent() {
        let json_only = super::parse(&["--json".to_string()]).expect("parse");
        assert!(json_only.last_message.is_none());
        let file_only =
            super::parse(&["--last-message".to_string(), "/tmp/a".to_string()]).expect("parse");
        assert!(!file_only.render.json);
    }

    #[test]
    fn headless_loop_flags_share_the_slash_duration_grammar() {
        let launch = parse(&args(&[
            "-p",
            "--loop-every",
            "1m30s",
            "--loop-max",
            "2",
        ]))
        .expect("parse");
        assert!(launch.render.plain);
        let loop_spec = launch.headless_loop.expect("headless loop");
        assert_eq!(loop_spec.max_runs, Some(2));
        assert!(matches!(
            loop_spec.trigger,
            super::HeadlessLoopTrigger::Every { duration, .. }
                if duration == std::time::Duration::from_secs(90)
        ));

        assert!(parse(&args(&["--loop-max", "2"])).is_err());
        assert!(parse(&args(&[
            "--loop-every",
            "1s",
            "--loop-until",
            "true",
        ]))
        .is_err());
    }

    #[test]
    fn defaults_are_a_plain_repl_that_can_spawn() {
        let launch = parse(&args(&[])).expect("parse");
        assert_eq!(launch.action, Action::Repl);
        assert!(!launch.open.disable_spawn_family);
        assert!(!launch.permission_mode_explicit);
        assert!(!launch.render.render_markdown);
        assert!(!launch.render.verbose_stderr);
        assert!(launch.open.resume.is_none());
    }

    #[test]
    fn flags_land_in_open_options() {
        let launch = parse(&args(&[
            "--permission-mode",
            "bypassPermissions",
            "--effort",
            "max",
            "--continue",
            "--no-spawn",
            "--render-markdown",
            "--verbose-stderr",
            "--status",
        ]))
        .expect("parse");
        assert_eq!(launch.action, Action::Status);
        assert_eq!(launch.open.permission_mode, PermissionMode::DangerFullAccess);
        assert!(launch.permission_mode_explicit);
        assert_eq!(launch.open.effort, Some(Effort::Max));
        assert_eq!(launch.open.resume.as_deref(), Some("latest"));
        assert!(launch.open.disable_spawn_family);
        assert!(launch.render.render_markdown);
        assert!(launch.render.verbose_stderr);
    }

    /// A bench that hands zo one tool needs zo to hold to it: the list is the
    /// registry's spelling, a typo is refused at launch, MCP names pass as
    /// written, and no flag means every tool as before.
    #[test]
    fn allowed_tools_are_the_registrys_names_or_refused() {
        let launch = parse(&args(&["--allowed-tools", "computer, Computer"])).expect("parse");
        assert_eq!(
            launch.open.allowed_tools,
            Some(["Computer".to_string()].into_iter().collect())
        );
        let launch = parse(&args(&["--allowed-tools", "Computer,mcp__ctx7__query"])).expect("parse");
        assert_eq!(
            launch.open.allowed_tools,
            Some(
                ["Computer".to_string(), "mcp__ctx7__query".to_string()]
                    .into_iter()
                    .collect()
            )
        );
        let Err(typo) = parse(&args(&["--allowed-tools", "Computr"])) else {
            panic!("a name the registry does not know is refused");
        };
        assert!(typo.contains("Computr"), "{typo}");
        assert!(parse(&args(&["--allowed-tools", " , "])).is_err());
        assert!(parse(&args(&["--allowed-tools"])).is_err());
        assert_eq!(parse(&args(&[])).expect("parse").open.allowed_tools, None);
    }

    /// Every long flag the parser accepts must appear in `USAGE`.
    ///
    /// `--doctor` was missing, then `--continue` turned out to be missing the
    /// same way — a flag can be added to the match arm and simply forgotten in
    /// the help text, and nothing said so. The parser is the authority here, so
    /// read the arms out of the source and hold the text to them.
    #[test]
    fn every_parsed_flag_is_documented_in_usage() {
        // The match arms are the parser's own list; a new flag joins it by
        // construction, which is exactly why it is the authority.
        let source = include_str!("args.rs");
        let mut undocumented = Vec::new();
        for line in source.lines() {
            let trimmed = line.trim();
            let Some(arms) = trimmed.strip_suffix(" => {") else {
                continue;
            };
            for arm in arms.split('|') {
                let flag = arm.trim().trim_matches('"');
                if !flag.starts_with("--") {
                    continue;
                }
                if !super::USAGE.contains(flag) {
                    undocumented.push(flag.to_string());
                }
            }
        }
        assert!(
            undocumented.is_empty(),
            "parsed but absent from USAGE: {undocumented:?}"
        );
    }

    #[test]
    fn unknown_flag_and_missing_value_are_errors() {
        assert!(parse(&args(&["--bogus"])).is_err());
        assert!(parse(&args(&["--model"])).is_err());
        assert!(parse(&args(&["--effort", "turbo"])).is_err());
        assert!(parse(&args(&["--events-bind"])).is_err());
    }

    #[test]
    fn the_parser_records_only_an_explicit_events_bind_override() {
        assert!(parse(&args(&[])).expect("parse").events_bind.is_none());
        let asked = parse(&args(&["--events-bind", "127.0.0.1:0"])).expect("parse");
        assert_eq!(asked.events_bind.as_deref(), Some("127.0.0.1:0"));
        // 주소의 유효성은 파서가 아니라 채널이 판정한다
        // (`ide::channel::auth::resolve_loopback_bind`) — 루프백 강제까지
        // 파서에 두면 같은 규칙이 두 곳에 살게 된다.
        assert_eq!(
            parse(&args(&["--events-bind", "0.0.0.0:1"]))
                .expect("parse")
                .events_bind
                .as_deref(),
            Some("0.0.0.0:1")
        );
    }
}
