//! `zo-ide` — zerocode IDE 전용 plain CLI.
//!
//! forge-code(zo)의 코어(runtime·tools·api·core-types·commands) 위에 서는
//! 신작 프런트엔드다. ratatui App·HUD·모달·attach/serve 클라이언트는 링크
//! 경로에 존재하지 않는다.
//!
//! 순서가 중요하다: 세션(런타임·MCP·LSP·플러그인)은 tokio **밖**에서 열고,
//! 그다음 멀티스레드 런타임을 만들어 REPL 을 `block_on` 한다 — 런타임 빌드는
//! ambient tokio 런타임 안에서 패닉하고, 질문 채널의 `blocking_recv` 는
//! 멀티스레드 런타임을 요구한다(zo-cli `run_repl` 과 같은 배치).

use std::io::IsTerminal;
use std::process::ExitCode;

use zo_ide::ide::args::{self, Action};
use zo_ide::ide::channel::capabilities::{Contract, Requested};
use zo_ide::ide::events::{self, EventsChannel, EventsConfig};
use zo_ide::ide::run_loop;
use zo_ide::launch_contract;
use zo_ide::permission_mode::interactive_default_permission_mode;
use zo_ide::session::plain_session::PlainSession;
use zo_ide::tui;

fn main() -> ExitCode {
    // First, before any thread: the router keys the window handed this zo for
    // its own requests leave the environment, so no MCP server, tool shell or
    // hook it spawns inherits them (api reads them from its own table).
    api::adopt_launch_keys(zo_ide::ide::ROUTER_KEY_ENV_PREFIX);
    // A zo the window did not launch (typed into a shell pane) was handed no
    // router keys; one it needs is read from the window's keychain item into
    // the same table, when first needed — nothing is read here.
    api::find_router_keys_in_keychain(
        zo_ide::ide::ROUTER_KEY_ENV_PREFIX,
        zo_ide::ide::ROUTER_KEYCHAIN_SERVICE_PREFIX,
    );
    // And the service keys the window's settings keep under their own names
    // (TypeSafe's), which no launch hands over at all — same road, same table.
    api::find_service_keys_in_keychain(
        &[api::SYSTEMONE_API_KEY_ENV],
        zo_ide::ide::SERVICE_KEYCHAIN_SERVICE_PREFIX,
    );
    // This process owns its workspaces: zo's own bookkeeping (`.zo/turns`,
    // `.zo/dream`) lives beside the transcript under `~/.zo/projects/<slug>`,
    // never in the person's checkout. The crates keep the in-tree default for
    // their own tests; the host is the one that asks.
    runtime::relocate_traces_out_of_tree();
    let mut process_exit = zo_ide::session::process_lifecycle::install();
    match run(&mut process_exit) {
        Ok((reason, code)) => {
            process_exit.finish(reason, None);
            ExitCode::from(code)
        }
        Err(error) => {
            process_exit.finish("error", Some(&error.to_string()));
            ExitCode::FAILURE
        }
    }
}

/// What a verb answers with: the name the exit is recorded under and its code,
/// or why it could not.
type Answered = Result<(&'static str, u8), Box<dyn std::error::Error>>;

/// The verbs that answer without opening a session, and what each takes.
///
/// A table rather than a ladder of `if`s: the ladder reached five and pushed
/// `run` one line past the length the lint keeps, which is the lint doing its
/// job — five spellings of "is the first word this" is a shape, and a shape
/// belongs in one place.
///
/// Diagnosis stays independent of session startup: `--doctor` must answer
/// without credentials, a trusted workspace or a provider runtime, and the
/// others reach programs of their own for the same reason.
fn answered_before_a_session(argv: &[String]) -> Option<Answered> {
    let verb = argv.first()?.as_str();
    let rest = &argv[1..];
    match verb {
        "--doctor" => Some(std::env::current_dir().map_err(Into::into).map(|cwd| {
            println!("{}", zo_ide::doctor::run(&cwd));
            success("doctor")
        })),
        "cron" => Some(run_cron(rest)),
        "scoreboard" => Some(run_scoreboard(rest)),
        "decision-shadow" => Some(run_decision_shadow(rest)),
        "jev" => Some(run_jev(rest)),
        "mcp" => Some(run_mcp(rest)),
        "vault" => Some(run_vault(rest)),
        _ => None,
    }
}

fn run(
    process_exit: &mut zo_ide::session::process_lifecycle::ProcessExitGuard,
) -> Result<(&'static str, u8), Box<dyn std::error::Error>> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if let Some(answered) = answered_before_a_session(&argv) {
        return answered;
    }
    let mut launch = args::parse(&argv)?;
    if let Some(done) = run_without_a_session(&launch)? {
        return Ok(done);
    }

    std::env::set_current_dir(&launch.open.cwd)?;
    // The exact launch guard, decided here and nowhere later: before the
    // stderr redirect (so a refusal reaches the pane), before the trust gate,
    // before the session opens — before any task input can run. A teammate
    // is guarded in `run_teammate`, after its brief has said what it opens.
    let guarded = match guard_root_launch(&mut launch) {
        Ok(guarded) => guarded,
        Err(failure) => return refuse_launch(&launch.requested, launch.render, launch.events_bind.as_deref(), failure),
    };
    if launch.action == Action::Repl {
        // 세션 열기(자격증명 해석·정리 스레드)가 stderr 에 말하기 **전에**
        // 로그로 돌린다 — run_loop 안에서 옮기면 배너보다 앞선 줄들이 이미
        // 패인에 닿아 있었다(워커 보고의 측정된 잔여 결함). --status 는
        // 진단 표면이므로 stderr 를 그대로 둔다.
        run_loop::install_stderr_redirect(launch.render);
    }
    if !launch.permission_mode_explicit && launch.teammate.is_none() {
        // 신뢰 게이트: 첫 방문 폴더는 플레인 3지선다, 저장된 답은 재사용.
        //
        // A teammate is the one run that must never be asked: its parent
        // already decided the mode and put it in the brief, and the question
        // would sit on a pane the parent is blocked on and nobody is typing
        // at. (Measured: without this, a teammate pane opened on the trust
        // menu and answered nothing forever.)
        launch.open.permission_mode = interactive_default_permission_mode(false);
    }

    // Is a person actually at the keyboard? The parser only saw `--plain`; a
    // pipe or a redirect is just as headless, and the session's system prompt
    // is chosen from this before it opens (and then frozen for the session's
    // life, because the prompt is replayed verbatim on resume).
    //
    // The same three facts decide the front-end below; they are read once here
    // so the prompt and the front-end can never disagree about which one this
    // run is.
    let interactive = !launch.render.plain
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && tui::terminal_is_usable();
    launch.open.headless = !interactive;
    // And whether a sub-agent of this run may become a pane. A `--plain` or
    // piped run's bytes ARE its answer, so a child whose transcript lands in
    // another pane would be output its caller never receives; the decision
    // itself is `runtime::subagent_panes::mode`, which also needs the team
    // coordinates in the environment. Declared here because the spawn that
    // asks runs on a worker thread with no front-end to ask.
    runtime::subagent_panes::declare_on_screen(interactive);

    // A pane opened by a parent zo: one turn from its brief, then the answer
    // written back beside it. Everything above still applies — the session is
    // opened the same way, the front-end is the same one a person drives —
    // and the two differences are that the first turn arrives from a file and
    // that the process leaves when it ends.
    if let Some(directory) = launch.teammate.clone() {
        return run_teammate(&tokio_runtime()?, launch, &directory, process_exit);
    }

    let session = PlainSession::open(launch.open)?;
    if let Err(failure) = confirm_guarded(guarded.as_ref(), &launch.requested, &session) {
        // Opened, never spoken to: the session is dropped with the refusal.
        drop(session);
        return refuse_launch(&launch.requested, launch.render, launch.events_bind.as_deref(), failure);
    }
    if launch.action == Action::PromptInput {
        // Printed from the opened session, not rebuilt from defaults: MCP
        // activation and a resumed session's reminders are part of what the
        // model is actually shown.
        //
        // The provider's exact count rides along when it can be had. A tiny
        // current-thread runtime rather than the multi-threaded one built
        // below: this is one HTTP round-trip and then the process exits.
        let exact = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()
            .and_then(|rt| rt.block_on(session.provider_input_tokens()));
        print!(
            "{}",
            session.prompt_input().with_provider_tokens(exact).render()
        );
        return Ok(success("prompt_input"));
    }
    if launch.action == Action::Status {
        let status = session.status();
        println!(
            "{} · {} · effort {} · session {} · {}{}",
            status.model,
            status.permission_mode,
            status.effort.unwrap_or("-"),
            status.session_id,
            zo_ide::status_format::format_context_usage(
                status.context_tokens as u64,
                api::context_window_for_model(&status.model),
            ),
            if session.resumed() { " · resumed" } else { "" }
        );
        return Ok(success("status"));
    }

    let tokio_rt = tokio_runtime()?;
    // 이벤트 채널은 세션이 열린 **뒤**, 프런트엔드가 돌기 **전**에 선다.
    // 뒤인 이유는 세션 id·모델·권한이 있어야 `session.info` 가 진실을 말하기
    // 때문이고, 전인 이유는 창이 패인을 띄우자마자 붙기 때문이다. 소켓 바인딩은
    // tokio 안에서만 되므로 런타임을 만든 다음 자리가 여기 하나뿐이다.
    if let Some(path) = open_events_channel(
        &tokio_rt,
        launch.events_bind.as_deref(),
        interactive,
        &session,
        &launch.requested,
        None,
        guarded.as_ref().map(|guarded| &guarded.contract),
    )? {
        // This session may become a PARENT (t-2513): its children carry this
        // file to watch it live, route their MCP calls back here, and hear
        // its `auth.reload`. Declared for the spawn's worker thread, which
        // has no front-end to ask.
        runtime::subagent_panes::declare_parent_channel(Some(path.clone()));
        process_exit.track_events_addr_file(path);
    }
    if let Some(channel) = events::channel() {
        channel.set_child_fanout(Some(session.child_auth_reload_fanout()));
        channel.set_mcp_bridge(session.mcp_channel_bridge());
    }
    // 모드 이분법: 사람의 터미널이면 codex 문법의 대화형 프런트, 그 밖(파이프·
    // 리다이렉트·`--plain`)이면 기존 append-only 경로 그대로다. 파이프 골든은
    // 이 분기 뒤에서도 바이트 하나 안 바뀐다.
    let (reason, exit_code) = run_frontend(
        &tokio_rt,
        session,
        launch.render,
        launch.last_message,
        interactive,
        launch.headless_loop,
    )?;
    Ok((reason_label(reason), exit_code))
}

/// `zo scoreboard …` reads the evidence ledgers against a baseline and, asked
/// to, files each regression as a ledger task — the scoreboard beat's hands,
/// fired by a cron (docs/design/scoreboard-beat-20260911.md). No session.
fn run_scoreboard(args: &[String]) -> Result<(&'static str, u8), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    Ok(match zo_ide::scoreboard_cli::run(args, &cwd, now) {
        Ok(report) => {
            println!("{report}");
            success("scoreboard")
        }
        Err(message) => {
            eprintln!("zo scoreboard: {message}");
            ("scoreboard-refused", 1)
        }
    })
}

/// `zo jev summary` counts every seat's ledger; `zo jev ask|choose|score`
/// puts an agent's own question to the seat and says the answer with its
/// exit code as well as its text (ask: 0 yes, 1 no, 2 nothing answered;
/// choose/score: 0 answered, 2 not). No session. A refused command line
/// prints why and exits with the verb's own refusal code.
fn run_jev(args: &[String]) -> Result<(&'static str, u8), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    let now_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX);
    Ok(match zo_ide::jev_cli::run(args, &cwd, now_ms, zo_ide::local_offset_seconds()) {
        Ok(report) => {
            println!("{}", report.text);
            if report.exit == zo_ide::autonomy::limits::HEADLESS_LOOP_EXIT_DONE {
                success("jev")
            } else {
                ("jev-answered-no", report.exit)
            }
        }
        Err(refused) => {
            eprintln!("zo jev: {}", refused.message);
            ("jev-refused", refused.exit)
        }
    })
}

fn run_decision_shadow(args: &[String]) -> Result<(&'static str, u8), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    Ok(match zo_ide::decision_shadow_cli::run(args, &cwd) {
        Ok(report) => {
            println!("{}", report.text);
            if report.answered {
                success("decision-shadow")
            } else {
                ("decision-shadow-unanswered", 1)
            }
        }
        Err(message) => {
            eprintln!("zo decision-shadow: {message}");
            ("decision-shadow-refused", 1)
        }
    })
}

/// `zo mcp …` reads and writes the configured MCP servers from outside a
/// session: the same settings documents `ConfigLoader` merges, written back
/// through the loader's own parser. `--doctor`'s principle exactly — no
/// credentials, no workspace trust, no provider runtime.
fn run_mcp(args: &[String]) -> Result<(&'static str, u8), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    Ok(match zo_ide::mcp_cli::run(args, &cwd) {
        Ok(report) => {
            println!("{}", report.text);
            success("mcp")
        }
        Err(message) => {
            eprintln!("zo mcp: {message}");
            ("mcp-refused", 1)
        }
    })
}

/// `zo vault path …` answers how two pages of the second brain connect, with
/// the window's own calculator over the same scanner — no session, no
/// credentials, no workspace trust (t-5966 G3).
fn run_vault(args: &[String]) -> Result<(&'static str, u8), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    Ok(match zo_ide::vault_cli::run(args, &cwd) {
        Ok(report) => {
            println!("{}", report.text);
            success("vault")
        }
        Err(message) => {
            eprintln!("zo vault: {message}");
            ("vault-refused", 1)
        }
    })
}

/// `zo cron …` writes a workspace's cron registry without opening a session
/// there — the same writer the tools use, from outside. A refusal is one
/// sentence on stderr and a non-zero exit, never a session.
fn run_cron(args: &[String]) -> Result<(&'static str, u8), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    Ok(match zo_ide::cron_cli::run(args, &cwd) {
        Ok(receipt) => {
            println!("{receipt}");
            success("cron")
        }
        Err(message) => {
            eprintln!("zo cron: {message}");
            ("cron-refused", 1)
        }
    })
}

/// The actions that end before a session exists — help, version, the recent
/// session list, the model catalog. `None` means the launch goes on.
fn run_without_a_session(
    launch: &args::Launch,
) -> Result<Option<(&'static str, u8)>, Box<dyn std::error::Error>> {
    Ok(Some(match launch.action {
        Action::Help => {
            println!("{}", args::USAGE);
            success("help")
        }
        Action::Version => {
            println!("zo {}", env!("CARGO_PKG_VERSION"));
            success("version")
        }
        Action::OrchestrationAccuracy => {
            // One JSON line for the cwd project, built from the same outcome
            // store the router learns from. The window's board card execs this
            // instead of linking zo or re-deriving the state slug.
            let report = runtime::read_orchestration_accuracy(
                &launch.open.cwd,
                runtime::ACCURACY_MIN_DECISIVE,
            )?;
            println!("{}", serde_json::to_string(&report)?);
            success("orchestration-accuracy")
        }
        Action::Sessions => {
            // The registry is keyed by workspace, so list from the requested cwd.
            std::env::set_current_dir(&launch.open.cwd)?;
            println!("{}", zo_ide::resume::render_recent_sessions(20)?);
            success("sessions")
        }
        Action::Models { refresh } => {
            println!("{}", zo_ide::models_cli::render(refresh, launch.render.json)?);
            success("models")
        }
        Action::Commands => {
            println!("{}", zo_ide::slash::render_catalog(launch.render.json));
            success("commands")
        }
        Action::Status | Action::PromptInput | Action::Repl => return Ok(None),
    }))
}

const fn success(reason: &'static str) -> (&'static str, u8) {
    (
        reason,
        zo_ide::autonomy::limits::HEADLESS_LOOP_EXIT_DONE,
    )
}

/// The root launch's guard. A teammate is guarded in `run_teammate` once its
/// brief has settled the selection; here the command line is the selection.
/// An accepted contract pins it for the session (`exact_selection`).
fn guard_root_launch(
    launch: &mut args::Launch,
) -> Result<Option<launch_contract::Guarded>, launch_contract::Failure> {
    if launch.teammate.is_some() {
        return Ok(None);
    }
    let guarded = guard_launch(
        launch,
        launch.requested.model.as_deref(),
        launch.requested.effort.as_deref(),
    )?;
    launch.open.exact_selection = guarded.is_some();
    launch.open.person_model_pin = guarded.as_ref()
        .filter(|_| launch.requested.model.is_some())
        .map(|guarded| guarded.accepted.model.clone());
    Ok(guarded)
}

/// After the session opened under a contract: the selection it carries must
/// be the accepted one, or the launch ends with `effective-mismatch` and the
/// verdict says which side moved.
fn confirm_guarded(
    guarded: Option<&launch_contract::Guarded>,
    requested: &Requested,
    session: &PlainSession,
) -> Result<(), launch_contract::Failure> {
    let Some(guarded) = guarded else {
        return Ok(());
    };
    let status = session.status();
    launch_contract::confirm_effective(&guarded.accepted, &status.model, status.effort).map_err(|refusal| {
        let contract = launch_contract::contract(&guarded.request, requested, &Err(refusal.clone()));
        launch_contract::Failure::Refused(Box::new(launch_contract::Refused {
            request: guarded.request.clone(),
            contract,
            refusal,
        }))
    })
}

/// Run the exact launch guard over the selection the command line (or a
/// teammate's brief) resolved. `Ok(None)` is the legacy policy.
fn guard_launch(
    launch: &args::Launch,
    requested_model: Option<&str>,
    requested_effort: Option<&str>,
) -> Result<Option<launch_contract::Guarded>, launch_contract::Failure> {
    let selection = launch_contract::Selection {
        requested_model,
        model: &launch.open.model,
        requested_effort,
        effort: launch.open.effort,
    };
    launch_contract::guard(
        launch.launch_contract.as_deref(),
        &launch.requested,
        &selection,
        &launch_contract::PublishedCatalog,
    )
}

/// End a guarded launch that did not proceed. A usage error is an ordinary
/// error. A refusal says its reason once on stderr (and, for `--json`, as one
/// typed line on stdout), publishes the verdict on the events channel when a
/// host wired one — holding the socket until the host has read it or the
/// deadline passes — and exits with the contract's own code. Nothing was run.
fn refuse_launch(
    requested: &Requested,
    render: args::RenderFlags,
    events_bind: Option<&str>,
    failure: launch_contract::Failure,
) -> Result<(&'static str, u8), Box<dyn std::error::Error>> {
    let launch_contract::Refused { request, contract, refusal } = match failure {
        launch_contract::Failure::Usage(why) => return Err(why.into()),
        launch_contract::Failure::Refused(refused) => *refused,
    };
    if render.json {
        let receipt = serde_json::to_value(&contract).unwrap_or_default();
        println!("{}", zo_ide::ide::render::launch_refused_json(refusal.reason, &refusal.detail, &receipt));
    }
    eprintln!("{}", refusal.line());
    let hosted = events_bind.is_some()
        || std::env::var(events::BIND_ENV).is_ok_and(|bind| !bind.trim().is_empty());
    if hosted {
        publish_refusal(requested, &request, &contract, events_bind)?;
    }
    Ok(("launch_refused", launch_contract::EXIT_CODE))
}

/// The refused launch's channel: the same wiring a hosted launch opens, with
/// the request id (or the pid) standing in for the session that never
/// existed, and no discovery file — nothing here is a session to adopt.
fn publish_refusal(
    requested: &Requested,
    request: &launch_contract::Request,
    contract: &Contract,
    events_bind: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let session_id = request
        .request_id
        .as_deref()
        .map_or_else(|| format!("launch-refused-{}", std::process::id()), |id| format!("launch-refused-{id}"));
    let Some(config) = EventsConfig::from_launch(events_bind, &session_id, false) else {
        return Ok(());
    };
    let tokio_rt = tokio_runtime()?;
    let channel = tokio_rt.block_on(EventsChannel::open(&config))?;
    channel.install_refused_capabilities(requested, contract);
    tokio_rt.block_on(channel.wait_for_capabilities_read(launch_contract::CHANNEL_HOLD));
    drop(channel);
    Ok(())
}

const fn reason_label(reason: run_loop::ExitReason) -> &'static str {
    match reason {
        run_loop::ExitReason::UserExit => "exit",
        run_loop::ExitReason::OutputClosed => "output_closed",
        run_loop::ExitReason::LoopCompleted => "loop_completed",
        run_loop::ExitReason::AutonomousLimit => "autonomy_limit",
        run_loop::ExitReason::Interrupted => "interrupted",
    }
}

fn run_frontend(
    tokio_rt: &tokio::runtime::Runtime,
    session: PlainSession,
    render: zo_ide::ide::args::RenderFlags,
    last_message: Option<std::path::PathBuf>,
    interactive: bool,
    headless_loop: Option<zo_ide::ide::args::HeadlessLoop>,
) -> Result<(run_loop::ExitReason, u8), Box<dyn std::error::Error>> {
    if interactive {
        let reason = tokio_rt.block_on(tui::run_with_last_message(
            session,
            render,
            last_message,
        ))?;
        return Ok((
            reason,
            zo_ide::autonomy::limits::HEADLESS_LOOP_EXIT_DONE,
        ));
    }
    if let Some(headless_loop) = headless_loop {
        let outcome = tokio_rt.block_on(run_loop::run_headless_loop(
            session,
            render,
            last_message,
            headless_loop,
        ))?;
        return Ok((outcome.reason, outcome.exit_code));
    }
    let reason = tokio_rt.block_on(run_loop::run_with_last_message(
        session,
        render,
        last_message,
    ))?;
    Ok((
        reason,
        zo_ide::autonomy::limits::HEADLESS_LOOP_EXIT_DONE,
    ))
}

/// IDE 이벤트 채널을 열고 프로세스 전역에 놓는다.
///
/// 대화형 TUI 는 명시 배선이 없어도 포트 0에 붙고 앱 발견 파일을 낸다.
/// headless 표면은 명시 주소가 있을 때만 기존 채널을 연다. 열기로 했는데 열지
/// 못하면 **실패로 끝낸다**: 창은 이 채널로만 권한 프롬프트를 볼 수 있어서,
/// 조용히 없는 채로 뜬 패인은 물음에 답할 수 없으면서 멀쩡해 보인다.
fn open_events_channel(
    tokio_rt: &tokio::runtime::Runtime,
    bind: Option<&str>,
    interactive: bool,
    session: &PlainSession,
    requested: &Requested,
    brief: Option<&runtime::subagent_panes::Brief>,
    contract: Option<&Contract>,
) -> Result<Option<std::path::PathBuf>, Box<dyn std::error::Error>> {
    let Some(config) = EventsConfig::from_launch(bind, &session.handle.id, interactive) else {
        return Ok(None);
    };
    let channel = tokio_rt.block_on(EventsChannel::open(&config))?;
    channel.install_capabilities(requested, session, brief, contract);
    // 창이 붙자마자 묻는 것들(`session.list`/`info`/`subscribe`)이 첫 요청부터
    // 진실을 말하도록, 상태와 히스토리를 세우는 자리에서 한 번 밀어 넣는다.
    channel.publish_status(&session.status(), &session.cwd);
    channel.set_history(&session.replay_items());
    let discovery_file = config.discovery_file.clone();
    events::install(channel);
    Ok(discovery_file)
}

/// The runtime both front-ends run on.
///
/// Built after the session is open, never before: a runtime build inside an
/// ambient tokio runtime panics, and the question channel's `blocking_recv`
/// needs the multi-threaded flavour.
fn tokio_runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .max_blocking_threads(512)
        .enable_all()
        .build()
}

/// One turn as somebody else's teammate.
///
/// The brief is read BEFORE the session opens, because half of what it says —
/// the model, the effort, the permission mode, the working directory — is how
/// the session has to be opened. A brief that cannot be read ends the process
/// with the reason on stderr and, where the directory allows it, a
/// `result.json` saying so: a parent waiting for a file it will never get is
/// the one failure this whole road exists to avoid.
#[allow(clippy::too_many_lines)] // one ordered launch: brief → harness → guard → session → channel → life
fn run_teammate(
    tokio_rt: &tokio::runtime::Runtime,
    mut launch: zo_ide::ide::args::Launch,
    directory: &std::path::Path,
    process_exit: &mut zo_ide::session::process_lifecycle::ProcessExitGuard,
) -> Result<(&'static str, u8), Box<dyn std::error::Error>> {
    use runtime::subagent_panes::{Brief, Exit, Limits, TeammateResult};

    // This process is a child: no panes for its helpers, no host pre-analysis
    // of the brief it was handed (`runtime::subagent_panes::nested`).
    runtime::subagent_panes::declare_nested(true);
    let brief = match Brief::read(directory) {
        Ok(brief) => brief,
        Err(why) => {
            // Nothing is known about the parent except where it is listening,
            // so the id is the directory's own name — which is how the parent
            // addresses this child anyway.
            let mut refused = TeammateResult::new(
                directory
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
                    .as_str(),
                Exit::Error,
            );
            refused.error = Some(why.clone());
            let _ = refused.write(directory);
            return Err(why.into());
        }
    };

    // What the parent decided about this child, in the fields that decide how
    // a session opens. Each one is only applied when the brief carries it, so
    // a parent that said nothing leaves this run's own defaults standing.
    if let Some(model) = brief.model.as_deref().filter(|one| !one.trim().is_empty()) {
        launch.open.model = zo_ide::cli_args::resolve_model_alias(model);
    }
    if let Some(effort) = brief
        .effort
        .as_deref()
        .and_then(zo_ide::effort::Effort::from_token)
    {
        launch.open.effort = Some(effort);
    }
    // The mode as the harness spells it (`PermissionMode::as_str`, all five
    // words — a custom role's `prompt` is a real mode, not a typo), then the
    // CLI's three-word labels for a brief written by hand.
    if let Some(mode) = brief.permission_mode.as_deref().and_then(|label| {
        runtime::PermissionMode::parse(label).or_else(|| {
            zo_ide::permission_mode::normalize_permission_mode(label)
                .map(zo_ide::permission_mode::permission_mode_from_label)
        })
    }) {
        launch.open.permission_mode = mode;
    }
    // The harness (t-2513 contract 1). A v2 brief carries the one the parent
    // resolved — prompt, tools, mode, rules — and the child runs it as
    // written; re-deriving any of it here would be the drift the contract
    // closes. A v1 brief (an older parent) carries none, so the child falls
    // back to the type's table, and the capability snapshot says `derived`.
    match brief.harness.as_ref() {
        Some(harness) => {
            if !harness.allowed_tools.is_empty() {
                launch.open.allowed_tools = Some(harness.allowed_tools.iter().cloned().collect());
            }
            launch.open.teammate_harness = Some(zo_ide::session::plain_session::TeammateHarness {
                system_prompt: harness.system_prompt.clone(),
                inspection_shell: harness.inspection_shell,
                permission_rules: harness
                    .permission_rules
                    .as_ref()
                    .map(runtime::subagent_panes::PermissionRules::to_config),
            });
            // The parent's MCP tools, answered by the parent over its channel
            // — the inline passthrough with a socket in the middle.
            launch.open.remote_mcp = harness
                .mcp
                .clone()
                .filter(|route| !route.tools.is_empty());
        }
        None => {
            // The tools this harness is allowed, from the same table an
            // in-process sub-agent of this type gets. An Explore that could
            // write files in a pane would be a different agent from the
            // Explore its parent asked for.
            if let Some(kind) = brief
                .subagent_type
                .as_deref()
                .filter(|one| !one.trim().is_empty())
            {
                let mut allowed = tools::allowed_tools_for_subagent(kind);
                // Old briefs cannot carry the constraint. Preserve their old
                // shell-free role set until the parent sends a full harness.
                if tools::inspection_shell_for_subagent(kind) { allowed.remove("bash"); }
                if !allowed.is_empty() {
                    launch.open.allowed_tools = Some(allowed);
                }
            }
        }
    }
    // The parent's registry (t-2513 §2.5): the same record the parent opened,
    // under the parent's session id, so what this child stamps and reaps
    // lands where the parent reads.
    if let (Some(locator), Some(parent)) = (brief.registry_locator.clone(), brief.parent_session.clone()) {
        launch.open.parent_registry = Some(zo_ide::session::plain_session::ParentRegistry {
            session_id: parent,
            locator,
        });
    }
    // A re-cut teammate continues the transcript its parent named (t-2513
    // §2.4); the brief wins over the command line, which only echoes it.
    if let Some(transcript) = brief.resume_transcript.as_deref().filter(|path| path.is_file()) {
        launch.open.resume = Some(transcript.to_string_lossy().into_owned());
    }
    if let Some(cwd) = brief.cwd.as_deref().filter(|path| path.is_dir()) {
        std::env::set_current_dir(cwd)?;
        launch.open.cwd = cwd.to_path_buf();
    }

    // The guard reads the selection the brief settled — a parent's model or
    // effort is the request when the command line gave none — and refuses
    // by name before the session opens. The parent waiting on `result.json`
    // is told, exactly as it is for a brief that cannot be read.
    let requested_model = launch.requested.model.clone().or_else(|| brief.model.clone());
    let requested_effort = launch.requested.effort.clone().or_else(|| brief.effort.clone());
    let guarded = match guard_launch(&launch, requested_model.as_deref(), requested_effort.as_deref()) {
        Ok(guarded) => guarded,
        Err(failure) => {
            let mut refused = TeammateResult::new(&brief.agent_id, Exit::Error);
            refused.error = Some(match &failure {
                launch_contract::Failure::Usage(why) => why.clone(),
                launch_contract::Failure::Refused(refused) => refused.refusal.line(),
            });
            let _ = refused.write(directory);
            return refuse_launch(&launch.requested, launch.render, launch.events_bind.as_deref(), failure);
        }
    };
    if guarded.is_some() {
        launch.open.exact_selection = true;
    }

    let banner = zo_ide::tui::strings::teammate_banner(
        brief.parent_session.as_deref(),
        brief
            .subagent_type
            .as_deref()
            .filter(|one| !one.trim().is_empty())
            .unwrap_or("subagent"),
    );
    let prompt = brief.prompt.clone();
    let lifecycle = zo_ide::teammate::Lifecycle::from_brief(directory, &brief, Limits::load());
    let session = PlainSession::open(launch.open)?;
    let discovery = open_events_channel(tokio_rt, launch.events_bind.as_deref(), true, &session, &launch.requested, Some(&brief),
        guarded.as_ref().map(|guarded| &guarded.contract))?;
    if let Some(channel) = events::channel() {
        // This pane is a teammate: a steer between turns opens the next one,
        // and `teammate.close` is answered (t-2513 §2.2).
        channel.set_idle_steer(true);
    }
    if let Some(discovery) = discovery {
        // The parent finds this child's channel beside the brief — it does
        // not know the child's pid, so the pid-named discovery file is not
        // its to find. Tracked so a clean exit removes it.
        if let Err(why) = lifecycle.publish_channel(&discovery) {
            eprintln!("zo: could not publish the teammate's channel file: {why}");
        }
        process_exit.track_events_addr_file(discovery);
    }
    // A teammate that could not even start its first turn still tells its
    // parent, in the same file the parent is waiting on.
    let agent_id = brief.agent_id.clone();
    let first_turn = lifecycle.first_turn;
    let life = match tokio_rt.block_on(tui::run_teammate(session, launch.render, prompt, banner, lifecycle)) {
        Ok(life) => life,
        Err(error) => {
            if TeammateResult::read_turn(directory, first_turn).is_none() {
                let mut refused = TeammateResult::new(&agent_id, Exit::Error);
                refused.error = Some(error.to_string());
                let _ = refused.write_turn(directory, first_turn);
            }
            return Err(error);
        }
    };
    let _ = life.turns;
    Ok(success("teammate"))
}
