//! Where other CLIs keep the login a person made with them, spelled once for
//! both sides of the window/agent boundary.
//!
//! The window reads these files for its usage gauges and its login rows
//! (`usage_grok`, `usage_kimi`, `cli_login`); zo reads the same files to list
//! the models a login may use and, for Grok, to speak as it (t-6248). zo's
//! `api` crate cannot depend on this crate, so it spells the names once more
//! and a zo-side test pins its spelling to this one — a CLI that moves its
//! file turns that test red before either side reads the wrong place.
//!
//! Only the places and the rules for reading them live here. Neither side
//! ever refreshes or rewrites a file named below: each CLI owns its token's
//! lifecycle, and a rotated refresh token spent by anybody else logs the CLI
//! itself out.

/// The Grok CLI (`grok login`, OIDC): `$GROK_HOME/auth.json`, one entry per
/// OAuth issuer, the session under `key` with an ISO `expires_at`
/// (`grok-session-paths.ts:40-46`, `grok-auth.ts`).
pub mod grok {
    /// The variable that moves the CLI's home.
    pub const HOME_VAR: &str = "GROK_HOME";
    /// The home below the person's own when the variable is unset.
    pub const HOME_DIR: &str = ".grok";
    /// The login file inside that home.
    pub const AUTH_FILE: &str = "auth.json";
    /// Grok's own OAuth issuer — bare, or with the `::` suffix the CLI
    /// appends to tell sessions under one issuer apart (`grok-auth.ts:67,81-83`).
    pub const PREFERRED_ISSUER: &str = "https://auth.x.ai";
    /// How long before its stamp a token already counts as gone
    /// (`grok-auth.ts:133`).
    pub const TOKEN_SKEW_MS: i64 = 5 * 60 * 1000;
}

/// The Kimi Code CLI (`kimi login`, device code):
/// `$KIMI_CODE_HOME/credentials/kimi-code.json`, an `access_token` beside an
/// `expires_at` in Unix seconds (`kimi-runtime-home.ts:17-19`,
/// `kimi-fetcher.ts`; the CLI's data-locations page names the folder).
pub mod kimi {
    /// The variable that moves the CLI's home.
    pub const HOME_VAR: &str = "KIMI_CODE_HOME";
    /// The home below the person's own when the variable is unset.
    pub const HOME_DIR: &str = ".kimi-code";
    /// The login file, below the home.
    pub const CREDENTIALS_TAIL: [&str; 2] = ["credentials", "kimi-code.json"];
    /// A token expiring inside this margin is already gone
    /// (`kimi-fetcher.ts:126-127`).
    pub const EXPIRY_SKEW_SECONDS: i64 = 5;
    /// The variable the CLI honours for a self-hosted or staging API.
    pub const BASE_URL_VAR: &str = "KIMI_CODE_BASE_URL";
    /// The API the CLI speaks to — `{base}/usages` for the gauge,
    /// `{base}/models` for the catalog.
    pub const DEFAULT_BASE_URL: &str = "https://api.kimi.com/coding/v1";
}
