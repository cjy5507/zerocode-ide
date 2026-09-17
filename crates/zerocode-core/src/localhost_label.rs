//! Names for the loopback ports a workspace is serving.
//!
//! A dev server printed as `http://localhost:5173` says nothing about which
//! checkout is serving it, and three workspaces of one repository print the
//! same sentence. Orca answers by giving each attributed port a hostname of
//! its own — `ui-auth.orca.localhost` — served by a small reverse proxy that
//! relabels the host and forwards everything else untouched.
//!
//! This module is the pure half of that: the label formula, the route
//! registry, the URL assembly and the Host-header lookup. It owns no socket,
//! reads no clock and touches no path, so the proxy half and the window
//! command can both be built on it and tested apart from it.
//!
//! Ported from `shared/localhost-worktree-labels.ts` (the formula, whole) and
//! the label bookkeeping of `main/localhost-worktree-label-proxy.ts` (the
//! registry). Behaviour is the original's except where a comment says
//! otherwise; the three deliberate departures are marked **DEPARTURE**.
//!
//! Two of the original's own latent flaws are ported rather than quietly
//! fixed, because a silent divergence is worse than a documented one:
//! a project slug already 48 characters long swallows its `-main` suffix
//! whole (so a primary worktree becomes indistinguishable from the bare
//! project), and `master` is not special-cased anywhere, so a
//! `master`-primary repository gets the bare label the formula's own comment
//! says it wants to avoid. Both are pinned by tests so a future change to
//! either is a decision rather than an accident.

use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

use serde::{Deserialize, Serialize};
use url::Url;

/// `HOST_LABEL_MAX_LENGTH`, `localhost-worktree-labels.ts:1`.
///
/// A DNS label may be 63 characters; 48 leaves room for the collision suffix
/// (`-999`) and the dot that follows, so every label this module can mint
/// still fits in one.
const HOST_LABEL_MAX_CHARS: usize = 48;

/// What a name with no ASCII alphanumerics becomes (`:73`). A hostname cannot
/// be empty, and refusing the route would lose a real port over a name.
const HOST_LABEL_FALLBACK: &str = "workspace";

/// The branch whose worktree is a project's primary (`:81-83`).
const PRIMARY_BRANCH: &str = "main";

/// Appended to the project slug for a primary worktree (`:82`) — project
/// first, so every project's primary does not collapse into one `main`.
const PRIMARY_SUFFIX: &str = "-main";

/// The zone the labels live in. Orca's is `.orca.localhost`
/// (`localhost-worktree-label-proxy.ts:22`); the brand is the one thing a
/// white-label may change. `.localhost` resolves to the loopback in every
/// browser without touching a hosts file, which is the whole reason the
/// original picked it.
pub const LABEL_HOST_SUFFIX: &str = ".zerocode.localhost";

/// The five hosts a loopback URL may name (`:4`), compared only after
/// [`normalize_loopback_hostname`]. `127.0.0.2`, `localhost.localdomain` and
/// `::ffff:127.0.0.1` are deliberately outside: the set exists to bound what
/// the proxy will forward to, and a wider set is a wider open door.
const LOOPBACK_HOSTS: [&str; 5] = ["localhost", "127.0.0.1", "0.0.0.0", "::1", "::"];

/// The wildcard bind addresses and the address of the same family that can
/// actually be connected to (`:31-40`). Connecting to a wildcard fails on
/// Windows, and resolving `localhost` may pick the wrong family for a
/// single-family listener — so the mapping preserves the family.
const WILDCARD_V4: &str = "0.0.0.0";
const LOOPBACK_V4: &str = "127.0.0.1";
const WILDCARD_V6: &str = "::";
const LOOPBACK_V6: &str = "::1";

/// How a listener spells "every interface" when it is naming a bind rather
/// than an address (`advertised-url-watcher.ts:186-189`). Not a URL host — but
/// dev servers print it as one, and it belongs in this file because this file
/// is where the loopback vocabulary lives.
const WILDCARD_ANY: &str = "*";

/// The bare label occupies the first slot, so the first collision is `-2`
/// and the search stops before `-1000` (`proxy.ts:101-111`).
const FIRST_COLLISION_SUFFIX: usize = 2;
const MAX_COLLISION_SUFFIX: usize = 999;

/// **DEPARTURE (ours).** Orca's registry has no ceiling; a house rule says an
/// unbounded collection is itself a defect. The bound sits above the suffix
/// ladder on purpose: were it lower, the map would fill before `-999` could
/// ever be reached and one of the two refusals would be code no input can
/// produce.
const MAX_ROUTES: usize = 1024;

/// The byte a path, a URL and an id cannot contain — the same separator
/// `last_status::key` uses for the same reason. Orca joins its route key with
/// `:`, which a URL carries twice.
const ROUTE_KEY_SEPARATOR: char = '\u{0}';

/// Who and where a route is for: the words the label is made of, and the ids
/// that decide when two requests are the same route.
///
/// A struct rather than five string parameters on purpose — five arguments of
/// one type is a swap waiting to happen, and the original passes an object
/// too (`:42-48`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelInput<'a> {
    pub project_name: &'a str,
    /// The worktree's own name — a branch name, in practice. Read for the
    /// trailing-`main` test even when the path below supplies the label.
    pub worktree_name: &'a str,
    /// The checkout's path, when known. **DEPARTURE:** the original's main
    /// process drops this field before registering, so its production labels
    /// always come from the branch name — yet its own unit test pins that the
    /// path wins and that a `owner74/branch` prefix must never reach a
    /// hostname (`localhost-worktree-labels.test.ts:27-35`). We implement the
    /// tested contract.
    pub worktree_path: Option<&'a str>,
    pub repo_id: Option<&'a str>,
    pub worktree_id: Option<&'a str>,
}

/// One registration: who it is for, what it points at, and the identity the
/// label is derived from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteSpec<'a> {
    pub identity: LabelInput<'a>,
    /// The opaque identity whose teardown releases this route. Required and
    /// non-empty — **DEPARTURE:** Orca evicts by `worktreeId` alone, so a
    /// route registered without one can never be removed, and its own comment
    /// admits the leak (`proxy.ts:52-54`). A display name is not an identity;
    /// two projects can each hold a checkout called `main`.
    pub owner: &'a str,
    pub target_url: &'a str,
}

/// A route as the registry holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredRoute {
    pub label: HostLabel,
    pub key: RouteKey,
    pub owner: String,
    pub target: Url,
}

/// What a registration hands back to the window: the address to open, and the
/// label inside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabeledRoute {
    pub url: String,
    pub label: String,
}

/// One hostname label — ASCII letters, digits and dashes, never empty.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostLabel(String);

impl HostLabel {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HostLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Borrow<str> for HostLabel {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// What makes two registrations the same route.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RouteKey(String);

impl fmt::Display for RouteKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why a target could not be a proxy target (`proxy.ts:213-219`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetError {
    NotAUrl,
    NotHttp,
}

impl fmt::Display for TargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TargetError::NotAUrl => f.write_str("target is not a url"),
            TargetError::NotHttp => f.write_str("only http targets can be labeled"),
        }
    }
}

impl std::error::Error for TargetError {}

/// Why a registration was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterError {
    Target(TargetError),
    /// Every suffix through `-999` was taken.
    NoAvailableLabel,
    /// The registry is at `MAX_ROUTES`.
    TooManyRoutes,
    /// A route with no owner could never be released.
    EmptyOwner,
}

impl fmt::Display for RegisterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegisterError::Target(inner) => write!(f, "{inner}"),
            RegisterError::NoAvailableLabel => f.write_str("no label is available for that name"),
            RegisterError::TooManyRoutes => f.write_str("too many labeled routes"),
            RegisterError::EmptyOwner => f.write_str("a labeled route needs an owner"),
        }
    }
}

impl std::error::Error for RegisterError {}

impl From<TargetError> for RegisterError {
    fn from(inner: TargetError) -> Self {
        RegisterError::Target(inner)
    }
}

/// A name as a hostname label (`:64-74`).
///
/// Lowercase first — the original lowercases the whole string before
/// filtering, so a codepoint whose lowercase form contains an ASCII letter
/// keeps it. Straight quotes vanish rather than becoming separators (so
/// `don't` is one word); every other run of non-alphanumerics collapses to a
/// single dash. Dashes never lead or trail, including after the cut.
///
/// **Not** `zerocode_orchestrator::naming::slugify`: both outputs are
/// ASCII-only, but hostname labels drop quotes instead of treating them as
/// separators and have their own length and fallback rules. A non-ASCII label
/// would be IDNA-encoded on its way into a URL and the registry, keyed on the
/// pre-encoded spelling, would fail to find its own route. The ASCII filter
/// here is load-bearing.
pub fn slugify_host_label(raw: &str) -> HostLabel {
    let mut slug = String::with_capacity(raw.len());
    let mut pending_separator = false;
    // No leading trim: whitespace becomes a separator that never gets flushed
    // while the slug is still empty, which is the trim the original spells out.
    for ch in raw.chars().flat_map(char::to_lowercase) {
        if ch == '\'' || ch == '"' {
            continue;
        }
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            if pending_separator && !slug.is_empty() {
                slug.push('-');
            }
            pending_separator = false;
            slug.push(ch);
        } else {
            pending_separator = true;
        }
    }
    // ASCII by now, so the cut can only land on a character boundary.
    slug.truncate(HOST_LABEL_MAX_CHARS);
    while slug.ends_with('-') {
        slug.pop();
    }
    HostLabel(if slug.is_empty() {
        HOST_LABEL_FALLBACK.to_string()
    } else {
        slug
    })
}

/// The last non-empty segment of a path or a branch name (`:87-94`).
///
/// Both separators in one class, deliberately: `std::path` would not treat
/// `\` as a separator away from Windows, and a Windows-shaped value can
/// arrive from a machine that is not this one.
fn short_worktree_name(value: &str) -> &str {
    value
        .split(['/', '\\'])
        .map(str::trim)
        .rfind(|part| !part.is_empty())
        .unwrap_or(value)
}

/// Whether a name ends in the primary branch (`TRAILING_MAIN_PATTERN`, `:2`):
/// `main` alone, or `main` after a dash, an underscore, a slash or a space.
///
/// The original's character class omits the backslash, and a Windows path
/// still reaches the primary branch — through the segment split above, not
/// through this test. Removing either road breaks Windows.
fn ends_with_primary_branch(raw_worktree_name: &str) -> bool {
    let Some(cut) = raw_worktree_name.len().checked_sub(PRIMARY_BRANCH.len()) else {
        return false;
    };
    let Some(tail) = raw_worktree_name.get(cut..) else {
        return false;
    };
    if !tail.eq_ignore_ascii_case(PRIMARY_BRANCH) {
        return false;
    }
    match raw_worktree_name[..cut].chars().next_back() {
        None => true,
        Some(ch) => ch == '-' || ch == '_' || ch == '/' || ch.is_whitespace(),
    }
}

/// The hostname label for one checkout (`:76-85`).
///
/// A primary worktree carries its project, because otherwise every project on
/// the machine would be serving from `main`. Everything else is named by its
/// own folder, which is shorter and is what the person typed.
pub fn host_label_for(input: &LabelInput<'_>) -> HostLabel {
    let named = input.worktree_path.unwrap_or(input.worktree_name);
    let worktree_slug = slugify_host_label(short_worktree_name(named));
    if worktree_slug.as_str() == PRIMARY_BRANCH || ends_with_primary_branch(input.worktree_name) {
        let project_slug = slugify_host_label(input.project_name);
        return slugify_host_label(&format!("{project_slug}{PRIMARY_SUFFIX}"));
    }
    worktree_slug
}

/// What makes two registrations the same route (`:96-104`).
///
/// The strongest identity available wins, and the target is always part of
/// the key — two ports of one checkout are two routes, not one. **DEPARTURE:**
/// each tier is tagged, so a project literally named `worktree` cannot forge
/// the key of a worktree id.
pub fn route_key(input: &LabelInput<'_>, target_url: &str) -> RouteKey {
    let separator = ROUTE_KEY_SEPARATOR;
    if let Some(worktree_id) = input.worktree_id.filter(|value| !value.is_empty()) {
        return RouteKey(format!(
            "worktree{separator}{worktree_id}{separator}{target_url}"
        ));
    }
    if let Some(repo_id) = input.repo_id.filter(|value| !value.is_empty()) {
        let name = input.worktree_name;
        return RouteKey(format!(
            "repo{separator}{repo_id}{separator}{name}{separator}{target_url}"
        ));
    }
    let project = input.project_name;
    let name = input.worktree_name;
    RouteKey(format!(
        "name{separator}{project}{separator}{name}{separator}{target_url}"
    ))
}

/// A hostname as the loopback set spells it (`:6-8`): one bracket off each
/// edge, lowercased. Not a validator — every other host normalizer in this
/// codebase rejects what it does not recognize, and this one must hand back
/// strangers unchanged for the membership test to answer.
pub fn normalize_loopback_hostname(hostname: &str) -> String {
    let bare = hostname.strip_prefix('[').unwrap_or(hostname);
    let bare = bare.strip_suffix(']').unwrap_or(bare);
    bare.to_lowercase()
}

/// Whether a hostname is one of the five loopback spellings (`:4`).
pub fn is_loopback_hostname(hostname: &str) -> bool {
    let bare = normalize_loopback_hostname(hostname);
    LOOPBACK_HOSTS.contains(&bare.as_str())
}

/// Whether a hostname names every interface rather than one
/// (`advertised-url-watcher.ts:186-189`).
///
/// Such a host is inside [`is_loopback_hostname`] — it is a legitimate thing
/// for a *listener* to be bound to — and outside anything a browser can open.
/// Callers that need an address rather than a bind ask this first.
pub fn is_unspecified_host(hostname: &str) -> bool {
    matches!(
        normalize_loopback_hostname(hostname).as_str(),
        WILDCARD_V4 | WILDCARD_V6 | WILDCARD_ANY
    )
}

/// A loopback URL with an explicit port, or nothing (`:12-26`).
///
/// Only such a URL can be attributed to a scanned workspace port, so only
/// such a URL can be labeled. The explicit-port rule leans on WHATWG
/// canonicalization: `http://localhost:80/` has no port once parsed, and
/// `http://2130706433:3000` is `127.0.0.1` — which is exactly why the gate
/// parses rather than pattern-matches.
pub fn parse_loopback_url_with_port(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return None;
    }
    url.port()?;
    if !is_loopback_hostname(url.host_str()?) {
        return None;
    }
    Some(url)
}

/// A URL the proxy may forward to (`proxy.ts:213-219`).
///
/// http only. The original carries `https` branches it can never reach, and
/// its upgrade path would have spoken plain TCP to a TLS port; refusing at
/// the door makes that unrepresentable instead of unreachable.
pub fn parse_proxy_target(raw: &str) -> Result<Url, TargetError> {
    let url = Url::parse(raw).map_err(|_| TargetError::NotAUrl)?;
    if url.scheme() != "http" {
        return Err(TargetError::NotHttp);
    }
    Ok(url)
}

/// The address to actually open a socket to (`:31-40`).
///
/// A wildcard bind becomes the loopback of its own family; everything else is
/// handed back byte for byte, brackets included, because the caller may be
/// putting it back into a URL.
pub fn connectable_loopback_host(hostname: &str) -> &str {
    match normalize_loopback_hostname(hostname).as_str() {
        WILDCARD_V4 => LOOPBACK_V4,
        WILDCARD_V6 => LOOPBACK_V6,
        _ => hostname,
    }
}

/// The labeled address for a target (`proxy.ts:115-121`): the target's path
/// and query, under the label's hostname, on the proxy's port.
pub fn labeled_url(label: &HostLabel, target: &Url, proxy_port: u16) -> Url {
    let mut labeled = target.clone();
    let host = format!("{label}{LABEL_HOST_SUFFIX}");
    labeled
        .set_host(Some(&host))
        .expect("a slug label under the label zone is always a valid domain");
    labeled
        .set_port(Some(proxy_port))
        .expect("an http url always has an authority to carry a port");
    labeled
}

/// The label a request is asking for, read off its `Host` header
/// (`proxy.ts:199-207`). The port is dropped, the comparison is
/// case-insensitive, and a host outside the label zone is nobody's route.
pub fn label_from_host_header(host_header: Option<&str>) -> Option<String> {
    let host = host_header?.split(':').next()?.to_ascii_lowercase();
    let label = host.strip_suffix(LABEL_HOST_SUFFIX)?;
    if label.is_empty() {
        return None;
    }
    Some(label.to_string())
}

/// Which label serves which target, and who to release it with.
///
/// Two maps, as the original keeps them (`proxy.ts:27-28`): labels to routes,
/// so a request can be answered from its Host header, and route keys to
/// labels, so the same port asked for twice keeps the name a person may
/// already have bookmarked.
#[derive(Debug, Default)]
pub struct LabelRegistry {
    routes: BTreeMap<HostLabel, RegisteredRoute>,
    labels_by_key: HashMap<RouteKey, HostLabel>,
    /// Names that were released, and the route each belonged to.
    ///
    /// **DEPARTURE:** the original frees a label the moment its worktree goes,
    /// so the next checkout of the same name inherits it. A page served under
    /// a label may have registered a service worker — `*.localhost` is a
    /// secure context — and that worker would then answer for a workspace
    /// that is not the one it came from. A retired name is therefore only
    /// ever handed back to the same route it left.
    retired: BTreeMap<HostLabel, RouteKey>,
}

impl LabelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Give a target a label, or hand back the one it already had.
    ///
    /// The order is the original's (`proxy.ts:32-45`): the target is
    /// validated first, then the label is reused or minted, then both maps are
    /// written. A returning route refreshes its target rather than minting a
    /// second name for the same port.
    pub fn register(&mut self, spec: &RouteSpec<'_>) -> Result<HostLabel, RegisterError> {
        if spec.owner.is_empty() {
            return Err(RegisterError::EmptyOwner);
        }
        let target = parse_proxy_target(spec.target_url)?;
        // **DEPARTURE:** keyed on the target's ORIGIN, not the whole URL as
        // the original keys it (`:96-104`). The proxy replaces the path and
        // query of every forwarded request with the incoming one's
        // (`proxy.ts:245-251`), so the path was never part of what a route
        // IS — keying on it mints a second name for the same port the first
        // time somebody clicks a deeper link, and the bookmark the label
        // exists to make loses its meaning.
        let key = route_key(
            &spec.identity,
            target.origin().ascii_serialization().as_str(),
        );
        let label = match self.labels_by_key.get(&key) {
            Some(held) => held.clone(),
            None => {
                // Retired names still occupy the ceiling: at the bound the
                // registry refuses to name anything new rather than recycling
                // a name, because refusing is safe and recycling is not.
                if self.routes.len() + self.retired.len() >= MAX_ROUTES {
                    return Err(RegisterError::TooManyRoutes);
                }
                self.next_available_label(&host_label_for(&spec.identity), &key)?
            }
        };
        self.retired.remove(&label);
        self.routes.insert(
            label.clone(),
            RegisteredRoute {
                label: label.clone(),
                key: key.clone(),
                owner: spec.owner.to_string(),
                target,
            },
        );
        self.labels_by_key.insert(key, label.clone());
        Ok(label)
    }

    /// The route a label names, if it is one this registry minted.
    pub fn route(&self, label: &str) -> Option<&RegisteredRoute> {
        self.routes.get(label)
    }

    /// Drop every route an owner holds, and hand back the labels that were
    /// taken (`proxy.ts:52-63`). Collected before removal — the original
    /// deletes while walking its map, which JavaScript allows and Rust does
    /// not. The labels come back rather than a count because a caller that
    /// serves them has to know which names just stopped meaning anything.
    pub fn release_owner(&mut self, owner: &str) -> Vec<HostLabel> {
        let doomed: Vec<(HostLabel, RouteKey)> = self
            .routes
            .values()
            .filter(|route| route.owner == owner)
            .map(|route| (route.label.clone(), route.key.clone()))
            .collect();
        for (label, key) in &doomed {
            self.routes.remove(label);
            self.labels_by_key.remove(key);
            self.retired.insert(label.clone(), key.clone());
        }
        doomed.into_iter().map(|(label, _)| label).collect()
    }

    pub fn len(&self) -> usize {
        self.routes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }

    /// The base name if it is free, else the first free `base-N`
    /// (`proxy.ts:101-111`). A name is free when no route holds it and no
    /// other route retired it.
    fn next_available_label(
        &self,
        base: &HostLabel,
        for_key: &RouteKey,
    ) -> Result<HostLabel, RegisterError> {
        let free = |candidate: &HostLabel| {
            !self.routes.contains_key(candidate.as_str())
                && self
                    .retired
                    .get(candidate)
                    .is_none_or(|held| held == for_key)
        };
        if free(base) {
            return Ok(base.clone());
        }
        for suffix in FIRST_COLLISION_SUFFIX..=MAX_COLLISION_SUFFIX {
            let candidate = HostLabel(format!("{base}-{suffix}"));
            if free(&candidate) {
                return Ok(candidate);
            }
        }
        Err(RegisterError::NoAvailableLabel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(project: &'a str, worktree: &'a str) -> LabelInput<'a> {
        LabelInput {
            project_name: project,
            worktree_name: worktree,
            worktree_path: None,
            repo_id: None,
            worktree_id: None,
        }
    }

    fn spec<'a>(identity: LabelInput<'a>, owner: &'a str, target: &'a str) -> RouteSpec<'a> {
        RouteSpec {
            identity,
            owner,
            target_url: target,
        }
    }

    fn slug(raw: &str) -> String {
        slugify_host_label(raw).as_str().to_string()
    }

    fn label(project: &str, worktree: &str) -> String {
        host_label_for(&input(project, worktree))
            .as_str()
            .to_string()
    }

    /// Trim, case, the collapse of a whole run, and the trailing punctuation
    /// that becomes a dash and is then stripped — all in one name, the way
    /// the original's own test asks it.
    #[test]
    fn a_prose_name_becomes_one_hyphen_run() {
        assert_eq!(slug("  Drive DB Mismatch!  "), "drive-db-mismatch");
        assert_eq!(slug("UI___Auth"), "ui-auth");
    }

    /// A straight apostrophe vanishes so `don't` stays one word; a curly one
    /// is not in the class and splits the word instead.
    #[test]
    fn a_straight_apostrophe_joins_words_and_a_curly_one_splits_them() {
        assert_eq!(slug("don't stop"), "dont-stop");
        assert_eq!(slug("don\u{2019}t stop"), "don-t-stop");
        assert_eq!(slug("say \"hi\" now"), "say-hi-now");
    }

    /// A hostname cannot carry Hangul, so a name made only of it falls back —
    /// this is the exact input where the house slug would have kept the word.
    #[test]
    fn a_name_with_no_ascii_alphanumerics_falls_back() {
        assert_eq!(slug("드레인 게이트"), HOST_LABEL_FALLBACK);
        assert_eq!(slug("!!!"), HOST_LABEL_FALLBACK);
        assert_eq!(slug("   "), HOST_LABEL_FALLBACK);
        assert_eq!(slug("\"'"), HOST_LABEL_FALLBACK);
        assert_eq!(slug(""), HOST_LABEL_FALLBACK);
    }

    /// The cap counts the finished slug, and a cut that lands on a separator
    /// leaves no dash hanging off the end.
    #[test]
    fn the_cap_counts_the_trimmed_slug_and_a_cut_separator_leaves_no_dash() {
        let long = format!("{} b", "a".repeat(47));
        assert_eq!(slug(&long), "a".repeat(47));
        let longer = format!("{} bcd", "a".repeat(46));
        assert_eq!(slug(&longer), format!("{}-b", "a".repeat(46)));
        assert!(slug(&"z".repeat(80)).len() == HOST_LABEL_MAX_CHARS);
    }

    /// A primary worktree carries its project, so two projects both serving
    /// from `main` are still two names.
    #[test]
    fn a_primary_worktree_keeps_its_project() {
        assert_eq!(label("Snap Studio", "main"), "snap-studio-main");
        assert_ne!(label("Snap Studio", "main"), "main");
        assert_ne!(label("Snap Studio", "main"), "main-snap-studio");
    }

    /// Everything else is named by itself — the project prefix would only
    /// make the name longer without making it clearer.
    #[test]
    fn a_secondary_worktree_carries_no_project_prefix() {
        assert_eq!(label("Snap Studio", "analytics"), "analytics");
    }

    /// The folder wins over the branch name, so a branch owner's prefix never
    /// reaches a hostname (the original's own test).
    #[test]
    fn the_worktree_folder_beats_a_branch_owner_prefix() {
        let identity = LabelInput {
            worktree_path: Some("/Users/j/snapstudio/ui-auth"),
            ..input("Snap Studio", "gatsby74/table-summary")
        };
        assert_eq!(host_label_for(&identity).as_str(), "ui-auth");
    }

    /// The primary test reads the raw branch name even when the path is what
    /// supplies the label — a checkout of `feature/main` is still primary.
    #[test]
    fn trailing_main_is_read_off_the_raw_branch_name_not_the_folder() {
        let identity = LabelInput {
            worktree_path: Some("/x/ui-auth"),
            ..input("Snap Studio", "feature/main")
        };
        assert_eq!(host_label_for(&identity).as_str(), "snap-studio-main");
    }

    /// `main` counts only after a separator, so `domain` and `mainline` are
    /// ordinary names.
    #[test]
    fn main_is_matched_only_after_a_separator() {
        for primary in [
            "main",
            "MAIN",
            "feature-main",
            "feature_main",
            "feature main",
        ] {
            assert_eq!(label("Proj", primary), "proj-main", "{primary}");
        }
        for ordinary in ["domain", "remain", "mainline", "main-2"] {
            assert_ne!(label("Proj", ordinary), "proj-main", "{ordinary}");
        }
    }

    /// A Windows-shaped name reaches the primary branch through the segment
    /// split, because the separator test above omits the backslash on
    /// purpose. Both roads are load-bearing.
    #[test]
    fn a_windows_path_reaches_the_primary_branch_through_the_split_not_the_regex() {
        assert!(!ends_with_primary_branch("repo\\main"));
        assert_eq!(label("Proj", "repo\\main"), "proj-main");
        assert_eq!(short_worktree_name("C:\\repo\\main"), "main");
    }

    /// Ported flaw, pinned: once the project slug fills the cap, the `-main`
    /// suffix is eaten a character at a time until a primary worktree and the
    /// bare project are the same name.
    #[test]
    fn a_long_project_loses_the_main_suffix_one_character_at_a_time() {
        for (project_len, expected_suffix) in [(43, "-main"), (44, "-mai"), (45, "-ma"), (46, "-m")]
        {
            let project = "p".repeat(project_len);
            let labeled = label(&project, "main");
            assert_eq!(
                labeled,
                format!("{project}{expected_suffix}"),
                "project of {project_len}"
            );
        }
        // One character further and the suffix is a bare dash, which the
        // trailing strip then eats: a primary worktree and its project have
        // become the same name.
        let nearly = "p".repeat(47);
        assert_eq!(label(&nearly, "main"), nearly);
        let full = "p".repeat(HOST_LABEL_MAX_CHARS);
        assert_eq!(label(&full, "main"), full);
    }

    /// Ported flaw, pinned: only `main` is primary, so a `master` repository
    /// gets the bare collapse the formula says it wants to avoid.
    #[test]
    fn master_is_not_a_primary_branch_anywhere() {
        assert_eq!(label("Snap Studio", "master"), "master");
    }

    /// An unnameable project still yields a name, because the fallback runs
    /// on the composed string too.
    #[test]
    fn an_unnameable_project_still_gets_workspace_main() {
        assert_eq!(label("!!!", "main"), "workspace-main");
    }

    /// An empty path is a value, not an absence: it wins over the name and
    /// takes the fallback with it. Only a missing path falls through.
    #[test]
    fn an_empty_worktree_path_wins_over_the_name() {
        let empty = LabelInput {
            worktree_path: Some(""),
            ..input("Proj", "analytics")
        };
        assert_eq!(host_label_for(&empty).as_str(), HOST_LABEL_FALLBACK);
        let absent = LabelInput {
            worktree_path: None,
            ..input("Proj", "analytics")
        };
        assert_eq!(host_label_for(&absent).as_str(), "analytics");
    }

    /// The last non-empty segment wins across both separators, and a value
    /// with no segments at all is handed back whole.
    #[test]
    fn the_last_non_empty_segment_wins_across_both_separators() {
        assert_eq!(short_worktree_name("/a/b"), "b");
        assert_eq!(short_worktree_name("/a/b/"), "b");
        assert_eq!(short_worktree_name("/a/b/  "), "b");
        assert_eq!(short_worktree_name("C:\\repo\\ui-auth"), "ui-auth");
        assert_eq!(short_worktree_name("/"), "/");
    }

    /// Two ports of one checkout are two routes — the rule the signature
    /// alone would not give you, and the one the original's test pins.
    #[test]
    fn two_ports_of_one_worktree_get_two_keys() {
        let identity = LabelInput {
            worktree_id: Some("repo#ui-auth"),
            ..input("Proj", "ui-auth")
        };
        assert_ne!(
            route_key(&identity, "http://localhost:5173/"),
            route_key(&identity, "http://localhost:7777/")
        );
    }

    /// The strongest identity available decides the key, and an empty id is
    /// an absent one.
    #[test]
    fn precedence_is_worktree_then_repo_then_names_and_an_empty_id_falls_through() {
        let target = "http://localhost:5173/";
        let full = LabelInput {
            repo_id: Some("repo"),
            worktree_id: Some("wt"),
            ..input("Proj", "ui-auth")
        };
        let renamed = LabelInput {
            project_name: "Other",
            worktree_name: "other",
            ..full
        };
        assert_eq!(route_key(&full, target), route_key(&renamed, target));
        let hollow = LabelInput {
            worktree_id: Some(""),
            ..full
        };
        assert_ne!(route_key(&hollow, target), route_key(&full, target));
        assert_eq!(
            route_key(&hollow, target),
            route_key(
                &LabelInput {
                    worktree_id: None,
                    ..full
                },
                target
            )
        );
    }

    /// A name cannot forge another tier's key, because every tier is tagged
    /// and joined on a byte a name cannot hold.
    #[test]
    fn keys_cannot_collide_across_identities() {
        let target = "http://localhost:5173/";
        let forged = LabelInput {
            project_name: "worktree",
            worktree_name: "wt",
            ..input("worktree", "wt")
        };
        let real = LabelInput {
            worktree_id: Some("wt"),
            ..input("Proj", "ui-auth")
        };
        assert_ne!(route_key(&forged, target), route_key(&real, target));
    }

    /// Only a loopback URL that names its port can be attributed to a scanned
    /// port, so only that shape is a candidate.
    #[test]
    fn a_loopback_url_needs_an_explicit_port() {
        assert!(parse_loopback_url_with_port("http://localhost/").is_none());
        assert!(parse_loopback_url_with_port("http://localhost:80/").is_none());
        assert!(parse_loopback_url_with_port("https://localhost:443/").is_none());
        assert!(parse_loopback_url_with_port("http://localhost:5173/app").is_some());
        assert!(parse_loopback_url_with_port("http://localhost:0/").is_some());
    }

    /// Both http schemes may be labeled; only http may be forwarded to.
    #[test]
    fn only_http_and_https_are_candidates_and_only_http_is_a_proxy_target() {
        for refused in [
            "ws://localhost:5173/",
            "file:///tmp/x",
            "chrome-extension://abc/",
            "//localhost:5173",
            "/relative",
        ] {
            assert!(parse_loopback_url_with_port(refused).is_none(), "{refused}");
        }
        assert!(parse_loopback_url_with_port("https://localhost:9999/").is_some());
        assert_eq!(
            parse_proxy_target("https://localhost:9999/"),
            Err(TargetError::NotHttp)
        );
        assert_eq!(parse_proxy_target("not a url"), Err(TargetError::NotAUrl));
        assert!(parse_proxy_target("http://127.0.0.1:5173/").is_ok());
    }

    /// The set is five spellings and no more — a wider set is a wider door
    /// for the proxy the registry feeds.
    #[test]
    fn the_loopback_set_is_five_hosts_and_no_more() {
        for member in LOOPBACK_HOSTS {
            assert!(is_loopback_hostname(member), "{member}");
        }
        for stranger in ["127.0.0.2", "localhost.localdomain", "::ffff:127.0.0.1"] {
            assert!(!is_loopback_hostname(stranger), "{stranger}");
        }
    }

    /// The gate leans on canonicalization rather than pattern-matching, which
    /// is what closes the numeric and mixed-case spellings of the loopback.
    #[test]
    fn url_canonicalization_is_what_the_gate_leans_on() {
        for spelled in [
            "http://[0:0:0:0:0:0:0:1]:3000/",
            "http://127.000.000.001:3000/",
            "http://2130706433:3000/",
            "http://LocalHost:5173/",
        ] {
            assert!(parse_loopback_url_with_port(spelled).is_some(), "{spelled}");
        }
    }

    /// One bracket comes off each edge and nowhere else.
    #[test]
    fn one_bracket_is_stripped_from_each_edge_and_nowhere_else() {
        assert_eq!(normalize_loopback_hostname("[[::1]]"), "[::1]");
        assert_eq!(normalize_loopback_hostname("[abc"), "abc");
        assert_eq!(normalize_loopback_hostname("a]b"), "a]b");
        assert_eq!(normalize_loopback_hostname("[::1]"), "::1");
        assert_eq!(normalize_loopback_hostname("LOCALHOST"), "localhost");
    }

    /// A wildcard bind becomes an address of its own family; a stranger comes
    /// back byte for byte, brackets and all.
    #[test]
    fn a_wildcard_bind_becomes_a_connectable_address_of_the_same_family() {
        assert_eq!(connectable_loopback_host("0.0.0.0"), "127.0.0.1");
        assert_eq!(connectable_loopback_host("::"), "::1");
        assert_eq!(connectable_loopback_host("[::]"), "::1");
        assert_eq!(connectable_loopback_host("[::1]"), "[::1]");
        assert_eq!(connectable_loopback_host("LOCALHOST"), "LOCALHOST");
        assert_eq!(connectable_loopback_host("example.com"), "example.com");
    }

    /// One port is one name, however deep the link that asked for it — the
    /// departure from the original, whose key carries the path and so mints a
    /// second name the first time somebody clicks past the root.
    #[test]
    fn the_same_route_keeps_its_label_and_refreshes_its_target() {
        let mut registry = LabelRegistry::new();
        let identity = LabelInput {
            worktree_id: Some("wt"),
            ..input("Proj", "ui-auth")
        };
        let first = registry
            .register(&spec(identity, "/w/ui-auth", "http://localhost:5173/"))
            .unwrap();
        let again = registry
            .register(&spec(identity, "/w/ui-auth", "http://localhost:5173/app"))
            .unwrap();
        assert_eq!(first, again);
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry.route(first.as_str()).unwrap().target.as_str(),
            "http://localhost:5173/app"
        );
    }

    /// The bare name holds the first slot, so the first collision is `-2`.
    #[test]
    fn a_second_worktree_of_the_same_name_takes_the_two_suffix() {
        let mut registry = LabelRegistry::new();
        let mine = LabelInput {
            worktree_id: Some("a"),
            ..input("Proj", "ui-auth")
        };
        let theirs = LabelInput {
            worktree_id: Some("b"),
            ..input("Other", "ui-auth")
        };
        assert_eq!(
            registry
                .register(&spec(mine, "/w/a", "http://localhost:5173/"))
                .unwrap()
                .as_str(),
            "ui-auth"
        );
        assert_eq!(
            registry
                .register(&spec(theirs, "/w/b", "http://localhost:5174/"))
                .unwrap()
                .as_str(),
            "ui-auth-2"
        );
    }

    /// One name can be claimed 999 times and no more — the bare label plus
    /// `-2` through `-999`.
    #[test]
    fn the_thousandth_claimant_of_one_name_is_refused() {
        let mut registry = LabelRegistry::new();
        let fill = |registry: &mut LabelRegistry, index: usize| {
            let owner = format!("/w/{index}");
            let id = format!("wt{index}");
            let target = format!("http://localhost:{}/", 5000 + index);
            let identity = LabelInput {
                worktree_id: Some(&id),
                ..input("Proj", "ui-auth")
            };
            registry.register(&spec(identity, &owner, &target))
        };
        for index in 0..MAX_COLLISION_SUFFIX {
            fill(&mut registry, index).unwrap();
        }
        assert!(registry.route("ui-auth").is_some());
        assert!(registry.route("ui-auth-999").is_some());
        assert_eq!(registry.len(), MAX_COLLISION_SUFFIX);
        assert_eq!(
            fill(&mut registry, MAX_COLLISION_SUFFIX),
            Err(RegisterError::NoAvailableLabel)
        );
    }

    /// And the map itself refuses to grow without bound, whatever the names.
    #[test]
    fn the_registry_refuses_to_grow_without_bound() {
        let mut registry = LabelRegistry::new();
        for index in 0..MAX_ROUTES {
            let owner = format!("/w/{index}");
            let name = format!("wt-{index}");
            let target = format!("http://localhost:{}/", 5000 + index);
            let identity = LabelInput {
                worktree_id: Some(&owner),
                ..input("Proj", &name)
            };
            registry.register(&spec(identity, &owner, &target)).unwrap();
        }
        assert_eq!(registry.len(), MAX_ROUTES);
        let identity = LabelInput {
            worktree_id: Some("overflow"),
            ..input("Proj", "one-more")
        };
        assert_eq!(
            registry.register(&spec(identity, "/w/x", "http://localhost:9999/")),
            Err(RegisterError::TooManyRoutes)
        );
    }

    /// Releasing one checkout leaves its neighbours standing, and empties
    /// both maps for the one it took.
    #[test]
    fn releasing_one_worktree_leaves_its_neighbours_standing() {
        let mut registry = LabelRegistry::new();
        let mine = LabelInput {
            worktree_id: Some("a"),
            ..input("Proj", "ui-auth")
        };
        let theirs = LabelInput {
            worktree_id: Some("b"),
            ..input("Proj", "analytics")
        };
        let gone = registry
            .register(&spec(mine, "/w/a", "http://localhost:5173/"))
            .unwrap();
        let kept = registry
            .register(&spec(theirs, "/w/b", "http://localhost:5174/"))
            .unwrap();
        assert_eq!(registry.release_owner("/w/a"), vec![gone.clone()]);
        assert!(registry.route(gone.as_str()).is_none());
        assert!(registry.route(kept.as_str()).is_some());
        // The key map went with it: the same route registers fresh rather
        // than resurrecting a label nothing holds.
        let again = registry
            .register(&spec(mine, "/w/a", "http://localhost:5173/"))
            .unwrap();
        assert_eq!(again, gone);
        assert_eq!(registry.len(), 2);
    }

    /// A released name goes back only to the route it left — a stranger who
    /// asks for it gets the next one instead, because a page served under a
    /// name may have left a service worker behind it.
    #[test]
    fn a_retired_name_returns_only_to_the_route_that_left_it() {
        let mut registry = LabelRegistry::new();
        let mine = LabelInput {
            worktree_id: Some("a"),
            ..input("Proj", "ui-auth")
        };
        let stranger = LabelInput {
            worktree_id: Some("b"),
            ..input("Proj", "ui-auth")
        };
        let target = "http://localhost:5173/";
        let first = registry.register(&spec(mine, "/w/a", target)).unwrap();
        assert_eq!(registry.release_owner("/w/a"), vec![first.clone()]);
        assert_eq!(
            registry
                .register(&spec(stranger, "/w/b", target))
                .unwrap()
                .as_str(),
            "ui-auth-2"
        );
        // The one who left it may have it back.
        assert_eq!(
            registry.register(&spec(mine, "/w/a", target)).unwrap(),
            first
        );
    }

    /// A route needs an owner, or it could never be released.
    #[test]
    fn a_route_without_an_owner_is_refused() {
        let mut registry = LabelRegistry::new();
        assert_eq!(
            registry.register(&spec(
                input("Proj", "ui-auth"),
                "",
                "http://localhost:5173/"
            )),
            Err(RegisterError::EmptyOwner)
        );
        assert_eq!(
            registry.register(&spec(
                input("Proj", "ui-auth"),
                "/w/a",
                "https://localhost:5173/"
            )),
            Err(RegisterError::Target(TargetError::NotHttp))
        );
        assert!(registry.is_empty());
    }

    /// The labeled address keeps everything but the authority.
    #[test]
    fn a_labeled_url_keeps_the_targets_path_and_query_and_takes_the_proxy_port() {
        let target = Url::parse("http://127.0.0.1:5173/app?q=1#top").unwrap();
        let labeled = labeled_url(&HostLabel("ui-auth".to_string()), &target, 61234);
        assert_eq!(
            labeled.as_str(),
            "http://ui-auth.zerocode.localhost:61234/app?q=1#top"
        );
    }

    /// A Host header finds its route however it is cased, and a stranger
    /// finds nothing.
    #[test]
    fn a_host_header_finds_its_route_case_insensitively_and_a_stranger_finds_nothing() {
        assert_eq!(
            label_from_host_header(Some("UI-Auth.zerocode.localhost:61234")).as_deref(),
            Some("ui-auth")
        );
        for stranger in [
            "zerocode.localhost",
            ".zerocode.localhost",
            "ui-auth.orca.localhost",
            "localhost:5173",
        ] {
            assert!(
                label_from_host_header(Some(stranger)).is_none(),
                "{stranger}"
            );
        }
        assert!(label_from_host_header(None).is_none());
    }

    /// Every label this module can mint still fits inside one DNS label, and
    /// the whole host inside one name.
    #[test]
    fn every_label_we_can_mint_fits_inside_one_dns_label() {
        let widest = format!(
            "{}-{MAX_COLLISION_SUFFIX}",
            "a".repeat(HOST_LABEL_MAX_CHARS)
        );
        assert!(widest.len() <= 63, "{}", widest.len());
        assert!(widest.len() + LABEL_HOST_SUFFIX.len() <= 253);
    }
}
