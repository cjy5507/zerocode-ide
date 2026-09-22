//! A workspace's software composition, read from the lockfiles it already
//! has, and the known vulnerabilities in it, read from OSV's answers.
//!
//! Pure: no network and no disk. The window walks the workspace, reads the
//! lockfiles and asks OSV (`zerocode-shell`'s `supply_chain`); this module
//! decides what a component is, which components may be asked about at all,
//! what an answer means, and which nodes and edges the picture draws
//! (docs/design/knowledge-supply-chain-20260917.md §5).
//!
//! # What may leave the machine
//!
//! Only a package its lockfile says came from one of the two public
//! registries — crates.io and registry.npmjs.org — is ever put in a query
//! ([`osv_queries`]), and a query carries that package's ecosystem, name and
//! version and nothing else. A git dependency, a path dependency, a workspace
//! member, a package from any other registry and a package whose lockfile does
//! not say where it came from never leave: their names can be an
//! organisation's private vocabulary, and OSV knows nothing about them anyway.
//!
//! # Wire names
//!
//! `camelCase`, unlike this crate's `snake_case` rule: these types are the
//! window's command answer (§5.3), which reads camelCase the way it reads its
//! other command answers — `readiness::AgentReadinessSnapshot` is the same
//! exception for the same reason.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::second_brain_graph::EdgeProvenance;

/// A package ecosystem this layer reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ecosystem {
    Cargo,
    Npm,
}

/// Every spelling of one ecosystem, in one row: the lockfile that names its
/// packages, its package-URL type (purl-spec `PURL-TYPES`), and the word the
/// OSV schema files its advisories under ("Defined ecosystems").
struct EcosystemRow {
    ecosystem: Ecosystem,
    lockfile: &'static str,
    purl_type: &'static str,
    osv: &'static str,
}

/// One row per [`Ecosystem`], in declaration order (a test pins the order).
static ECOSYSTEMS: [EcosystemRow; 2] = [
    EcosystemRow {
        ecosystem: Ecosystem::Cargo,
        lockfile: "Cargo.lock",
        purl_type: "cargo",
        osv: "crates.io",
    },
    EcosystemRow {
        ecosystem: Ecosystem::Npm,
        lockfile: "package-lock.json",
        purl_type: "npm",
        osv: "npm",
    },
];

impl Ecosystem {
    fn row(self) -> &'static EcosystemRow {
        &ECOSYSTEMS[self as usize]
    }

    /// The ecosystem whose lockfile is named `file_name`.
    #[must_use]
    pub fn of_lockfile(file_name: &str) -> Option<Self> {
        ECOSYSTEMS
            .iter()
            .find(|row| row.lockfile == file_name)
            .map(|row| row.ecosystem)
    }

    /// The lockfile's file name.
    #[must_use]
    pub fn lockfile(self) -> &'static str {
        self.row().lockfile
    }

    /// The package-URL type (`pkg:<type>/…`).
    #[must_use]
    pub fn purl_type(self) -> &'static str {
        self.row().purl_type
    }

    /// The ecosystem as OSV names it, in a query and in a record's `affected`.
    #[must_use]
    pub fn osv_name(self) -> &'static str {
        self.row().osv
    }

    fn of_osv_name(word: &str) -> Option<Self> {
        ECOSYSTEMS
            .iter()
            .find(|row| row.osv == word)
            .map(|row| row.ecosystem)
    }
}

/// Where a component came from — and so whether it may be asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// crates.io or registry.npmjs.org: the only origin ever asked about.
    Registry,
    /// A git repository.
    Git,
    /// A directory, or a tarball on disk: a workspace member, a path
    /// dependency, an npm `file:` dependency.
    Path,
    /// Any other registry, or a package whose lockfile does not say where it
    /// came from.
    Private,
}

impl Origin {
    /// The wire spelling, matching the serde rename — also the word a
    /// component id carries when the lockfile names no source.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Registry => "registry",
            Self::Git => "git",
            Self::Path => "path",
            Self::Private => "private",
        }
    }
}

/// crates.io's index, in both spellings a Cargo.lock writes it: the git index
/// every lockfile has named since Cargo 1.0, and the sparse protocol's URL
/// (RFC 2789) in a lockfile written with crates.io replaced by it.
const CRATES_IO_SOURCES: [&str; 2] = [
    "registry+https://github.com/rust-lang/crates.io-index",
    "sparse+https://index.crates.io/",
];

/// How a Cargo.lock spells a git source: `git+https://…?branch=…#<commit>`.
const CARGO_GIT_SOURCE: &str = "git+";

/// What sets a git source's pinned commit off from the source itself. Cargo's
/// `SourceId` equality ignores that commit (`precise`), so a dependency entry
/// and a package are compared without it.
const CARGO_PRECISE_MARK: char = '#';

/// npm's public registry, as its tarball URLs begin. npm also reads this host
/// as "the configured default registry" when it installs, but it WRITES the
/// host it actually resolved against: a mirror's lockfile names the mirror.
const NPM_REGISTRY: &str = "https://registry.npmjs.org/";

/// The `resolved` spellings of a git dependency: npm's own `git+…` and
/// `git://` URLs and the hosted shortcuts npm-package-arg accepts.
const NPM_GIT_SOURCES: [&str; 6] = [
    "git+",
    "git://",
    "github:",
    "gitlab:",
    "bitbucket:",
    "gist:",
];

/// npm's spelling of a dependency on a file or a directory.
const NPM_FILE_SOURCE: &str = "file:";

/// The directory npm installs packages under — one path segment.
const NPM_NODE_MODULES: &str = "node_modules";

/// The dependency fields that name what is installed for ANY `packages`
/// entry, the edges out of every node …
const NPM_DEPENDENCY_FIELDS: [&str; 3] =
    ["dependencies", "optionalDependencies", "peerDependencies"];

/// … and the one field only a member's own entry installs: npm installs the
/// project's and each workspace's dev dependencies, never a dependency's.
const NPM_MEMBER_DEPENDENCY_FIELD: &str = "devDependencies";

/// RFC 3986's unreserved punctuation: the bytes a purl leaves unencoded in a
/// name segment and a version, beside letters and digits.
const PURL_UNRESERVED: &[u8] = b"-._~";

/// One package at one version from one source, as the lockfiles name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    /// Unique within an answer. A public-registry package is its purl — the
    /// registry makes a name and a version one package. Anything else is
    /// `<purl> (<source>)`, Cargo's own `name version (source)` spelling, with
    /// the origin standing in for a source the lockfile does not name: a
    /// member built from `crates/app` and crates.io's `app` at the same
    /// version are two components.
    pub id: String,
    /// `pkg:cargo/serde@1.0.219`, `pkg:npm/%40scope/name@1.2.3`.
    pub purl: String,
    pub ecosystem: Ecosystem,
    pub name: String,
    pub version: String,
    pub origin: Origin,
    /// Where the lockfile says it came from (Cargo's `source`, npm's
    /// `resolved`). `None` for a public-registry package, whose source is the
    /// registry, and for a directory the lockfile names no source for.
    pub source: Option<String>,
    /// Built from the workspace's own directories rather than fetched: a
    /// Cargo package without a source, an npm entry outside `node_modules`.
    /// Neither lockfile tells a workspace member from a path dependency.
    pub member: bool,
    /// The components this one depends on, as ids, sorted. Off the wire: the
    /// answer's [`SupplyGraph::edges`] carry them.
    #[serde(skip)]
    pub dependencies: Vec<String>,
    /// The lockfiles that name it, workspace-relative, sorted.
    pub lockfiles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupplyError {
    /// The lockfile is not the format its name promises.
    Unreadable { lockfile: String, why: String },
    /// An OSV answer that does not have the shape OSV documents.
    Answer(String),
}

impl SupplyError {
    fn unreadable(lockfile: &str, why: &str) -> Self {
        Self::Unreadable {
            lockfile: lockfile.to_string(),
            why: why.to_string(),
        }
    }
}

impl std::fmt::Display for SupplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable { lockfile, why } => write!(f, "{lockfile}: {why}"),
            Self::Answer(why) => write!(f, "OSV answer: {why}"),
        }
    }
}

impl std::error::Error for SupplyError {}

/// The package URL of `name@version` (purl-spec): the type, the name — an npm
/// scope is its namespace segment — and the version, each segment
/// percent-encoded outside letters, digits and RFC 3986's unreserved `-._~`, so
/// `@scope` reads `%40scope` and a build's `+` reads `%2B`.
#[must_use]
pub fn purl(ecosystem: Ecosystem, name: &str, version: &str) -> String {
    let path = name
        .split('/')
        .map(purl_encoded)
        .collect::<Vec<_>>()
        .join("/");
    let mut purl = format!("pkg:{}/{path}", ecosystem.purl_type());
    if !version.is_empty() {
        purl.push('@');
        purl.push_str(&purl_encoded(version));
    }
    purl
}

fn purl_encoded(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || PURL_UNRESERVED.contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            // Writing to a `String` cannot fail.
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// What makes two lockfile entries one component.
#[derive(Debug, Clone, Copy)]
struct Identity<'a> {
    ecosystem: Ecosystem,
    name: &'a str,
    version: &'a str,
    origin: Origin,
    source: Option<&'a str>,
}

impl Identity<'_> {
    /// The source a component keeps: none for a public-registry package.
    fn kept_source(&self) -> Option<&str> {
        self.source.filter(|_| self.origin != Origin::Registry)
    }

    fn id_of(&self, purl: &str) -> String {
        match (self.origin, self.kept_source()) {
            (Origin::Registry, _) => purl.to_string(),
            (_, Some(source)) => format!("{purl} ({source})"),
            (origin, None) => format!("{purl} ({})", origin.as_str()),
        }
    }

    fn id(&self) -> String {
        self.id_of(&purl(self.ecosystem, self.name, self.version))
    }

    fn component(&self, member: bool, dependencies: Vec<String>, lockfile: &str) -> Component {
        let purl = purl(self.ecosystem, self.name, self.version);
        Component {
            id: self.id_of(&purl),
            purl,
            ecosystem: self.ecosystem,
            name: self.name.to_string(),
            version: self.version.to_string(),
            origin: self.origin,
            source: self.kept_source().map(str::to_string),
            member,
            dependencies,
            lockfiles: vec![lockfile.to_string()],
        }
    }
}

/// The components a lockfile names, read by its ecosystem's reader.
///
/// # Errors
///
/// [`SupplyError::Unreadable`] when the text is not a lockfile of that shape.
pub fn lockfile_components(
    ecosystem: Ecosystem,
    text: &str,
    lockfile: &str,
) -> Result<Vec<Component>, SupplyError> {
    match ecosystem {
        Ecosystem::Cargo => cargo_lock_components(text, lockfile),
        Ecosystem::Npm => npm_lock_components(text, lockfile),
    }
}

#[derive(Deserialize)]
struct CargoLock {
    #[serde(default)]
    package: Vec<CargoLockPackage>,
}

#[derive(Deserialize)]
struct CargoLockPackage {
    name: String,
    version: String,
    source: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
}

impl CargoLockPackage {
    fn identity(&self) -> Identity<'_> {
        let source = self.source.as_deref();
        let origin = match source {
            None => Origin::Path,
            Some(source) if CRATES_IO_SOURCES.contains(&source) => Origin::Registry,
            Some(source) if source.starts_with(CARGO_GIT_SOURCE) => Origin::Git,
            Some(_) => Origin::Private,
        };
        Identity {
            ecosystem: Ecosystem::Cargo,
            name: &self.name,
            version: &self.version,
            origin,
            source,
        }
    }
}

/// A source as cargo compares two of them — without the commit it pins.
fn cargo_source_key(source: &str) -> &str {
    source
        .split_once(CARGO_PRECISE_MARK)
        .map_or(source, |(compared, _)| compared)
}

/// The package a Cargo.lock `dependencies` entry names, found the way cargo
/// finds it (`EncodablePackageId::from_str` and `lookup_id` in cargo's
/// `core/resolver/encode.rs`).
///
/// Cargo writes an entry as briefly as it stays unambiguous: `name` when one
/// version of the name is locked, `name version` when one source holds that
/// version, `name version (source)` otherwise. Without a source, the one
/// package built from a directory wins, as path dependencies never carry a
/// source. A name or a version that picks no single package is what cargo
/// calls a bad git merge and resolves to nothing, as it does there.
///
/// # Errors
///
/// The entry, when a third word is not a parenthesised source: cargo refuses
/// that lockfile ("invalid serialized `PackageId`").
fn cargo_dependency<'a>(
    by_name: &HashMap<&str, Vec<&'a CargoLockPackage>>,
    entry: &str,
) -> Result<Option<&'a CargoLockPackage>, String> {
    let mut words = entry.splitn(3, ' ');
    let name = words.next().unwrap_or_default();
    let version = words.next();
    let source = match words.next() {
        None => None,
        Some(word) => Some(
            word.strip_prefix('(')
                .and_then(|inner| inner.strip_suffix(')'))
                .ok_or_else(|| format!("`{entry}` is not a package id"))?,
        ),
    };
    let Some(named) = by_name.get(name) else {
        return Ok(None);
    };
    let version = if let Some(version) = version {
        version
    } else {
        let mut versions = named.iter().map(|package| package.version.as_str());
        let first = versions.next().unwrap_or_default();
        if versions.any(|other| other != first) {
            return Ok(None);
        }
        first
    };
    let candidates: Vec<&'a CargoLockPackage> = named
        .iter()
        .copied()
        .filter(|package| package.version == version)
        .collect();
    Ok(if let Some(source) = source {
        candidates.into_iter().find(|package| {
            package.source.as_deref().map(cargo_source_key) == Some(cargo_source_key(source))
        })
    } else {
        let mut directories = candidates.iter().filter(|package| package.source.is_none());
        match (directories.next(), directories.next()) {
            (Some(only), None) => Some(*only),
            (Some(_), Some(_)) => None,
            (None, _) => match candidates.as_slice() {
                [only] => Some(*only),
                _ => None,
            },
        }
    })
}

/// Every package a `Cargo.lock` holds, one component per package id.
///
/// # Errors
///
/// [`SupplyError::Unreadable`] when the text is not TOML of a lockfile's
/// shape, or a dependency entry is not a package id.
pub fn cargo_lock_components(text: &str, lockfile: &str) -> Result<Vec<Component>, SupplyError> {
    let lock: CargoLock =
        toml::from_str(text).map_err(|error| SupplyError::unreadable(lockfile, error.message()))?;
    let mut by_name: HashMap<&str, Vec<&CargoLockPackage>> = HashMap::new();
    for package in &lock.package {
        by_name
            .entry(package.name.as_str())
            .or_default()
            .push(package);
    }
    let mut components = Vec::with_capacity(lock.package.len());
    for package in &lock.package {
        let mut dependencies = Vec::with_capacity(package.dependencies.len());
        for entry in &package.dependencies {
            let found = cargo_dependency(&by_name, entry)
                .map_err(|why| SupplyError::unreadable(lockfile, &why))?;
            dependencies.extend(found.map(|found| found.identity().id()));
        }
        components.push(package.identity().component(
            package.source.is_none(),
            dependencies,
            lockfile,
        ));
    }
    Ok(merge_components([components]))
}

/// The byte offset of a key's last `node_modules` segment.
fn npm_last_node_modules(key: &str) -> Option<usize> {
    let mut at = 0;
    let mut found = None;
    for segment in key.split('/') {
        if segment == NPM_NODE_MODULES {
            found = Some(at);
        }
        at += segment.len() + 1;
    }
    found
}

/// The package directory a `node_modules` key is installed in:
/// `a/node_modules/b` is in `a`, `node_modules/@s/p` in the root (`""`).
fn npm_enclosing(key: &str) -> Option<&str> {
    npm_last_node_modules(key).map(|at| key[..at].trim_end_matches('/'))
}

/// Whether an entry is a link: a second name, under `node_modules`, for a
/// directory the lockfile lists under its own key.
fn npm_linked(entry: &Value) -> bool {
    entry.get("link").and_then(Value::as_bool).unwrap_or(false)
}

/// The origin a `resolved` value spells.
fn npm_origin(resolved: &str) -> Origin {
    if resolved.starts_with(NPM_REGISTRY) {
        Origin::Registry
    } else if NPM_GIT_SOURCES
        .iter()
        .any(|prefix| resolved.starts_with(prefix))
    {
        Origin::Git
    } else if resolved.starts_with(NPM_FILE_SOURCE) {
        Origin::Path
    } else {
        Origin::Private
    }
}

/// Where the entry at `key` came from, and the source that says so.
///
/// A member is its own directory. An installed entry says where with
/// `resolved`. A package bundled in another's tarball (`inBundle`) names no
/// source of its own and came from wherever its bundler came from. Anything
/// else is a package whose lockfile does not say — never a public name to ask
/// about.
fn npm_provenance<'a>(
    packages: &'a Map<String, Value>,
    key: &'a str,
    entry: &'a Value,
) -> (Origin, Option<&'a str>) {
    let Some(enclosing) = npm_enclosing(key) else {
        return (Origin::Path, None);
    };
    if let Some(resolved) = entry.get("resolved").and_then(Value::as_str) {
        return (npm_origin(resolved), Some(resolved));
    }
    let bundled = entry
        .get("inBundle")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if bundled
        && npm_last_node_modules(enclosing).is_some()
        && let Some(bundler) = packages.get(enclosing)
    {
        return npm_provenance(packages, enclosing, bundler);
    }
    (Origin::Private, None)
}

/// What one `packages` key installs.
#[derive(Debug, Clone, Copy)]
struct NpmInstalled<'a> {
    identity: Identity<'a>,
    member: bool,
}

/// The name an entry installs: its own `name` (an alias's real package, a
/// workspace's manifest name), else the folder under `node_modules`, else —
/// for the root and a workspace that wrote none — the lockfile's `name` or
/// the directory's.
fn npm_name<'a>(key: &'a str, entry: &'a Value, lock_name: Option<&'a str>) -> Option<&'a str> {
    if let Some(name) = entry.get("name").and_then(Value::as_str) {
        return Some(name);
    }
    match npm_last_node_modules(key) {
        Some(at) => key
            .get(at + NPM_NODE_MODULES.len() + 1..)
            .filter(|name| !name.is_empty()),
        None if key.is_empty() => lock_name,
        None => key.rsplit('/').next().filter(|name| !name.is_empty()),
    }
}

/// The key Node's `require(name)` loads from a package at `key`: the nearest
/// `node_modules/<name>` in the directories from `key` up to the root, never
/// searching inside a directory that is itself `node_modules` (Node's
/// `NODE_MODULES_PATHS`).
fn npm_resolve<'a>(
    installed: &HashMap<&str, NpmInstalled<'a>>,
    key: &str,
    name: &str,
) -> Option<NpmInstalled<'a>> {
    let segments: Vec<&str> = if key.is_empty() {
        Vec::new()
    } else {
        key.split('/').collect()
    };
    for end in (0..=segments.len()).rev() {
        if end > 0 && segments[end - 1] == NPM_NODE_MODULES {
            continue;
        }
        let directory = segments[..end].join("/");
        let candidate = if directory.is_empty() {
            format!("{NPM_NODE_MODULES}/{name}")
        } else {
            format!("{directory}/{NPM_NODE_MODULES}/{name}")
        };
        if let Some(found) = installed.get(candidate.as_str()) {
            return Some(*found);
        }
    }
    None
}

/// Every package a lockfileVersion 2 or 3 `package-lock.json` holds.
///
/// The root (`""`) and every key outside `node_modules` are members. A link is
/// not a component: it is how a member is reached from `node_modules`, and it
/// resolves to that member. A dependency is found the way Node finds it
/// (`NODE_MODULES_PATHS`), from the fields every entry installs plus a member's
/// `devDependencies`.
///
/// # Errors
///
/// [`SupplyError::Unreadable`] when the text is not JSON, or is a lockfile
/// older than version 2 (no `packages` map).
pub fn npm_lock_components(text: &str, lockfile: &str) -> Result<Vec<Component>, SupplyError> {
    let root: Value = serde_json::from_str(text)
        .map_err(|error| SupplyError::unreadable(lockfile, &error.to_string()))?;
    let packages = root
        .get("packages")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            SupplyError::unreadable(
                lockfile,
                "no `packages` map — lockfileVersion 1 is not read",
            )
        })?;
    let lock_name = root.get("name").and_then(Value::as_str);
    let mut installed: HashMap<&str, NpmInstalled<'_>> = HashMap::with_capacity(packages.len());
    for (key, entry) in packages {
        if npm_linked(entry) {
            continue;
        }
        let Some(name) = npm_name(key, entry, lock_name) else {
            continue;
        };
        let (origin, source) = npm_provenance(packages, key, entry);
        let identity = Identity {
            ecosystem: Ecosystem::Npm,
            name,
            version: entry
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            origin,
            source,
        };
        let member = npm_last_node_modules(key).is_none();
        installed.insert(key.as_str(), NpmInstalled { identity, member });
    }
    for (key, entry) in packages {
        if !npm_linked(entry) {
            continue;
        }
        let target = entry
            .get("resolved")
            .and_then(Value::as_str)
            .and_then(|target| installed.get(target).copied());
        if let Some(target) = target {
            installed.insert(key.as_str(), target);
        }
    }
    let mut components = Vec::with_capacity(packages.len());
    for (key, entry) in packages {
        if npm_linked(entry) {
            continue;
        }
        let Some(held) = installed.get(key.as_str()).copied() else {
            continue;
        };
        let member_field = held.member.then_some(&NPM_MEMBER_DEPENDENCY_FIELD);
        let dependencies = NPM_DEPENDENCY_FIELDS
            .iter()
            .chain(member_field)
            .filter_map(|field| entry.get(*field).and_then(Value::as_object))
            .flat_map(Map::keys)
            .filter_map(|name| npm_resolve(&installed, key, name))
            .map(|found| found.identity.id())
            .collect();
        components.push(held.identity.component(held.member, dependencies, lockfile));
    }
    Ok(merge_components([components]))
}

/// One component per id across every lockfile: the dependencies and the
/// lockfiles that name it are unioned. Sorted by id.
#[must_use]
pub fn merge_components(lists: impl IntoIterator<Item = Vec<Component>>) -> Vec<Component> {
    let mut merged: BTreeMap<String, Component> = BTreeMap::new();
    for component in lists.into_iter().flatten() {
        match merged.entry(component.id.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(component);
            }
            Entry::Occupied(mut slot) => {
                let held = slot.get_mut();
                held.member |= component.member;
                held.dependencies.extend(component.dependencies);
                held.lockfiles.extend(component.lockfiles);
            }
        }
    }
    merged
        .into_values()
        .map(|mut component| {
            component.dependencies.sort();
            component.dependencies.dedup();
            component.lockfiles.sort();
            component.lockfiles.dedup();
            component
        })
        .collect()
}

/// The identity of a set of lockfiles' contents: SHA-256 over each path and
/// its text, each length-prefixed so two different sets can never spell one
/// stream. Order-sensitive; the caller hands them sorted.
#[must_use]
pub fn lockfile_fingerprint<'a>(lockfiles: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let mut hasher = Sha256::new();
    for (path, text) in lockfiles {
        for part in [path.as_bytes(), text.as_bytes()] {
            hasher.update((part.len() as u64).to_le_bytes());
            hasher.update(part);
        }
    }
    format!("{:x}", hasher.finalize())
}

/// How many queries one `POST /v1/querybatch` may carry — the service's own
/// bound, as its Go bindings name it (`osvdev.MaxQueriesPerQueryBatchRequest`).
pub const OSV_BATCH_MAX_QUERIES: usize = 1_000;

/// Where a vulnerability's page is on osv.dev — the card's outbound link.
pub const OSV_VULNERABILITY_PAGE: &str = "https://osv.dev/vulnerability/";

/// One question OSV may be sent. Only [`osv_queries`] makes one, and it makes
/// one only for a component the privacy rule lets leave the machine — so a
/// query body cannot be built for anything else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsvQuery {
    purl: String,
    ecosystem: Ecosystem,
    name: String,
    version: String,
    page_token: Option<String>,
}

impl OsvQuery {
    /// The purl of the component asked about — its id, being a
    /// public-registry package's.
    #[must_use]
    pub fn purl(&self) -> &str {
        &self.purl
    }

    /// The page token this asks with, for a later page of an answer.
    #[must_use]
    pub fn page_token(&self) -> Option<&str> {
        self.page_token.as_deref()
    }

    /// The same question, asking for the page `token` names.
    #[must_use]
    pub fn next_page(&self, token: &str) -> Self {
        Self {
            page_token: Some(token.to_string()),
            ..self.clone()
        }
    }
}

/// The questions the components may be asked: a public-registry package with
/// a version, never a member — a version-less query would be answered with
/// every advisory the package ever had. Sorted by purl.
#[must_use]
pub fn osv_queries(components: &[Component]) -> Vec<OsvQuery> {
    let mut queries: Vec<OsvQuery> = components
        .iter()
        .filter(|component| {
            component.origin == Origin::Registry
                && !component.member
                && !component.version.is_empty()
        })
        .map(|component| OsvQuery {
            purl: component.purl.clone(),
            ecosystem: component.ecosystem,
            name: component.name.clone(),
            version: component.version.clone(),
            page_token: None,
        })
        .collect();
    queries.sort_by(|left, right| left.purl.cmp(&right.purl));
    queries
}

/// The body of `POST /v1/querybatch`: each question in OSV's package form —
/// ecosystem, name, version — and its page token when it asks for a later
/// page. Nothing else about the workspace is in it.
#[must_use]
pub fn osv_batch_body(queries: &[OsvQuery]) -> Value {
    let queries: Vec<Value> = queries
        .iter()
        .map(|query| {
            let mut asked = json!({
                "package": { "ecosystem": query.ecosystem.osv_name(), "name": query.name },
                "version": query.version,
            });
            if let Some(token) = &query.page_token {
                asked["page_token"] = json!(token);
            }
            asked
        })
        .collect();
    json!({ "queries": queries })
}

/// What OSV answered for one question of a batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsvBatchResult {
    /// The advisory ids, in OSV's order. A batch answer carries ids only; the
    /// records are a second read each ([`read_osv_record`]).
    pub ids: Vec<String>,
    /// Set when more ids wait on a later page: ask the same question again
    /// with it ([`OsvQuery::next_page`]).
    pub next_page_token: Option<String>,
}

/// Read a `POST /v1/querybatch` answer to `asked` questions, in their order.
///
/// # Errors
///
/// [`SupplyError::Answer`] when the answer is not `{ "results": [...] }` with
/// one result per question, or names a vulnerability without an id.
pub fn read_osv_batch(answer: &Value, asked: usize) -> Result<Vec<OsvBatchResult>, SupplyError> {
    let results = answer
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| SupplyError::Answer("no `results` array".to_string()))?;
    if results.len() != asked {
        return Err(SupplyError::Answer(format!(
            "{} results for {asked} queries",
            results.len()
        )));
    }
    results
        .iter()
        .map(|result| {
            let ids = match result.get("vulns") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(vulns)) => vulns
                    .iter()
                    .map(|vuln| {
                        vuln.get("id")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .ok_or_else(|| {
                                SupplyError::Answer("a vulnerability without an `id`".to_string())
                            })
                    })
                    .collect::<Result<_, _>>()?,
                Some(_) => return Err(SupplyError::Answer("`vulns` is not an array".to_string())),
            };
            let next_page_token = result
                .get("next_page_token")
                .and_then(Value::as_str)
                .filter(|token| !token.is_empty())
                .map(str::to_string);
            Ok(OsvBatchResult {
                ids,
                next_page_token,
            })
        })
        .collect()
}

/// The qualitative ratings of CVSS v3.1 §5 (Table 14), and `Unknown` for a
/// record that neither carries a v3 vector nor a database's own rating.
/// Ordered least to most severe, `Unknown` below all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Unknown,
    None,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    /// The rating's one word — the same word the wire carries, so a document
    /// and a payload can never spell one rating two ways
    /// (`a_severitys_word_is_its_wire_word` holds them together).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

/// CVSS v3.1 §5, Table 14: each rating's lowest score, most severe first. A
/// score below the last row is 0.0, rated "None".
const CVSS_RATINGS: [(f64, Severity); 4] = [
    (9.0, Severity::Critical),
    (7.0, Severity::High),
    (4.0, Severity::Medium),
    (0.1, Severity::Low),
];

/// The rating a CVSS v3 base score is given.
#[must_use]
pub fn severity_of_score(score: f64) -> Severity {
    CVSS_RATINGS
        .iter()
        .find(|(floor, _)| score >= *floor)
        .map_or(Severity::None, |(_, rating)| *rating)
}

/// GitHub's advisory database writes its own rating beside the vectors
/// (`database_specific.severity` on a GHSA record). It rates a record that has
/// no v3 vector to score — a v4-only record.
const DATABASE_RATINGS: [(&str, Severity); 4] = [
    ("CRITICAL", Severity::Critical),
    ("HIGH", Severity::High),
    ("MODERATE", Severity::Medium),
    ("LOW", Severity::Low),
];

/// The vector prefixes scored here. v3.0 and v3.1 share the base equations;
/// v3.1 only re-spelled `Roundup` (see [`cvss_roundup`]).
const CVSS3_VERSIONS: [&str; 2] = ["CVSS:3.0", "CVSS:3.1"];

/// One base metric and its weights (CVSS v3.1 §7.4, Table 16).
struct CvssMetric {
    key: &'static str,
    weights: &'static [(&'static str, f64)],
}

const ATTACK_VECTOR: CvssMetric = CvssMetric {
    key: "AV",
    weights: &[("N", 0.85), ("A", 0.62), ("L", 0.55), ("P", 0.2)],
};
const ATTACK_COMPLEXITY: CvssMetric = CvssMetric {
    key: "AC",
    weights: &[("L", 0.77), ("H", 0.44)],
};
/// Privileges Required weighs more when the scope changes, so it is two rows.
const PRIVILEGES_SCOPE_UNCHANGED: CvssMetric = CvssMetric {
    key: "PR",
    weights: &[("N", 0.85), ("L", 0.62), ("H", 0.27)],
};
const PRIVILEGES_SCOPE_CHANGED: CvssMetric = CvssMetric {
    key: "PR",
    weights: &[("N", 0.85), ("L", 0.68), ("H", 0.5)],
};
const USER_INTERACTION: CvssMetric = CvssMetric {
    key: "UI",
    weights: &[("N", 0.85), ("R", 0.62)],
};
/// Confidentiality, Integrity and Availability share one set of weights.
const IMPACT_KEYS: [&str; 3] = ["C", "I", "A"];
const IMPACT_WEIGHTS: &[(&str, f64)] = &[("H", 0.56), ("L", 0.22), ("N", 0.0)];
/// Scope: `U`nchanged or `C`hanged.
const SCOPE_KEY: &str = "S";
const SCOPE_VALUES: [(&str, bool); 2] = [("U", false), ("C", true)];

/// The base equations' coefficients (CVSS v3.1 §7.1), named after the terms
/// they scale.
const IMPACT_SCOPE_UNCHANGED: f64 = 6.42;
const IMPACT_SCOPE_CHANGED: f64 = 7.52;
const IMPACT_SCOPE_CHANGED_OFFSET: f64 = 0.029;
const IMPACT_SCOPE_CHANGED_TAIL: f64 = 3.25;
const IMPACT_SCOPE_CHANGED_TAIL_OFFSET: f64 = 0.02;
const IMPACT_SCOPE_CHANGED_TAIL_POWER: i32 = 15;
const EXPLOITABILITY: f64 = 8.22;
const SCOPE_CHANGED_FACTOR: f64 = 1.08;
const MAX_BASE_SCORE: f64 = 10.0;

/// `Roundup`'s working unit (CVSS v3.1 Appendix A): hundred-thousandths, and
/// the ten thousand of them in a tenth.
const ROUNDUP_SCALE: f64 = 100_000.0;
const ROUNDUP_TENTH: f64 = 10_000.0;

/// CVSS v3.1's `Roundup`: the smallest one-decimal number not below the input,
/// decided on a whole count of hundred-thousandths so float error cannot lift
/// 4.000000000000001 to 4.1. v3.0 spelled it as a plain ceiling; across all
/// 2,592 base vectors the two spellings agree (checked exhaustively when this
/// was written), so one `Roundup` scores both versions.
fn cvss_roundup(value: f64) -> f64 {
    let tenths = (value * ROUNDUP_SCALE).round() / ROUNDUP_TENTH;
    let tenths = if tenths.fract() > 0.0 {
        tenths.floor() + 1.0
    } else {
        tenths
    };
    tenths / (ROUNDUP_SCALE / ROUNDUP_TENTH)
}

/// The base score of a CVSS v3.0 or v3.1 vector (`CVSS:3.1/AV:N/AC:L/…`), by
/// the specification's equations (§7.1) and weights (§7.4). `None` for any
/// other version, a missing or unknown base metric value, or a metric written
/// twice — a vector the specification calls invalid.
#[must_use]
pub fn cvss3_base_score(vector: &str) -> Option<f64> {
    let mut parts = vector.split('/');
    if !CVSS3_VERSIONS.contains(&parts.next()?) {
        return None;
    }
    let mut metrics: HashMap<&str, &str> = HashMap::new();
    for part in parts {
        let (key, value) = part.split_once(':')?;
        if metrics.insert(key, value).is_some() {
            return None;
        }
    }
    let weight_in = |key: &str, weights: &[(&str, f64)]| -> Option<f64> {
        let value = metrics.get(key)?;
        weights
            .iter()
            .find(|(named, _)| named == value)
            .map(|(_, weight)| *weight)
    };
    let weight = |metric: &CvssMetric| weight_in(metric.key, metric.weights);
    let scope_changed = SCOPE_VALUES
        .iter()
        .find(|(named, _)| Some(named) == metrics.get(SCOPE_KEY))
        .map(|(_, changed)| *changed)?;
    let privileges = if scope_changed {
        weight(&PRIVILEGES_SCOPE_CHANGED)?
    } else {
        weight(&PRIVILEGES_SCOPE_UNCHANGED)?
    };
    let exploitability = EXPLOITABILITY
        * weight(&ATTACK_VECTOR)?
        * weight(&ATTACK_COMPLEXITY)?
        * privileges
        * weight(&USER_INTERACTION)?;
    let mut unharmed = 1.0;
    for key in IMPACT_KEYS {
        unharmed *= 1.0 - weight_in(key, IMPACT_WEIGHTS)?;
    }
    let impact_subscore = 1.0 - unharmed;
    let impact = if scope_changed {
        IMPACT_SCOPE_CHANGED * (impact_subscore - IMPACT_SCOPE_CHANGED_OFFSET)
            - IMPACT_SCOPE_CHANGED_TAIL
                * (impact_subscore - IMPACT_SCOPE_CHANGED_TAIL_OFFSET)
                    .powi(IMPACT_SCOPE_CHANGED_TAIL_POWER)
    } else {
        IMPACT_SCOPE_UNCHANGED * impact_subscore
    };
    if impact <= 0.0 {
        return Some(0.0);
    }
    let total = if scope_changed {
        SCOPE_CHANGED_FACTOR * (impact + exploitability)
    } else {
        impact + exploitability
    };
    Some(cvss_roundup(total.min(MAX_BASE_SCORE)))
}

/// A version a fix landed in, for one package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fix {
    pub ecosystem: Ecosystem,
    pub name: String,
    pub version: String,
}

/// One OSV record, as much of it as the picture and the card use — what the
/// cache keeps between lookups.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OsvRecord {
    /// `RUSTSEC-2020-0071`, `GHSA-wcg3-cvx6-7396`.
    pub id: String,
    /// The other databases' names for it, sorted, without its own id.
    pub aliases: Vec<String>,
    /// The record's summary, or the first line of its details.
    pub summary: String,
    pub severity: Severity,
    /// The CVSS v3 base score, when the record carries a v3 vector.
    pub score: Option<f64>,
    /// The `fixed` events of every range, per package of an ecosystem this
    /// layer reads, in the record's own order.
    pub fixed: Vec<Fix>,
    /// The Rust advisory database's word for an advisory that is not a
    /// vulnerability as such:
    /// `unmaintained`, `unsound`, `notice`.
    pub informational: Option<String>,
    /// The database took it back. A withdrawn record draws nothing.
    pub withdrawn: bool,
}

/// Read a `GET /v1/vulns/{id}` answer.
///
/// # Errors
///
/// [`SupplyError::Answer`] when the record has no `id`.
pub fn read_osv_record(record: &Value) -> Result<OsvRecord, SupplyError> {
    let id = record
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| SupplyError::Answer("a vulnerability without an `id`".to_string()))?
        .to_string();
    let mut aliases: Vec<String> = record
        .get("aliases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|alias| *alias != id)
        .map(str::to_string)
        .collect();
    aliases.sort();
    aliases.dedup();
    let score = record
        .get("severity")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("CVSS_V3"))
        .filter_map(|entry| entry.get("score").and_then(Value::as_str))
        .find_map(cvss3_base_score);
    let severity = score.map_or_else(|| database_rating(record), severity_of_score);
    let affected: Vec<&Value> = record
        .get("affected")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let mut fixed: Vec<Fix> = Vec::new();
    for entry in &affected {
        let package = entry.get("package");
        let ecosystem = package
            .and_then(|package| package.get("ecosystem"))
            .and_then(Value::as_str)
            .and_then(Ecosystem::of_osv_name);
        let name = package
            .and_then(|package| package.get("name"))
            .and_then(Value::as_str);
        let (Some(ecosystem), Some(name)) = (ecosystem, name) else {
            continue;
        };
        let versions = entry
            .get("ranges")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|range| range.get("events").and_then(Value::as_array))
            .flatten()
            .filter_map(|event| event.get("fixed").and_then(Value::as_str));
        for version in versions {
            let fix = Fix {
                ecosystem,
                name: name.to_string(),
                version: version.to_string(),
            };
            if !fixed.contains(&fix) {
                fixed.push(fix);
            }
        }
    }
    let informational = affected
        .iter()
        .filter_map(|entry| entry.get("database_specific"))
        .find_map(|specific| specific.get("informational").and_then(Value::as_str))
        .map(str::to_string);
    let summary = record
        .get("summary")
        .and_then(Value::as_str)
        .filter(|summary| !summary.trim().is_empty())
        .or_else(|| {
            record
                .get("details")
                .and_then(Value::as_str)
                .and_then(|details| details.lines().find(|line| !line.trim().is_empty()))
        })
        .unwrap_or_default()
        .trim()
        .to_string();
    Ok(OsvRecord {
        id,
        aliases,
        summary,
        severity,
        score,
        fixed,
        informational,
        withdrawn: record.get("withdrawn").is_some_and(|when| !when.is_null()),
    })
}

fn database_rating(record: &Value) -> Severity {
    let word = record
        .get("database_specific")
        .and_then(|specific| specific.get("severity"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    DATABASE_RATINGS
        .iter()
        .find(|(named, _)| named.eq_ignore_ascii_case(word))
        .map_or(Severity::Unknown, |(_, rating)| *rating)
}

/// The advisory databases in the order a group of aliases is named by: the
/// ecosystem's own database first (the Rust advisory database files crates),
/// then GitHub's, which
/// files every ecosystem. Any other id comes after both, alphabetically.
const ADVISORY_ID_PRECEDENCE: [&str; 2] = ["RUSTSEC-", "GHSA-"];

/// One vulnerability as the picture draws it: every OSV record that names the
/// same advisory (through `aliases`) folded into one triangle.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Vulnerability {
    /// The group's name: a `RUSTSEC-` id, else a `GHSA-` id, else the first
    /// other id alphabetically.
    pub id: String,
    /// Every other name the group's records carry, sorted.
    pub aliases: Vec<String>,
    pub summary: String,
    /// The most severe rating any of the group's records gives.
    pub severity: Severity,
    /// The highest CVSS v3 base score among them.
    pub score: Option<f64>,
    /// The fixes for the packages this vulnerability affects in this
    /// workspace, in the records' order.
    pub fixed: Vec<Fix>,
    pub informational: Option<String>,
    /// The record on osv.dev.
    pub url: String,
}

/// What an edge means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupplyEdgeKind {
    /// Component → component. The vault's own word for the same relation
    /// (`second_brain_graph::EdgeKind::DependsOn`), so one kind of line draws
    /// both; a test pins the two spellings together.
    DependsOn,
    /// Vulnerability → component.
    Affects,
}

/// Reading a graph as work: what to raise, and what raising it closes.
pub mod report;

pub use report::{ReportAdvisory, ReportRaise, SupplyReport, markdown, report};

/// One relation, as indices: `from` and `to` index
/// [`SupplyGraph::components`], except an `affects` edge's `from`, which
/// indexes [`SupplyGraph::vulnerabilities`]. Indices rather than ids for the
/// vault graph's reason: the window turns them straight into typed arrays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SupplyEdge {
    pub from: u32,
    pub to: u32,
    pub kind: SupplyEdgeKind,
    /// The road that wrote the line (t-5966): a lockfile or an OSV record is
    /// the machine's measurement, so every supply edge is
    /// [`EdgeProvenance::Measured`] — carried on the wire so the window
    /// dresses and filters it from the answer.
    pub provenance: EdgeProvenance,
}

/// The components, the vulnerabilities that reach them, and the edges.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupplyGraph {
    /// Sorted by id.
    pub components: Vec<Component>,
    /// Most severe first, then by id.
    pub vulnerabilities: Vec<Vulnerability>,
    /// Sorted by kind, then `from`, then `to`.
    pub edges: Vec<SupplyEdge>,
}

/// What OSV said about the components asked: by purl, the ids of the records
/// each was answered with. A purl OSV knows nothing about is absent.
pub type OsvFindings = BTreeMap<String, Vec<String>>;

/// The bounds keep a graph far below `u32::MAX` nodes; saturating is the
/// answer that cannot forge an index into another node.
fn index_of(at: usize) -> u32 {
    u32::try_from(at).unwrap_or(u32::MAX)
}

/// The records grouped by alias: two records are one advisory when either
/// names the other. Groups in order of their first record.
fn alias_groups(records: &[&OsvRecord]) -> Vec<Vec<usize>> {
    fn root(parent: &mut [usize], mut at: usize) -> usize {
        while parent[at] != at {
            parent[at] = parent[parent[at]];
            at = parent[at];
        }
        at
    }
    let by_id: HashMap<&str, usize> = records
        .iter()
        .enumerate()
        .map(|(at, record)| (record.id.as_str(), at))
        .collect();
    let mut parent: Vec<usize> = (0..records.len()).collect();
    for (at, record) in records.iter().enumerate() {
        for alias in &record.aliases {
            if let Some(&other) = by_id.get(alias.as_str()) {
                let (left, right) = (root(&mut parent, at), root(&mut parent, other));
                parent[left.max(right)] = left.min(right);
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for at in 0..records.len() {
        let group = root(&mut parent, at);
        groups.entry(group).or_default().push(at);
    }
    groups.into_values().collect()
}

/// The order ids are chosen by to name a group.
fn advisory_rank(id: &str) -> (usize, &str) {
    let rank = ADVISORY_ID_PRECEDENCE
        .iter()
        .position(|prefix| id.starts_with(prefix))
        .unwrap_or(ADVISORY_ID_PRECEDENCE.len());
    (rank, id)
}

/// The picture: components with their `depends_on` edges, and — from what OSV
/// found — one vulnerability per advisory with its `affects` edges. A record
/// that is withdrawn, or that no component was answered with, draws nothing.
#[must_use]
pub fn supply_graph(
    components: Vec<Component>,
    findings: &OsvFindings,
    records: &[OsvRecord],
) -> SupplyGraph {
    let position: HashMap<&str, usize> = components
        .iter()
        .enumerate()
        .map(|(at, component)| (component.id.as_str(), at))
        .collect();
    let mut edges: BTreeSet<(SupplyEdgeKind, u32, u32)> = BTreeSet::new();
    for (at, component) in components.iter().enumerate() {
        for dependency in &component.dependencies {
            if let Some(&to) = position.get(dependency.as_str()) {
                edges.insert((SupplyEdgeKind::DependsOn, index_of(at), index_of(to)));
            }
        }
    }

    let mut live: Vec<&OsvRecord> = records.iter().filter(|record| !record.withdrawn).collect();
    live.sort_by(|left, right| left.id.cmp(&right.id));
    live.dedup_by(|left, right| left.id == right.id);
    let groups = alias_groups(&live);
    let records_in_order = &live;
    let group_of: HashMap<&str, usize> = groups
        .iter()
        .enumerate()
        .flat_map(|(group, members)| {
            members
                .iter()
                .map(move |at| (records_in_order[*at].id.as_str(), group))
        })
        .collect();

    let mut affected: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); groups.len()];
    for (at, component) in components.iter().enumerate() {
        let Some(ids) = findings.get(&component.purl) else {
            continue;
        };
        if component.id != component.purl {
            // Only a public-registry component is ever asked about, and its
            // id is its purl: a finding never lands on a private namesake.
            continue;
        }
        for id in ids {
            if let Some(&group) = group_of.get(id.as_str()) {
                affected[group].insert(at);
            }
        }
    }

    let mut vulnerabilities: Vec<(Vulnerability, &BTreeSet<usize>)> = groups
        .iter()
        .zip(&affected)
        .filter(|(_, reached)| !reached.is_empty())
        .map(|(members, reached)| {
            let members: Vec<&OsvRecord> = members.iter().map(|at| live[*at]).collect();
            (fold_group(&members, &components, reached), reached)
        })
        .collect();
    vulnerabilities.sort_by(|(left, _), (right, _)| {
        right
            .severity
            .cmp(&left.severity)
            .then_with(|| left.id.cmp(&right.id))
    });
    for (at, (_, reached)) in vulnerabilities.iter().enumerate() {
        for component in *reached {
            edges.insert((SupplyEdgeKind::Affects, index_of(at), index_of(*component)));
        }
    }
    SupplyGraph {
        vulnerabilities: vulnerabilities
            .into_iter()
            .map(|(vulnerability, _)| vulnerability)
            .collect(),
        edges: edges
            .into_iter()
            .map(|(kind, from, to)| SupplyEdge {
                from,
                to,
                kind,
                provenance: EdgeProvenance::Measured,
            })
            .collect(),
        components,
    }
}

/// One group of alias records as one vulnerability.
fn fold_group(
    members: &[&OsvRecord],
    components: &[Component],
    reached: &BTreeSet<usize>,
) -> Vulnerability {
    let named = members
        .iter()
        .min_by(|left, right| advisory_rank(&left.id).cmp(&advisory_rank(&right.id)))
        .copied()
        .unwrap_or(members[0]);
    let mut aliases: Vec<String> = members
        .iter()
        .flat_map(|record| std::iter::once(&record.id).chain(&record.aliases))
        .filter(|alias| **alias != named.id)
        .cloned()
        .collect();
    aliases.sort();
    aliases.dedup();
    let summary = std::iter::once(named)
        .chain(members.iter().copied())
        .map(|record| record.summary.as_str())
        .find(|summary| !summary.is_empty())
        .unwrap_or_default()
        .to_string();
    let packages: BTreeSet<(Ecosystem, &str)> = reached
        .iter()
        .map(|at| (components[*at].ecosystem, components[*at].name.as_str()))
        .collect();
    let mut fixed: Vec<Fix> = Vec::new();
    for fix in members.iter().flat_map(|record| &record.fixed) {
        if packages.contains(&(fix.ecosystem, fix.name.as_str())) && !fixed.contains(fix) {
            fixed.push(fix.clone());
        }
    }
    Vulnerability {
        id: named.id.clone(),
        aliases,
        summary,
        severity: members
            .iter()
            .map(|record| record.severity)
            .max()
            .unwrap_or(Severity::Unknown),
        score: members
            .iter()
            .filter_map(|record| record.score)
            .reduce(f64::max),
        fixed,
        informational: members
            .iter()
            .find_map(|record| record.informational.clone()),
        url: format!("{OSV_VULNERABILITY_PAGE}{}", named.id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ecosystem_row_sits_at_its_variant_and_spells_the_wire_word() {
        for (at, row) in ECOSYSTEMS.iter().enumerate() {
            assert_eq!(
                row.ecosystem as usize, at,
                "{} is out of order",
                row.lockfile
            );
            assert_eq!(
                serde_json::to_value(row.ecosystem).expect("an ecosystem"),
                json!(row.purl_type),
                "the wire word is the purl type"
            );
            assert_eq!(Ecosystem::of_lockfile(row.lockfile), Some(row.ecosystem));
            assert_eq!(Ecosystem::of_osv_name(row.osv), Some(row.ecosystem));
        }
        assert_eq!(Ecosystem::of_lockfile("yarn.lock"), None);
        for origin in [Origin::Registry, Origin::Git, Origin::Path, Origin::Private] {
            assert_eq!(
                serde_json::to_value(origin).expect("an origin"),
                json!(origin.as_str())
            );
        }
    }

    #[test]
    fn a_purl_encodes_an_npm_scope_and_a_builds_plus() {
        assert_eq!(
            purl(Ecosystem::Npm, "@scope/name", "1.2.3"),
            "pkg:npm/%40scope/name@1.2.3"
        );
        assert_eq!(
            purl(Ecosystem::Cargo, "toml", "0.9.12+spec-1.1.0"),
            "pkg:cargo/toml@0.9.12%2Bspec-1.1.0"
        );
        assert_eq!(
            purl(Ecosystem::Npm, "site", ""),
            "pkg:npm/site",
            "no version, no `@`"
        );
    }

    const CARGO_LOCK: &str = r#"
version = 4

[[package]]
name = "app"
version = "0.1.0"
dependencies = [
 "helper",
 "serde",
 "time 0.3.36",
]

[[package]]
name = "helper"
version = "0.2.0"
source = "git+https://github.com/example/helper?branch=main#abc123"

[[package]]
name = "serde"
version = "1.0.219"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "00"

[[package]]
name = "time"
version = "0.1.45"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "time"
version = "0.3.36"
source = "sparse+https://index.crates.io/"
dependencies = [
 "serde",
]

[[package]]
name = "internal-secret-crate"
version = "9.9.9"
source = "registry+https://crates.example.com/index"
"#;

    fn by<'a>(components: &'a [Component], id: &str) -> &'a Component {
        components
            .iter()
            .find(|component| component.id == id)
            .unwrap_or_else(|| panic!("no component `{id}` in {components:#?}"))
    }

    #[test]
    fn a_cargo_lock_names_members_origins_and_resolved_dependencies() {
        let components = cargo_lock_components(CARGO_LOCK, "Cargo.lock").expect("lockfile");
        let app = by(&components, "pkg:cargo/app@0.1.0 (path)");
        assert!(app.member);
        assert_eq!(app.origin, Origin::Path);
        assert_eq!(app.source, None);
        assert_eq!(
            app.dependencies,
            vec![
                "pkg:cargo/helper@0.2.0 (git+https://github.com/example/helper?branch=main#abc123)"
                    .to_string(),
                "pkg:cargo/serde@1.0.219".to_string(),
                "pkg:cargo/time@0.3.36".to_string(),
            ],
            "a name alone resolves to its one package, a name and version to that version"
        );
        let helper = by(
            &components,
            "pkg:cargo/helper@0.2.0 (git+https://github.com/example/helper?branch=main#abc123)",
        );
        assert_eq!(helper.origin, Origin::Git);
        assert!(!helper.member);
        let serde = by(&components, "pkg:cargo/serde@1.0.219");
        assert_eq!(
            (serde.origin, serde.source.as_deref()),
            (Origin::Registry, None)
        );
        assert_eq!(
            by(&components, "pkg:cargo/time@0.3.36").origin,
            Origin::Registry,
            "the sparse index is crates.io too"
        );
        assert_eq!(
            by(
                &components,
                "pkg:cargo/internal-secret-crate@9.9.9 (registry+https://crates.example.com/index)"
            )
            .origin,
            Origin::Private
        );
        assert_eq!(components.len(), 6);
    }

    #[test]
    fn a_name_alone_resolves_only_when_one_version_of_it_is_locked() {
        let lock = r#"
[[package]]
name = "app"
version = "0.1.0"
dependencies = ["dup"]

[[package]]
name = "dup"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "dup"
version = "2.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
        let components = cargo_lock_components(lock, "Cargo.lock").expect("lockfile");
        assert!(
            by(&components, "pkg:cargo/app@0.1.0 (path)")
                .dependencies
                .is_empty(),
            "cargo resolves an ambiguous bare name to nothing, not to the first version"
        );
    }

    #[test]
    fn without_a_source_the_one_directory_package_is_the_dependency_as_cargo_decides() {
        let lock = r#"
[[package]]
name = "app"
version = "0.1.0"
dependencies = ["shim 1.0.0", "twin 1.0.0 (git+https://github.com/example/twin#def)"]

[[package]]
name = "shim"
version = "1.0.0"

[[package]]
name = "shim"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "twin"
version = "1.0.0"
source = "git+https://github.com/example/twin#abc"

[[package]]
name = "twin"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
        let components = cargo_lock_components(lock, "Cargo.lock").expect("lockfile");
        assert_eq!(
            by(&components, "pkg:cargo/app@0.1.0 (path)").dependencies,
            vec![
                "pkg:cargo/shim@1.0.0 (path)".to_string(),
                "pkg:cargo/twin@1.0.0 (git+https://github.com/example/twin#abc)".to_string(),
            ],
            "no source picks the path package; a source is matched without its pinned commit"
        );
        assert_eq!(
            components
                .iter()
                .filter(|component| component.name == "shim")
                .count(),
            2,
            "a directory package and a registry package of one name and version are two"
        );
        let asked: Vec<String> = osv_queries(&components)
            .iter()
            .map(|query| query.purl().to_string())
            .collect();
        assert_eq!(
            asked,
            vec![
                "pkg:cargo/shim@1.0.0".to_string(),
                "pkg:cargo/twin@1.0.0".to_string()
            ],
            "the registry twin of a member is still asked about"
        );
    }

    #[test]
    fn a_dependency_entry_that_is_not_a_package_id_refuses_the_lockfile() {
        let lock = r#"
[[package]]
name = "app"
version = "0.1.0"
dependencies = ["serde 1.0.219 registry+https://github.com/rust-lang/crates.io-index"]

[[package]]
name = "serde"
version = "1.0.219"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
        let error = cargo_lock_components(lock, "Cargo.lock").expect_err("cargo refuses it too");
        assert!(
            matches!(error, SupplyError::Unreadable { ref lockfile, .. } if lockfile == "Cargo.lock")
        );
    }

    #[test]
    fn a_lockfile_that_is_not_toml_is_named_in_the_refusal() {
        let error =
            cargo_lock_components("[[package]\nname =", "zo-ide/Cargo.lock").expect_err("broken");
        assert!(
            matches!(error, SupplyError::Unreadable { ref lockfile, .. } if lockfile == "zo-ide/Cargo.lock")
        );
    }

    const NPM_LOCK: &str = r#"{
  "name": "site",
  "lockfileVersion": 3,
  "packages": {
    "": {
      "name": "site",
      "workspaces": ["packages/tool"],
      "dependencies": { "left": "^1.0.0", "@scope/pkg": "^2.0.0" },
      "devDependencies": { "tester": "^4.0.0" }
    },
    "node_modules/left": {
      "version": "1.0.0",
      "resolved": "https://registry.npmjs.org/left/-/left-1.0.0.tgz",
      "dependencies": { "right": "^3.0.0" },
      "devDependencies": { "tester": "^4.0.0" }
    },
    "node_modules/left/node_modules/right": {
      "version": "3.1.0",
      "resolved": "https://registry.npmjs.org/right/-/right-3.1.0.tgz"
    },
    "node_modules/right": {
      "version": "2.0.0",
      "resolved": "https://registry.npmjs.org/right/-/right-2.0.0.tgz"
    },
    "node_modules/tester": {
      "version": "4.0.0",
      "resolved": "https://registry.npmjs.org/tester/-/tester-4.0.0.tgz",
      "dev": true
    },
    "node_modules/@scope/pkg": {
      "version": "2.0.1",
      "resolved": "https://npm.example.com/@scope/pkg/-/pkg-2.0.1.tgz",
      "dependencies": { "right": "^2.0.0", "bundled": "^1.0.0" }
    },
    "node_modules/@scope/pkg/node_modules/bundled": {
      "version": "1.0.0",
      "inBundle": true
    },
    "node_modules/aliased": {
      "name": "real-name",
      "version": "5.0.0",
      "resolved": "https://registry.npmjs.org/real-name/-/real-name-5.0.0.tgz"
    },
    "node_modules/from-git": {
      "version": "0.1.0",
      "resolved": "git+ssh://git@github.com/example/from-git.git#abc"
    },
    "node_modules/unsaid": {
      "version": "7.0.0"
    },
    "packages/tool": {
      "name": "tool",
      "version": "0.0.1",
      "dependencies": { "hoisted-once": "^1.0.0" },
      "devDependencies": { "tester": "^4.0.0" }
    },
    "packages/node_modules/hoisted-once": {
      "version": "1.0.0",
      "resolved": "https://registry.npmjs.org/hoisted-once/-/hoisted-once-1.0.0.tgz"
    },
    "node_modules/tool": { "resolved": "packages/tool", "link": true }
  }
}"#;

    #[test]
    fn an_npm_lock_resolves_like_node_and_keeps_a_scope_in_its_purl() {
        let components = npm_lock_components(NPM_LOCK, "package-lock.json").expect("lockfile");
        let root = by(&components, "pkg:npm/site (path)");
        assert!(root.member);
        assert_eq!(
            root.dependencies,
            vec![
                "pkg:npm/%40scope/pkg@2.0.1 (https://npm.example.com/@scope/pkg/-/pkg-2.0.1.tgz)"
                    .to_string(),
                "pkg:npm/left@1.0.0".to_string(),
                "pkg:npm/tester@4.0.0".to_string(),
            ],
            "a member's dev dependencies are its edges too"
        );
        assert_eq!(
            by(&components, "pkg:npm/left@1.0.0").dependencies,
            vec!["pkg:npm/right@3.1.0".to_string()],
            "the nested copy under left wins over the hoisted one, and a dependency's dev \
             dependencies were never installed"
        );
        let scoped = by(
            &components,
            "pkg:npm/%40scope/pkg@2.0.1 (https://npm.example.com/@scope/pkg/-/pkg-2.0.1.tgz)",
        );
        assert_eq!(scoped.origin, Origin::Private);
        assert_eq!(
            scoped.dependencies,
            vec![
                "pkg:npm/bundled@1.0.0 (https://npm.example.com/@scope/pkg/-/pkg-2.0.1.tgz)"
                    .to_string(),
                "pkg:npm/right@2.0.0".to_string(),
            ]
        );
        assert_eq!(
            by(
                &components,
                "pkg:npm/bundled@1.0.0 (https://npm.example.com/@scope/pkg/-/pkg-2.0.1.tgz)"
            )
            .origin,
            Origin::Private,
            "a bundled package came from where its bundler came from"
        );
        let tool = by(&components, "pkg:npm/tool@0.0.1 (path)");
        assert!(tool.member, "a workspace is a member");
        assert_eq!(
            tool.dependencies,
            vec![
                "pkg:npm/hoisted-once@1.0.0".to_string(),
                "pkg:npm/tester@4.0.0".to_string()
            ],
            "Node searches every ancestor directory that is not itself node_modules"
        );
        assert!(
            !components
                .iter()
                .any(|component| component.name == "tool" && !component.member),
            "the link under node_modules is the workspace, not a second component"
        );
        assert_eq!(
            by(&components, "pkg:npm/real-name@5.0.0").origin,
            Origin::Registry,
            "an alias installs its real package"
        );
        assert_eq!(
            by(
                &components,
                "pkg:npm/from-git@0.1.0 (git+ssh://git@github.com/example/from-git.git#abc)"
            )
            .origin,
            Origin::Git
        );
        assert_eq!(
            by(&components, "pkg:npm/unsaid@7.0.0 (private)").origin,
            Origin::Private,
            "a lockfile that does not say where a package came from never names it to OSV"
        );
        assert_eq!(components.len(), 12);
    }

    #[test]
    fn a_root_that_wrote_no_name_is_still_the_member_the_lockfile_names() {
        let lock = r#"{ "name": "unnamed-site", "lockfileVersion": 3, "packages": {
            "": { "dependencies": { "left": "^1.0.0" } },
            "node_modules/left": { "version": "1.0.0", "resolved": "https://registry.npmjs.org/left/-/left-1.0.0.tgz" }
        } }"#;
        let components = npm_lock_components(lock, "package-lock.json").expect("lockfile");
        let root = by(&components, "pkg:npm/unnamed-site (path)");
        assert!(root.member);
        assert_eq!(root.dependencies, vec!["pkg:npm/left@1.0.0".to_string()]);
    }

    #[test]
    fn a_lockfile_version_1_is_refused_by_name() {
        let error = npm_lock_components(
            r#"{ "lockfileVersion": 1, "dependencies": {} }"#,
            "web/package-lock.json",
        )
        .expect_err("no packages map");
        assert!(
            matches!(error, SupplyError::Unreadable { ref lockfile, ref why }
            if lockfile == "web/package-lock.json" && why.contains("lockfileVersion 1"))
        );
    }

    #[test]
    fn only_public_registry_packages_with_a_version_are_ever_asked_about() {
        let npm_without_version = r#"{ "lockfileVersion": 3, "packages": {
            "": { "name": "site-member-name" },
            "node_modules/versionless": { "resolved": "https://registry.npmjs.org/versionless/-/versionless.tgz" }
        } }"#;
        let components = merge_components([
            cargo_lock_components(CARGO_LOCK, "Cargo.lock").expect("cargo"),
            npm_lock_components(NPM_LOCK, "package-lock.json").expect("npm"),
            npm_lock_components(npm_without_version, "web/package-lock.json").expect("npm"),
        ]);
        let queries = osv_queries(&components);
        let asked: Vec<&str> = queries.iter().map(OsvQuery::purl).collect();
        assert_eq!(
            asked,
            vec![
                "pkg:cargo/serde@1.0.219",
                "pkg:cargo/time@0.1.45",
                "pkg:cargo/time@0.3.36",
                "pkg:npm/hoisted-once@1.0.0",
                "pkg:npm/left@1.0.0",
                "pkg:npm/real-name@5.0.0",
                "pkg:npm/right@2.0.0",
                "pkg:npm/right@3.1.0",
                "pkg:npm/tester@4.0.0",
            ],
            "no git, path, member, private-registry, unsaid or version-less package is asked"
        );
        let body = osv_batch_body(&queries);
        let text = body.to_string();
        for private in [
            "app",
            "helper",
            "example",
            "internal-secret-crate",
            "scope",
            "bundled",
            "from-git",
            "unsaid",
            "tool",
            "site",
            "versionless",
            "crates.example.com",
            "npm.example.com",
            "Cargo.lock",
        ] {
            assert!(
                !text.contains(private),
                "`{private}` left the machine: {text}"
            );
        }
        assert_eq!(body["queries"].as_array().map(Vec::len), Some(9));
        assert_eq!(
            body["queries"][0],
            json!({ "package": { "ecosystem": "crates.io", "name": "serde" }, "version": "1.0.219" }),
            "a question is the ecosystem, the name and the version, and nothing else"
        );
        assert_eq!(
            body["queries"][4],
            json!({ "package": { "ecosystem": "npm", "name": "left" }, "version": "1.0.0" })
        );
    }

    #[test]
    fn merging_lockfiles_keeps_one_component_per_id_and_unions_what_names_it() {
        let merged = merge_components([
            cargo_lock_components(CARGO_LOCK, "Cargo.lock").expect("one"),
            cargo_lock_components(CARGO_LOCK, "zo-ide/Cargo.lock").expect("two"),
        ]);
        let serde = by(&merged, "pkg:cargo/serde@1.0.219");
        assert_eq!(
            serde.lockfiles,
            vec!["Cargo.lock".to_string(), "zo-ide/Cargo.lock".to_string()]
        );
        assert_eq!(merged.len(), 6);
    }

    #[test]
    fn a_batch_answer_pairs_ids_with_the_questions_and_carries_a_later_page() {
        let answer = json!({ "results": [
            {},
            { "vulns": [ { "id": "RUSTSEC-2020-0071", "modified": "x" } ], "next_page_token": "page-2" }
        ] });
        let read = read_osv_batch(&answer, 2).expect("answer");
        assert_eq!(
            read,
            vec![
                OsvBatchResult {
                    ids: vec![],
                    next_page_token: None
                },
                OsvBatchResult {
                    ids: vec!["RUSTSEC-2020-0071".to_string()],
                    next_page_token: Some("page-2".to_string()),
                },
            ]
        );
        assert!(
            read_osv_batch(&json!({ "results": [] }), 1).is_err(),
            "a short answer is refused"
        );
        assert!(
            read_osv_batch(&json!({ "results": [ { "vulns": [ {} ] } ] }), 1).is_err(),
            "an id-less vulnerability is refused"
        );
        assert!(
            read_osv_batch(&json!({ "vulns": [] }), 0).is_err(),
            "no results array is refused"
        );

        let components = cargo_lock_components(CARGO_LOCK, "Cargo.lock").expect("lockfile");
        let first = osv_queries(&components).remove(0);
        let later = first.next_page("page-2");
        assert_eq!(
            (first.page_token(), later.page_token()),
            (None, Some("page-2"))
        );
        assert_eq!(
            osv_batch_body(&[later])["queries"][0],
            json!({ "package": { "ecosystem": "crates.io", "name": "serde" }, "version": "1.0.219", "page_token": "page-2" })
        );
    }

    /// Each score is the NVD CVSS v3 calculator's for the same vector
    /// (nvd.nist.gov/vuln-metrics/cvss/v3-calculator), read back on
    /// 2026-09-17 — the rating boundaries 0.0, 3.9/4.0, 6.9/7.0 and 8.9/9.0
    /// among them.
    #[test]
    fn the_cvss_base_score_is_the_nvd_calculators() {
        let cases = [
            ("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:N", 0.0),
            ("CVSS:3.0/AV:P/AC:H/PR:H/UI:R/S:U/C:L/I:N/A:N", 1.6),
            ("CVSS:3.1/AV:N/AC:H/PR:H/UI:R/S:U/C:L/I:L/A:L", 3.9),
            ("CVSS:3.1/AV:N/AC:H/PR:N/UI:N/S:C/C:L/I:N/A:N", 4.0),
            ("CVSS:3.1/AV:N/AC:H/PR:N/UI:R/S:C/C:L/I:L/A:N", 4.7),
            ("CVSS:3.1/AV:L/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H", 6.2),
            ("CVSS:3.1/AV:N/AC:L/PR:H/UI:R/S:C/C:H/I:L/A:N", 6.9),
            ("CVSS:3.1/AV:N/AC:H/PR:N/UI:N/S:U/C:H/I:L/A:L", 7.0),
            ("CVSS:3.1/AV:N/AC:H/PR:N/UI:N/S:C/C:H/I:H/A:L", 8.9),
            ("CVSS:3.1/AV:N/AC:H/PR:N/UI:N/S:C/C:H/I:H/A:H", 9.0),
            ("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H", 9.8),
            ("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:C/C:H/I:H/A:H", 10.0),
        ];
        for (vector, expected) in cases {
            let score = cvss3_base_score(vector).expect(vector);
            assert!(
                (score - expected).abs() < 1e-9,
                "{vector}: {score} != {expected}"
            );
        }
        assert_eq!(
            cvss3_base_score("CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:N/SI:N/SA:N"),
            None
        );
        assert_eq!(
            cvss3_base_score("CVSS:3.1/AV:N/AC:L"),
            None,
            "a missing base metric"
        );
        assert_eq!(
            cvss3_base_score("CVSS:3.1/AV:N/AV:P/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"),
            None,
            "a metric written twice is an invalid vector, not its last value"
        );
        assert_eq!(
            cvss3_base_score("CVSS:3.1/AV:X/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"),
            None
        );
    }

    #[test]
    fn the_rating_boundaries_are_table_14s() {
        for (score, rating) in [
            (0.0, Severity::None),
            (0.1, Severity::Low),
            (3.9, Severity::Low),
            (4.0, Severity::Medium),
            (6.9, Severity::Medium),
            (7.0, Severity::High),
            (8.9, Severity::High),
            (9.0, Severity::Critical),
            (10.0, Severity::Critical),
        ] {
            assert_eq!(severity_of_score(score), rating, "{score}");
        }
        assert!(Severity::Unknown < Severity::None && Severity::High < Severity::Critical);
    }

    /// An excerpt of OSV's RUSTSEC-2020-0071 as `GET /v1/vulns/{id}` answered it.
    fn time_record() -> Value {
        json!({
            "id": "RUSTSEC-2020-0071",
            "summary": "Potential segfault in the time crate",
            "details": "### Impact\n\nThe affected functions set environment variables without synchronization.",
            "aliases": ["CVE-2020-26235", "GHSA-wcg3-cvx6-7396"],
            "affected": [{
                "package": { "name": "time", "ecosystem": "crates.io", "purl": "pkg:cargo/time" },
                "ranges": [{ "type": "SEMVER", "events": [
                    { "introduced": "0.0.0-0" }, { "fixed": "0.2.0" },
                    { "introduced": "0.2.1-0" }, { "fixed": "0.2.1" },
                    { "introduced": "0.2.7-0" }, { "fixed": "0.2.23" }
                ] }],
                "database_specific": { "cvss": "CVSS:3.1/AV:L/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H", "informational": null }
            }],
            "severity": [{ "type": "CVSS_V3", "score": "CVSS:3.1/AV:L/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H" }]
        })
    }

    #[test]
    fn a_vulnerability_record_reads_its_score_aliases_fixes_and_informational_word() {
        let read = read_osv_record(&time_record()).expect("record");
        assert_eq!(read.id, "RUSTSEC-2020-0071");
        assert_eq!(
            read.aliases,
            vec![
                "CVE-2020-26235".to_string(),
                "GHSA-wcg3-cvx6-7396".to_string()
            ]
        );
        assert_eq!(read.score, Some(6.2));
        assert_eq!(read.severity, Severity::Medium);
        assert_eq!(
            read.fixed
                .iter()
                .map(|fix| fix.version.as_str())
                .collect::<Vec<_>>(),
            vec!["0.2.0", "0.2.1", "0.2.23"],
            "the record's own order — `0.2.23` is not sorted before `0.2.3`"
        );
        assert_eq!(read.informational, None);
        assert!(!read.withdrawn);

        let unmaintained = json!({
            "id": "RUSTSEC-2024-0384",
            "summary": "`instant` is unmaintained",
            "affected": [{
                "package": { "name": "instant", "ecosystem": "crates.io", "purl": "pkg:cargo/instant" },
                "ranges": [{ "type": "SEMVER", "events": [{ "introduced": "0.0.0-0" }] }],
                "database_specific": { "cvss": null, "informational": "unmaintained" }
            }]
        });
        let read = read_osv_record(&unmaintained).expect("record");
        assert_eq!((read.severity, read.score), (Severity::Unknown, None));
        assert_eq!(read.informational.as_deref(), Some("unmaintained"));
        assert!(read.fixed.is_empty());
    }

    #[test]
    fn a_v4_only_record_is_rated_by_the_databases_own_word() {
        let v4_only = json!({
            "id": "GHSA-xxxx-yyyy-zzzz",
            "details": "\nfirst line\nsecond line",
            "severity": [{ "type": "CVSS_V4", "score": "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:N/SI:N/SA:N" }],
            "database_specific": { "severity": "MODERATE" },
            "withdrawn": "2026-01-01T00:00:00Z"
        });
        let read = read_osv_record(&v4_only).expect("v4 record");
        assert_eq!(read.score, None);
        assert_eq!(
            read.severity,
            Severity::Medium,
            "the database's own word rates a v4-only record"
        );
        assert_eq!(read.summary, "first line");
        assert!(read.withdrawn);
        assert!(read_osv_record(&json!({ "summary": "no id" })).is_err());
    }

    fn record(id: &str, aliases: &[&str], severity: Severity, fixed: &[(&str, &str)]) -> OsvRecord {
        OsvRecord {
            id: id.to_string(),
            aliases: aliases.iter().map(|alias| (*alias).to_string()).collect(),
            summary: format!("{id} summary"),
            severity,
            score: None,
            fixed: fixed
                .iter()
                .map(|(name, version)| Fix {
                    ecosystem: Ecosystem::Cargo,
                    name: (*name).to_string(),
                    version: (*version).to_string(),
                })
                .collect(),
            informational: None,
            withdrawn: false,
        }
    }

    #[test]
    fn aliases_fold_into_one_vulnerability_named_by_the_ecosystems_database() {
        let components = cargo_lock_components(CARGO_LOCK, "Cargo.lock").expect("lockfile");
        let findings: OsvFindings = BTreeMap::from([
            (
                "pkg:cargo/time@0.1.45".to_string(),
                vec![
                    "GHSA-wcg3-cvx6-7396".to_string(),
                    "RUSTSEC-2020-0071".to_string(),
                ],
            ),
            (
                "pkg:cargo/serde@1.0.219".to_string(),
                vec!["RUSTSEC-2099-0001".to_string()],
            ),
        ]);
        let mut rustsec = record(
            "RUSTSEC-2020-0071",
            &["CVE-2020-26235", "GHSA-wcg3-cvx6-7396"],
            Severity::Medium,
            &[("time", "0.2.23"), ("other-crate", "1.0.0")],
        );
        rustsec.score = Some(6.2);
        let ghsa = record(
            "GHSA-wcg3-cvx6-7396",
            &["CVE-2020-26235"],
            Severity::High,
            &[("time", "0.2.23")],
        );
        let mut withdrawn = record("RUSTSEC-2099-0001", &[], Severity::Critical, &[]);
        withdrawn.withdrawn = true;
        let unreached = record("RUSTSEC-2000-0000", &[], Severity::Critical, &[]);

        let graph = supply_graph(
            components,
            &findings,
            &[ghsa, withdrawn, rustsec, unreached],
        );
        assert_eq!(
            graph.vulnerabilities.len(),
            1,
            "one advisory, whatever databases file it"
        );
        let vulnerability = &graph.vulnerabilities[0];
        assert_eq!(vulnerability.id, "RUSTSEC-2020-0071");
        assert_eq!(
            vulnerability.aliases,
            vec![
                "CVE-2020-26235".to_string(),
                "GHSA-wcg3-cvx6-7396".to_string()
            ]
        );
        assert_eq!(
            vulnerability.severity,
            Severity::High,
            "the most severe rating of the group"
        );
        assert_eq!(vulnerability.score, Some(6.2));
        assert_eq!(vulnerability.summary, "RUSTSEC-2020-0071 summary");
        assert_eq!(
            vulnerability.fixed,
            vec![Fix {
                ecosystem: Ecosystem::Cargo,
                name: "time".to_string(),
                version: "0.2.23".to_string()
            }],
            "only fixes for the packages it reaches here"
        );
        assert_eq!(
            vulnerability.url,
            "https://osv.dev/vulnerability/RUSTSEC-2020-0071"
        );

        let time = graph
            .components
            .iter()
            .position(|component| component.id == "pkg:cargo/time@0.1.45")
            .expect("time");
        let affects: Vec<&SupplyEdge> = graph
            .edges
            .iter()
            .filter(|edge| edge.kind == SupplyEdgeKind::Affects)
            .collect();
        assert_eq!(
            affects,
            vec![&SupplyEdge {
                from: 0,
                to: index_of(time),
                kind: SupplyEdgeKind::Affects,
                provenance: EdgeProvenance::Measured,
            }],
            "the withdrawn record reaches serde with nothing"
        );
        let app = graph
            .components
            .iter()
            .position(|component| component.id == "pkg:cargo/app@0.1.0 (path)")
            .expect("app");
        let depends: Vec<u32> = graph
            .edges
            .iter()
            .filter(|edge| edge.kind == SupplyEdgeKind::DependsOn && edge.from == index_of(app))
            .map(|edge| edge.to)
            .collect();
        assert_eq!(depends.len(), 3, "app → helper, serde, time 0.3");
    }

    #[test]
    fn a_finding_never_lands_on_a_private_namesake() {
        let lock = r#"
[[package]]
name = "shim"
version = "1.0.0"
"#;
        let components = cargo_lock_components(lock, "Cargo.lock").expect("lockfile");
        let findings: OsvFindings = BTreeMap::from([(
            "pkg:cargo/shim@1.0.0".to_string(),
            vec!["RUSTSEC-2021-0001".to_string()],
        )]);
        let graph = supply_graph(
            components,
            &findings,
            &[record("RUSTSEC-2021-0001", &[], Severity::High, &[])],
        );
        assert!(graph.vulnerabilities.is_empty() && graph.edges.is_empty());
    }

    #[test]
    fn the_answer_speaks_camel_case_and_the_vaults_edge_word() {
        let components = cargo_lock_components(CARGO_LOCK, "Cargo.lock").expect("lockfile");
        let graph = supply_graph(components, &OsvFindings::new(), &[]);
        let wire = serde_json::to_value(&graph).expect("graph");
        let component = &wire["components"][0];
        let mut keys: Vec<&str> = component
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "ecosystem",
                "id",
                "lockfiles",
                "member",
                "name",
                "origin",
                "purl",
                "source",
                "version"
            ],
            "dependencies travel as edges, not twice"
        );
        assert_eq!(
            serde_json::to_value(SupplyEdgeKind::DependsOn).expect("kind"),
            json!(crate::second_brain_graph::EdgeKind::DependsOn.as_str()),
            "one word for one relation, in the vault and in the supply chain"
        );
        assert_eq!(
            serde_json::to_value(SupplyEdgeKind::Affects).expect("kind"),
            json!("affects")
        );
        assert_eq!(
            serde_json::to_value(Severity::None).expect("rating"),
            json!("none")
        );
        let record = serde_json::to_value(record("RUSTSEC-2020-0071", &[], Severity::Low, &[]))
            .expect("record");
        assert!(record.get("informational").is_some() && record.get("withdrawn").is_some());
    }

    #[test]
    fn the_fingerprint_moves_with_any_byte_or_path_and_with_nothing_else() {
        let one = lockfile_fingerprint([("Cargo.lock", "a"), ("package-lock.json", "b")]);
        assert_eq!(
            one,
            lockfile_fingerprint([("Cargo.lock", "a"), ("package-lock.json", "b")])
        );
        assert_ne!(
            one,
            lockfile_fingerprint([("Cargo.lock", "a"), ("package-lock.json", "c")])
        );
        assert_ne!(
            one,
            lockfile_fingerprint([("zo-ide/Cargo.lock", "a"), ("package-lock.json", "b")])
        );
        assert_ne!(
            lockfile_fingerprint([("ab", "c")]),
            lockfile_fingerprint([("a", "bc")]),
            "a path and its text are framed apart"
        );
        assert_eq!(one.len(), 64);
    }

    /// G5 (docs/design/knowledge-supply-chain-20260917.md §2): the components
    /// this parser reads from this repository's three lockfiles, each and
    /// merged — the numbers an independent count (python `tomllib`/`json`,
    /// sharing nothing with this file) is held against.
    #[test]
    #[ignore = "a measurement, printed; not a check"]
    fn supply_chain_counts_this_repositorys_lockfiles() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut lists = Vec::new();
        for lockfile in ["Cargo.lock", "zo-ide/Cargo.lock", "package-lock.json"] {
            let text = std::fs::read_to_string(root.join(lockfile)).expect(lockfile);
            let file_name = lockfile.rsplit('/').next().unwrap_or(lockfile);
            let ecosystem = Ecosystem::of_lockfile(file_name).expect("a lockfile this layer reads");
            let components = lockfile_components(ecosystem, &text, lockfile).expect(lockfile);
            println!("{lockfile}: {} components", components.len());
            lists.push(components);
        }
        let merged = merge_components(lists);
        println!(
            "merged: {} components, {} asked",
            merged.len(),
            osv_queries(&merged).len()
        );
    }
}
