//! One login road for every agent whose own CLI signs itself in.
//!
//! Claude and Codex each grew a button that ran the CLI's login verb in a
//! home this window manages and waited for the credential file to change
//! (`accounts.rs`, `codex_accounts.rs`). Grok and Kimi wanted the same road
//! and a third copy of the loop was the wrong answer — so the loop lives
//! here once ([`run_login`]), and what differs per provider is a ROW of
//! [`CLI_LOGINS`]: the home variable, how the login is started, how it is
//! proven complete, how it is ended. Every door — the settings row, the
//! status bar's sign-in, the readiness invalidation, the tests — is driven
//! off that table, so a provider that gains OAuth is one row here and one
//! fixture in the tests, never a branch on its name.
//!
//! **This window never sees a credential.** A row's proof reads the witness
//! through the parser the usage gauge already reads it with, and the parsed
//! token is never kept, logged or sent. The Kimi row is stricter still: its
//! file is watched and never written — a refresh token rotated by anybody
//! but the CLI logs the live `kimi` session out (`usage_kimi`'s head
//! comment).
//!
//! The survey of forty CLIs (t-6009, `agent-login-survey.md`) found three
//! shapes, and a row carries each as a field rather than as a branch:
//!
//! * **How it starts** — [`LoginRoad`]. A headless verb that opens its own
//!   browser (`grok login`, `codex login`); a verb that needs a screen
//!   because it prints a device code or insists on a TTY (`kimi login`,
//!   `mmx auth login`), typed into one of this window's shells; a slash
//!   command inside the TUI (`/login`, `/auth`) for a CLI with no verb at
//!   all; or nothing, for a CLI that signs in on its first bare run.
//! * **How it is proven** — [`Proof`]. A credential file the CLI writes,
//!   judged by the gauge's parser (sixteen CLIs); or, where macOS holds the
//!   login in the keychain or a database, the CLI's own status command
//!   (`claude auth status`, `codex login status`).
//! * **How it ends** — [`LogoutRoad`]. A verb, or a slash command.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::{usage_grok, usage_kimi};

/// How long a login may take: a browser is waiting for a human.
pub(crate) const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

/// How often a witness FILE is read while a login runs.
///
/// Measured on the fake CLIs in the tests below: the row flips within one
/// poll of the write, which is a wait a person watching a browser cannot
/// tell from instant. Halving it would double the reads of a file that
/// changes exactly once.
pub(crate) const WITNESS_POLL: Duration = Duration::from_millis(250);

/// How often a STATUS COMMAND is asked while a login is watched. A process
/// per poll, so eight times slower than the file: the answer moves once,
/// and a person typing a device code does not notice two seconds.
pub(crate) const STATUS_POLL: Duration = Duration::from_secs(2);

/// How long a headless login is given to exit on its own once the witness
/// has landed, before it is killed. Orca's `postAuthExitTimeout` is the same
/// idea: a CLI that does exit should be allowed to; one that holds a
/// callback server open would otherwise keep its port against the next
/// login.
pub(crate) const POST_AUTH_GRACE: Duration = Duration::from_secs(2);

/// How a CLI's login is started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoginRoad {
    /// `<program> <args…>`, headless: the CLI opens its own browser and
    /// writes the witness. Run by the backend with both pipes drained.
    Verb(&'static [&'static str]),
    /// `<program> <args…>` that needs a screen — it prints a device code to
    /// stderr, or refuses to run without a TTY — so the window types it into
    /// one of its own shells and the backend watches the proof.
    PaneVerb(&'static [&'static str]),
    /// No verb: the TUI is opened in a pane and this slash command typed.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "a road the table accepts and the window walks (t-6003); no catalog row takes \
                      it yet — the expect lifts itself when one does"
        )
    )]
    TuiCommand(&'static str),
    /// The bare program signs in on its first run; nothing is typed.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "a road the table accepts and the window walks (t-6003); no catalog row takes \
                      it yet — the expect lifts itself when one does"
        )
    )]
    FirstRun,
}

impl LoginRoad {
    /// The word the window branches on — the ROW's answer, so no door has
    /// to know which agent it is looking at.
    const fn word(self) -> &'static str {
        match self {
            Self::Verb(_) => "verb",
            Self::PaneVerb(_) => "pane-verb",
            Self::TuiCommand(_) => "tui",
            Self::FirstRun => "first-run",
        }
    }

    const fn tui_command(self) -> Option<&'static str> {
        match self {
            Self::TuiCommand(command) => Some(command),
            Self::Verb(_) | Self::PaneVerb(_) | Self::FirstRun => None,
        }
    }

    const fn pane_args(self) -> Option<&'static [&'static str]> {
        match self {
            Self::PaneVerb(args) => Some(args),
            Self::Verb(_) | Self::TuiCommand(_) | Self::FirstRun => None,
        }
    }
}

/// How a CLI's login is ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogoutRoad {
    Verb(&'static [&'static str]),
    TuiCommand(&'static str),
}

impl LogoutRoad {
    const fn word(self) -> &'static str {
        match self {
            Self::Verb(_) => "verb",
            Self::TuiCommand(_) => "tui",
        }
    }

    const fn tui_command(self) -> Option<&'static str> {
        match self {
            Self::TuiCommand(command) => Some(command),
            Self::Verb(_) => None,
        }
    }
}

/// How a login is proven complete — and, later, still standing.
#[derive(Clone, Copy)]
pub(crate) enum Proof {
    /// A credential file the CLI writes into its home.
    File {
        /// The file inside the home: the witness a login writes and a logout
        /// removes.
        witness: fn(&Path) -> PathBuf,
        /// Whether this witness TEXT holds a login, judged by the gauge's
        /// own parser at `now_ms`. An expired-but-refreshable session
        /// counts: the CLI refreshes it on its next run, and a row that
        /// called it signed out would send the person through a login they
        /// do not need.
        holds: fn(&str, i64) -> bool,
        /// Who the witness text says is signed in — an email or a user id,
        /// never a token.
        account: fn(&str, i64) -> Option<String>,
    },
    /// No file this window may read (a keychain, a database): the CLI's own
    /// status command, exit 0 when signed in — the shape `claude auth
    /// status` and `codex login status` document.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the survey's B shape, exercised by the tests' fake CLI (t-6003); no catalog \
                      row takes it yet — the expect lifts itself when one does"
        )
    )]
    Status(&'static [&'static str]),
}

/// One provider's login road. Every field is a fact about the CLI, and the
/// ones that name files or read them are the usage module's own functions
/// rather than copies of its constants — one answer to "where does this CLI
/// keep its login and what counts as one", read by the gauge and by this
/// road alike.
pub(crate) struct CliLogin {
    /// The catalog id, which is also the usage provider's id.
    pub(crate) agent: &'static str,
    /// The environment variable the CLI reads its home from. Handed to the
    /// child explicitly so the CLI and the window agree on the home even
    /// when the person's shell would have resolved it differently.
    pub(crate) home_var: &'static str,
    /// The home this machine's CLI uses — the variable, or the default under
    /// `$HOME`.
    pub(crate) home: fn() -> Option<PathBuf>,
    pub(crate) proof: Proof,
    pub(crate) login: LoginRoad,
    pub(crate) logout: LogoutRoad,
}

/// The rows. Grok and Kimi first, each spelled the way the survey's evidence
/// spells it; a provider that gains an OAuth login is one more row here and
/// one more fixture in `tests::FIXTURES`.
pub(crate) const CLI_LOGINS: &[CliLogin] = &[
    // `grok login` is "Browser OIDC (default)" and opens the browser itself
    // (docs.x.ai/build/cli/reference; 02-authentication.md#L9), writing
    // `$GROK_HOME/auth.json`; `grok logout` removes it.
    CliLogin {
        agent: "grok",
        home_var: usage_grok::HOME_VAR,
        home: usage_grok::grok_home,
        proof: Proof::File {
            witness: usage_grok::auth_file,
            holds: |text, now_ms| {
                matches!(
                    usage_grok::parse_auth(text, now_ms),
                    usage_grok::Auth::Held(_)
                )
            },
            account: usage_grok::signed_in_as,
        },
        login: LoginRoad::Verb(&["login"]),
        logout: LogoutRoad::Verb(&["logout"]),
    },
    // `kimi login` is the RFC 8628 device-code flow "without entering the
    // TUI": it "prints the verification URL and user code to stderr, then
    // polls" and exits 0/1 (kimi-command.md) — a person has to READ that
    // code, so the verb runs in a shell of this window, not in a pipe. The
    // login lands in `$KIMI_CODE_HOME/credentials/kimi-code.json`; there is
    // no logout verb, only `/logout` inside the TUI (getting-started.md#L93,
    // kimi-command.md#L136).
    CliLogin {
        agent: "kimi",
        home_var: usage_kimi::HOME_VAR,
        home: usage_kimi::kimi_home,
        proof: Proof::File {
            witness: usage_kimi::credentials_file,
            holds: |text, now_ms| {
                matches!(
                    usage_kimi::parse_credentials(text, now_ms / 1_000),
                    usage_kimi::Credentials::Fresh(_) | usage_kimi::Credentials::Expired
                )
            },
            // The credentials file carries a token and its expiry and no
            // name.
            account: |_, _| None,
        },
        login: LoginRoad::PaneVerb(&["login"]),
        logout: LogoutRoad::TuiCommand("/logout"),
    },
];

pub(crate) fn row(agent: &str) -> Option<&'static CliLogin> {
    CLI_LOGINS.iter().find(|row| row.agent == agent)
}

impl CliLogin {
    /// The home this row resolves to on this machine.
    fn home(&self) -> Result<PathBuf, String> {
        (self.home)().ok_or_else(|| "홈 디렉터리를 찾지 못했습니다".to_string())
    }

    /// The credential file under `home`, for a row proven by one.
    fn witness_in(&self, home: &Path) -> Option<PathBuf> {
        match self.proof {
            Proof::File { witness, .. } => Some(witness(home)),
            Proof::Status(_) => None,
        }
    }

    /// Whether this row's login stands right now: the witness file judged
    /// by the gauge's parser, or the status command's exit code. `program`
    /// is needed only for the latter; a row proven by a file answers with
    /// no CLI on the machine at all.
    pub(crate) fn holds_now(&self, program: Option<&str>, home: &Path, now_ms: i64) -> bool {
        match self.proof {
            Proof::File { witness, holds, .. } => {
                std::fs::read_to_string(witness(home)).is_ok_and(|text| holds(&text, now_ms))
            }
            Proof::Status(args) => program.is_some_and(|program| {
                command(program, self.home_var, home)
                    .args(args)
                    .output()
                    .is_ok_and(|output| output.status.success())
            }),
        }
    }

    fn account_in(&self, home: &Path, now_ms: i64) -> Option<String> {
        let Proof::File { account, .. } = self.proof else {
            return None;
        };
        let text = std::fs::read_to_string(self.witness_in(home)?).ok()?;
        account(&text, now_ms)
    }

    /// The shell line the window types for a [`LoginRoad::PaneVerb`] row:
    /// the home in the CLI's own variable, then the verb — the same
    /// environment the headless runner would have built, spelled for a
    /// shell because the person has to see what the CLI prints.
    fn pane_command(&self, program: &str, home: &Path) -> Option<String> {
        let args = self.login.pane_args()?;
        let mut line = format!(
            "{}={} {}",
            self.home_var,
            shell_quote(&home.to_string_lossy()),
            shell_quote(program)
        );
        for arg in args {
            line.push(' ');
            line.push_str(&shell_quote(arg));
        }
        Some(line)
    }
}

/// One shell word, single-quoted unless it needs no quoting at all.
fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./=:@".contains(&b));
    if plain {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// The CLI process every road here starts, aimed at one home.
///
/// One builder because every clause is load-bearing and each road used to
/// spell all of them: the shell's PATH (a Finder-launched app's own PATH has
/// no homebrew or npm on it, and that is where these CLIs are installed —
/// see `shell_path`); this home in the CLI's own variable, so an ambient one
/// cannot make every home answer for the same person; and stdin closed with
/// both pipes taken, so a prompt cannot hang the window and a pipe nobody
/// reads cannot fill.
pub(crate) fn command(program: &str, home_var: &str, home: &Path) -> Command {
    let mut command = crate::proc::quiet_command(program);
    if let Some(path) = crate::shell_path::hydrated() {
        command.env("PATH", path);
    }
    command
        .env(home_var, home)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    command
}

/// The credential file as it stood before a login, so a login that writes
/// nothing is not mistaken for one that did (Orca takes the same snapshot).
pub(crate) struct Witness<'a> {
    file: &'a Path,
    before: Option<String>,
    holds: &'a dyn Fn(&str) -> bool,
}

impl<'a> Witness<'a> {
    /// Snapshot `file` now. `holds` judges the text a login leaves; the
    /// runner calls the login complete only when the file has CHANGED and
    /// `holds` accepts what is there.
    pub(crate) fn taken(file: &'a Path, holds: &'a dyn Fn(&str) -> bool) -> Self {
        Self {
            file,
            before: std::fs::read_to_string(file).ok(),
            holds,
        }
    }

    /// Has a login landed since the snapshot?
    fn arrived(&self) -> bool {
        match std::fs::read_to_string(self.file) {
            Ok(now) => self.before.as_deref() != Some(now.as_str()) && (self.holds)(&now),
            Err(_) => false,
        }
    }

    /// Is the file gone, or no longer a login?
    fn departed(&self) -> bool {
        match std::fs::read_to_string(self.file) {
            Ok(now) => !(self.holds)(&now),
            Err(_) => true,
        }
    }
}

/// Run a CLI's own login against a home and wait for it.
///
/// With a witness, the file first: a login of this shape does not always
/// exit on its own — `codex login` holds a local callback server open, so
/// waiting for the process would time out on every SUCCESSFUL login, the
/// worst shape a wait can have. Without one (a keychain login), the process
/// is the only thing to wait for. Both output pipes are drained on their
/// own threads: a pipe nobody reads fills, and then the login blocks
/// mid-print and times out looking innocent. The stderr tail is the only
/// place the CLI says WHY a login failed, so it is what the error carries.
///
/// Blocking and long: the caller puts it on a pool.
pub(crate) fn run_login(
    program: &str,
    args: &[&str],
    home_var: &str,
    home: &Path,
    witness: Option<Witness<'_>>,
) -> Result<(), String> {
    run_login_within(program, args, home_var, home, witness, LOGIN_TIMEOUT)
}

fn run_login_within(
    program: &str,
    args: &[&str],
    home_var: &str,
    home: &Path,
    witness: Option<Witness<'_>>,
    timeout: Duration,
) -> Result<(), String> {
    use std::io::Read;
    std::fs::create_dir_all(home).map_err(|error| error.to_string())?;
    let mut child = command(program, home_var, home)
        .args(args)
        .spawn()
        .map_err(|error| format!("{program}을(를) 실행할 수 없습니다: {error}"))?;
    let stdout = child.stdout.take();
    let _stdout_drain = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stream) = stdout {
            let _ = stream.read_to_string(&mut text);
        }
    });
    let stderr = child.stderr.take();
    let stderr_drain = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stream) = stderr {
            let _ = stream.read_to_string(&mut text);
        }
        text
    });

    let began = Instant::now();
    let mut landed_at: Option<Instant> = None;
    loop {
        if landed_at.is_none() && witness.as_ref().is_some_and(Witness::arrived) {
            landed_at = Some(Instant::now());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                return if landed_at.is_some() || status.success() {
                    Ok(())
                } else {
                    let why = stderr_drain
                        .join()
                        .ok()
                        .map(|text| text.trim().lines().last().unwrap_or_default().to_string())
                        .filter(|line| !line.is_empty());
                    Err(match why {
                        Some(line) => format!("로그인이 실패했습니다: {line}"),
                        None => "로그인이 실패했습니다".into(),
                    })
                };
            }
            Ok(None) => {
                // Signed in and still running — give it its moment to leave,
                // then take it down.
                if landed_at.is_some_and(|at| at.elapsed() >= POST_AUTH_GRACE) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(());
                }
                if began.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("로그인이 완료되지 않았습니다".into());
                }
                std::thread::sleep(WITNESS_POLL);
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

/// Wait for a witness to arrive (`signed_in`) or depart (`!signed_in`)
/// without running anything — the pane roads, where the CLI is in a pane
/// the window opened and the person is typing at it.
fn wait_for_witness_within(
    witness: &Witness<'_>,
    signed_in: bool,
    timeout: Duration,
) -> Result<(), String> {
    wait_until(
        || {
            if signed_in {
                witness.arrived()
            } else {
                witness.departed()
            }
        },
        signed_in,
        timeout,
        WITNESS_POLL,
    )
}

/// The same wait against a status command: asked every `poll` until it
/// answers the way the caller wants.
fn wait_for_status_within(
    program: &str,
    args: &[&str],
    home_var: &str,
    home: &Path,
    signed_in: bool,
    timeout: Duration,
    poll: Duration,
) -> Result<(), String> {
    wait_until(
        || {
            let standing = command(program, home_var, home)
                .args(args)
                .output()
                .is_ok_and(|output| output.status.success());
            standing == signed_in
        },
        signed_in,
        timeout,
        poll,
    )
}

fn wait_until(
    mut done: impl FnMut() -> bool,
    signed_in: bool,
    timeout: Duration,
    poll: Duration,
) -> Result<(), String> {
    let began = Instant::now();
    loop {
        if done() {
            return Ok(());
        }
        if began.elapsed() >= timeout {
            return Err(if signed_in {
                "로그인이 완료되지 않았습니다".into()
            } else {
                "로그아웃이 완료되지 않았습니다".into()
            });
        }
        std::thread::sleep(poll);
    }
}

/* ---- this machine's login, per row ------------------------------------- */

/// Sign this machine's login in through the row's headless verb.
///
/// Only a [`LoginRoad::Verb`] row comes here; every other road is the
/// pane's and the window's, and this side only [`watch`]es for them.
pub(crate) fn login_headless(row: &CliLogin, program: &str, now_ms: i64) -> Result<(), String> {
    let home = row.home()?;
    login_headless_in(row, program, &home, now_ms)
}

/// [`login_headless`] against a named home, so a test can hand it a
/// sandbox instead of the developer's own. After the run the proof is
/// judged once more: a CLI that exited zero without leaving a login has not
/// logged anybody in.
fn login_headless_in(
    row: &CliLogin,
    program: &str,
    home: &Path,
    now_ms: i64,
) -> Result<(), String> {
    let LoginRoad::Verb(args) = row.login else {
        return Err(format!("{}의 로그인은 터미널 판에서 진행합니다", row.agent));
    };
    match row.proof {
        Proof::File { witness, holds, .. } => {
            let file = witness(home);
            let judge = |text: &str| holds(text, now_ms);
            let taken = Witness::taken(&file, &judge);
            run_login(program, args, row.home_var, home, Some(taken))?;
        }
        Proof::Status(_) => run_login(program, args, row.home_var, home, None)?,
    }
    if !row.holds_now(Some(program), home, now_ms) {
        return Err("로그인이 자격 증명을 남기지 않았습니다".into());
    }
    Ok(())
}

/// Sign this machine's login out through the row's headless verb.
pub(crate) fn logout_headless(row: &CliLogin, program: &str, now_ms: i64) -> Result<(), String> {
    let home = row.home()?;
    logout_headless_in(row, program, &home, now_ms)
}

/// The CLI's own verb, never a file removal by us: the CLI knows what else
/// it keeps beside the credential. The verdict is the proof — a logout that
/// exited badly but left no login behind still logged out.
fn logout_headless_in(
    row: &CliLogin,
    program: &str,
    home: &Path,
    now_ms: i64,
) -> Result<(), String> {
    let LogoutRoad::Verb(args) = row.logout else {
        return Err(format!(
            "{}의 로그아웃은 터미널 판에서 진행합니다",
            row.agent
        ));
    };
    let output = command(program, row.home_var, home)
        .args(args)
        .output()
        .map_err(|error| format!("{program}을(를) 실행할 수 없습니다: {error}"))?;
    if output.status.success() || !row.holds_now(Some(program), home, now_ms) {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if said.is_empty() {
        "로그아웃이 완료되지 않았습니다".to_string()
    } else {
        said
    })
}

/// Watch this machine's proof until it holds a login (`signed_in`) or stops
/// holding one — the pane roads, after the window has opened the CLI in one
/// of its panes and typed the row's verb or command.
pub(crate) fn watch(
    row: &CliLogin,
    program: Option<&str>,
    signed_in: bool,
    now_ms: i64,
) -> Result<(), String> {
    let home = row.home()?;
    watch_in(
        row,
        program,
        &home,
        signed_in,
        now_ms,
        LOGIN_TIMEOUT,
        STATUS_POLL,
    )
}

fn watch_in(
    row: &CliLogin,
    program: Option<&str>,
    home: &Path,
    signed_in: bool,
    now_ms: i64,
    timeout: Duration,
    status_poll: Duration,
) -> Result<(), String> {
    match row.proof {
        Proof::File { witness, holds, .. } => {
            let file = witness(home);
            let judge = |text: &str| holds(text, now_ms);
            let taken = Witness::taken(&file, &judge);
            wait_for_witness_within(&taken, signed_in, timeout)
        }
        Proof::Status(args) => {
            let program = program
                .ok_or_else(|| format!("이 기계에서 {}을(를) 찾지 못했습니다", row.agent))?;
            wait_for_status_within(
                program,
                args,
                row.home_var,
                home,
                signed_in,
                timeout,
                status_poll,
            )
        }
    }
}

/* ---- the settings row ----------------------------------------------------- */

/// One row of the settings card: what the table says and what this machine
/// holds, in the words the window branches on.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct Standing {
    pub(crate) agent: &'static str,
    pub(crate) name: &'static str,
    pub(crate) homepage_url: &'static str,
    pub(crate) installed: bool,
    pub(crate) signed_in: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) account: Option<String>,
    /// The home, with `$HOME` folded to `~` — a caption, never a path a door
    /// opens.
    pub(crate) home: String,
    /// `verb`, `pane-verb`, `tui` or `first-run`: which road the window
    /// walks.
    pub(crate) road: &'static str,
    /// The slash command a `tui` row types, once its TUI is up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tui_login: Option<&'static str>,
    /// The shell line a `pane-verb` row types into a plain shell.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pane_command: Option<String>,
    pub(crate) logout_road: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tui_logout: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct Report {
    pub(crate) rows: Vec<Standing>,
}

/// Every row, as this machine stands. `program_of` is the catalog's own
/// detection (`agent_program`), handed in so a test can name a fake.
pub(crate) fn report(program_of: impl Fn(&str) -> Option<String>, now_ms: i64) -> Report {
    let rows = CLI_LOGINS
        .iter()
        .filter_map(|row| {
            let home = row.home().ok()?;
            standing(row, program_of(row.agent).as_deref(), &home, now_ms)
        })
        .collect();
    Report { rows }
}

fn standing(row: &CliLogin, program: Option<&str>, home: &Path, now_ms: i64) -> Option<Standing> {
    let spec = zerocode_core::agent_spec(row.agent)?;
    let signed_in = row.holds_now(program, home, now_ms);
    Some(Standing {
        agent: row.agent,
        name: spec.name,
        homepage_url: spec.homepage_url,
        installed: program.is_some(),
        signed_in,
        account: signed_in.then(|| row.account_in(home, now_ms)).flatten(),
        home: with_tilde(home),
        road: row.login.word(),
        tui_login: row.login.tui_command(),
        pane_command: program.and_then(|program| row.pane_command(program, home)),
        logout_road: row.logout.word(),
        tui_logout: row.logout.tui_command(),
    })
}

/// `$HOME/x` as `~/x`, for a caption. Anything not under the home is shown
/// as it is.
fn with_tilde(path: &Path) -> String {
    let shown = path.display().to_string();
    match dirs::home_dir() {
        Some(home) => match path.strip_prefix(&home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => shown,
        },
        None => shown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One witness the row's parser accepts, per row proven by a file. A
    /// provider joins the table by adding its row above and its fixture
    /// here; the first test holds the two lists together. No real token
    /// anywhere: the values are the words `fixture-token`, and the email is
    /// example.com's.
    const FIXTURES: &[(&str, &str)] = &[
        (
            "grok",
            concat!(
                "{\"https://auth.x.ai\":{\"key\":\"fixture-token\",",
                "\"email\":\"person@example.com\",\"user_id\":\"u-1\",",
                "\"expires_at\":\"2099-01-01T00:00:00Z\"}}"
            ),
        ),
        (
            "kimi",
            "{\"access_token\":\"fixture-token\",\"expires_at\":4102444800}",
        ),
    ];

    const NOW_MS: i64 = 1_790_000_000_000;

    fn fixture(agent: &str) -> &'static str {
        FIXTURES
            .iter()
            .find(|(named, _)| *named == agent)
            .map(|(_, text)| *text)
            .unwrap_or_else(|| panic!("no login fixture for `{agent}` — add one to FIXTURES"))
    }

    /// A row with its home pointed inside `root`, so no test reads the
    /// developer's own `~/.grok` or `~/.kimi-code`.
    fn sandboxed_home(row: &CliLogin, root: &Path) -> PathBuf {
        let home = root.join("homes").join(row.agent);
        // The witness's own directory too: Kimi keeps its file one level
        // down, and a login writes into a directory the CLI made.
        let deepest = row.witness_in(&home).unwrap_or_else(|| home.join("marker"));
        std::fs::create_dir_all(deepest.parent().expect("a witness has a directory"))
            .expect("home");
        home
    }

    fn sandboxed_places(row: &CliLogin, root: &Path) -> (PathBuf, PathBuf) {
        let home = sandboxed_home(row, root);
        let witness = row.witness_in(&home).expect("a row proven by a file");
        (home, witness)
    }

    /// A fake CLI on disk: `body` is its shell, run with the row's home
    /// variable in the environment exactly as the runner hands it.
    fn fake_cli(root: &Path, name: &str, body: &str) -> String {
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).expect("bin");
        let path = bin.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake cli");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod fake cli");
        }
        path.to_string_lossy().into_owned()
    }

    fn since(stamp: &Path) -> Duration {
        let modified = std::fs::metadata(stamp)
            .and_then(|meta| meta.modified())
            .expect("the stamp's mtime");
        std::time::SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default()
    }

    /// A row of the survey's B shape, for the tests: the login is a verb,
    /// the proof is a status command, the logout is a verb. Named after a
    /// catalog agent that IS of that shape so `standing` can name it, but
    /// never the real one's road — the fake CLI below is the whole machine.
    const STATUS_ROW: CliLogin = CliLogin {
        agent: "claude",
        home_var: "FIXTURE_HOME",
        home: || None,
        proof: Proof::Status(&["auth", "status"]),
        login: LoginRoad::Verb(&["auth", "login"]),
        logout: LogoutRoad::Verb(&["auth", "logout"]),
    };

    #[test]
    fn every_row_is_a_catalog_agent_whose_parser_accepts_its_fixture_and_refuses_an_empty_file() {
        let root = tempfile::tempdir().expect("sandbox");
        let filed = CLI_LOGINS
            .iter()
            .filter(|row| matches!(row.proof, Proof::File { .. }))
            .count();
        assert_eq!(
            filed,
            FIXTURES.len(),
            "a file-proven row without a fixture, or a fixture without a row"
        );
        for row in CLI_LOGINS {
            assert!(
                zerocode_core::agent_spec(row.agent).is_some(),
                "`{}` is not a catalog agent",
                row.agent
            );
            let Proof::File { holds, .. } = row.proof else {
                continue;
            };
            let (home, witness) = sandboxed_places(row, root.path());
            assert!(
                !row.holds_now(None, &home, NOW_MS),
                "{}: a missing witness read as a login",
                row.agent
            );
            assert!(
                !holds("{}", NOW_MS),
                "{}: an empty witness read as a login",
                row.agent
            );
            assert!(
                holds(fixture(row.agent), NOW_MS),
                "{}: the parser refused its own fixture",
                row.agent
            );
            std::fs::write(&witness, fixture(row.agent)).expect("fixture witness");
            assert!(row.holds_now(None, &home, NOW_MS));
        }
        // The usage module's own home is what the row points at — the same
        // directory the gauge reads, so a login here is a login there.
        let grok = row("grok").expect("grok row");
        assert_eq!((grok.home)(), usage_grok::grok_home());
        let kimi = row("kimi").expect("kimi row");
        assert_eq!((kimi.home)(), usage_kimi::kimi_home());
    }

    /// The headless road, end to end against a fake CLI that behaves like
    /// `codex login`: writes the witness and then does NOT exit. The runner
    /// returns once the witness is there, hands the CLI its grace, and takes
    /// it down — with the home in the row's own variable.
    #[test]
    fn a_headless_login_returns_when_the_witness_lands_and_kills_the_lingering_cli() {
        let root = tempfile::tempdir().expect("sandbox");
        let grok = row("grok").expect("grok row");
        let (home, witness) = sandboxed_places(grok, root.path());
        let seen_home = root.path().join("seen-home");
        let pid_file = root.path().join("pid");
        let wrote_at = root.path().join("wrote-at");
        let cli = fake_cli(
            root.path(),
            "grok",
            &format!(
                "[ \"$1\" = login ] || exit 9\n\
                 printf '%s' \"${}\" > '{}'\n\
                 printf '%s' $$ > '{}'\n\
                 sleep 0.1\n\
                 mkdir -p \"$(dirname '{}')\"\n\
                 printf '%s' '{}' > '{}'\n\
                 : > '{}'\n\
                 exec sleep 30",
                grok.home_var,
                seen_home.display(),
                pid_file.display(),
                witness.display(),
                fixture("grok"),
                witness.display(),
                wrote_at.display(),
            ),
        );
        let began = Instant::now();
        login_headless_in(grok, &cli, &home, NOW_MS).expect("the login");
        let took = began.elapsed();
        let since_write = since(&wrote_at);
        assert!(
            grok.holds_now(None, &home, NOW_MS),
            "the witness is not a login after the run"
        );
        assert_eq!(
            std::fs::read_to_string(&seen_home).expect("the cli saw a home"),
            home.to_string_lossy(),
            "the CLI was not handed the row's home in the row's variable"
        );
        // The CLI was still sleeping: the runner must have killed it after
        // its grace, not waited out the 30 s.
        assert!(
            took >= POST_AUTH_GRACE && took < POST_AUTH_GRACE + Duration::from_secs(3),
            "the runner returned after {took:?}; expected the grace and one poll"
        );
        let pid = std::fs::read_to_string(&pid_file).expect("pid");
        // Through the crate's own builder, as every child here is: the
        // quiet-children contract counts a bare `Command::new` in a test too.
        let alive = crate::proc::quiet_command("kill")
            .args(["-0", pid.trim()])
            .output()
            .expect("kill -0")
            .status
            .success();
        assert!(!alive, "the lingering CLI (pid {pid}) survived the login");
        // Measured for the report: the file landed when `wrote_at` was
        // touched; the runner noticed within one poll (the rest is grace).
        let noticed_within = since_write.saturating_sub(POST_AUTH_GRACE);
        eprintln!(
            "measured: witness→return {since_write:?} = grace {POST_AUTH_GRACE:?} + notice ≤{noticed_within:?}"
        );
        assert!(
            noticed_within <= WITNESS_POLL + Duration::from_millis(500),
            "the witness was noticed {noticed_within:?} after it landed"
        );
    }

    #[test]
    fn a_headless_login_that_leaves_no_login_is_refused_and_a_failure_names_the_clis_last_word() {
        let root = tempfile::tempdir().expect("sandbox");
        let grok = row("grok").expect("grok row");
        let (home, _) = sandboxed_places(grok, root.path());
        // Exit zero, wrote nothing: the runner is satisfied (the CLI said it
        // was done) but the ROAD is not — nobody logged in.
        let quiet = fake_cli(root.path(), "quiet-grok", "exit 0");
        assert_eq!(
            login_headless_in(grok, &quiet, &home, NOW_MS),
            Err("로그인이 자격 증명을 남기지 않았습니다".to_string())
        );
        let loud = fake_cli(
            root.path(),
            "loud-grok",
            "echo 'first line' >&2\necho 'SuperGrok subscription required' >&2\nexit 1",
        );
        assert_eq!(
            login_headless_in(grok, &loud, &home, NOW_MS),
            Err("로그인이 실패했습니다: SuperGrok subscription required".to_string())
        );
        let missing = root.path().join("no-such-cli");
        let said = login_headless_in(grok, &missing.to_string_lossy(), &home, NOW_MS);
        assert!(
            said.as_ref()
                .is_err_and(|why| why.contains("실행할 수 없습니다")),
            "a missing CLI did not say so: {said:?}"
        );
        // A row whose verb needs a screen has nothing to run in a pipe, and
        // says so rather than losing the device code down a drain.
        let kimi = row("kimi").expect("kimi row");
        let kimi_home = sandboxed_home(kimi, root.path());
        assert_eq!(
            login_headless_in(kimi, &quiet, &kimi_home, NOW_MS),
            Err("kimi의 로그인은 터미널 판에서 진행합니다".to_string())
        );
        assert_eq!(
            logout_headless_in(kimi, &quiet, &kimi_home, NOW_MS),
            Err("kimi의 로그아웃은 터미널 판에서 진행합니다".to_string())
        );
    }

    /// The pane roads' half: nothing is run, the witness is watched. A file
    /// that appears flips the wait within one poll; one that never appears
    /// runs the wait out.
    #[test]
    fn a_watch_flips_within_one_poll_of_the_file_and_times_out_without_it() {
        let root = tempfile::tempdir().expect("sandbox");
        let kimi = row("kimi").expect("kimi row");
        let (home, witness) = sandboxed_places(kimi, root.path());

        let writer_witness = witness.clone();
        let wrote_at = root.path().join("wrote-at");
        let stamp = wrote_at.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            std::fs::write(&writer_witness, fixture("kimi")).expect("write");
            std::fs::write(&stamp, b"").expect("stamp");
        });
        watch_in(
            kimi,
            None,
            &home,
            true,
            NOW_MS,
            Duration::from_secs(5),
            STATUS_POLL,
        )
        .expect("the login landed");
        let latency = since(&wrote_at);
        writer.join().expect("writer");
        eprintln!("measured: watch noticed the witness {latency:?} after the write");
        assert!(
            latency <= WITNESS_POLL + Duration::from_millis(200),
            "the watch noticed the file only {latency:?} after it landed"
        );

        // Signing out: the same watch, the other direction.
        let gone = witness.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            std::fs::remove_file(gone).expect("remove");
        });
        watch_in(
            kimi,
            None,
            &home,
            false,
            NOW_MS,
            Duration::from_secs(5),
            STATUS_POLL,
        )
        .expect("the logout landed");

        // And a wait with nothing to see runs out and says so.
        assert_eq!(
            watch_in(
                kimi,
                None,
                &home,
                true,
                NOW_MS,
                Duration::from_millis(300),
                STATUS_POLL
            ),
            Err("로그인이 완료되지 않았습니다".to_string())
        );
    }

    /// The witness must CHANGE: a login into a home that already held one
    /// is not complete until the file is rewritten.
    #[test]
    fn a_witness_that_already_held_a_login_has_to_change_before_it_counts() {
        let root = tempfile::tempdir().expect("sandbox");
        let grok = row("grok").expect("grok row");
        let (_, witness) = sandboxed_places(grok, root.path());
        let Proof::File { holds, .. } = grok.proof else {
            panic!("grok is proven by a file")
        };
        std::fs::write(&witness, fixture("grok")).expect("held login");
        let judge = |text: &str| holds(text, NOW_MS);
        let taken = Witness::taken(&witness, &judge);
        assert!(
            !taken.arrived(),
            "an unchanged witness counted as a new login"
        );
        let rewritten = fixture("grok").replace("u-1", "u-2");
        std::fs::write(&witness, rewritten).expect("relogin");
        assert!(taken.arrived());
        // A rewrite that is not a login is not an arrival either.
        std::fs::write(&witness, "{}").expect("emptied");
        assert!(!taken.arrived());
        assert!(taken.departed());
    }

    /// Logging out is the CLI's verb, judged by the proof.
    #[test]
    fn a_headless_logout_is_the_clis_verb_and_the_witness_is_the_verdict() {
        let root = tempfile::tempdir().expect("sandbox");
        let grok = row("grok").expect("grok row");
        let (home, witness) = sandboxed_places(grok, root.path());
        let file_name = witness
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        // The verb removes the file: signed out.
        std::fs::write(&witness, fixture("grok")).expect("login");
        let removing = fake_cli(
            root.path(),
            "grok-out",
            &format!(
                "[ \"$1\" = logout ] || exit 9\nrm -f \"${}/{file_name}\"",
                grok.home_var
            ),
        );
        logout_headless_in(grok, &removing, &home, NOW_MS).expect("logout");
        assert!(!grok.holds_now(None, &home, NOW_MS));
        // The verb fails but the login is already gone: still signed out.
        let failing = fake_cli(root.path(), "grok-fail", "echo 'not logged in' >&2\nexit 1");
        logout_headless_in(grok, &failing, &home, NOW_MS).expect("nothing to log out of");
        // The verb fails and the login stands: the CLI's word is the error.
        std::fs::write(&witness, fixture("grok")).expect("login again");
        assert_eq!(
            logout_headless_in(grok, &failing, &home, NOW_MS),
            Err("not logged in".to_string())
        );
        assert!(
            grok.holds_now(None, &home, NOW_MS),
            "a failed logout removed the file itself"
        );
    }

    /// The survey's B shape end to end: a login verb that exits when it is
    /// done, a status command as the only proof, a logout verb. The fake
    /// CLI keeps its "keychain" as a marker file the row never reads.
    #[test]
    fn a_status_proven_row_logs_in_out_and_is_watched_through_the_clis_own_status_command() {
        let root = tempfile::tempdir().expect("sandbox");
        let row = &STATUS_ROW;
        let home = sandboxed_home(row, root.path());
        let cli = fake_cli(
            root.path(),
            "claude",
            &format!(
                "keychain=\"${}/keychain\"\n\
                 case \"$1 $2\" in\n\
                   'auth login') sleep 0.1; : > \"$keychain\"; exit 0;;\n\
                   'auth status') [ -f \"$keychain\" ];;\n\
                   'auth logout') rm -f \"$keychain\"; exit 0;;\n\
                   *) exit 9;;\n\
                 esac",
                row.home_var
            ),
        );
        assert!(!row.holds_now(Some(&cli), &home, NOW_MS));
        assert!(
            !row.holds_now(None, &home, NOW_MS),
            "a status row answered without a CLI to ask"
        );
        login_headless_in(row, &cli, &home, NOW_MS).expect("the login");
        assert!(row.holds_now(Some(&cli), &home, NOW_MS));
        logout_headless_in(row, &cli, &home, NOW_MS).expect("the logout");
        assert!(!row.holds_now(Some(&cli), &home, NOW_MS));
        // The watch, against the status command rather than a file.
        let keychain = home.join("keychain");
        let wrote_at = root.path().join("wrote-at");
        let stamp = wrote_at.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            std::fs::write(keychain, b"").expect("sign in behind the watch");
            std::fs::write(stamp, b"").expect("stamp");
        });
        watch_in(
            row,
            Some(&cli),
            &home,
            true,
            NOW_MS,
            Duration::from_secs(5),
            Duration::from_millis(100),
        )
        .expect("the status flipped");
        eprintln!(
            "measured: status watch (100 ms poll) noticed the login {:?} after it landed",
            since(&wrote_at)
        );
        // A status row cannot be watched without its CLI.
        assert!(
            watch_in(
                row,
                None,
                &home,
                true,
                NOW_MS,
                Duration::from_secs(1),
                STATUS_POLL
            )
            .is_err_and(|why| why.contains("찾지 못했습니다"))
        );
        // And a login verb that exits zero but leaves the status saying no
        // is refused, exactly as a file row's silent CLI is.
        let liar = fake_cli(
            root.path(),
            "liar",
            "case \"$1 $2\" in 'auth login') exit 0;; *) exit 1;; esac",
        );
        assert_eq!(
            login_headless_in(row, &liar, &home, NOW_MS),
            Err("로그인이 자격 증명을 남기지 않았습니다".to_string())
        );
    }

    /// A `pane-verb` row's shell line: the home in the CLI's own variable,
    /// then the verb, every word quoted only when it has to be.
    #[test]
    fn a_pane_verb_row_spells_the_shell_line_the_window_types() {
        let kimi = row("kimi").expect("kimi row");
        let line = kimi
            .pane_command("/opt/homebrew/bin/kimi", Path::new("/Users/dev/.kimi-code"))
            .expect("kimi types its verb");
        assert_eq!(
            line,
            format!(
                "{}=/Users/dev/.kimi-code /opt/homebrew/bin/kimi login",
                kimi.home_var
            )
        );
        let spaced = kimi
            .pane_command("kimi", Path::new("/Users/dev/My Files/.kimi-code"))
            .expect("quoted");
        assert_eq!(
            spaced,
            format!(
                "{}='/Users/dev/My Files/.kimi-code' kimi login",
                kimi.home_var
            )
        );
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        // A headless row types nothing.
        let grok = row("grok").expect("grok row");
        assert_eq!(
            grok.pane_command("grok", Path::new("/Users/dev/.grok")),
            None
        );
    }

    /// The settings row says what the table and the machine say, in the
    /// words the window branches on — and never a token.
    #[test]
    fn the_report_speaks_the_rows_road_and_names_nobody_it_cannot_name() {
        let root = tempfile::tempdir().expect("sandbox");
        let mut rows = Vec::new();
        for row in CLI_LOGINS {
            let (home, witness) = sandboxed_places(row, root.path());
            std::fs::write(&witness, fixture(row.agent)).expect("login");
            let program = (row.agent == "grok").then_some("/usr/local/bin/grok");
            rows.push(standing(row, program, &home, NOW_MS).expect("a standing"));
        }
        let grok = rows
            .iter()
            .find(|row| row.agent == "grok")
            .expect("grok row");
        assert!(grok.installed && grok.signed_in);
        assert_eq!(grok.account.as_deref(), Some("person@example.com"));
        assert_eq!((grok.road, grok.tui_login), ("verb", None));
        assert_eq!(grok.pane_command, None);
        assert_eq!((grok.logout_road, grok.tui_logout), ("verb", None));
        assert_eq!(grok.name, "Grok");
        assert_eq!(
            grok.homepage_url,
            zerocode_core::agent_spec("grok")
                .expect("spec")
                .homepage_url
        );
        let kimi = rows
            .iter()
            .find(|row| row.agent == "kimi")
            .expect("kimi row");
        assert!(!kimi.installed && kimi.signed_in);
        assert_eq!(kimi.account, None, "the Kimi witness names nobody");
        assert_eq!((kimi.road, kimi.tui_login), ("pane-verb", None));
        assert_eq!(
            kimi.pane_command, None,
            "a CLI that is not installed has no line to type"
        );
        assert_eq!(
            (kimi.logout_road, kimi.tui_logout),
            ("tui", Some("/logout"))
        );
        for row in &rows {
            let json = serde_json::to_string(row).expect("json");
            assert!(
                !json.contains("fixture-token") && !json.contains("\"key\""),
                "a token reached the settings row: {json}"
            );
            assert!(row.home.starts_with('~') || row.home.starts_with('/'));
        }
        // With the CLI found, the pane row carries its line.
        let kimi_row = super::row("kimi").expect("kimi row");
        let kimi_home = sandboxed_home(kimi_row, root.path());
        let found = standing(kimi_row, Some("kimi"), &kimi_home, NOW_MS).expect("a standing");
        assert!(found.installed);
        assert!(
            found
                .pane_command
                .as_deref()
                .is_some_and(|line| line.ends_with(" kimi login")),
            "{:?}",
            found.pane_command
        );
        // The machine-wide report walks the same rows against the real
        // homes: one standing per row, nothing invented.
        let report = report(|_| None, NOW_MS);
        assert_eq!(report.rows.len(), CLI_LOGINS.len());
        assert!(report.rows.iter().all(|row| !row.installed));
    }

    /// A moved login forgets the agent's readiness row, so the next reader
    /// observes it again rather than reading a verdict from before the login.
    #[test]
    fn a_moved_login_forgets_the_agents_readiness_row() {
        use zerocode_core::readiness::Purpose;
        for row in CLI_LOGINS {
            let before = crate::readiness_runtime::ensure(row.agent, Purpose::Display);
            assert_eq!(before.agent, row.agent);
            assert!(
                crate::readiness_runtime::observe(row.agent).is_some(),
                "{}: nothing held after an ensure",
                row.agent
            );
            crate::readiness_runtime::invalidate(row.agent);
            assert!(
                crate::readiness_runtime::observe(row.agent).is_none(),
                "{}: the readiness row survived the login moving",
                row.agent
            );
        }
    }

    /// The words the window branches on, one per road — the JS reads these
    /// off the row and never an agent's name.
    #[test]
    fn every_road_has_the_word_the_window_branches_on() {
        assert_eq!(LoginRoad::Verb(&["login"]).word(), "verb");
        assert_eq!(LoginRoad::PaneVerb(&["login"]).word(), "pane-verb");
        assert_eq!(LoginRoad::TuiCommand("/login").word(), "tui");
        assert_eq!(LoginRoad::FirstRun.word(), "first-run");
        assert_eq!(LoginRoad::TuiCommand("/auth").tui_command(), Some("/auth"));
        assert_eq!(LoginRoad::FirstRun.tui_command(), None);
        assert_eq!(LoginRoad::FirstRun.pane_args(), None);
        assert_eq!(LogoutRoad::Verb(&["logout"]).word(), "verb");
        assert_eq!(LogoutRoad::TuiCommand("/logout").word(), "tui");
        assert_eq!(
            LogoutRoad::TuiCommand("/logout").tui_command(),
            Some("/logout")
        );
    }

    #[test]
    fn the_home_caption_folds_the_home_directory_to_a_tilde() {
        let Some(home) = dirs::home_dir() else { return };
        assert_eq!(with_tilde(&home.join(".grok")), "~/.grok");
        assert_eq!(with_tilde(&home), "~");
        assert_eq!(with_tilde(Path::new("/opt/kimi")), "/opt/kimi");
    }
}
