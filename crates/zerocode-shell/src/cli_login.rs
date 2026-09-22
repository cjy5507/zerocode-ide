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
//!   because it prints a device code, asks which provider to sign into, or
//!   insists on a TTY (`kimi login`, `mmx auth login`), typed into one of
//!   this window's shells; a slash command inside the TUI (`/login`,
//!   `/auth`) for a CLI with no verb at all; or nothing, for a CLI that
//!   signs in on its first bare run.
//! * **How it is proven** — [`Proof`]. A credential file the CLI writes,
//!   judged by the gauge's parser where a gauge already reads it and by the
//!   file's own shape ([`Holds`]) where none does; the CLI's own status
//!   command, where macOS holds the login in the keychain (`claude auth
//!   status`, `agent status --format json`); or nothing at all, for a CLI
//!   that keeps its login in a keyring and answers no status command —
//!   the window can still OPEN that login, and says plainly that it cannot
//!   read the result.
//! * **How it ends** — [`LogoutRoad`]. A verb, a verb that needs a screen
//!   (it asks which provider, or asks twice), a slash command, or nothing:
//!   several CLIs document no logout at all, and a row that invented one
//!   would be deleting a credential the CLI still expects to find.
//!
//! Three OAuth CLIs of that survey are deliberately NOT rows here, because
//! each already has a road of its own on the same settings screen and a
//! second one would be two buttons for one login: `claude` (the Claude
//! accounts card, which asks `claude auth status` per account with the
//! overriding variables blanked), `codex` (the Codex accounts card, which
//! runs on this file's own runner) and `antigravity` (the window's Google
//! login, which is the credential its gauge reads — `agy`'s own keyring
//! session is a different login). `amp` is not a row either: the survey
//! could not confirm where it keeps its session.

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

/// How long ONE status command may take before the card gives up on it and
/// says so.
///
/// The settings card asks every status row at once, and `Command::output()`
/// waits forever — one wedged CLI would hold the whole list, which is the
/// shape `accounts::LOGIN_PROBE_DEADLINE` already exists for. Shorter than
/// that one (20 s): this list is painted on arrival at a pane rather than
/// after a person pressed something, and a row that could not answer says
/// 「답하지 않았습니다」 rather than 「로그아웃됨」.
pub(crate) const STATUS_DEADLINE: Duration = Duration::from_secs(8);

/// How a CLI's login is started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoginRoad {
    /// `<program> <args…>`, headless: the CLI opens its own browser and
    /// writes the witness. Run by the backend with both pipes drained.
    Verb(&'static [&'static str]),
    /// `<program> <args…>` that needs a screen — it prints a device code to
    /// stderr, asks which provider to sign into, or refuses to run without a
    /// TTY — so the window types it into one of its own shells and the
    /// backend watches the proof.
    PaneVerb(&'static [&'static str]),
    /// No verb: the CLI is opened in a pane and this slash command typed.
    TuiCommand(&'static str),
    /// The bare program signs in on its first run; nothing is typed.
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
///
/// [`Self::None`] is a real answer and the survey's most common one after a
/// verb: `cline`, `ante`, `aider`, `goose` and `mistral-vibe` document no
/// logout at all. The card offers no way out for those rows rather than
/// removing a file the CLI never said it could lose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogoutRoad {
    Verb(&'static [&'static str]),
    /// A logout verb that needs a screen: it asks which provider to drop
    /// (`opencode auth logout`) or asks the person to confirm
    /// (`autohand logout`).
    PaneVerb(&'static [&'static str]),
    TuiCommand(&'static str),
    None,
}

impl LogoutRoad {
    const fn word(self) -> &'static str {
        match self {
            Self::Verb(_) => "verb",
            Self::PaneVerb(_) => "pane-verb",
            Self::TuiCommand(_) => "tui",
            Self::None => "none",
        }
    }

    const fn tui_command(self) -> Option<&'static str> {
        match self {
            Self::TuiCommand(command) => Some(command),
            Self::Verb(_) | Self::PaneVerb(_) | Self::None => None,
        }
    }

    const fn pane_args(self) -> Option<&'static [&'static str]> {
        match self {
            Self::PaneVerb(args) => Some(args),
            Self::Verb(_) | Self::TuiCommand(_) | Self::None => None,
        }
    }
}

/// What in a credential file IS the login — the slice a row watches.
///
/// A file the CLI writes only when somebody signs in can be judged whole
/// ([`Self::Written`]), but most of these files carry other things beside
/// the login: `crush.json` is also state, `~/.copilot/config.json` is also
/// application state, and `auth.json` holds one entry per provider. Such a
/// row names the KEY PATH that is its login, so an unrelated write to the
/// same file is not read as somebody signing in.
#[derive(Clone, Copy)]
pub(crate) enum Holds {
    /// Any non-blank text: a file that exists only while a login does.
    Written,
    /// A JSON document with a non-empty value at this key path — the empty
    /// path asking only that the document itself hold something.
    Json(&'static [&'static str]),
    /// A dotenv file assigning this variable a non-empty value (`aider`
    /// appends `OPENROUTER_API_KEY="…"` to `~/.aider/oauth-keys.env`).
    EnvVar(&'static str),
}

/// What a status command's answer has to be for the login to stand.
#[derive(Clone, Copy)]
pub(crate) enum Says {
    /// Exit 0 when signed in, non-zero when not — the shape `claude auth
    /// status` and `codex login status` document.
    Exit0,
    /// Exit 0 either way, and the verdict is in the JSON it prints: a
    /// non-empty value at this key path (the empty path asking only that
    /// the document hold something).
    Json(&'static [&'static str]),
}

/// How a login is proven complete — and, later, still standing.
#[derive(Clone, Copy)]
pub(crate) enum Proof {
    /// A credential file a usage gauge in this window already reads, judged
    /// by the gauge's own parser rather than by a second reading of the
    /// same bytes.
    Gauge {
        /// The file inside the home: the witness a login writes and a logout
        /// removes.
        witness: fn(&Path) -> PathBuf,
        /// Whether this witness TEXT holds a login, judged at `now_ms`. An
        /// expired-but-refreshable session counts: the CLI refreshes it on
        /// its next run, and a row that called it signed out would send the
        /// person through a login they do not need.
        holds: fn(&str, i64) -> bool,
        /// Who the witness text says is signed in — an email or a user id,
        /// never a token.
        account: fn(&str, i64) -> Option<String>,
    },
    /// A credential file no gauge reads: the path under the row's home, and
    /// what in it is the login.
    File {
        witness: &'static [&'static str],
        holds: Holds,
    },
    /// No file this window may read (a keychain, a database): the CLI's own
    /// status command.
    Status {
        args: &'static [&'static str],
        says: Says,
    },
    /// A keyring and no status command: this window cannot tell whether the
    /// CLI is signed in, and says so rather than guessing 「로그아웃됨」.
    /// The login road still works — the CLI's own screen shows the result.
    Unreadable,
}

/// Whose login a row signs in.
///
/// Most rows name a catalog agent, and the catalog already knows its display
/// name, its vendor page and the name it answers to on PATH. Two providers
/// the survey found with an OAuth login have no catalog row, because this
/// window does not start them in a pane — Google's Gemini CLI and MiniMax's
/// `mmx`. Those rows carry the three facts themselves rather than teaching
/// the launch catalog to list a program nobody here launches.
#[derive(Clone, Copy)]
pub(crate) enum Cli {
    /// A row of `AGENT_SPECS`: name, vendor page and program come from there.
    Agent,
    /// A CLI outside the launch catalog.
    Own {
        program: &'static str,
        name: &'static str,
        homepage_url: &'static str,
    },
}

/// Where a CLI keeps its login on this machine.
#[derive(Clone, Copy)]
pub(crate) enum Home {
    /// A usage gauge already resolves this home, variable and all
    /// (`usage_grok::grok_home`); the row borrows it rather than spelling
    /// the same path a second time.
    Gauge(fn() -> Option<PathBuf>),
    /// The row's variable when the environment sets it, else these segments
    /// under `$HOME`. The empty list is a CLI whose home IS `$HOME`:
    /// `command-code` reads `~/.commandcode` and documents `HOME` as the
    /// only knob that moves it, so `HOME` is what the row hands the child.
    Under(&'static [&'static str]),
}

/// One provider's login road. Every field is a fact about the CLI, and the
/// ones a usage gauge already owns are that gauge's own functions rather
/// than copies of its constants — one answer to "where does this CLI keep
/// its login and what counts as one", read by the gauge and by this road
/// alike.
pub(crate) struct CliLogin {
    /// The catalog id, which is also the usage provider's id — or, for a
    /// CLI outside the catalog, the id the survey files it under.
    pub(crate) agent: &'static str,
    pub(crate) cli: Cli,
    /// The environment variable the CLI reads its home from. Handed to the
    /// child explicitly so the CLI and the window agree on the home even
    /// when the person's shell would have resolved it differently.
    pub(crate) home_var: &'static str,
    pub(crate) home: Home,
    pub(crate) proof: Proof,
    pub(crate) login: LoginRoad,
    pub(crate) logout: LogoutRoad,
    /// Variables that OUTRANK the stored login for this CLI, blanked for
    /// every child this file starts: GitHub's CLI reads `GH_TOKEN` before
    /// its own keychain entry, so a shell that exports one would make every
    /// login this window starts answer for somebody else.
    pub(crate) shadowed_by: &'static [&'static str],
}

/// The rows, in the survey's own three groups: a file this window can read,
/// a status command it can ask, and a login it cannot see at all. Every
/// value is the spelling its evidence quote uses (t-6009,
/// `agent-login-survey.md` and the `agent-login-survey-evidence/` files);
/// a provider that gains an OAuth login is one more row here and one more
/// fake CLI in `tests::FAKES`.
pub(crate) const CLI_LOGINS: &[CliLogin] = &[
    /* ---- A. a verb, a home variable, a file ------------------------------ */
    //
    // `grok login` is "Browser OIDC (default)" and opens the browser itself
    // (docs.x.ai/build/cli/reference; 02-authentication.md#L9), writing
    // `$GROK_HOME/auth.json`; `grok logout` removes it.
    CliLogin {
        agent: "grok",
        cli: Cli::Agent,
        home_var: usage_grok::HOME_VAR,
        home: Home::Gauge(usage_grok::grok_home),
        proof: Proof::Gauge {
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
        shadowed_by: &[],
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
        cli: Cli::Agent,
        home_var: usage_kimi::HOME_VAR,
        home: Home::Gauge(usage_kimi::kimi_home),
        proof: Proof::Gauge {
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
        shadowed_by: &[],
    },
    // Devin: "`devin auth login`" with a login-method menu the person picks
    // from, so it is typed into a shell; the token lands in
    // "`$XDG_DATA_HOME/devin/credentials.toml` when `XDG_DATA_HOME` is set;
    // otherwise `~/.local/share/devin/credentials.toml`" and
    // `devin auth logout` "Log out and remove stored credentials"
    // (docs.devin.ai/cli/reference/commands, /cli/enterprise/devin-auth).
    // TOML, and the file exists only while a login does, so the whole file
    // is the witness.
    CliLogin {
        agent: "devin",
        cli: Cli::Agent,
        home_var: "XDG_DATA_HOME",
        home: Home::Under(&[".local", "share"]),
        proof: Proof::File {
            witness: &["devin", "credentials.toml"],
            holds: Holds::Written,
        },
        login: LoginRoad::PaneVerb(&["auth", "login"]),
        logout: LogoutRoad::Verb(&["auth", "logout"]),
        shadowed_by: &[],
    },
    // opencode signs into one of ITS providers, and which one is the
    // person's choice: `opencode auth login` with no `-p` opens the picker,
    // and the browser and device flows print their URL into the terminal
    // (opencode.ai/docs/cli/#login). Credentials are one entry per provider
    // id in "~/.local/share/opencode/auth.json" (docs/cli/#login), under
    // `XDG_DATA_HOME` (packages/core/src/global.ts#L3-L11), so ANY entry is
    // a login. `opencode auth logout [provider]` asks which one to clear.
    CliLogin {
        agent: "opencode",
        cli: Cli::Agent,
        home_var: "XDG_DATA_HOME",
        home: Home::Under(&[".local", "share"]),
        proof: Proof::File {
            witness: &["opencode", "auth.json"],
            holds: Holds::Json(&[]),
        },
        login: LoginRoad::PaneVerb(&["auth", "login"]),
        logout: LogoutRoad::PaneVerb(&["auth", "logout"]),
        shadowed_by: &[],
    },
    // Kilo's own account is a device code — "Kilo Gateway (Device
    // Authorization)", "Open ${verificationUrl} and enter code: ${code}"
    // (kilo-gateway/src/plugin.ts#L15-L48, device-auth-tui.ts#L33) — so the
    // verb is typed where the person can read the code, with `-p kilo`
    // ("provider id or name to log in to (skips provider selection)",
    // kilo.ai/docs/code-with-ai/platforms/cli-reference) so the picker is
    // skipped. It is an opencode fork: `$XDG_DATA_HOME/kilo/auth.json`
    // keyed by provider (packages/core/src/global.ts#L12-L22,
    // packages/opencode/src/auth/index.ts#L11), and this row watches the
    // `kilo` entry its own road writes.
    CliLogin {
        agent: "kilo",
        cli: Cli::Agent,
        home_var: "XDG_DATA_HOME",
        home: Home::Under(&[".local", "share"]),
        proof: Proof::File {
            witness: &["kilo", "auth.json"],
            holds: Holds::Json(&["kilo"]),
        },
        login: LoginRoad::PaneVerb(&["auth", "login", "-p", "kilo"]),
        // Named, the logout asks nothing: the provider argument skips the
        // picker its empty form opens
        // (packages/opencode/src/cli/cmd/providers.ts#L527-L543).
        logout: LogoutRoad::Verb(&["auth", "logout", "kilo"]),
        shadowed_by: &[],
    },
    // Crush signs in with Charm Hyper's device code — "Open this URL and
    // enter the code: %s", and the non-TTY path runs "without requiring any
    // keypresses" (internal/login/login.go#L70-L99) — so the verb is typed
    // where the person can read the code. Its file is the CLI's own state
    // too, so the witness is the token itself: SetProviderAPIKey writes
    // "providers.%s.oauth" (internal/config/store.go#L603-L613) and crush's
    // own "already logged in" check reads "pc.OAuthToken != nil"
    // (internal/cmd/login.go#L77-L86). The file is the global DATA config,
    // "~/.local/share/crush/crush.json" under `CRUSH_GLOBAL_DATA`
    // (internal/config/scope.go#L9-L10, load.go#L1242-L1261). Named and
    // forced, the logout asks nothing: "Skip logout confirmation prompt"
    // (internal/cmd/logout.go).
    CliLogin {
        agent: "crush",
        cli: Cli::Agent,
        home_var: "CRUSH_GLOBAL_DATA",
        home: Home::Under(&[".local", "share", "crush"]),
        proof: Proof::File {
            witness: &["crush.json"],
            holds: Holds::Json(&["providers", "hyper", "oauth"]),
        },
        login: LoginRoad::PaneVerb(&["login"]),
        logout: LogoutRoad::Verb(&["logout", "hyper", "--force"]),
        shadowed_by: &[],
    },
    // Cline's own account is a device code and its CLI refuses to run that
    // flow blind — "OAuth login requires an interactive terminal session"
    // (apps/cli/src/commands/auth.ts#L198-L200) — so `cline auth cline`
    // (the positional shorthand, #L87) is typed into a shell. The login
    // lands in `$CLINE_DATA_DIR/settings/providers.json`
    // (sdk/packages/shared/src/storage/paths.ts#L179-L185,#L429) and the
    // witness is the field Cline itself calls configured:
    // "return !!settings?.auth?.accessToken"
    // (sdk/packages/core/src/auth/provider-auth-registry.ts#L206-L208).
    // The CLI has no logout at all — that lives in the VS Code extension.
    CliLogin {
        agent: "cline",
        cli: Cli::Agent,
        home_var: "CLINE_DATA_DIR",
        home: Home::Under(&[".cline", "data"]),
        proof: Proof::File {
            witness: &["settings", "providers.json"],
            holds: Holds::Json(&["providers", "cline", "settings", "auth", "accessToken"]),
        },
        login: LoginRoad::PaneVerb(&["auth", "cline"]),
        logout: LogoutRoad::None,
        shadowed_by: &[],
    },
    // Hermes's own account is Nous Portal, and its headless verb is
    // `hermes auth add nous --type oauth` — "hermes login has been removed.
    // Use hermes auth to manage OAuth credentials" (cli-commands.md#L663) —
    // which prints "2. If prompted, enter code: {user_code}"
    // (hermes_cli/auth_device_flow.py#L289-L312) and then polls, so the
    // person needs the screen. The store is `$HERMES_HOME/auth.json`
    // (hermes_cli/auth.py#L481-L482) and Nous sits at `providers.nous`
    // (auth_nous.py#L810-L827). Its own `auth status` cannot be the proof:
    // both branches print and return, exit 0 either way
    // (auth_commands.py#L680-L690).
    CliLogin {
        agent: "hermes",
        cli: Cli::Agent,
        home_var: "HERMES_HOME",
        home: Home::Under(&[".hermes"]),
        proof: Proof::File {
            witness: &["auth.json"],
            holds: Holds::Json(&["providers", "nous"]),
        },
        login: LoginRoad::PaneVerb(&["auth", "add", "nous", "--type", "oauth"]),
        logout: LogoutRoad::Verb(&["auth", "logout", "nous"]),
        shadowed_by: &[],
    },
    // MiniMax's `mmx` is not a catalog agent — this window does not start it
    // in a pane — and its OAuth refuses to run blind: "--api-key is required
    // in non-interactive mode." (src/commands/auth/login.ts#L127-L133), so
    // the interactive picker (OAuth global / China / paste a key) runs in a
    // shell. Both credentials live in one file, and they are exclusive: an
    // OAuth login writes "existing.oauth = creds" after deleting `api_key`
    // (src/auth/setup.ts#L91-L102), under `MMX_CONFIG_DIR`
    // (src/config/paths.ts#L6-L13). `mmx auth logout --yes` has no prompt in
    // it at all (src/commands/auth/logout.ts#L19-L48).
    CliLogin {
        agent: "minimax",
        cli: Cli::Own {
            program: "mmx",
            name: "MiniMax CLI",
            homepage_url: "https://github.com/MiniMax-AI/cli",
        },
        home_var: "MMX_CONFIG_DIR",
        home: Home::Under(&[".mmx"]),
        proof: Proof::File {
            witness: &["config.json"],
            holds: Holds::Json(&["oauth"]),
        },
        login: LoginRoad::PaneVerb(&["auth", "login"]),
        logout: LogoutRoad::Verb(&["auth", "logout", "--yes"]),
        shadowed_by: &[],
    },
    // MiMo Code is an opencode fork whose login is a browser round trip that
    // ends in a platform key — "MiMo Code supports direct redirection to the
    // Xiaomi MiMo API Platform for authorization login" — chosen from a
    // provider prompt, so it is typed into a shell. Its data dir is XDG's
    // ("data: path.join(xdgData!, APP)", packages/shared/src/global.ts#L44,
    // APP = "mimocode") with the login under the `xiaomi` key — the same key
    // its own `whoami` reads, "return yield* auth.get("xiaomi")"
    // (packages/opencode/src/cli/cmd/providers.ts#L698). `MIMOCODE_HOME`
    // moves that dir to `<home>/data` instead, which is why the row hands
    // the child `XDG_DATA_HOME` — the road the CLI takes when its own home
    // variable is unset. The logout "always prompts" for a provider
    // (providers.ts#L653-L676).
    CliLogin {
        agent: "mimo-code",
        cli: Cli::Agent,
        home_var: "XDG_DATA_HOME",
        home: Home::Under(&[".local", "share"]),
        proof: Proof::File {
            witness: &["mimocode", "auth.json"],
            holds: Holds::Json(&["xiaomi"]),
        },
        login: LoginRoad::PaneVerb(&["auth", "login"]),
        logout: LogoutRoad::PaneVerb(&["auth", "logout"]),
        shadowed_by: &[],
    },
    // Auggie's browser login uses a localhost callback and can be told to
    // stop before its agent starts: "login  Authenticate with Augment using
    // OAuth", "--headless  Exit after login instead of entering the
    // interactive agent" (`auggie --help`, npm @augmentcode/auggie 0.36.0),
    // leaving "~/.augment/session.json"
    // (docs.augmentcode.com/cli/automation/service-accounts).
    // `auggie logout` "Log out of Augment and remove stored OAuth session".
    // The cache directory is a flag with no variable behind it, so the home
    // this row hands the child is `HOME` itself.
    CliLogin {
        agent: "aug",
        cli: Cli::Agent,
        home_var: "HOME",
        home: Home::Under(&[]),
        proof: Proof::File {
            witness: &[".augment", "session.json"],
            holds: Holds::Json(&[]),
        },
        login: LoginRoad::Verb(&["login", "--headless"]),
        logout: LogoutRoad::Verb(&["logout"]),
        shadowed_by: &[],
    },
    // Autohand is a device code — "To sign in, visit:", "Or enter this code
    // manually:" (src/commands/login.ts#L216-L231) — so the person needs the
    // screen. Its config file is also its state, so the witness is the
    // `auth` block's token: "`token` | string | - | Authentication token for
    // API access" in "`~/.autohand/config.json` (default)" under
    // `AUTOHAND_HOME` "Base directory for all Autohand data"
    // (docs/config-reference.md). Logging out opens a confirmation modal
    // (src/commands/logout.ts#L35-L74), so it too is typed into a shell.
    CliLogin {
        agent: "autohand",
        cli: Cli::Agent,
        home_var: "AUTOHAND_HOME",
        home: Home::Under(&[".autohand"]),
        proof: Proof::File {
            witness: &["config.json"],
            holds: Holds::Json(&["auth", "token"]),
        },
        login: LoginRoad::PaneVerb(&["login"]),
        logout: LogoutRoad::PaneVerb(&["logout"]),
        shadowed_by: &[],
    },
    // Command Code opens a browser and finishes in the terminal — "Click the
    // Authorize button to sign in and return to the terminal after the login
    // successful message appears", and a pasted API key is an alternative
    // answer typed at the same prompt (commandcode.ai/docs/quickstart) — so
    // it runs where the person can answer. It is the survey's one CLI with
    // no home variable at all: "HOME / USERPROFILE  Home directory used to
    // resolve ~/.commandcode" (docs/settings), with the login in
    // "~/.commandcode/auth.json" and `cmd logout` "Remove stored
    // authentication" (docs/reference/cli).
    CliLogin {
        agent: "command-code",
        cli: Cli::Agent,
        home_var: "HOME",
        home: Home::Under(&[]),
        proof: Proof::File {
            witness: &[".commandcode", "auth.json"],
            // The file also carries who and when (`userId`, `userName`,
            // `keyName`, `authenticatedAt`); the key is the credential, and
            // the CLI's own status reads it (npm `command-code` 1.62.1).
            holds: Holds::Json(&["apiKey"]),
        },
        login: LoginRoad::PaneVerb(&["login"]),
        logout: LogoutRoad::Verb(&["logout"]),
        shadowed_by: &[],
    },
    // Ante's headless road reaches one account — "Sign in to an OAuth
    // provider without starting a TUI session. Today the supported provider
    // is `antix`" (docs-site/docs/reference/cli-reference.mdx#L94-L113) —
    // and it opens the browser itself. What it writes is
    // "`~/.ante/auth/*.json`" (storage-reference.mdx, cli-reference.mdx#L241)
    // under `ANTE_HOME`, and the docs never name the file inside that
    // directory; the CLI is not open source, so there is nothing to read.
    // The card opens the road and says it cannot see the result rather than
    // watching a file whose name it guessed.
    CliLogin {
        agent: "ante",
        cli: Cli::Agent,
        home_var: "ANTE_HOME",
        home: Home::Under(&[".ante"]),
        proof: Proof::Unreadable,
        login: LoginRoad::Verb(&["auth", "login", "antix"]),
        logout: LogoutRoad::None,
        shadowed_by: &[],
    },
    // Codebuff prints its URL and waits — "Open this URL in your browser to
    // log in:", "Please open the URL above manually to complete login."
    // (cli/src/login/plain-login.ts#L23-L56) — so the person reads it in a
    // shell. Its store is "`~/.config/manicode/credentials.json`" under
    // `FREEBUFF_CONFIG_DIR` (cli/src/utils/config-dir.ts#L16-L35,
    // utils/auth.ts#L38-L40), and a logout "Only removes the 'default'
    // field, preserving other credentials." (auth.ts#L225) — which is why
    // that field, and not the file, is the witness. The logout itself is the
    // TUI's `/logout` (data/slash-commands.ts#L215-L218).
    CliLogin {
        agent: "codebuff",
        cli: Cli::Agent,
        home_var: "FREEBUFF_CONFIG_DIR",
        home: Home::Under(&[".config", "manicode"]),
        proof: Proof::File {
            witness: &["credentials.json"],
            holds: Holds::Json(&["default"]),
        },
        login: LoginRoad::PaneVerb(&["login"]),
        logout: LogoutRoad::TuiCommand("/logout"),
        shadowed_by: &[],
    },
    /* ---- B. a verb, and a keychain instead of a file --------------------- */
    //
    // Copilot's token goes to the OS keychain — "By default, the CLI stores
    // your OAuth token in your operating system's keychain under the service
    // name `copilot-cli`" — but its own state file names who is signed in:
    // "Application state fields—such as `loggedInUsers`, `installedPlugins`,
    // `firstLaunchAt`, and `staff`—remain in `config.json`"
    // (docs.github.com …/cli-config-dir-reference#configjson), under
    // `COPILOT_HOME` "Override the configuration and state directory.
    // Default: `$HOME/.copilot`." The browser flow is headless here
    // ("--web-flow"), the way out is the TUI's `/logout` ("헤드리스 없음"),
    // and the token variables outrank the stored login — "Use the
    // `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, or `GITHUB_TOKEN` environment
    // variable (in order of precedence)" — so they are blanked for the
    // children this row starts.
    CliLogin {
        agent: "copilot",
        cli: Cli::Agent,
        home_var: "COPILOT_HOME",
        home: Home::Under(&[".copilot"]),
        // Not the `loggedInUsers` field: the docs name it and nothing more —
        // not its type, and not whether `/logout` takes an entry out of it —
        // so reading it could keep saying 「로그인됨」 after somebody left.
        // The keychain is the credential and this window does not open it.
        proof: Proof::Unreadable,
        login: LoginRoad::Verb(&["login", "--web-flow"]),
        logout: LogoutRoad::TuiCommand("/logout"),
        shadowed_by: &["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"],
    },
    // Cursor's CLI keeps its login in the macOS keychain and answers
    // `agent status --format json`. The exit code says nothing — it is 0
    // signed in AND signed out (only an internal error gives 1) — so the
    // verdict is the flag in its JSON: `isAuthenticated`, beside
    // `status: "authenticated" | "partially-authenticated" |
    // "unauthenticated" | "error"` (the shipped `status` command,
    // downloads.cursor.com's agent-cli-package, chunk 8054). `agent login`
    // opens the browser and exits on its own when the poll lands, and
    // `agent logout` asks nothing (chunks 3147, 3586). A caveat the card
    // cannot see: `isAuthenticated` means "tokens are stored", not "tokens
    // work".
    CliLogin {
        agent: "cursor",
        cli: Cli::Agent,
        home_var: "CURSOR_CONFIG_DIR",
        home: Home::Under(&[".cursor"]),
        proof: Proof::Status {
            args: &["status", "--format", "json"],
            says: Says::Json(&["isAuthenticated"]),
        },
        login: LoginRoad::Verb(&["login"]),
        logout: LogoutRoad::Verb(&["logout"]),
        shadowed_by: &[],
    },
    // Kiro's token is not under `KIRO_HOME` (the documented list of what
    // that variable moves does not include it), so the proof is its own
    // `whoami`: "To check which authentication method is active, run
    // kiro-cli whoami." The fields of `--format json` are undocumented, so
    // the row asks for none of them and reads the exit code — the logged-out
    // case is documented as an error ("**Not logged in error**: Login with
    // `kiro-cli login`") and the CLI's exit table gives errors 1 ("| 1 |
    // Failure | General failure (auth error, invalid args, operation failed)
    // |", kiro.dev/docs/reference/exit-codes). Locally the login asks for
    // one keypress and lets the BROWSER pick the provider ("You are
    // prompted to press Enter to complete sign-in in your browser."), so it
    // is typed into a shell.
    CliLogin {
        agent: "kiro",
        cli: Cli::Agent,
        home_var: "KIRO_HOME",
        home: Home::Under(&[".kiro"]),
        proof: Proof::Status {
            args: &["whoami"],
            says: Says::Exit0,
        },
        login: LoginRoad::PaneVerb(&["login"]),
        logout: LogoutRoad::Verb(&["logout"]),
        shadowed_by: &[],
    },
    // TRAE's CLI documents `traecli login status` and never what it prints
    // or exits with, and its credential file is undocumented too — so this
    // window claims nothing about the state. The login itself is a TUI
    // screen ("首次打开 CLI 会在 TUI 内自动引导登录，登录界面支持账号登录和自定义域登录。",
    // docs.trae.cn/cli_get-started-with-trae-code-cli-2), and `traecli
    // logout` is the documented way out (cli_command-line-parameters).
    CliLogin {
        agent: "trae",
        cli: Cli::Agent,
        home_var: "TRAE_HOME",
        home: Home::Under(&[".trae"]),
        proof: Proof::Unreadable,
        login: LoginRoad::PaneVerb(&["login"]),
        logout: LogoutRoad::Verb(&["logout"]),
        shadowed_by: &[],
    },
    // OpenClaw keeps its credentials in SQLite and prints them as JSON:
    // `models auth list --json` writes "{ agentId, agentDir, authStatePath,
    // provider, profiles }" and exits 0 whether or not `profiles` is empty
    // (src/commands/models/auth-list.ts#L160-L179), so the verdict is that
    // array. Its login refuses a pipe outright — "models auth login requires
    // an interactive TTY" (src/commands/models/auth.ts#L1321-L1324) — and
    // without `--provider` it opens a picker (#L1155-L1159), which is the
    // person's choice to make. There is no way out the card can offer:
    // `models auth logout <profileId>` needs an id this window does not know.
    CliLogin {
        agent: "openclaw",
        cli: Cli::Agent,
        home_var: "OPENCLAW_STATE_DIR",
        home: Home::Under(&[".openclaw"]),
        proof: Proof::Status {
            args: &["models", "auth", "list", "--json"],
            says: Says::Json(&["profiles"]),
        },
        login: LoginRoad::PaneVerb(&["models", "auth", "login"]),
        logout: LogoutRoad::None,
        shadowed_by: &[],
    },
    // omp stores its accounts in SQLite (`~/.omp/agent/agent.db`) and has no
    // command that reports what is in it without going to the network:
    // `auth-broker list` is the catalog of providers it KNOWS, `auth-broker
    // status` health-pings a remote broker, and `usage --json` fetches
    // provider usage endpoints (docs/auth-broker-gateway.md#L61-L66,
    // cli/usage-cli.ts#L1100-L1180). So the card opens the road and reads
    // nothing. With no provider, both verbs open a numbered picker
    // (auth-broker-cli.ts#L217-L225,#L421-L457), which is a screen.
    CliLogin {
        agent: "omp",
        cli: Cli::Agent,
        home_var: "PI_CODING_AGENT_DIR",
        home: Home::Under(&[".omp", "agent"]),
        proof: Proof::Unreadable,
        login: LoginRoad::PaneVerb(&["auth-broker", "login"]),
        logout: LogoutRoad::PaneVerb(&["auth-broker", "logout"]),
        shadowed_by: &[],
    },
    // OpenClaude is a Claude Code fork and answers like one: its `auth
    // status` ends in "process.exit(loggedIn ? 0 : 1)"
    // (src/cli/handlers/auth.ts#L318), with the login and logout verbs its
    // own help names. The home is its own variable — "Preferred config
    // directory override. Defaults to ~/.openclaude when unset."
    // (web/src/data/configuration.ts) — and the credential is the keychain
    // item that variable scopes. One trap the row must disarm: that
    // `loggedIn` is also true when an API key variable is set
    // (auth.ts#L243-L244), so the child is handed the same blanked
    // variables the Claude accounts card blanks.
    CliLogin {
        agent: "openclaude",
        cli: Cli::Agent,
        home_var: "OPENCLAUDE_CONFIG_DIR",
        home: Home::Under(&[".openclaude"]),
        proof: Proof::Status {
            args: &["auth", "status"],
            says: Says::Exit0,
        },
        login: LoginRoad::Verb(&["auth", "login"]),
        logout: LogoutRoad::Verb(&["auth", "logout"]),
        shadowed_by: zerocode_core::account::OVERRIDING_AUTH_VARS,
    },
    /* ---- C. a screen of the CLI's own ------------------------------------ */
    //
    // Pi has no login verb — "Use `/login` in interactive mode, then select a
    // provider:" — and keeps what it gets in "`~/.pi/agent/auth.json` and
    // auto-refresh when expired." under `PI_CODING_AGENT_DIR` "Override the
    // config directory; default is `~/.pi/agent`"
    // (packages/coding-agent/docs/providers.md#L17-L27,
    // docs/environment-variables.md#L81). `/logout` "to clear credentials."
    CliLogin {
        agent: "pi",
        cli: Cli::Agent,
        home_var: "PI_CODING_AGENT_DIR",
        home: Home::Under(&[".pi", "agent"]),
        proof: Proof::File {
            witness: &["auth.json"],
            holds: Holds::Json(&[]),
        },
        login: LoginRoad::TuiCommand("/login"),
        logout: LogoutRoad::TuiCommand("/logout"),
        shadowed_by: &[],
    },
    // Prime Agent is Pi's shape with Pi's words — "On first launch, run
    // `/login` to choose a subscription or API-key provider.", "Tokens are
    // stored in `~/.prime/agent/auth.json`", `PRIME_AGENT_CODING_AGENT_DIR`
    // "Override config directory; default is `~/.prime/agent`"
    // (README.md#L73, packages/coding-agent/docs/providers.md#L23,
    // docs/usage.md#L363).
    CliLogin {
        agent: "prime-agent",
        cli: Cli::Agent,
        home_var: "PRIME_AGENT_CODING_AGENT_DIR",
        home: Home::Under(&[".prime", "agent"]),
        proof: Proof::File {
            witness: &["auth.json"],
            holds: Holds::Json(&[]),
        },
        login: LoginRoad::TuiCommand("/login"),
        logout: LogoutRoad::TuiCommand("/logout"),
        shadowed_by: &[],
    },
    // Droid signs in from its own screen — "| `/login` | Sign in to Factory
    // |", "| `/logout` | Sign out of Factory |"
    // (docs.factory.ai/droid-cli/cli-reference) — and its credential is not
    // documented anywhere: the binary's own words are "set
    // FACTORY_DISABLE_KEYRING=1 to store credentials on disk instead.", so
    // the keychain is where it goes by default. This window opens that
    // screen and says plainly that it cannot read the result.
    CliLogin {
        agent: "droid",
        cli: Cli::Agent,
        home_var: "HOME",
        home: Home::Under(&[]),
        proof: Proof::Unreadable,
        login: LoginRoad::TuiCommand("/login"),
        logout: LogoutRoad::TuiCommand("/logout"),
        shadowed_by: &[],
    },
    // goose puts every provider behind one wizard — "Run `goose configure`
    // and select **GitHub Copilot**", "Configuring {} using OAuth device code
    // flow..." (goose-docs.ai/docs/getting-started/providers,
    // crates/goose-cli/src/commands/configure.rs#L446) — and stores what it
    // gets in "the system keyring (keychain on macOS) for storing secrets."
    // (docs/guides/config-files). `GOOSE_PATH_ROOT` moves its files but not
    // the keyring, so this row hands the child no root of its own: pointing
    // goose somewhere else would hide the login from the person's own goose.
    CliLogin {
        agent: "goose",
        cli: Cli::Agent,
        home_var: "HOME",
        home: Home::Under(&[]),
        proof: Proof::Unreadable,
        login: LoginRoad::PaneVerb(&["configure"]),
        logout: LogoutRoad::None,
        shadowed_by: &[],
    },
    // aider offers OpenRouter's PKCE login only when it starts with no model
    // and no key — "Login to OpenRouter or create a free account?",
    // (aider/onboarding.py#L94) — so the road is the bare program, and the
    // key it comes back with is appended to "~/.aider/oauth-keys.env"
    // (aider/main.py#L370, onboarding.py#L363-L365) as
    // `OPENROUTER_API_KEY="…"`. The path is `Path.home()`-bound, so `HOME`
    // is the only knob. No logout is documented.
    CliLogin {
        agent: "aider",
        cli: Cli::Agent,
        home_var: "HOME",
        home: Home::Under(&[]),
        proof: Proof::File {
            witness: &[".aider", "oauth-keys.env"],
            holds: Holds::EnvVar("OPENROUTER_API_KEY"),
        },
        login: LoginRoad::FirstRun,
        logout: LogoutRoad::None,
        shadowed_by: &[],
    },
    // Mistral Vibe's browser sign-in lives in its setup wizard — "You can
    // rerun the setup wizard at any time using vibe --setup ."
    // (docs.mistral.ai/mistral-vibe/introduction) — and since 2.17.0 "API
    // keys are now stored in the OS keyring instead of plain text"
    // (CHANGELOG.md), with the keyring item keyed by variable name rather
    // than by `VIBE_HOME` (vibe/utils/keyring.py#L11-L25). Nothing here to
    // read, and `/whoami` is the CLI's own answer.
    CliLogin {
        agent: "mistral-vibe",
        cli: Cli::Agent,
        home_var: "VIBE_HOME",
        home: Home::Under(&[".vibe"]),
        proof: Proof::Unreadable,
        login: LoginRoad::PaneVerb(&["--setup"]),
        logout: LogoutRoad::None,
        shadowed_by: &[],
    },
    // Gemini CLI is not in the launch catalog — this window does not start
    // it in a pane — so its row carries its own name and page. Its login is
    // its first-run menu or `/auth` inside the TUI: "name: 'signin',
    // altNames: ['login'], description: 'Sign in or change the
    // authentication method'," and out is "name: 'signout', altNames:
    // ['logout']" (packages/cli/src/ui/commands/authCommand.ts#L17-L31).
    // The credential is "`oauth_creds.json`" under `GEMINI_CLI_HOME`'s
    // `.gemini/` (packages/core/src/config/storage.ts#L22,#L256,
    // docs/reference/configuration.md#L2626-L2629).
    CliLogin {
        agent: "gemini",
        cli: Cli::Own {
            program: "gemini",
            name: "Gemini CLI",
            homepage_url: "https://github.com/google-gemini/gemini-cli",
        },
        home_var: "GEMINI_CLI_HOME",
        home: Home::Under(&[]),
        proof: Proof::File {
            witness: &[".gemini", "oauth_creds.json"],
            holds: Holds::Json(&[]),
        },
        login: LoginRoad::TuiCommand("/auth"),
        logout: LogoutRoad::TuiCommand("/auth signout"),
        shadowed_by: &[],
    },
];

pub(crate) fn row(agent: &str) -> Option<&'static CliLogin> {
    CLI_LOGINS.iter().find(|row| row.agent == agent)
}

/// What this window can say about a row's login right now.
///
/// Three answers, not two: a status command that could not be started or
/// did not answer in time has learned NOTHING about the credential, and the
/// settings card says so. Folding that into 「로그아웃됨」 is the mistake the
/// Claude accounts card already paid for once (`accounts::login_alive`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    In,
    Out,
    Unknown,
}

impl Verdict {
    const fn signed_in(self) -> bool {
        matches!(self, Self::In)
    }

    const fn known(self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

impl Cli {
    const fn word(self) -> &'static str {
        match self {
            // A catalog agent opens through the one launch door; a CLI the
            // catalog does not know is opened by typing its line into a
            // plain shell of this window.
            Self::Agent => "agent",
            Self::Own { .. } => "shell",
        }
    }
}

impl CliLogin {
    /// The home this row resolves to on this machine.
    fn home(&self) -> Result<PathBuf, String> {
        self.home_in(
            &|name| std::env::var_os(name).map(PathBuf::from),
            dirs::home_dir,
        )
        .ok_or_else(|| "홈 디렉터리를 찾지 못했습니다".to_string())
    }

    /// [`Self::home`] against a named environment and a named `$HOME`, so a
    /// test can stand a machine of its own beside the developer's.
    fn home_in(
        &self,
        var: &dyn Fn(&str) -> Option<PathBuf>,
        home_dir: impl Fn() -> Option<PathBuf>,
    ) -> Option<PathBuf> {
        match self.home {
            Home::Gauge(resolve) => resolve(),
            Home::Under(segments) => var(self.home_var)
                .filter(|held| !held.as_os_str().is_empty())
                .or_else(|| {
                    let mut home = home_dir()?;
                    home.extend(segments);
                    Some(home)
                }),
        }
    }

    /// The credential file under `home`, for a row proven by one.
    fn witness_in(&self, home: &Path) -> Option<PathBuf> {
        match self.proof {
            Proof::Gauge { witness, .. } => Some(witness(home)),
            Proof::File { witness, .. } => Some(
                witness
                    .iter()
                    .fold(home.to_path_buf(), |path, step| path.join(step)),
            ),
            Proof::Status { .. } | Proof::Unreadable => None,
        }
    }

    /// What in the file IS this row's login, as text — `None` when there is
    /// none there. The runner watches this slice rather than the whole file,
    /// so a CLI writing its unrelated state into the same document is not
    /// read as somebody signing in.
    fn login_in_text(&self, text: &str, now_ms: i64) -> Option<String> {
        match self.proof {
            Proof::Gauge { holds, .. } => holds(text, now_ms).then(|| text.to_string()),
            Proof::File { holds, .. } => holds.slice(text),
            Proof::Status { .. } | Proof::Unreadable => None,
        }
    }

    /// Whether this row's login stands right now: the witness file judged
    /// by the gauge's parser or the file's own shape, or the CLI's status
    /// command. `program` is needed only for the latter; a row proven by a
    /// file answers with no CLI on the machine at all.
    pub(crate) fn verdict(&self, program: Option<&str>, home: &Path, now_ms: i64) -> Verdict {
        match self.proof {
            Proof::Gauge { .. } | Proof::File { .. } => {
                let held = self
                    .witness_in(home)
                    .and_then(|file| std::fs::read_to_string(file).ok())
                    .and_then(|text| self.login_in_text(&text, now_ms));
                if held.is_some() {
                    Verdict::In
                } else {
                    Verdict::Out
                }
            }
            Proof::Status { args, says } => {
                let Some(program) = program else {
                    return Verdict::Unknown;
                };
                ask_status(
                    self.command(program, home).args(args),
                    says,
                    STATUS_DEADLINE,
                )
            }
            Proof::Unreadable => Verdict::Unknown,
        }
    }

    fn account_in(&self, home: &Path, now_ms: i64) -> Option<String> {
        let Proof::Gauge { account, .. } = self.proof else {
            return None;
        };
        let text = std::fs::read_to_string(self.witness_in(home)?).ok()?;
        account(&text, now_ms)
    }

    /// The CLI process this row starts, aimed at one home: the shared
    /// builder, and then the variables that would outrank the stored login
    /// taken off the child's environment.
    fn command(&self, program: &str, home: &Path) -> Command {
        let mut command = command(program, self.home_var, home);
        for name in self.shadowed_by {
            command.env_remove(name);
        }
        command
    }

    /// The name this row looks for on PATH.
    fn program_name(&self) -> Option<&'static str> {
        match self.cli {
            Cli::Agent => zerocode_core::agent_spec(self.agent).map(|spec| spec.detect),
            Cli::Own { program, .. } => Some(program),
        }
    }

    /// The shell line the window types for a road that needs a screen: the
    /// home in the CLI's own variable, the variables that would shadow the
    /// login unset, then the verb — the same environment the headless runner
    /// would have built, spelled for a shell because the person has to see
    /// what the CLI prints.
    ///
    /// A CLI the launch catalog does not know takes this road for its TUI
    /// commands too: there is no agent pane to open, so the window types the
    /// bare program into a plain shell and the person types the row's slash
    /// command at it.
    fn pane_command(&self, program: &str, home: &Path) -> Option<String> {
        let args = self.login.pane_args().or_else(|| {
            matches!(self.cli, Cli::Own { .. })
                .then_some(self.login.tui_command().map(|_| &[] as &[&str]))
                .flatten()
        })?;
        Some(self.shell_line(program, home, args))
    }

    /// The same line for the way OUT, where the logout verb needs a screen
    /// of its own (it asks which provider, or asks twice).
    fn logout_command(&self, program: &str, home: &Path) -> Option<String> {
        let args = self.logout.pane_args().or_else(|| {
            matches!(self.cli, Cli::Own { .. })
                .then_some(self.logout.tui_command().map(|_| &[] as &[&str]))
                .flatten()
        })?;
        Some(self.shell_line(program, home, args))
    }

    fn shell_line(&self, program: &str, home: &Path, args: &[&str]) -> String {
        let mut line = String::new();
        for name in self.shadowed_by {
            line.push_str("env -u ");
            line.push_str(&shell_quote(name));
            line.push(' ');
        }
        line.push_str(&format!(
            "{}={} {}",
            self.home_var,
            shell_quote(&home.to_string_lossy()),
            shell_quote(program)
        ));
        for arg in args {
            line.push(' ');
            line.push_str(&shell_quote(arg));
        }
        line
    }
}

impl Holds {
    /// The part of `text` that IS a login, or `None` when there is none.
    fn slice(self, text: &str) -> Option<String> {
        match self {
            Self::Written => {
                let trimmed = text.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            }
            Self::Json(path) => json_slice(text, path),
            Self::EnvVar(name) => env_slice(text, name),
        }
    }
}

/// The value at `path` in a JSON document, serialized, when it holds
/// something. The empty path asks the document itself.
///
/// "Holds something" is the same question everywhere: `null`, `""`, `[]` and
/// `{}` are a key that exists and says nobody is signed in — which is what
/// `loggedInUsers` looks like after a logout.
fn json_slice(text: &str, path: &[&str]) -> Option<String> {
    let mut value = &serde_json::from_str::<serde_json::Value>(text).ok()?;
    for step in path {
        value = value.get(step)?;
    }
    let empty = match value {
        serde_json::Value::Null => true,
        serde_json::Value::String(said) => said.is_empty(),
        serde_json::Value::Array(rows) => rows.is_empty(),
        serde_json::Value::Object(fields) => fields.is_empty(),
        // `false` is the answer a status command gives when nobody is
        // signed in — `"isAuthenticated": false` is Cursor's own word for
        // it — so a key holding it holds no login.
        serde_json::Value::Bool(said) => !said,
        serde_json::Value::Number(_) => false,
    };
    (!empty).then(|| value.to_string())
}

/// The value a dotenv line assigns `name`, when it is not empty — the shape
/// `aider` appends its OpenRouter key in (`KEY="…"`).
fn env_slice(text: &str, name: &str) -> Option<String> {
    text.lines().rev().find_map(|line| {
        let (said, value) = line.split_once('=')?;
        let said = said.trim().strip_prefix("export ").unwrap_or(said.trim());
        if said != name {
            return None;
        }
        let value = value.trim().trim_matches(['"', '\'']).trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// Ask a CLI's own status command, abandoning it at the deadline.
///
/// Its stdout is read WHILE it runs: a child that fills the pipe buffer
/// never exits, and the deadline below would turn a healthy answer into a
/// kill (`accounts::login_status_in` learned this first). A command that
/// could not be started, was killed at the deadline, or printed something
/// that is not the JSON its row asked for has learned nothing — that is
/// [`Verdict::Unknown`], never 「로그아웃됨」.
fn ask_status(command: &mut Command, says: Says, deadline: Duration) -> Verdict {
    use std::io::Read as _;
    let Ok(mut child) = command.spawn() else {
        return Verdict::Unknown;
    };
    let drained = child.stdout.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut said = Vec::new();
            let _ = pipe.read_to_end(&mut said);
            said
        })
    });
    let until = Instant::now() + deadline;
    // Backing off rather than one long poll: most of these answer in tens of
    // milliseconds (a process start and a file read), and a fixed 250 ms
    // look made every row wait a quarter second for an answer that was
    // already there. The ceiling keeps a slow one cheap to wait on.
    let mut look = Duration::from_millis(5);
    let ended = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < until => {
                std::thread::sleep(look);
                look = (look * 2).min(WITNESS_POLL);
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let said = drained
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let Some(ended) = ended else {
        return Verdict::Unknown;
    };
    match says {
        Says::Exit0 => {
            if ended.success() {
                Verdict::In
            } else {
                Verdict::Out
            }
        }
        Says::Json(path) => {
            let Ok(text) = String::from_utf8(said) else {
                return Verdict::Unknown;
            };
            if serde_json::from_str::<serde_json::Value>(&text).is_err() {
                return Verdict::Unknown;
            }
            if json_slice(&text, path).is_some() {
                Verdict::In
            } else {
                Verdict::Out
            }
        }
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
/// What in a witness's text IS the login, as the runner reads it: the text
/// itself for a file that exists only while a login does, and the value at
/// a key path for one the CLI shares with its own state.
type LoginIn<'a> = Box<dyn Fn(&str) -> Option<String> + 'a>;

pub(crate) struct Witness<'a> {
    file: &'a Path,
    before: Option<String>,
    login: LoginIn<'a>,
}

impl<'a> Witness<'a> {
    /// Snapshot `file` now. `holds` judges the text a login leaves; the
    /// runner calls the login complete only when the file has CHANGED and
    /// `holds` accepts what is there.
    pub(crate) fn taken(file: &'a Path, holds: &'a dyn Fn(&str) -> bool) -> Self {
        Self::sliced(file, Box::new(|text| holds(text).then(|| text.to_string())))
    }

    /// The same snapshot of the LOGIN inside the file rather than of the
    /// whole file: several of these documents carry the CLI's state beside
    /// the credential (`crush.json`, `~/.copilot/config.json`), and a state
    /// write during a login is not somebody signing in.
    fn sliced(file: &'a Path, login: LoginIn<'a>) -> Self {
        let before = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| login(&text));
        Self {
            file,
            before,
            login,
        }
    }

    fn now(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.file).ok()?;
        (self.login)(&text)
    }

    /// Has a login landed since the snapshot?
    fn arrived(&self) -> bool {
        match self.now() {
            Some(held) => self.before.as_deref() != Some(held.as_str()),
            None => false,
        }
    }

    /// Is the file gone, or no longer a login?
    fn departed(&self) -> bool {
        self.now().is_none()
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
    run_login_within(
        command(program, home_var, home),
        args,
        home,
        witness,
        LOGIN_TIMEOUT,
    )
}

/// The same runner behind one ROW, whose own builder also takes the
/// variables that would shadow its login off the child.
fn run_login_with(
    row: &CliLogin,
    program: &str,
    args: &[&str],
    home: &Path,
    witness: Option<Witness<'_>>,
) -> Result<(), String> {
    run_login_within(
        row.command(program, home),
        args,
        home,
        witness,
        LOGIN_TIMEOUT,
    )
}

fn run_login_within(
    mut command: Command,
    args: &[&str],
    home: &Path,
    witness: Option<Witness<'_>>,
    timeout: Duration,
) -> Result<(), String> {
    use std::io::Read;
    let program = command.get_program().to_string_lossy().into_owned();
    std::fs::create_dir_all(home).map_err(|error| error.to_string())?;
    let mut child = command
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
/// answers the way the caller wants. An unanswered ask is not an answer —
/// the wait keeps asking until the ceiling.
#[expect(
    clippy::too_many_arguments,
    reason = "the row, the machine's program and the wait's own three bounds; \
              a struct for one call site would hide them"
)]
fn wait_for_status_within(
    row: &CliLogin,
    program: &str,
    args: &[&str],
    says: Says,
    home: &Path,
    signed_in: bool,
    timeout: Duration,
    poll: Duration,
) -> Result<(), String> {
    wait_until(
        || {
            let standing = ask_status(row.command(program, home).args(args), says, STATUS_DEADLINE);
            standing.known() && standing.signed_in() == signed_in
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
    match row.witness_in(home) {
        Some(file) => {
            let taken = Witness::sliced(&file, Box::new(|text| row.login_in_text(text, now_ms)));
            run_login_with(row, program, args, home, Some(taken))?;
        }
        // A keychain row has no file to watch: the process leaving IS the
        // end of the round trip, and the status command below is the word
        // on whether anybody arrived.
        None => run_login_with(row, program, args, home, None)?,
    }
    if row.verdict(Some(program), home, now_ms) == Verdict::Out {
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
    let args = match row.logout {
        LogoutRoad::Verb(args) => args,
        LogoutRoad::PaneVerb(_) | LogoutRoad::TuiCommand(_) => {
            return Err(format!(
                "{}의 로그아웃은 터미널 판에서 진행합니다",
                row.agent
            ));
        }
        // Several of these CLIs document no logout at all. Removing the
        // file ourselves would take a credential the CLI still expects to
        // find, so the card offers nothing and this says why.
        LogoutRoad::None => {
            return Err(format!(
                "{}의 로그아웃 동사는 공식 문서에 없습니다",
                row.agent
            ));
        }
    };
    let output = row
        .command(program, home)
        .args(args)
        .output()
        .map_err(|error| format!("{program}을(를) 실행할 수 없습니다: {error}"))?;
    if output.status.success() || row.verdict(Some(program), home, now_ms) != Verdict::In {
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
        Proof::Gauge { .. } | Proof::File { .. } => {
            let file = row
                .witness_in(home)
                .ok_or_else(|| "이 행에는 기다릴 자격 증명 파일이 없습니다".to_string())?;
            let taken = Witness::sliced(&file, Box::new(|text| row.login_in_text(text, now_ms)));
            wait_for_witness_within(&taken, signed_in, timeout)
        }
        Proof::Status { args, says } => {
            let program = program
                .ok_or_else(|| format!("이 기계에서 {}을(를) 찾지 못했습니다", row.agent))?;
            wait_for_status_within(
                row,
                program,
                args,
                says,
                home,
                signed_in,
                timeout,
                status_poll,
            )
        }
        // Nothing to watch: the CLI's own screen is where this login lands,
        // and the card says so instead of waiting for an answer that never
        // comes.
        Proof::Unreadable => Err(format!("{}의 로그인은 이 창이 읽지 못합니다", row.agent)),
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
    /// The command this row looks for on PATH, which is not always the id:
    /// `aug` is `auggie` and `mimo-code` is `mimo`, and the row that says
    /// 「PATH에서 찾지 못했습니다」 has to name what it looked for.
    pub(crate) program: &'static str,
    pub(crate) installed: bool,
    pub(crate) signed_in: bool,
    /// Whether this window could read the login AT ALL. False for a row
    /// whose credential is in a keyring with no status command behind it,
    /// and for a status command that did not answer — neither of which is
    /// 「로그아웃됨」.
    pub(crate) known: bool,
    /// `file`, `status` or `none`: what the window asked to get that answer.
    pub(crate) proof: &'static str,
    /// `agent` or `shell`: whether this CLI's own screen is opened through
    /// the launch door, or by typing its line into a plain shell because the
    /// launch catalog does not know it.
    pub(crate) opens: &'static str,
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
    /// The shell line a road that needs a screen types into a plain shell.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pane_command: Option<String>,
    /// `verb`, `pane-verb`, `tui` or `none`.
    pub(crate) logout_road: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tui_logout: Option<&'static str>,
    /// The shell line the way OUT types, for a logout that needs a screen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) logout_command: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct Report {
    pub(crate) rows: Vec<Standing>,
}

/// Every row, as this machine stands. `program_of` is the machine's own
/// detection — the catalog's for a catalog agent, a PATH walk for a CLI it
/// does not know — handed in so a test can name a fake.
///
/// The status rows are asked in PARALLEL and the file rows are not asked at
/// all: one CLI start each, and a table of thirty would otherwise paint the
/// card a CLI at a time. Each ask carries its own deadline
/// ([`STATUS_DEADLINE`]), so the slowest row bounds the list rather than
/// ending it.
pub(crate) fn report(program_of: &dyn Fn(&CliLogin) -> Option<String>, now_ms: i64) -> Report {
    let machine: Vec<(&CliLogin, Option<String>, PathBuf)> = CLI_LOGINS
        .iter()
        .filter_map(|row| {
            let home = row.home().ok()?;
            Some((row, program_of(row), home))
        })
        .collect();
    let asked: Vec<Verdict> = std::thread::scope(|scope| {
        let threads: Vec<_> = machine
            .iter()
            .map(|(row, program, home)| {
                scope.spawn(move || row.verdict(program.as_deref(), home, now_ms))
            })
            .collect();
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap_or(Verdict::Unknown))
            .collect()
    });
    let rows = machine
        .iter()
        .zip(asked)
        .filter_map(|((row, program, home), verdict)| {
            standing(row, program.as_deref(), home, now_ms, verdict)
        })
        .collect();
    Report { rows }
}

fn standing(
    row: &CliLogin,
    program: Option<&str>,
    home: &Path,
    now_ms: i64,
    verdict: Verdict,
) -> Option<Standing> {
    let (name, homepage_url) = match row.cli {
        Cli::Agent => {
            let spec = zerocode_core::agent_spec(row.agent)?;
            (spec.name, spec.homepage_url)
        }
        Cli::Own {
            name, homepage_url, ..
        } => (name, homepage_url),
    };
    let signed_in = verdict.signed_in();
    Some(Standing {
        agent: row.agent,
        name,
        homepage_url,
        program: row.program_name()?,
        installed: program.is_some(),
        signed_in,
        known: verdict.known(),
        proof: match row.proof {
            Proof::Gauge { .. } | Proof::File { .. } => "file",
            Proof::Status { .. } => "status",
            Proof::Unreadable => "none",
        },
        opens: row.cli.word(),
        account: signed_in.then(|| row.account_in(home, now_ms)).flatten(),
        home: with_tilde(home),
        road: row.login.word(),
        tui_login: row.login.tui_command(),
        pane_command: program.and_then(|program| row.pane_command(program, home)),
        logout_road: row.logout.word(),
        tui_logout: row.logout.tui_command(),
        logout_command: program.and_then(|program| row.logout_command(program, home)),
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

    /// One fake CLI per row, spelled from the SURVEY rather than read off the
    /// row it tests: the words the CLI answers to, the home variable it must
    /// be handed, what a login leaves behind and what its status command
    /// says. A row whose verb, home or witness drifts from its evidence stops
    /// matching its fake, and the row's own test goes red naming the words it
    /// was handed instead.
    ///
    /// No real credential anywhere: every value is the word `fixture-token`
    /// and every address is example.com's.
    struct Fake {
        agent: &'static str,
        /// What the login road hands the CLI, joined by spaces — the slash
        /// command for a TUI row, and the empty string for a first-run row.
        login: &'static str,
        /// The same for the way out, where the row has one.
        logout: Option<&'static str>,
        /// The variable the child must carry the home in.
        home_var: &'static str,
        /// The credential file under that home, and a text the row must read
        /// as a login. `None` for a login this window cannot see.
        witness: Option<(&'static str, &'static str)>,
        /// The status command the row asks, and what the CLI answers signed
        /// in and signed out (stdout, and the exit code when signed out).
        status: Option<Status>,
    }

    struct Status {
        args: &'static str,
        signed_in: &'static str,
        signed_out: &'static str,
        out_code: i32,
    }

    const FAKES: &[Fake] = &[
        Fake {
            agent: "grok",
            login: "login",
            logout: Some("logout"),
            home_var: "GROK_HOME",
            witness: Some((
                "auth.json",
                concat!(
                    "{\"https://auth.x.ai\":{\"key\":\"fixture-token\",",
                    "\"email\":\"person@example.com\",\"user_id\":\"u-1\",",
                    "\"expires_at\":\"2099-01-01T00:00:00Z\"}}"
                ),
            )),
            status: None,
        },
        Fake {
            agent: "kimi",
            login: "login",
            logout: Some("/logout"),
            home_var: "KIMI_CODE_HOME",
            witness: Some((
                "credentials/kimi-code.json",
                "{\"access_token\":\"fixture-token\",\"expires_at\":4102444800}",
            )),
            status: None,
        },
        Fake {
            agent: "devin",
            login: "auth login",
            logout: Some("auth logout"),
            home_var: "XDG_DATA_HOME",
            witness: Some((
                "devin/credentials.toml",
                "[credentials]\ntoken = \"fixture-token\"\n",
            )),
            status: None,
        },
        Fake {
            agent: "opencode",
            login: "auth login",
            logout: Some("auth logout"),
            home_var: "XDG_DATA_HOME",
            witness: Some((
                "opencode/auth.json",
                "{\"anthropic\":{\"type\":\"api\",\"key\":\"fixture-token\"}}",
            )),
            status: None,
        },
        Fake {
            agent: "kilo",
            login: "auth login -p kilo",
            logout: Some("auth logout kilo"),
            home_var: "XDG_DATA_HOME",
            witness: Some((
                "kilo/auth.json",
                "{\"kilo\":{\"type\":\"oauth\",\"access\":\"fixture-token\"}}",
            )),
            status: None,
        },
        Fake {
            agent: "crush",
            login: "login",
            logout: Some("logout hyper --force"),
            home_var: "CRUSH_GLOBAL_DATA",
            witness: Some((
                "crush.json",
                concat!(
                    "{\"providers\":{\"hyper\":{\"oauth\":{\"access_token\":\"fixture-token\",",
                    "\"expires_at\":4102444800}}}}"
                ),
            )),
            status: None,
        },
        Fake {
            agent: "cline",
            login: "auth cline",
            logout: None,
            home_var: "CLINE_DATA_DIR",
            witness: Some((
                "settings/providers.json",
                concat!(
                    "{\"version\":1,\"lastUsedProvider\":\"cline\",\"providers\":{\"cline\":",
                    "{\"settings\":{\"auth\":{\"accessToken\":\"workos:fixture-token\"}},",
                    "\"tokenSource\":\"oauth\"}}}"
                ),
            )),
            status: None,
        },
        Fake {
            agent: "hermes",
            login: "auth add nous --type oauth",
            logout: Some("auth logout nous"),
            home_var: "HERMES_HOME",
            witness: Some((
                "auth.json",
                concat!(
                    "{\"version\":1,\"providers\":{\"nous\":{\"access_token\":\"fixture-token\",",
                    "\"expires_at\":4102444800}}}"
                ),
            )),
            status: None,
        },
        Fake {
            agent: "minimax",
            login: "auth login",
            logout: Some("auth logout --yes"),
            home_var: "MMX_CONFIG_DIR",
            witness: Some((
                "config.json",
                concat!(
                    "{\"region\":\"global\",\"oauth\":{\"access_token\":\"fixture-token\",",
                    "\"refresh_token\":\"fixture-token\",\"expires_at\":\"2099-01-01T00:00:00Z\"}}"
                ),
            )),
            status: None,
        },
        Fake {
            agent: "mimo-code",
            login: "auth login",
            logout: Some("auth logout"),
            home_var: "XDG_DATA_HOME",
            witness: Some((
                "mimocode/auth.json",
                "{\"xiaomi\":{\"type\":\"api\",\"key\":\"fixture-token\"}}",
            )),
            status: None,
        },
        Fake {
            agent: "aug",
            login: "login --headless",
            logout: Some("logout"),
            home_var: "HOME",
            witness: Some((
                ".augment/session.json",
                "{\"accessToken\":\"fixture-token\",\"tenantURL\":\"https://example.com/\"}",
            )),
            status: None,
        },
        Fake {
            agent: "autohand",
            login: "login",
            logout: Some("logout"),
            home_var: "AUTOHAND_HOME",
            witness: Some((
                "config.json",
                concat!(
                    "{\"model\":\"fixture\",\"auth\":{\"token\":\"fixture-token\",",
                    "\"user\":\"person@example.com\",\"expiresAt\":4102444800}}"
                ),
            )),
            status: None,
        },
        Fake {
            agent: "command-code",
            login: "login",
            logout: Some("logout"),
            home_var: "HOME",
            witness: Some((
                ".commandcode/auth.json",
                concat!(
                    "{\"apiKey\":\"fixture-token\",\"userId\":\"u-1\",",
                    "\"userName\":\"person\",\"authenticatedAt\":1}"
                ),
            )),
            status: None,
        },
        Fake {
            agent: "ante",
            login: "auth login antix",
            logout: None,
            home_var: "ANTE_HOME",
            witness: None,
            status: None,
        },
        Fake {
            agent: "codebuff",
            login: "login",
            logout: Some("/logout"),
            home_var: "FREEBUFF_CONFIG_DIR",
            witness: Some((
                "credentials.json",
                concat!(
                    "{\"default\":{\"name\":\"person\",\"email\":\"person@example.com\",",
                    "\"authToken\":\"fixture-token\"}}"
                ),
            )),
            status: None,
        },
        Fake {
            agent: "copilot",
            login: "login --web-flow",
            logout: Some("/logout"),
            home_var: "COPILOT_HOME",
            witness: None,
            status: None,
        },
        Fake {
            agent: "cursor",
            login: "login",
            logout: Some("logout"),
            home_var: "CURSOR_CONFIG_DIR",
            witness: None,
            status: Some(Status {
                args: "status --format json",
                signed_in: concat!(
                    "{\"status\":\"authenticated\",\"isAuthenticated\":true,",
                    "\"hasAccessToken\":true,\"hasRefreshToken\":true}"
                ),
                signed_out: concat!(
                    "{\"status\":\"unauthenticated\",\"isAuthenticated\":false,",
                    "\"hasAccessToken\":false,\"message\":\"Not logged in\"}"
                ),
                // Cursor's status exits 0 either way, which is exactly why
                // the row reads its JSON instead.
                out_code: 0,
            }),
        },
        Fake {
            agent: "kiro",
            login: "login",
            logout: Some("logout"),
            home_var: "KIRO_HOME",
            witness: None,
            status: Some(Status {
                args: "whoami",
                signed_in: "Logged in with Builder ID\n",
                signed_out: "Not logged in\n",
                out_code: 1,
            }),
        },
        Fake {
            agent: "trae",
            login: "login",
            logout: Some("logout"),
            home_var: "TRAE_HOME",
            witness: None,
            status: None,
        },
        Fake {
            agent: "openclaw",
            login: "models auth login",
            logout: None,
            home_var: "OPENCLAW_STATE_DIR",
            witness: None,
            status: Some(Status {
                args: "models auth list --json",
                signed_in: concat!(
                    "{\"agentId\":\"main\",\"provider\":null,\"profiles\":[",
                    "{\"id\":\"openai:default\",\"provider\":\"openai\",\"type\":\"oauth\",",
                    "\"label\":\"ChatGPT\"}]}"
                ),
                signed_out: "{\"agentId\":\"main\",\"provider\":null,\"profiles\":[]}",
                out_code: 0,
            }),
        },
        Fake {
            agent: "omp",
            login: "auth-broker login",
            logout: Some("auth-broker logout"),
            home_var: "PI_CODING_AGENT_DIR",
            witness: None,
            status: None,
        },
        Fake {
            agent: "openclaude",
            login: "auth login",
            logout: Some("auth logout"),
            home_var: "OPENCLAUDE_CONFIG_DIR",
            witness: None,
            status: Some(Status {
                args: "auth status",
                signed_in: "{\"loggedIn\":true,\"authMethod\":\"claude.ai\"}\n",
                signed_out: "{\"loggedIn\":false,\"authMethod\":\"none\"}\n",
                out_code: 1,
            }),
        },
        Fake {
            agent: "pi",
            login: "/login",
            logout: Some("/logout"),
            home_var: "PI_CODING_AGENT_DIR",
            witness: Some((
                "auth.json",
                concat!(
                    "{\"anthropic\":{\"type\":\"oauth\",\"access\":\"fixture-token\",",
                    "\"refresh\":\"fixture-token\",\"expires\":4102444800000}}"
                ),
            )),
            status: None,
        },
        Fake {
            agent: "prime-agent",
            login: "/login",
            logout: Some("/logout"),
            home_var: "PRIME_AGENT_CODING_AGENT_DIR",
            witness: Some((
                "auth.json",
                "{\"prime-inference\":{\"type\":\"api_key\",\"key\":\"fixture-token\"}}",
            )),
            status: None,
        },
        Fake {
            agent: "droid",
            login: "/login",
            logout: Some("/logout"),
            home_var: "HOME",
            witness: None,
            status: None,
        },
        Fake {
            agent: "goose",
            login: "configure",
            logout: None,
            home_var: "HOME",
            witness: None,
            status: None,
        },
        Fake {
            agent: "aider",
            login: "",
            logout: None,
            home_var: "HOME",
            witness: Some((
                ".aider/oauth-keys.env",
                "OPENROUTER_API_KEY=\"fixture-token\"\n",
            )),
            status: None,
        },
        Fake {
            agent: "mistral-vibe",
            login: "--setup",
            logout: None,
            home_var: "VIBE_HOME",
            witness: None,
            status: None,
        },
        Fake {
            agent: "gemini",
            login: "/auth",
            logout: Some("/auth signout"),
            home_var: "GEMINI_CLI_HOME",
            witness: Some((
                ".gemini/oauth_creds.json",
                concat!(
                    "{\"access_token\":\"fixture-token\",\"refresh_token\":\"fixture-token\",",
                    "\"token_type\":\"Bearer\",\"expiry_date\":4102444800000}"
                ),
            )),
            status: None,
        },
    ];

    const NOW_MS: i64 = 1_790_000_000_000;

    fn fake(agent: &str) -> &'static Fake {
        FAKES
            .iter()
            .find(|fake| fake.agent == agent)
            .unwrap_or_else(|| panic!("no fake CLI for `{agent}` — add one to FAKES"))
    }

    /// How long a test waits for the other half of a road, and how often it
    /// looks. The work is a file write on the same disk and a `/bin/sh` that
    /// does it, so a road lands in milliseconds when the machine is idle —
    /// and the ceiling is generous because it is only ever paid when
    /// something is broken: this suite runs every test at once, and under
    /// that load a process start alone took seconds.
    const WAIT: Duration = Duration::from_secs(30);
    const POLL: Duration = Duration::from_millis(100);

    /// A row with its home pointed inside `root`, so no test reads the
    /// developer's own `~/.grok` or `~/.kimi-code`.
    fn sandboxed_home(row: &CliLogin, root: &Path) -> PathBuf {
        let home = root.join("homes").join(row.agent);
        // The witness's own directory too: several CLIs keep their file one
        // or two levels down, and a login writes into a directory the CLI
        // made.
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

    /// The fake CLI for one row: it answers ONLY the words the survey says
    /// that CLI answers, only in the home the row is supposed to hand it,
    /// and it records every call so a road that types the wrong thing is
    /// caught even when nothing was supposed to change.
    fn fake_cli_for(fake: &Fake, root: &Path, home: &Path) -> String {
        let home = home.to_string_lossy().into_owned();
        let mut body = format!(
            "home=\"${{{var}}}\"\n\
             [ \"$home\" = '{home}' ] || {{ echo \"wrong {var}: $home\" >&2; exit 8; }}\n\
             mkdir -p \"$home\"\n\
             printf '%s\\n' \"$*\" >> \"$home/.called\"\n\
             case \"$*\" in\n",
            var = fake.home_var,
        );
        let landing = match fake.witness {
            Some((file, text)) => format!(
                "mkdir -p \"$(dirname \"$home/{file}\")\"; printf '%s' '{text}' > \"$home/{file}\";"
            ),
            None => String::new(),
        };
        let removal = match fake.witness {
            Some((file, _)) => format!("rm -f \"$home/{file}\";"),
            None => String::new(),
        };
        body.push_str(&format!(
            "  '{login}') {landing} : > \"$home/.keychain\"; exit 0;;\n",
            login = fake.login,
        ));
        if let Some(logout) = fake.logout {
            body.push_str(&format!(
                "  '{logout}') {removal} rm -f \"$home/.keychain\"; exit 0;;\n"
            ));
        }
        if let Some(status) = &fake.status {
            body.push_str(&format!(
                "  '{args}') if [ -f \"$home/.keychain\" ]; then printf '%s' '{signed_in}'; exit 0; \
                 else printf '%s' '{signed_out}'; exit {code}; fi;;\n",
                args = status.args,
                signed_in = status.signed_in,
                signed_out = status.signed_out,
                code = status.out_code,
            ));
        }
        body.push_str("  *) echo \"unexpected: $*\" >&2; exit 9;;\nesac");
        fake_cli(root, fake.agent, &body)
    }

    /// Every call the fake CLI has taken, one line each.
    fn calls(home: &Path) -> Vec<String> {
        std::fs::read_to_string(home.join(".called"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Run one shell line the way a person's terminal would.
    fn typed(line: &str) -> std::process::Output {
        crate::proc::quiet_command("/bin/sh")
            .arg("-c")
            .arg(line)
            .output()
            .expect("the shell")
    }

    /// The same on a thread of its own, held until `go` exists — the person
    /// finishing at their screen while the backend is already watching.
    ///
    /// Held rather than merely delayed, because the ORDER is the thing under
    /// test: a watch judges a login by what changed since it started
    /// looking, so a fake CLI that wrote its credential before the watch
    /// began would leave nothing to notice. The test writes `go` as its last
    /// act before the wait, and this waits for it and then a breath more.
    fn typed_when(go: PathBuf, line: String) -> std::thread::JoinHandle<std::process::Output> {
        std::thread::spawn(move || {
            while !go.exists() {
                std::thread::sleep(Duration::from_millis(5));
            }
            std::thread::sleep(Duration::from_millis(150));
            typed(&line)
        })
    }

    fn since(stamp: &Path) -> Duration {
        let modified = std::fs::metadata(stamp)
            .and_then(|meta| meta.modified())
            .expect("the stamp's mtime");
        std::time::SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default()
    }

    /// What a row says about a machine where nobody has signed in.
    fn nobody(row: &CliLogin) -> Verdict {
        if matches!(row.proof, Proof::Unreadable) {
            Verdict::Unknown
        } else {
            Verdict::Out
        }
    }

    /// The line the TUI roads' half is played with: the person's own CLI,
    /// in its own home, told the row's slash command.
    fn tui_line(row: &CliLogin, cli: &str, home: &Path, command: &str) -> String {
        format!(
            "{}={} {} {}",
            row.home_var,
            shell_quote(&home.to_string_lossy()),
            shell_quote(cli),
            command
        )
    }

    /// One row's road, walked the way the window walks it, against a fake
    /// CLI that answers only that row's own words.
    ///
    /// This is the test every row has: the window's half (the headless verb,
    /// or the line it types into a shell) and the person's half (the CLI
    /// writing its credential), and between them the proof flipping — or,
    /// for a login kept where this window may not look, the road opening and
    /// the card saying plainly that it cannot see the result.
    fn drive_the_road(agent: &str) {
        let root = tempfile::tempdir().expect("sandbox");
        let row = row(agent).unwrap_or_else(|| panic!("`{agent}` is not a row of CLI_LOGINS"));
        let fake = fake(agent);
        assert_eq!(
            row.home_var, fake.home_var,
            "{agent}: the row hands its child another home variable"
        );
        let home = sandboxed_home(row, root.path());
        let cli = fake_cli_for(fake, root.path(), &home);
        let readable = !matches!(row.proof, Proof::Unreadable);
        let go = root.path().join("go-ahead");

        assert_eq!(
            row.verdict(Some(&cli), &home, NOW_MS),
            nobody(row),
            "{agent}: a home nobody has signed into did not read as one"
        );

        // ---- in ----
        match row.login {
            LoginRoad::Verb(args) => {
                assert_eq!(
                    args.join(" "),
                    fake.login,
                    "{agent}: the headless verb is not the one its evidence spells"
                );
                login_headless_in(row, &cli, &home, NOW_MS)
                    .unwrap_or_else(|why| panic!("{agent}: the login: {why}"));
            }
            LoginRoad::PaneVerb(args) => {
                assert_eq!(
                    args.join(" "),
                    fake.login,
                    "{agent}: the verb typed into the shell is not the one its evidence spells"
                );
                let line = row
                    .pane_command(&cli, &home)
                    .unwrap_or_else(|| panic!("{agent}: a pane row with no line to type"));
                if readable {
                    let person = typed_when(go.clone(), line);
                    std::fs::write(&go, b"").expect("the go-ahead");
                    watch_in(row, Some(&cli), &home, true, NOW_MS, WAIT, POLL)
                        .unwrap_or_else(|why| panic!("{agent}: the login never landed: {why}"));
                    let said = person.join().expect("the shell");
                    assert!(
                        said.status.success(),
                        "{agent}: the CLI refused its own line"
                    );
                    std::fs::remove_file(&go).expect("the go-ahead");
                } else {
                    let said = typed(&line);
                    assert!(
                        said.status.success(),
                        "{agent}: the CLI refused its own line"
                    );
                }
            }
            LoginRoad::TuiCommand(command) => {
                assert_eq!(
                    command, fake.login,
                    "{agent}: the slash command is not the one its evidence spells"
                );
                let line = tui_line(row, &cli, &home, command);
                if readable {
                    let person = typed_when(go.clone(), line);
                    std::fs::write(&go, b"").expect("the go-ahead");
                    watch_in(row, Some(&cli), &home, true, NOW_MS, WAIT, POLL)
                        .unwrap_or_else(|why| panic!("{agent}: the login never landed: {why}"));
                    assert!(person.join().expect("the shell").status.success());
                    std::fs::remove_file(&go).expect("the go-ahead");
                } else {
                    assert!(typed(&line).status.success());
                }
            }
            LoginRoad::FirstRun => {
                assert_eq!(
                    "", fake.login,
                    "{agent}: a first-run row whose CLI expects words"
                );
                let line = tui_line(row, &cli, &home, "");
                let person = typed_when(go.clone(), line);
                std::fs::write(&go, b"").expect("the go-ahead");
                watch_in(row, Some(&cli), &home, true, NOW_MS, WAIT, POLL)
                    .unwrap_or_else(|why| panic!("{agent}: the login never landed: {why}"));
                assert!(person.join().expect("the shell").status.success());
                std::fs::remove_file(&go).expect("the go-ahead");
            }
        }
        let taken = calls(&home);
        assert!(
            taken.iter().any(|said| said == fake.login),
            "{agent}: the CLI was never handed its own login words; it was handed {taken:?}"
        );
        let known: Vec<&str> = [
            Some(fake.login),
            fake.logout,
            fake.status.as_ref().map(|s| s.args),
        ]
        .into_iter()
        .flatten()
        .collect();
        for said in &taken {
            assert!(
                known.contains(&said.as_str()),
                "{agent}: the CLI was handed words its row does not carry: {said:?}"
            );
        }
        assert_eq!(
            row.verdict(Some(&cli), &home, NOW_MS),
            if readable {
                Verdict::In
            } else {
                Verdict::Unknown
            },
            "{agent}: the login did not change what the row says"
        );
        if !readable {
            assert!(
                watch_in(row, Some(&cli), &home, true, NOW_MS, POLL, POLL)
                    .is_err_and(|why| why.contains("읽지 못합니다")),
                "{agent}: a login this window cannot read was waited on anyway"
            );
        }

        // ---- and out ----
        match row.logout {
            LogoutRoad::Verb(args) => {
                assert_eq!(
                    args.join(" "),
                    fake.logout.unwrap_or_default(),
                    "{agent}: the logout verb is not the one its evidence spells"
                );
                logout_headless_in(row, &cli, &home, NOW_MS)
                    .unwrap_or_else(|why| panic!("{agent}: the logout: {why}"));
            }
            LogoutRoad::PaneVerb(args) => {
                assert_eq!(args.join(" "), fake.logout.unwrap_or_default());
                let line = row
                    .logout_command(&cli, &home)
                    .unwrap_or_else(|| panic!("{agent}: a pane logout with no line to type"));
                if readable {
                    let person = typed_when(go.clone(), line);
                    std::fs::write(&go, b"").expect("the go-ahead");
                    watch_in(row, Some(&cli), &home, false, NOW_MS, WAIT, POLL)
                        .unwrap_or_else(|why| panic!("{agent}: the logout never landed: {why}"));
                    assert!(person.join().expect("the shell").status.success());
                    std::fs::remove_file(&go).expect("the go-ahead");
                } else {
                    assert!(typed(&line).status.success());
                }
            }
            LogoutRoad::TuiCommand(command) => {
                assert_eq!(command, fake.logout.unwrap_or_default());
                let line = tui_line(row, &cli, &home, command);
                if readable {
                    let person = typed_when(go.clone(), line);
                    std::fs::write(&go, b"").expect("the go-ahead");
                    watch_in(row, Some(&cli), &home, false, NOW_MS, WAIT, POLL)
                        .unwrap_or_else(|why| panic!("{agent}: the logout never landed: {why}"));
                    assert!(person.join().expect("the shell").status.success());
                    std::fs::remove_file(&go).expect("the go-ahead");
                } else {
                    assert!(typed(&line).status.success());
                }
            }
            LogoutRoad::None => {
                assert!(
                    fake.logout.is_none(),
                    "{agent}: its CLI has a way out and the row does not carry it"
                );
                assert!(
                    logout_headless_in(row, &cli, &home, NOW_MS)
                        .is_err_and(|why| why.contains("로그아웃")),
                    "{agent}: a row with no documented logout ran one anyway"
                );
                return;
            }
        }
        assert_eq!(
            row.verdict(Some(&cli), &home, NOW_MS),
            nobody(row),
            "{agent}: the logout did not change what the row says"
        );
    }

    /// One test per row, so a provider that drifts from its evidence is
    /// named by the failure rather than found inside a loop.
    macro_rules! road_tests {
        ($($name:ident => $agent:literal),* $(,)?) => {
            $(
                #[test]
                fn $name() {
                    drive_the_road($agent);
                }
            )*
        };
    }

    road_tests! {
        the_grok_row_walks_its_own_road => "grok",
        the_kimi_row_walks_its_own_road => "kimi",
        the_devin_row_walks_its_own_road => "devin",
        the_opencode_row_walks_its_own_road => "opencode",
        the_kilo_row_walks_its_own_road => "kilo",
        the_crush_row_walks_its_own_road => "crush",
        the_cline_row_walks_its_own_road => "cline",
        the_hermes_row_walks_its_own_road => "hermes",
        the_minimax_row_walks_its_own_road => "minimax",
        the_mimo_code_row_walks_its_own_road => "mimo-code",
        the_aug_row_walks_its_own_road => "aug",
        the_autohand_row_walks_its_own_road => "autohand",
        the_command_code_row_walks_its_own_road => "command-code",
        the_ante_row_walks_its_own_road => "ante",
        the_codebuff_row_walks_its_own_road => "codebuff",
        the_copilot_row_walks_its_own_road => "copilot",
        the_cursor_row_walks_its_own_road => "cursor",
        the_kiro_row_walks_its_own_road => "kiro",
        the_trae_row_walks_its_own_road => "trae",
        the_openclaw_row_walks_its_own_road => "openclaw",
        the_omp_row_walks_its_own_road => "omp",
        the_openclaude_row_walks_its_own_road => "openclaude",
        the_pi_row_walks_its_own_road => "pi",
        the_prime_agent_row_walks_its_own_road => "prime-agent",
        the_droid_row_walks_its_own_road => "droid",
        the_goose_row_walks_its_own_road => "goose",
        the_aider_row_walks_its_own_road => "aider",
        the_mistral_vibe_row_walks_its_own_road => "mistral-vibe",
        the_gemini_row_walks_its_own_road => "gemini",
    }

    /* ---- the table against the survey it came from ----------------------- */

    /// Every id the survey (t-6009) found an OAuth login for, in its own
    /// three groups. The table below is this list minus the three whose
    /// login already has a card of its own on the same screen, minus the one
    /// whose storage the survey could not confirm.
    const SURVEYED_OAUTH: &[&str] = &[
        // A: a headless verb, a home variable and a file.
        "codex",
        "grok",
        "kimi",
        "devin",
        "opencode",
        "kilo",
        "crush",
        "cline",
        "aug",
        "autohand",
        "command-code",
        "codebuff",
        "hermes",
        "ante",
        "minimax",
        "mimo-code",
        // B: a verb, and a keychain or a database instead of a file.
        "claude",
        "copilot",
        "cursor",
        "kiro",
        "trae",
        "openclaw",
        "omp",
        "openclaude",
        // C: a screen of the CLI's own.
        "gemini",
        "antigravity",
        "pi",
        "prime-agent",
        "droid",
        "goose",
        "aider",
        "mistral-vibe",
    ];

    /// The three whose road is another card on this same screen — the Claude
    /// accounts card, the Codex accounts card, and the window's own Google
    /// login (whose credential is what the Antigravity gauge reads; `agy`'s
    /// keyring session is a different login).
    const OWN_CARDS: &[&str] = &["claude", "codex", "antigravity"];

    /// What the survey found NO login road for, spelled here so a future row
    /// cannot quietly claim one: the API-key providers, this window's own
    /// harness, and `amp`, whose storage is unconfirmed.
    const NOT_A_ROW: &[&str] = &[
        "amp",
        "glm",
        "mimo",
        "opencode-go",
        "continue",
        "qwen-code",
        "rovo",
        "zo",
    ];

    #[test]
    fn the_table_is_the_surveys_oauth_rows_minus_the_ones_with_a_card_of_their_own() {
        let ids: Vec<&str> = CLI_LOGINS.iter().map(|row| row.agent).collect();
        let mut seen = std::collections::BTreeSet::new();
        for id in &ids {
            assert!(seen.insert(*id), "`{id}` is in the table twice");
        }
        for id in &ids {
            assert!(
                SURVEYED_OAUTH.contains(id),
                "`{id}` is a row the survey never found an OAuth login for"
            );
            assert!(
                !OWN_CARDS.contains(id),
                "`{id}` already has a card of its own"
            );
            assert!(!NOT_A_ROW.contains(id), "`{id}` has no login road to walk");
        }
        let owed: Vec<&&str> = SURVEYED_OAUTH
            .iter()
            .filter(|id| !OWN_CARDS.contains(id) && !seen.contains(*id))
            .collect();
        assert!(
            owed.is_empty(),
            "the survey found these OAuth logins and the table does not carry them: {owed:?}"
        );
        // One fake CLI per row, and no fake without a row.
        let faked: std::collections::BTreeSet<&str> = FAKES.iter().map(|fake| fake.agent).collect();
        assert_eq!(
            seen, faked,
            "a row without a fake CLI, or a fake without a row"
        );
    }

    #[test]
    fn a_row_names_a_catalog_agent_or_carries_the_three_facts_the_catalog_would_have() {
        for row in CLI_LOGINS {
            match row.cli {
                Cli::Agent => {
                    let spec = zerocode_core::agent_spec(row.agent)
                        .unwrap_or_else(|| panic!("`{}` is not a catalog agent", row.agent));
                    assert_eq!(
                        row.program_name(),
                        Some(spec.detect),
                        "{}: the row looks for a name the catalog does not detect",
                        row.agent
                    );
                }
                Cli::Own {
                    program,
                    name,
                    homepage_url,
                } => {
                    assert!(
                        zerocode_core::agent_spec(row.agent).is_none(),
                        "{}: a catalog agent carrying its own name as well",
                        row.agent
                    );
                    assert!(!program.is_empty() && !name.is_empty());
                    assert!(
                        homepage_url.starts_with("https://"),
                        "{}: a vendor page that is not one",
                        row.agent
                    );
                }
            }
        }
    }

    /// The window reads a login; it never keeps one. Every fixture in this
    /// file is the word `fixture-token` and every address is example.com's,
    /// and the settings row a person's machine produces carries neither.
    #[test]
    fn no_row_of_the_card_carries_a_credential() {
        let root = tempfile::tempdir().expect("sandbox");
        let mut rows = Vec::new();
        for row in CLI_LOGINS {
            let fake = fake(row.agent);
            let home = sandboxed_home(row, root.path());
            if let (Some(file), Some((_, text))) = (row.witness_in(&home), fake.witness) {
                std::fs::write(&file, text).expect("a login");
            }
            rows.push(
                standing(row, None, &home, NOW_MS, row.verdict(None, &home, NOW_MS))
                    .expect("a standing"),
            );
        }
        for row in &rows {
            let json = serde_json::to_string(row).expect("json");
            assert!(
                !json.contains("fixture-token") && !json.contains("access_token"),
                "a credential reached the settings row: {json}"
            );
            assert!(row.home.starts_with('~') || row.home.starts_with('/'));
        }
        // The one row whose witness names a person names them and nothing
        // else — an address, never a token.
        let grok = rows.iter().find(|row| row.agent == "grok").expect("grok");
        assert_eq!(grok.account.as_deref(), Some("person@example.com"));
    }

    /* ---- the runner ------------------------------------------------------ */

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
        let (_, text) = fake("grok").witness.expect("a grok witness");
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
                text,
                witness.display(),
                wrote_at.display(),
            ),
        );
        let began = Instant::now();
        login_headless_in(grok, &cli, &home, NOW_MS).expect("the login");
        let took = began.elapsed();
        let since_write = since(&wrote_at);
        assert_eq!(
            grok.verdict(None, &home, NOW_MS),
            Verdict::In,
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
        // And a row whose CLI documents no way out says THAT, rather than
        // sending a person to a pane to type a verb that does not exist.
        let cline = row("cline").expect("cline row");
        let cline_home = sandboxed_home(cline, root.path());
        assert_eq!(
            logout_headless_in(cline, &quiet, &cline_home, NOW_MS),
            Err("cline의 로그아웃 동사는 공식 문서에 없습니다".to_string())
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
        let (_, text) = fake("kimi").witness.expect("a kimi witness");

        let writer_witness = witness.clone();
        let wrote_at = root.path().join("wrote-at");
        let stamp = wrote_at.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            std::fs::write(&writer_witness, text).expect("write");
            std::fs::write(&stamp, b"").expect("stamp");
        });
        watch_in(kimi, None, &home, true, NOW_MS, WAIT, STATUS_POLL).expect("the login landed");
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
        watch_in(kimi, None, &home, false, NOW_MS, WAIT, STATUS_POLL).expect("the logout landed");

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

    /// The witness must CHANGE — and what must change is the LOGIN inside
    /// it, not the file. Several of these CLIs keep their state in the same
    /// document, and a state write during a login is not somebody signing in.
    #[test]
    fn a_witness_that_already_held_a_login_has_to_change_and_state_beside_it_does_not_count() {
        let root = tempfile::tempdir().expect("sandbox");
        let grok = row("grok").expect("grok row");
        let (_, witness) = sandboxed_places(grok, root.path());
        let (_, text) = fake("grok").witness.expect("a grok witness");
        std::fs::write(&witness, text).expect("held login");
        let taken = Witness::sliced(&witness, Box::new(|said| grok.login_in_text(said, NOW_MS)));
        assert!(
            !taken.arrived(),
            "an unchanged witness counted as a new login"
        );
        let rewritten = text.replace("u-1", "u-2");
        std::fs::write(&witness, rewritten).expect("relogin");
        assert!(taken.arrived());
        // A rewrite that is not a login is not an arrival either.
        std::fs::write(&witness, "{}").expect("emptied");
        assert!(!taken.arrived());
        assert!(taken.departed());

        // Crush keeps its state in the file its token lives in: a write that
        // leaves `providers.hyper.oauth` exactly as it was is not a login.
        let crush = row("crush").expect("crush row");
        let (_, shared) = sandboxed_places(crush, root.path());
        let (_, held) = fake("crush").witness.expect("a crush witness");
        std::fs::write(&shared, held).expect("held login");
        let watching = Witness::sliced(&shared, Box::new(|said| crush.login_in_text(said, NOW_MS)));
        let mut state: serde_json::Value = serde_json::from_str(held).expect("json");
        state["lastUsedModel"] = serde_json::json!("fixture");
        std::fs::write(&shared, state.to_string()).expect("a state write");
        assert!(
            !watching.arrived(),
            "an unrelated write to the same file counted as a login"
        );
        state["providers"]["hyper"]["oauth"]["access_token"] = serde_json::json!("fixture-token-2");
        std::fs::write(&shared, state.to_string()).expect("a second login");
        assert!(
            watching.arrived(),
            "a new token was not read as a new login"
        );
    }

    /// Logging out is the CLI's verb, judged by the proof.
    #[test]
    fn a_headless_logout_is_the_clis_verb_and_the_witness_is_the_verdict() {
        let root = tempfile::tempdir().expect("sandbox");
        let grok = row("grok").expect("grok row");
        let (home, witness) = sandboxed_places(grok, root.path());
        let (_, text) = fake("grok").witness.expect("a grok witness");
        let file_name = witness
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        // The verb removes the file: signed out.
        std::fs::write(&witness, text).expect("login");
        let removing = fake_cli(
            root.path(),
            "grok-out",
            &format!(
                "[ \"$1\" = logout ] || exit 9\nrm -f \"${}/{file_name}\"",
                grok.home_var
            ),
        );
        logout_headless_in(grok, &removing, &home, NOW_MS).expect("logout");
        assert_eq!(grok.verdict(None, &home, NOW_MS), Verdict::Out);
        // The verb fails but the login is already gone: still signed out.
        let failing = fake_cli(root.path(), "grok-fail", "echo 'not logged in' >&2\nexit 1");
        logout_headless_in(grok, &failing, &home, NOW_MS).expect("nothing to log out of");
        // The verb fails and the login stands: the CLI's word is the error.
        std::fs::write(&witness, text).expect("login again");
        assert_eq!(
            logout_headless_in(grok, &failing, &home, NOW_MS),
            Err("not logged in".to_string())
        );
        assert_eq!(
            grok.verdict(None, &home, NOW_MS),
            Verdict::In,
            "a failed logout removed the file itself"
        );
    }

    /// A status row's three answers. Cursor's `status` exits 0 whether or not
    /// anybody is signed in, which is exactly why the row reads its JSON —
    /// and a CLI that cannot be asked at all says nothing, which is neither
    /// 「로그인됨」 nor 「로그아웃됨」.
    #[test]
    fn a_status_row_reads_the_clis_answer_and_an_unanswered_ask_says_nothing() {
        let root = tempfile::tempdir().expect("sandbox");
        let cursor = row("cursor").expect("cursor row");
        let home = sandboxed_home(cursor, root.path());
        let cli = fake_cli_for(fake("cursor"), root.path(), &home);
        assert_eq!(cursor.verdict(Some(&cli), &home, NOW_MS), Verdict::Out);
        assert_eq!(
            cursor.verdict(None, &home, NOW_MS),
            Verdict::Unknown,
            "a status row answered with no CLI to ask"
        );
        login_headless_in(cursor, &cli, &home, NOW_MS).expect("the login");
        let began = Instant::now();
        let standing = cursor.verdict(Some(&cli), &home, NOW_MS);
        eprintln!("measured: one status ask took {:?}", began.elapsed());
        assert_eq!(standing, Verdict::In);

        // A CLI that says nothing at all, and one that is not there: both
        // unknown.
        let mute = fake_cli(root.path(), "mute", "exit 0");
        assert_eq!(cursor.verdict(Some(&mute), &home, NOW_MS), Verdict::Unknown);
        let missing = root
            .path()
            .join("no-such-cli")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            cursor.verdict(Some(&missing), &home, NOW_MS),
            Verdict::Unknown
        );
        // One that hangs is abandoned at its deadline rather than holding
        // the card.
        let hanging = fake_cli(root.path(), "hanging", "sleep 30");
        let began = Instant::now();
        let said = ask_status(
            &mut crate::proc::quiet_command(&hanging),
            Says::Exit0,
            Duration::from_millis(300),
        );
        let waited = began.elapsed();
        assert_eq!(said, Verdict::Unknown);
        assert!(
            waited < Duration::from_secs(2),
            "a hung status held on for {waited:?}"
        );

        // The other shape: an exit code, which is what OpenClaude answers
        // with.
        let openclaude = row("openclaude").expect("openclaude row");
        let its_home = sandboxed_home(openclaude, root.path());
        let its_cli = fake_cli_for(fake("openclaude"), root.path(), &its_home);
        assert_eq!(
            openclaude.verdict(Some(&its_cli), &its_home, NOW_MS),
            Verdict::Out
        );
        login_headless_in(openclaude, &its_cli, &its_home, NOW_MS).expect("the login");
        assert_eq!(
            openclaude.verdict(Some(&its_cli), &its_home, NOW_MS),
            Verdict::In
        );
        // The status watch, against the command rather than a file.
        let logging_out = tui_line(openclaude, &its_cli, &its_home, "auth logout");
        let go = root.path().join("go-ahead");
        let person = typed_when(go.clone(), logging_out);
        std::fs::write(&go, b"").expect("the go-ahead");
        let began = Instant::now();
        watch_in(
            openclaude,
            Some(&its_cli),
            &its_home,
            false,
            NOW_MS,
            WAIT,
            POLL,
        )
        .expect("the status flipped");
        eprintln!(
            "measured: a status watch at a {POLL:?} poll noticed the change in {:?}",
            began.elapsed()
        );
        assert!(person.join().expect("the shell").status.success());
    }

    /// The card asks every status row at once, so the slowest CLI bounds the
    /// list instead of the sum of them all.
    #[test]
    fn the_status_asks_are_made_side_by_side() {
        let root = tempfile::tempdir().expect("sandbox");
        let slow = fake_cli(root.path(), "slow-status", "sleep 0.4; exit 0");
        let asks = 5;
        // What one ask costs on THIS machine right now: the sleep, a poll,
        // and whatever load the rest of the suite is putting on it.
        let began = Instant::now();
        let alone = ask_status(
            &mut crate::proc::quiet_command(&slow),
            Says::Exit0,
            STATUS_DEADLINE,
        );
        let one = began.elapsed();
        assert_eq!(alone, Verdict::In);
        let began = Instant::now();
        let verdicts: Vec<Verdict> = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..asks)
                .map(|_| {
                    scope.spawn(|| {
                        ask_status(
                            &mut crate::proc::quiet_command(&slow),
                            Says::Exit0,
                            STATUS_DEADLINE,
                        )
                    })
                })
                .collect();
            threads
                .into_iter()
                .map(|thread| thread.join().expect("ask"))
                .collect()
        });
        let together = began.elapsed();
        assert!(verdicts.iter().all(|said| *said == Verdict::In));
        eprintln!("measured: one status ask {one:?}; {asks} of them together {together:?}");
        assert!(
            together < one * 3,
            "{asks} asks took {together:?} against {one:?} for one — they were made one after another"
        );
    }

    /* ---- what the settings card is handed -------------------------------- */

    /// The settings row says what the table and the machine say, in the
    /// words the window branches on.
    #[test]
    fn the_report_speaks_the_rows_road_and_says_when_it_cannot_read_a_login() {
        let root = tempfile::tempdir().expect("sandbox");
        let grok = row("grok").expect("grok row");
        let (home, witness) = sandboxed_places(grok, root.path());
        let (_, text) = fake("grok").witness.expect("a grok witness");
        std::fs::write(&witness, text).expect("login");
        let signed_in = standing(
            grok,
            Some("/usr/local/bin/grok"),
            &home,
            NOW_MS,
            Verdict::In,
        )
        .expect("a standing");
        assert!(signed_in.installed && signed_in.signed_in && signed_in.known);
        assert_eq!(signed_in.account.as_deref(), Some("person@example.com"));
        assert_eq!((signed_in.road, signed_in.tui_login), ("verb", None));
        assert_eq!(signed_in.pane_command, None);
        assert_eq!(signed_in.proof, "file");
        assert_eq!(signed_in.opens, "agent");
        assert_eq!(signed_in.program, "grok");
        assert_eq!(signed_in.name, "Grok");

        // A row whose CLI is not on this machine names what it looked for,
        // which is not always the id.
        let aug = row("aug").expect("aug row");
        let aug_home = sandboxed_home(aug, root.path());
        let absent = standing(aug, None, &aug_home, NOW_MS, Verdict::Out).expect("a standing");
        assert_eq!(absent.program, "auggie");
        assert!(!absent.installed && !absent.signed_in && absent.known);
        assert_eq!(
            absent.pane_command, None,
            "a CLI that is not here has no line to type"
        );

        // A keyring row says it does not know, and offers no way out it
        // cannot walk.
        let vibe = row("mistral-vibe").expect("mistral-vibe row");
        let vibe_home = sandboxed_home(vibe, root.path());
        let unread =
            standing(vibe, Some("vibe"), &vibe_home, NOW_MS, Verdict::Unknown).expect("a standing");
        assert!(!unread.known && !unread.signed_in);
        assert_eq!(unread.proof, "none");
        assert_eq!(unread.logout_road, "none");
        assert_eq!(unread.tui_logout, None);
        assert!(
            unread
                .pane_command
                .as_deref()
                .is_some_and(|line| line.ends_with(" vibe --setup")),
            "{:?}",
            unread.pane_command
        );

        // A CLI the launch catalog does not know opens in a shell, and its
        // row carries the line that opens it.
        let gemini = row("gemini").expect("gemini row");
        let gemini_home = sandboxed_home(gemini, root.path());
        let outside = standing(gemini, Some("gemini"), &gemini_home, NOW_MS, Verdict::Out)
            .expect("a standing");
        assert_eq!(outside.opens, "shell");
        assert_eq!(outside.name, "Gemini CLI");
        assert_eq!((outside.road, outside.tui_login), ("tui", Some("/auth")));
        assert!(
            outside
                .pane_command
                .as_deref()
                .is_some_and(|line| line.ends_with(" gemini")),
            "{:?}",
            outside.pane_command
        );
        assert_eq!(outside.tui_logout, Some("/auth signout"));

        // A logout that needs a screen carries its own line.
        let opencode = row("opencode").expect("opencode row");
        let oc_home = sandboxed_home(opencode, root.path());
        let paned = standing(opencode, Some("opencode"), &oc_home, NOW_MS, Verdict::In)
            .expect("a standing");
        assert_eq!(paned.logout_road, "pane-verb");
        assert!(
            paned
                .logout_command
                .as_deref()
                .is_some_and(|line| line.ends_with(" opencode auth logout")),
            "{:?}",
            paned.logout_command
        );

        // The machine-wide report walks the same rows against the real
        // homes: one standing per row, nothing invented, and no CLI asked
        // for anything because none of them is named here.
        let began = Instant::now();
        let report = report(&|_| None, NOW_MS);
        let took = began.elapsed();
        eprintln!(
            "measured: the card's {} rows were read in {took:?}",
            report.rows.len()
        );
        assert_eq!(report.rows.len(), CLI_LOGINS.len());
        assert!(report.rows.iter().all(|row| !row.installed));
        assert!(
            took < Duration::from_millis(500),
            "reading the table took {took:?}"
        );
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

    /* ---- the pieces ------------------------------------------------------ */

    /// A row that needs a screen spells the line the window types: the home
    /// in the CLI's own variable, the variables that would outrank the login
    /// unset, then the verb — every word quoted only when it has to be.
    #[test]
    fn a_road_that_needs_a_screen_spells_the_shell_line_the_window_types() {
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
        // A row with variables that shadow its login takes them off the line
        // as well as off the child.
        let shadowed = CliLogin {
            agent: "copilot",
            cli: Cli::Agent,
            home_var: "COPILOT_HOME",
            home: Home::Under(&[".copilot"]),
            proof: Proof::Unreadable,
            login: LoginRoad::PaneVerb(&["login"]),
            logout: LogoutRoad::None,
            shadowed_by: &["GH_TOKEN"],
        };
        assert_eq!(
            shadowed
                .pane_command("copilot", Path::new("/Users/dev/.copilot"))
                .expect("a line"),
            "env -u GH_TOKEN COPILOT_HOME=/Users/dev/.copilot copilot login"
        );
    }

    /// The same variables come OFF the child the headless roads start.
    #[test]
    fn the_child_never_carries_a_variable_that_outranks_the_login() {
        let copilot = row("copilot").expect("copilot row");
        let command = copilot.command("copilot", Path::new("/Users/dev/.copilot"));
        let removed: Vec<String> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        for name in copilot.shadowed_by {
            assert!(
                removed.iter().any(|said| said == name),
                "{name} was left on the child"
            );
        }
        assert!(
            copilot.shadowed_by.contains(&"GH_TOKEN"),
            "the row stopped carrying the variable that outranks its login"
        );
        // And the row that borrows the Claude card's list borrows it whole.
        let openclaude = row("openclaude").expect("openclaude row");
        assert_eq!(
            openclaude.shadowed_by,
            zerocode_core::account::OVERRIDING_AUTH_VARS
        );
    }

    /// Where a row looks for its home: the CLI's own variable when the
    /// environment sets it, and the documented default under `$HOME` when it
    /// does not.
    #[test]
    fn a_home_is_the_clis_variable_or_the_default_under_the_persons_home() {
        let devin = row("devin").expect("devin row");
        let home = Path::new("/Users/dev");
        assert_eq!(
            devin.home_in(&|_| None, || Some(home.to_path_buf())),
            Some(home.join(".local").join("share")),
            "the XDG default is not where the CLI documents it"
        );
        assert_eq!(
            devin.home_in(
                &|name| (name == "XDG_DATA_HOME").then(|| PathBuf::from("/data")),
                || Some(home.to_path_buf())
            ),
            Some(PathBuf::from("/data")),
            "the row ignored the variable the CLI reads"
        );
        // A CLI with no variable of its own: its home IS the person's.
        let command_code = row("command-code").expect("command-code row");
        assert_eq!(command_code.home_var, "HOME");
        assert_eq!(
            command_code.home_in(&|_| None, || Some(home.to_path_buf())),
            Some(home.to_path_buf())
        );
        assert_eq!(
            command_code.witness_in(home),
            Some(home.join(".commandcode").join("auth.json"))
        );
        // The two rows a usage gauge already resolves answer the same as the
        // gauge, variable and all.
        let grok = row("grok").expect("grok row");
        assert_eq!(
            grok.home_in(&|_| None, dirs::home_dir),
            usage_grok::grok_home()
        );
        let kimi = row("kimi").expect("kimi row");
        assert_eq!(
            kimi.home_in(&|_| None, dirs::home_dir),
            usage_kimi::kimi_home()
        );
    }

    /// What counts as a login inside a file: a key that holds something.
    /// `null`, an empty string, an empty list and `false` are a key that
    /// says nobody is there — which is what these files look like after a
    /// logout.
    #[test]
    fn a_json_witness_holds_a_login_only_while_its_key_holds_something() {
        assert!(json_slice("{\"a\":{\"b\":\"x\"}}", &["a", "b"]).is_some());
        assert!(json_slice("{\"a\":{\"b\":\"x\"}}", &["a"]).is_some());
        assert!(json_slice("{\"a\":{}}", &["a"]).is_none());
        assert!(json_slice("{\"a\":[]}", &["a"]).is_none());
        assert!(json_slice("{\"a\":null}", &["a"]).is_none());
        assert!(json_slice("{\"a\":\"\"}", &["a"]).is_none());
        assert!(json_slice("{\"isAuthenticated\":false}", &["isAuthenticated"]).is_none());
        assert!(json_slice("{\"isAuthenticated\":true}", &["isAuthenticated"]).is_some());
        assert!(json_slice("{\"a\":1}", &["b"]).is_none());
        assert!(json_slice("{}", &[]).is_none());
        assert!(json_slice("{\"a\":1}", &[]).is_some());
        assert!(json_slice("not json", &[]).is_none());
        // The dotenv shape aider appends its key in.
        assert_eq!(
            env_slice(
                "# a note\nOPENROUTER_API_KEY=\"fixture-token\"\n",
                "OPENROUTER_API_KEY"
            ),
            Some("fixture-token".to_string())
        );
        assert_eq!(
            env_slice(
                "export OPENROUTER_API_KEY=fixture-token",
                "OPENROUTER_API_KEY"
            ),
            Some("fixture-token".to_string())
        );
        assert_eq!(
            env_slice("OPENROUTER_API_KEY=\n", "OPENROUTER_API_KEY"),
            None
        );
        assert_eq!(env_slice("OTHER=1\n", "OPENROUTER_API_KEY"), None);
        // The last assignment wins, the way a shell reading the file would.
        assert_eq!(
            env_slice(
                "OPENROUTER_API_KEY=old\nOPENROUTER_API_KEY=new\n",
                "OPENROUTER_API_KEY"
            ),
            Some("new".to_string())
        );
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
        assert_eq!(LogoutRoad::PaneVerb(&["logout"]).word(), "pane-verb");
        assert_eq!(LogoutRoad::TuiCommand("/logout").word(), "tui");
        assert_eq!(LogoutRoad::None.word(), "none");
        assert_eq!(
            LogoutRoad::TuiCommand("/logout").tui_command(),
            Some("/logout")
        );
        assert_eq!(LogoutRoad::None.tui_command(), None);
        assert_eq!(Cli::Agent.word(), "agent");
        assert_eq!(
            Cli::Own {
                program: "gemini",
                name: "Gemini CLI",
                homepage_url: "https://example.com"
            }
            .word(),
            "shell"
        );
        // Every word the JS branches on is one of the four it knows.
        for row in CLI_LOGINS {
            assert!(["verb", "pane-verb", "tui", "first-run"].contains(&row.login.word()));
            assert!(["verb", "pane-verb", "tui", "none"].contains(&row.logout.word()));
        }
    }

    #[test]
    fn the_home_caption_folds_the_home_directory_to_a_tilde() {
        let Some(home) = dirs::home_dir() else { return };
        assert_eq!(with_tilde(&home.join(".grok")), "~/.grok");
        assert_eq!(with_tilde(&home), "~");
        assert_eq!(with_tilde(Path::new("/opt/kimi")), "/opt/kimi");
    }
}
