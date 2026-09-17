//! What a dev server said its address was.
//!
//! A listening socket answers two questions — which port, and which interface
//! — and neither of them is the address the server *means*. `vite` binds
//! `0.0.0.0:5173` and prints `http://myapp.test:5173`; a container prints the
//! hostname it was reached by; a tunnel prints an https origin for a socket
//! that speaks plain http. The port scanner can only report the kernel's
//! answer, so a panel built on the scan alone offers an address that does not
//! work. Orca watches the terminal for the line the server printed and
//! remembers the origin per {workspace, port}
//! (`main/ports/advertised-url-watcher.ts:1-5`).
//!
//! This module is the remembering. It has no clock, no sockets, and no map of
//! processes: timestamps, terminal lines, and listener observations all arrive
//! as arguments, and every method that changes something hands back the list
//! of {workspace, port} pairs whose answer moved, for the caller to announce.
//! The original keeps a listener set and calls it inside a `try` (`:397-405`);
//! a returned list cannot throw into the middle of an eviction.
//!
//! ## What is deliberately not ported: the stripper
//!
//! The original accumulates raw PTY bytes and runs five regexes over them to
//! remove OSC, CSI, single escapes and control characters (`:14-24,109-118`),
//! because its main process has no terminal parser at that layer. Its own
//! comment names the sharpest consequence: a cursor move in a differential
//! redraw skips cells that are still on screen, so text that was never
//! adjacent would FUSE into one false URL — and it defends with a substituted
//! `[` (`:17-19`). It also buffers to the last newline so a sequence split
//! across two writes survives (`:5,68-84`).
//!
//! We already parse the stream into a grid (`zerocode_pty::TerminalGrid`),
//! which is a parser rather than a filter: it knows where a cursor move landed
//! and holds settled lines. So this module takes **lines**, and neither the
//! stripper nor the chunk buffer is ported. Nothing here can fuse, because
//! nothing here concatenates.

use std::collections::{HashMap, HashSet};

use url::Url;

use crate::localhost_label::{is_loopback_hostname, is_unspecified_host};

/// The longest run of URL characters still worth parsing (`:24`).
///
/// A line of terminal output can be a base64 blob with `http://` inside it;
/// past this length it is not an address anybody printed on purpose.
const CANDIDATE_MAX_CHARS: usize = 2048;

/// How many {workspace, port} answers are kept at once (`:13`).
pub const MAX_REMEMBERED: usize = 256;

/// How many terminals may hold un-replayed lines before the oldest is dropped
/// (`:11`). The original's reason ports exactly: a terminal whose spawn fails
/// never binds, and without a ceiling each failure leaves one entry forever.
const MAX_UNBOUND_SOURCES: usize = 32;

/// How much text one unbound terminal may hold (`:10`), counted in characters
/// rather than the original's UTF-16 code units — see [`AdvertisedUrls`].
const UNBOUND_CHARS: usize = 16 * 1024;

/// The characters that end a URL candidate (`:26` — the `[^\s<>"'\`]` class).
const CANDIDATE_STOPS: [char; 5] = ['<', '>', '"', '\'', '`'];

/// Punctuation that cannot end a real URL, however it ends a sentence
/// (`:129-131`).
const TRAILING_PUNCTUATION: [char; 12] =
    ['.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '\'', '"'];

/// What kind of address a server advertised — **and, by declaration order,
/// which kind wins** when two lines advertise the same port.
///
/// The original spells the ranking as a `switch` returning 3/2/1/0
/// (`:215-226`) and then compares those numbers. Here the ranking is the
/// enum's own order and `#[derive(Ord)]` carries it, so a fifth kind cannot be
/// added without deciding where it sits.
///
/// The order itself is the original's, and so is its reason: a name beats a
/// loopback address because a server that printed a name usually only answers
/// to it (certificates, cookie domains, virtual hosts), and loopback beats a
/// LAN address because both reach the same process on this machine while only
/// one of them is private.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostKind {
    /// A routable address on the open internet.
    PublicIp,
    /// RFC 1918, link-local, or their v6 equivalents.
    PrivateIp,
    /// This machine, by any of its spellings.
    Loopback,
    /// A DNS name — what a dev server means when it prints one.
    Custom,
}

/// http or https — **and, by declaration order, the tie-break** when two
/// lines advertise the same port with the same kind of host (`:231-233`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    Http,
    Https,
}

impl Scheme {
    fn of(url: &Url) -> Option<Self> {
        match url.scheme() {
            "http" => Some(Self::Http),
            "https" => Some(Self::Https),
            _ => None,
        }
    }
}

/// One address a server printed, as the ports panel would offer it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct AdvertisedUrl {
    /// Scheme, host and port — and nothing else.
    ///
    /// The original keeps only the origin for a stated reason (`:361-362`): a
    /// dev server's first printed line is often an OAuth callback or a URL
    /// carrying a session token, and a panel that offered the whole thing
    /// would put that token on a button.
    pub origin: String,
    /// The host as the URL spelled it, brackets and all for a v6 literal.
    pub host: String,
    pub kind: HostKind,
    pub scheme: Scheme,
    pub port: u16,
    /// The terminal the line came from.
    ///
    /// This is the eviction handle when a pane closes: an address advertised
    /// over an SSH forward has no listener PID on this machine, so the
    /// terminal going away is the only expiry signal it will ever get
    /// (`:288`).
    pub source: String,
    /// When the line was last seen, in whatever unit the caller counts.
    pub last_seen: u64,
    /// The listener this address has been checked against.
    ///
    /// Captured on the first scan that looks it up rather than when the line
    /// was read, because the terminal's own PID is the shell's, not the dev
    /// server's (`:41-42`). Once set, a different PID on that port means some
    /// other program is answering and the remembered address is not about it.
    pub validated_listener: Option<u32>,
}

/// A {workspace, port} whose answer moved.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub struct Change {
    pub owner: String,
    pub port: u16,
}

/// One listening socket a scan saw.
#[derive(Clone, Copy, Debug)]
pub struct Listener {
    pub port: u16,
    /// Absent when the scan could not attribute a process to the socket.
    pub pid: Option<u32>,
}

/// What a lookup found, and what the looking itself changed.
///
/// Reading is not free of consequence here: a lookup is where a remembered
/// address first meets a real listener, so it is where it gets pinned to one
/// — or thrown away for belonging to a program that is gone.
#[derive(Clone, Debug, Default)]
pub struct Found {
    pub url: Option<AdvertisedUrl>,
    pub changes: Vec<Change>,
}

/// What one scan does to one remembered address.
///
/// Two answers rather than one because the settling grace is spent by being
/// tested: the original consumes it inside the `&&` that reads it
/// (`:503-505`), and a predicate that only says yes-or-no would either leak
/// the grace forever or spend it on scans that never offered it.
#[derive(Clone, Copy, Debug)]
struct Verdict {
    evicts: bool,
    spends_grace: bool,
}

/// What a scan said about one port, the last time it looked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seen {
    /// No socket on that port.
    Absent,
    /// A socket, and the process holding it when the scan could name one.
    Present(Option<u32>),
}

/// A {workspace, port}, as the book is keyed.
///
/// **DEPARTURE:** the original builds the string `${worktreeId}::${port}` and
/// later parses the workspace back out of it by chopping off a `::${port}`
/// suffix (`:57-63`). A workspace whose own id ends in `::3000` therefore
/// reports changes for a workspace that does not exist. A pair cannot be
/// misread, so it is a pair.
type Key = (String, u16);

/// The book of advertised addresses.
///
/// Bounded on every axis the original bounds it on, and by the same numbers.
/// One count differs in unit: the original caps an unbound terminal's held
/// text in JavaScript string length, which is UTF-16 code units, so the same
/// buffer holds half as much CJK as ASCII; this counts characters, which is
/// what the cap is trying to be about.
#[derive(Debug, Default)]
pub struct AdvertisedUrls {
    /// Which workspace each terminal belongs to.
    owners: HashMap<String, String>,
    /// Lines that arrived before their terminal was bound, oldest terminal
    /// first. The original leans on JavaScript `Map` insertion order and
    /// re-inserts to refresh it (`:334-336`); insertion order is a property of
    /// that map rather than a rule, so the order is kept here on purpose.
    unbound: Vec<Unbound>,
    remembered: HashMap<Key, AdvertisedUrl>,
    /// What a scan had said about a port when its address was captured — or,
    /// while the address is still unpinned, what the last scan said.
    baseline: HashMap<Key, Seen>,
    /// Addresses allowed to survive exactly one scan that sees no listener.
    ///
    /// A dev server prints its banner before it accepts connections, so the
    /// first scan after the line can honestly find nothing (`:378-380`).
    settling: HashSet<Key>,
    /// The last scan, per workspace, so a newly captured address can be told
    /// what the world looked like when it arrived.
    snapshots: HashMap<String, HashMap<u16, Option<u32>>>,
    max_remembered: usize,
}

#[derive(Debug)]
struct Unbound {
    source: String,
    lines: Vec<String>,
    chars: usize,
}

impl AdvertisedUrls {
    pub fn new() -> Self {
        Self {
            max_remembered: MAX_REMEMBERED,
            ..Self::default()
        }
    }

    /// The same book with a smaller ceiling, for tests that would otherwise
    /// have to print 257 banners to reach the eviction path (`:243-244`).
    pub fn with_capacity(max_remembered: usize) -> Self {
        Self {
            max_remembered: max_remembered.max(1),
            ..Self::default()
        }
    }

    /// Say which workspace a terminal belongs to, and replay anything it said
    /// before anyone knew (`:296-306`).
    ///
    /// The original's reason ports: PTY output can arrive before the spawn
    /// handler has resolved the workspace, and those first lines are exactly
    /// the ones a dev server prints its address on.
    pub fn bind(&mut self, source: &str, owner: &str, now: u64) -> Vec<Change> {
        let held = self.take_unbound(source);
        if self.owners.get(source).map(String::as_str) == Some(owner) && held.is_empty() {
            return Vec::new();
        }
        self.owners.insert(source.to_string(), owner.to_string());
        let mut changes = Vec::new();
        for line in held {
            changes.extend(self.observe(source, &line, now));
        }
        dedupe(changes)
    }

    /// A terminal has gone. Everything it advertised goes with it (`:280-295`).
    pub fn unbind(&mut self, source: &str) -> Vec<Change> {
        self.owners.remove(source);
        self.take_unbound(source);
        let gone: Vec<Key> = self
            .remembered
            .iter()
            .filter(|(_, url)| url.source == source)
            .map(|(key, _)| key.clone())
            .collect();
        self.forget_keys(gone)
    }

    /// A workspace has gone. Its terminals stop being its, and its addresses
    /// are dropped rather than left for whoever reuses the id (`:308-333`).
    pub fn forget_owner(&mut self, owner: &str) -> Vec<Change> {
        self.owners.retain(|_, bound| bound != owner);
        self.snapshots.remove(owner);
        let gone: Vec<Key> = self
            .remembered
            .keys()
            .filter(|(key_owner, _)| key_owner == owner)
            .cloned()
            .collect();
        self.forget_keys(gone)
    }

    /// Read one settled line of terminal output (`:335-367`).
    pub fn observe(&mut self, source: &str, line: &str, now: u64) -> Vec<Change> {
        let Some(owner) = self.owners.get(source).cloned() else {
            self.hold_unbound(source, line);
            return Vec::new();
        };
        let mut changes = Vec::new();
        for url in urls_in_line(line) {
            changes.extend(self.consider(&url, source, &owner, now));
        }
        dedupe(changes)
    }

    fn consider(&mut self, url: &Url, source: &str, owner: &str, now: u64) -> Vec<Change> {
        let Some(scheme) = Scheme::of(url) else {
            return Vec::new();
        };
        let Some(host) = url.host_str() else {
            return Vec::new();
        };
        // A wildcard bind is not an address anybody can open, so the scanner's
        // own normalization is the better answer for that port (`:349-352`).
        if is_unspecified_host(host) {
            return Vec::new();
        }
        // `u16` answers the original's upper bound (`:344-346`) by making it
        // unrepresentable, but not its lower one: `http://host:0/` parses, and
        // zero is the kernel's word for "choose one for me" rather than a port
        // anything is listening on.
        let Some(port) = url.port_or_known_default().filter(|port| *port > 0) else {
            return Vec::new();
        };
        let candidate = AdvertisedUrl {
            origin: url.origin().ascii_serialization(),
            host: host.to_string(),
            kind: classify_host(host),
            scheme,
            port,
            source: source.to_string(),
            last_seen: now,
            validated_listener: None,
        };
        let key = (owner.to_string(), port);
        match self.remembered.get_mut(&key) {
            Some(existing) if existing.origin == candidate.origin => {
                // The same address said again. **DEPARTURE:** the original
                // replaces the entry (its `>=` recency admits an equal), which
                // silently resets the listener pin and re-grants the settling
                // grace on every hot-reload reprint (`:404-419`) — a banner
                // restating itself is not evidence about which process
                // listens. Here it refreshes recency, and hands the eviction
                // handle to the newest speaker so the address survives the
                // first terminal closing while the second still advertises it.
                existing.last_seen = now;
                if existing.source != source {
                    existing.source = source.to_string();
                }
                return Vec::new();
            }
            Some(existing) if !outranks(&candidate, existing) => {
                // Not better, but said again — recency is what keeps it from
                // being the next entry evicted (`:388-389`).
                existing.last_seen = now;
                return Vec::new();
            }
            // Past both arms above, the origin is different and the candidate
            // won: this is the one path where the answer actually moves.
            _ => (),
        };

        self.remembered.insert(key.clone(), candidate);
        match self.scan_state(owner, port) {
            Some(seen) => {
                self.baseline.insert(key.clone(), seen);
                if seen == Seen::Absent {
                    self.settling.insert(key.clone());
                } else {
                    self.settling.remove(&key);
                }
            }
            None => {
                // Nothing has scanned this workspace yet, so there is no state
                // to be measured against — grant the same one-scan grace.
                self.baseline.remove(&key);
                self.settling.insert(key.clone());
            }
        }

        let mut changes = self.enforce_ceiling();
        changes.push(Change {
            owner: owner.to_string(),
            port,
        });
        dedupe(changes)
    }

    /// The address remembered for a workspace's port, checked against the
    /// process that holds it now (`:432-452`).
    pub fn lookup(&mut self, owner: &str, port: u16, listener: Option<u32>) -> Found {
        let key = (owner.to_string(), port);
        let Some(entry) = self.remembered.get_mut(&key) else {
            return Found::default();
        };
        match (entry.validated_listener, listener) {
            (None, Some(pid)) => entry.validated_listener = Some(pid),
            (Some(pinned), Some(pid)) if pinned != pid => {
                return Found {
                    url: None,
                    changes: self.forget_keys(vec![key]),
                };
            }
            _ => {}
        }
        Found {
            url: self.remembered.get(&key).cloned(),
            changes: Vec::new(),
        }
    }

    /// The best address any of these workspaces advertised for a port
    /// (`:496-534`).
    ///
    /// The original's reason ports as written: an SSH port scan reports the
    /// whole connection's ports, not one workspace's, so the workspace that
    /// printed the banner may not be the one being asked about.
    pub fn best_across(&mut self, owners: &[&str], port: u16, listener: Option<u32>) -> Found {
        let mut changes = Vec::new();
        let mut best: Option<Key> = None;
        for owner in owners {
            let key = ((*owner).to_string(), port);
            let Some(candidate) = self.remembered.get(&key) else {
                continue;
            };
            if let (Some(pinned), Some(pid)) = (candidate.validated_listener, listener)
                && pinned != pid
            {
                changes.extend(self.forget_keys(vec![key]));
                continue;
            }
            let better = match best.as_ref().and_then(|held| self.remembered.get(held)) {
                None => true,
                Some(held) => self
                    .remembered
                    .get(&key)
                    .is_some_and(|candidate| outranks(candidate, held)),
            };
            if better {
                best = Some(key);
            }
        }
        let Some(key) = best else {
            return Found {
                url: None,
                changes: dedupe(changes),
            };
        };
        if let Some(pid) = listener {
            let entry = self
                .remembered
                .get_mut(&key)
                .expect("the winning key was read from this map a moment ago");
            if entry.validated_listener.is_none() {
                entry.validated_listener = Some(pid);
                self.baseline.remove(&key);
                self.settling.remove(&key);
            }
        }
        Found {
            url: self.remembered.get(&key).cloned(),
            changes: dedupe(changes),
        }
    }

    /// Forget one address, whatever its state (`:455-462`).
    pub fn invalidate(&mut self, owner: &str, port: u16) -> Vec<Change> {
        self.forget_keys(vec![(owner.to_string(), port)])
    }

    /// Reconcile the book against a scan (`:466-495`).
    ///
    /// An address that has never been checked against a listener stays tied to
    /// the listener state it was captured beside, so a port that later falls
    /// silent — or changes hands — cannot lazily bless it.
    pub fn reconcile(&mut self, owners: &[&str], listeners: &[Listener]) -> Vec<Change> {
        let observed = by_port(listeners);
        let watched: HashSet<&str> = owners.iter().copied().collect();
        let mut evict = Vec::new();
        let mut rebaseline = Vec::new();
        let mut spent = Vec::new();

        for (key, entry) in &self.remembered {
            if !watched.contains(key.0.as_str()) {
                continue;
            }
            let current = match observed.get(&key.1) {
                Some(pid) => Seen::Present(*pid),
                None => Seen::Absent,
            };
            let verdict = self.verdict_after_scan(key, entry, current);
            if verdict.spends_grace {
                spent.push(key.clone());
            }
            if verdict.evicts {
                evict.push(key.clone());
            } else if entry.validated_listener.is_none() {
                rebaseline.push((key.clone(), current));
            }
        }

        // The grace is spent where it was tested, not only where it saved
        // something — a scan that granted it has used it up either way.
        for key in spent {
            self.settling.remove(&key);
        }
        for (key, current) in rebaseline {
            self.baseline.insert(key, current);
        }
        for owner in owners {
            self.snapshots
                .insert((*owner).to_string(), observed.clone());
        }
        self.forget_keys(evict)
    }

    /// What one scan does to one remembered address (`:497-527`).
    fn verdict_after_scan(&self, key: &Key, entry: &AdvertisedUrl, current: Seen) -> Verdict {
        let baseline = self.baseline.get(key).copied();
        if current == Seen::Absent {
            // Never checked, never yet seen listening, and still owed its one
            // settling scan — the banner simply arrived first.
            let owed = entry.validated_listener.is_none()
                && !matches!(baseline, Some(Seen::Present(_)))
                && self.settling.contains(key);
            return Verdict {
                evicts: !owed,
                spends_grace: owed,
            };
        }
        if let (Some(pinned), Seen::Present(Some(pid))) = (entry.validated_listener, current)
            && pinned != pid
        {
            return Verdict {
                evicts: true,
                spends_grace: false,
            };
        }
        if matches!(baseline, Some(Seen::Absent)) {
            // The scan has caught up with the banner. That is the address
            // being confirmed, not contradicted.
            return Verdict {
                evicts: false,
                spends_grace: true,
            };
        }
        Verdict {
            evicts: entry.validated_listener.is_none()
                && baseline.is_some_and(|before| listener_changed(before, current)),
            spends_grace: false,
        }
    }

    /// Everything remembered, for a caller that wants to draw the whole book.
    pub fn iter(&self) -> impl Iterator<Item = (&str, u16, &AdvertisedUrl)> {
        self.remembered
            .iter()
            .map(|((owner, port), url)| (owner.as_str(), *port, url))
    }

    fn scan_state(&self, owner: &str, port: u16) -> Option<Seen> {
        let snapshot = self.snapshots.get(owner)?;
        Some(match snapshot.get(&port) {
            Some(pid) => Seen::Present(*pid),
            None => Seen::Absent,
        })
    }

    fn enforce_ceiling(&mut self) -> Vec<Change> {
        if self.remembered.len() <= self.max_remembered {
            return Vec::new();
        }
        let mut by_age: Vec<(u64, Key)> = self
            .remembered
            .iter()
            .map(|(key, url)| (url.last_seen, key.clone()))
            .collect();
        by_age.sort();
        let overflow = self.remembered.len() - self.max_remembered;
        let doomed: Vec<Key> = by_age
            .into_iter()
            .take(overflow)
            .map(|(_, key)| key)
            .collect();
        self.forget_keys(doomed)
    }

    fn forget_keys(&mut self, keys: Vec<Key>) -> Vec<Change> {
        let mut changes = Vec::new();
        for key in keys {
            self.baseline.remove(&key);
            self.settling.remove(&key);
            if self.remembered.remove(&key).is_some() {
                changes.push(Change {
                    owner: key.0,
                    port: key.1,
                });
            }
        }
        dedupe(changes)
    }

    fn hold_unbound(&mut self, source: &str, line: &str) {
        let held = match self.unbound.iter().position(|held| held.source == source) {
            Some(at) => {
                let held = self.unbound.remove(at);
                self.unbound.push(held);
                self.unbound.last_mut().expect("just pushed")
            }
            None => {
                self.unbound.push(Unbound {
                    source: source.to_string(),
                    lines: Vec::new(),
                    chars: 0,
                });
                self.unbound.last_mut().expect("just pushed")
            }
        };
        // Kept from the tail, like the original's `slice(-LIMIT)` (`:331`):
        // the newest text is where the banner is. A single line longer than
        // the whole allowance is itself cut to its tail — this is the module
        // whose thesis is that every retention point is bounded, and one
        // endless line must not be the exception.
        let mut kept: &str = line;
        if kept.chars().count() > UNBOUND_CHARS {
            let cut = kept
                .char_indices()
                .rev()
                .nth(UNBOUND_CHARS - 1)
                .map_or(0, |(index, _)| index);
            kept = &kept[cut..];
        }
        held.chars += kept.chars().count();
        held.lines.push(kept.to_string());
        while held.chars > UNBOUND_CHARS && held.lines.len() > 1 {
            let dropped = held.lines.remove(0);
            held.chars -= dropped.chars().count();
        }
        while self.unbound.len() > MAX_UNBOUND_SOURCES {
            self.unbound.remove(0);
        }
    }

    fn take_unbound(&mut self, source: &str) -> Vec<String> {
        match self.unbound.iter().position(|held| held.source == source) {
            Some(at) => self.unbound.remove(at).lines,
            None => Vec::new(),
        }
    }
}

/// Whether a candidate should take an existing entry's place (`:229-239`).
///
/// The whole of the original's three-step comparison is the tuple order:
/// kind first, then scheme, then recency. Asked only of a *different*
/// address — the same origin said again is a refresh, decided before this
/// question is put (see `consider`).
fn outranks(candidate: &AdvertisedUrl, existing: &AdvertisedUrl) -> bool {
    (candidate.kind, candidate.scheme, candidate.last_seen)
        >= (existing.kind, existing.scheme, existing.last_seen)
}

/// What kind of address a hostname is (`:150-172`).
pub fn classify_host(hostname: &str) -> HostKind {
    let bare = hostname
        .strip_prefix('[')
        .unwrap_or(hostname)
        .strip_suffix(']')
        .unwrap_or_else(|| hostname.strip_prefix('[').unwrap_or(hostname))
        .to_ascii_lowercase();
    if is_loopback_hostname(&bare) && !is_unspecified_host(&bare) {
        return HostKind::Loopback;
    }
    if let Ok(v4) = bare.parse::<std::net::Ipv4Addr>() {
        return classify_v4(v4);
    }
    if let Ok(v6) = bare.parse::<std::net::Ipv6Addr>() {
        // A v4 address wearing a v6 coat is the v4 address: WHATWG rewrites
        // `[::ffff:192.168.1.1]` into hextets, which the original's dot-count
        // sniff then files as PUBLIC — the lowest rank for a LAN address.
        if let Some(mapped) = v6.to_ipv4_mapped() {
            return classify_v4(mapped);
        }
        // **DEPARTURE:** the original sniffs v6 with `/^[0-9a-f:]+$/` and
        // reads the first hextet with `parseInt` (`:186-200`) — so any
        // address whose first group is elided is measured against `NaN`, and
        // the mere *prefix* letters `fc`/`fd` claim privacy (`fcd::1` is
        // `0fcd::1`, nowhere near fc00::/7). A parsed address answers both.
        if v6.is_loopback() {
            return HostKind::Loopback;
        }
        let unique_local = (v6.segments()[0] & 0xfe00) == 0xfc00;
        let link_local = (v6.segments()[0] & 0xffc0) == 0xfe80;
        return if unique_local || link_local {
            HostKind::PrivateIp
        } else {
            HostKind::PublicIp
        };
    }
    HostKind::Custom
}

/// The rank of a v4 address, wherever it was found.
///
/// **DEPARTURE:** the original's private list omits 127/8 entirely
/// (`:190-201`), so `127.0.0.2` — this same machine — ranks PUBLIC, below
/// every LAN address, and the first other banner overwrites it. The whole
/// loopback block is this machine; it ranks as this machine.
fn classify_v4(address: std::net::Ipv4Addr) -> HostKind {
    if address.is_loopback() {
        HostKind::Loopback
    } else if address.is_private() || address.is_link_local() {
        HostKind::PrivateIp
    } else {
        HostKind::PublicIp
    }
}

/// Every http(s) URL a settled line of terminal text contains (`:26,120-142`).
///
/// Permissive on the way in and strict on the way out: the scan only has to
/// find where a candidate starts and stops, and WHATWG parsing decides whether
/// what it found was an address at all.
pub fn urls_in_line(line: &str) -> Vec<Url> {
    // Byte-scanned rather than lowercased: this runs on every settled line of
    // every terminal, and an allocation per line to find the four letters
    // "http" is the module's whole budget spent before anything is found.
    // The original pays the same care with a five-`includes` prefilter (`:90-99`).
    let bytes = line.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(offset) = find_scheme_word(&bytes[at..]) {
        let start = at + offset;
        at = start + SCHEME_WORD.len();
        // The original's `\b`, which is ASCII in a regex without the `u` flag:
        // `xhttp://host` advertises nothing.
        if start > 0 && is_ascii_word_byte(bytes[start - 1]) {
            continue;
        }
        let rest = &bytes[at..];
        let scheme_len = if rest.starts_with(b"://") {
            b"://".len()
        } else if rest.len() >= 4 && rest[0].eq_ignore_ascii_case(&b's') && rest[1..4] == *b"://" {
            b"s://".len()
        } else {
            continue;
        };
        let candidate_start = at + scheme_len;
        let end = line[candidate_start..]
            .find(|ch: char| ch.is_whitespace() || CANDIDATE_STOPS.contains(&ch))
            .map_or(line.len(), |stop| candidate_start + stop);
        let candidate = &line[start..end];
        at = end;
        if candidate.chars().count() > CANDIDATE_MAX_CHARS {
            continue;
        }
        let candidate = candidate.trim_end_matches(TRAILING_PUNCTUATION);
        if let Some(url) = parse_advertised(candidate) {
            found.push(url);
        }
    }
    found
}

/// The four letters that start a scheme, in either case (`:26` — the `i` flag).
const SCHEME_WORD: &[u8; 4] = b"http";

/// Where "http" next appears, case-insensitively, without copying the line.
fn find_scheme_word(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < SCHEME_WORD.len() {
        return None;
    }
    (0..=bytes.len() - SCHEME_WORD.len()).find(|&index| {
        bytes[index..index + SCHEME_WORD.len()]
            .iter()
            .zip(SCHEME_WORD)
            .all(|(byte, want)| byte.to_ascii_lowercase() == *want)
    })
}

/// The `\w` of a JavaScript regex without the `u` flag — ASCII only. `http`
/// starts with an ASCII letter, so the byte before it is a whole character
/// and can be judged as one.
fn is_ascii_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// A candidate that is really an http(s) URL with a host (`:145-157`).
fn parse_advertised(candidate: &str) -> Option<Url> {
    let url = Url::parse(candidate).ok()?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return None;
    }
    url.host_str().filter(|host| !host.is_empty())?;
    Some(url)
}

/// One entry per port, keeping the PID only while it is unambiguous
/// (`:641-655`).
fn by_port(listeners: &[Listener]) -> HashMap<u16, Option<u32>> {
    let mut observed: HashMap<u16, Option<u32>> = HashMap::new();
    for listener in listeners {
        match observed.get(&listener.port) {
            None => {
                observed.insert(listener.port, listener.pid);
            }
            // **DEPARTURE:** the original also erases the PID when the other
            // row merely failed to name one (`:648-655`) — and a scanner that
            // cannot always name PIDs (Windows netstat without privilege)
            // would thereby switch reuse-detection off for every port it
            // reports. A missing name is missing information; only two
            // *different* names make the owner unknowable.
            Some(Some(held)) if listener.pid.is_some_and(|pid| pid != *held) => {
                // Two host-specific sockets on one port: which process owns
                // the address nobody can say, so nobody says it.
                observed.insert(listener.port, None);
            }
            Some(None) if listener.pid.is_some() => {
                observed.insert(listener.port, listener.pid);
            }
            Some(_) => {}
        }
    }
    observed
}

/// Whether two scans disagree about who holds a port (`:657-665`).
///
/// Presence changing is a disagreement; a PID that one scan could not name is
/// not, because an unnamed process is missing information rather than a
/// different process.
fn listener_changed(before: Seen, now: Seen) -> bool {
    match (before, now) {
        (Seen::Absent, Seen::Present(_)) | (Seen::Present(_), Seen::Absent) => true,
        (Seen::Present(Some(was)), Seen::Present(Some(is))) => was != is,
        _ => false,
    }
}

/// The same {workspace, port} announced once (`:667-681`).
fn dedupe(mut changes: Vec<Change>) -> Vec<Change> {
    let mut seen = HashSet::new();
    changes.retain(|change| seen.insert((change.owner.clone(), change.port)));
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_000;
    const PTY: &str = "pty-1";
    const WS: &str = "worktree-1";

    fn bound() -> AdvertisedUrls {
        let mut book = AdvertisedUrls::new();
        book.bind(PTY, WS, T0);
        book
    }

    fn origin_for(book: &mut AdvertisedUrls, port: u16) -> Option<String> {
        book.lookup(WS, port, None).url.map(|url| url.origin)
    }

    /// The line a dev server actually prints.
    #[test]
    fn a_banner_is_read_as_the_address_it_names() {
        let mut book = bound();
        let changes = book.observe(PTY, "  ➜  Local:   http://localhost:5173/", T0);
        assert_eq!(
            changes,
            vec![Change {
                owner: WS.to_string(),
                port: 5173
            }]
        );
        let url = book.lookup(WS, 5173, None).url.expect("remembered");
        assert_eq!(url.origin, "http://localhost:5173");
        assert_eq!(url.kind, HostKind::Loopback);
        assert_eq!(url.scheme, Scheme::Http);
        assert_eq!(url.source, PTY);
    }

    /// Only the origin is kept, because the rest of the line is often a secret.
    #[test]
    fn a_token_in_the_printed_url_is_not_remembered() {
        let mut book = bound();
        book.observe(
            PTY,
            "Ready at http://localhost:3000/auth/callback?token=hunter2#hash",
            T0,
        );
        let url = book.lookup(WS, 3000, None).url.expect("remembered");
        assert_eq!(url.origin, "http://localhost:3000");
        assert!(!url.origin.contains("hunter2"));
    }

    /// A name beats a loopback address, and https beats http within a kind.
    #[test]
    fn the_better_address_for_a_port_wins_whatever_order_they_arrive_in() {
        let mut book = bound();
        book.observe(PTY, "http://myapp.test:8080/", T0);
        book.observe(PTY, "http://127.0.0.1:8080/", T0 + 1);
        assert_eq!(
            origin_for(&mut book, 8080).as_deref(),
            Some("http://myapp.test:8080")
        );

        let mut book = bound();
        book.observe(PTY, "http://127.0.0.1:9000/", T0);
        book.observe(PTY, "http://myapp.test:9000/", T0 + 1);
        assert_eq!(
            origin_for(&mut book, 9000).as_deref(),
            Some("http://myapp.test:9000")
        );

        let mut book = bound();
        book.observe(PTY, "http://myapp.test:7000/", T0);
        book.observe(PTY, "https://myapp.test:7000/", T0 + 1);
        assert_eq!(
            origin_for(&mut book, 7000).as_deref(),
            Some("https://myapp.test:7000")
        );
        // ...and the weaker one said again does not take the place back.
        book.observe(PTY, "http://myapp.test:7000/", T0 + 2);
        assert_eq!(
            origin_for(&mut book, 7000).as_deref(),
            Some("https://myapp.test:7000")
        );
    }

    /// A wildcard bind is not an address a browser can open.
    #[test]
    fn a_wildcard_bind_printed_in_a_banner_is_not_an_address() {
        let mut book = bound();
        book.observe(
            PTY,
            "listening on http://0.0.0.0:4000 and http://[::]:4001",
            T0,
        );
        assert!(origin_for(&mut book, 4000).is_none());
        assert!(origin_for(&mut book, 4001).is_none());
    }

    /// Port zero is the kernel being asked to choose, not a port.
    #[test]
    fn port_zero_is_not_an_address() {
        let mut book = bound();
        assert!(
            book.observe(PTY, "serving on http://myapp.test:0/", T0)
                .is_empty()
        );
        assert!(origin_for(&mut book, 0).is_none());
    }

    /// Terminal punctuation is not part of the address.
    #[test]
    fn a_sentence_around_a_url_does_not_become_part_of_it() {
        let mut book = bound();
        book.observe(PTY, "Open (http://localhost:1234/app), then reload.", T0);
        assert_eq!(
            origin_for(&mut book, 1234).as_deref(),
            Some("http://localhost:1234")
        );
    }

    /// The original's word boundary, kept.
    #[test]
    fn a_word_ending_in_http_advertises_nothing() {
        assert!(urls_in_line("xhttp://localhost:5000/").is_empty());
        assert_eq!(urls_in_line("URL=http://localhost:5000/").len(), 1);
    }

    /// Both addresses on one line are read.
    #[test]
    fn two_addresses_on_one_line_are_both_read() {
        let found =
            urls_in_line("Local: http://localhost:5173/  Network: http://192.168.1.9:5173/");
        assert_eq!(found.len(), 2);
        let mut book = bound();
        book.observe(
            PTY,
            "Local: http://localhost:5173/  Network: http://192.168.1.9:5173/",
            T0,
        );
        // Loopback outranks a LAN address, whichever came second.
        assert_eq!(
            origin_for(&mut book, 5173).as_deref(),
            Some("http://localhost:5173")
        );
    }

    /// A blob with a scheme inside it is not a banner.
    #[test]
    fn an_endless_candidate_is_not_parsed() {
        let long = format!("http://localhost:5173/{}", "a".repeat(CANDIDATE_MAX_CHARS));
        assert!(urls_in_line(&long).is_empty());
    }

    /// Every kind of host, named.
    #[test]
    fn a_host_is_sorted_into_the_kind_that_decides_its_rank() {
        assert_eq!(classify_host("localhost"), HostKind::Loopback);
        assert_eq!(classify_host("127.0.0.1"), HostKind::Loopback);
        assert_eq!(classify_host("[::1]"), HostKind::Loopback);
        assert_eq!(classify_host("10.1.2.3"), HostKind::PrivateIp);
        assert_eq!(classify_host("172.16.0.1"), HostKind::PrivateIp);
        assert_eq!(classify_host("172.32.0.1"), HostKind::PublicIp);
        assert_eq!(classify_host("192.168.0.1"), HostKind::PrivateIp);
        assert_eq!(classify_host("169.254.1.1"), HostKind::PrivateIp);
        assert_eq!(classify_host("93.184.216.34"), HostKind::PublicIp);
        assert_eq!(classify_host("[fd00::1]"), HostKind::PrivateIp);
        assert_eq!(classify_host("[2001:db8::1]"), HostKind::PublicIp);
        assert_eq!(classify_host("myapp.test"), HostKind::Custom);
        // 127.0.0.2 is this same machine — the original files it PUBLIC, the
        // lowest rank, and the first other banner overwrites it.
        assert_eq!(classify_host("127.0.0.2"), HostKind::Loopback);
        // A v4 address in a v6 coat is the v4 address. WHATWG spells it in
        // hextets, which the original reads as a public v6.
        assert_eq!(classify_host("[::ffff:c0a8:101]"), HostKind::PrivateIp);
        assert_eq!(classify_host("[::ffff:7f00:1]"), HostKind::Loopback);
        // `fcd::1` is `0fcd::1` — nowhere near fc00::/7, whatever its first
        // two letters say. The original matches the letters.
        assert_eq!(classify_host("[fcd::1]"), HostKind::PublicIp);
        assert!(HostKind::Custom > HostKind::Loopback);
        assert!(HostKind::Loopback > HostKind::PrivateIp);
        assert!(HostKind::PrivateIp > HostKind::PublicIp);
    }

    /// The v6 shapes the original's hand-rolled sniff reads wrong.
    #[test]
    fn an_elided_first_hextet_is_still_the_address_it_is() {
        // `parseInt('')` is NaN, so the original calls this public.
        assert_eq!(classify_host("[::fe80]"), HostKind::PublicIp);
        assert_eq!(classify_host("[fe80::1]"), HostKind::PrivateIp);
        assert_eq!(classify_host("[febf::1]"), HostKind::PrivateIp);
        assert_eq!(classify_host("[fec0::1]"), HostKind::PublicIp);
    }

    /// Lines printed before anyone knew whose terminal it was are replayed.
    #[test]
    fn a_banner_printed_before_the_binding_is_not_lost() {
        let mut book = AdvertisedUrls::new();
        assert!(book.observe(PTY, "http://localhost:5173/", T0).is_empty());
        let changes = book.bind(PTY, WS, T0 + 1);
        assert_eq!(
            changes,
            vec![Change {
                owner: WS.to_string(),
                port: 5173
            }]
        );
        assert_eq!(
            origin_for(&mut book, 5173).as_deref(),
            Some("http://localhost:5173")
        );
    }

    /// A terminal that never binds cannot hold the book open forever.
    #[test]
    fn terminals_that_never_bind_are_bounded() {
        let mut book = AdvertisedUrls::new();
        for index in 0..MAX_UNBOUND_SOURCES + 4 {
            book.observe(&format!("pty-{index}"), "http://localhost:5173/", T0);
        }
        assert_eq!(book.unbound.len(), MAX_UNBOUND_SOURCES);
        // The oldest went first, and the newest is still there to replay.
        assert!(book.bind("pty-0", WS, T0).is_empty());
        assert!(!book.bind("pty-35", "worktree-2", T0).is_empty());
    }

    /// The terminal going away is the only expiry an SSH-forwarded address
    /// will ever get.
    #[test]
    fn closing_a_terminal_forgets_what_it_advertised() {
        let mut book = bound();
        book.observe(PTY, "http://myapp.test:8080/", T0);
        book.bind("pty-2", WS, T0);
        book.observe("pty-2", "http://other.test:9090/", T0);

        let changes = book.unbind(PTY);
        assert_eq!(
            changes,
            vec![Change {
                owner: WS.to_string(),
                port: 8080
            }]
        );
        assert!(origin_for(&mut book, 8080).is_none());
        assert!(origin_for(&mut book, 9090).is_some());
    }

    /// A workspace id is reused, so nothing of the old one may be left.
    #[test]
    fn forgetting_a_workspace_leaves_nothing_for_the_next_one() {
        let mut book = bound();
        book.observe(PTY, "http://myapp.test:8080/", T0);
        book.reconcile(
            &[WS],
            &[Listener {
                port: 8080,
                pid: Some(42),
            }],
        );

        let changes = book.forget_owner(WS);
        assert_eq!(
            changes,
            vec![Change {
                owner: WS.to_string(),
                port: 8080
            }]
        );
        assert!(book.snapshots.is_empty());
        assert!(book.baseline.is_empty());
        assert!(book.settling.is_empty());
        // The terminal is unbound too, so its next line is held rather than
        // credited to a workspace that no longer exists.
        assert!(book.observe(PTY, "http://myapp.test:8080/", T0).is_empty());
    }

    /// A workspace whose id ends in the cache separator is still itself.
    #[test]
    fn a_workspace_named_like_a_key_is_not_misread() {
        let odd = "worktree::8080";
        let mut book = AdvertisedUrls::new();
        book.bind(PTY, odd, T0);
        let changes = book.observe(PTY, "http://myapp.test:8080/", T0);
        assert_eq!(
            changes,
            vec![Change {
                owner: odd.to_string(),
                port: 8080
            }]
        );
    }

    /// A banner arrives before the socket does.
    #[test]
    fn one_scan_may_find_nothing_before_the_address_is_believed() {
        let mut book = bound();
        book.observe(PTY, "http://myapp.test:8080/", T0);
        // The settling scan: nothing listening yet, and the address survives.
        assert!(book.reconcile(&[WS], &[]).is_empty());
        assert!(origin_for(&mut book, 8080).is_some());
        // The second silent scan is the server not being there.
        assert_eq!(
            book.reconcile(&[WS], &[]),
            vec![Change {
                owner: WS.to_string(),
                port: 8080
            }]
        );
        assert!(origin_for(&mut book, 8080).is_none());
    }

    /// The scan catching up with the banner confirms it.
    #[test]
    fn the_scan_catching_up_with_the_banner_confirms_it() {
        let mut book = bound();
        book.observe(PTY, "http://myapp.test:8080/", T0);
        assert!(book.reconcile(&[WS], &[]).is_empty());
        assert!(
            book.reconcile(
                &[WS],
                &[Listener {
                    port: 8080,
                    pid: Some(7)
                }]
            )
            .is_empty()
        );
        // Having been confirmed, it no longer holds a settling grace: a later
        // silence is the server going away.
        assert_eq!(
            book.reconcile(&[WS], &[]),
            vec![Change {
                owner: WS.to_string(),
                port: 8080
            }]
        );
    }

    /// A hot-reload reprint of the same banner is not evidence about which
    /// process listens — the pin must survive it. (The original replaces the
    /// entry and silently unpins; a stranger asking next would be blessed.)
    #[test]
    fn a_reprinted_banner_does_not_unpin_its_listener() {
        let mut book = bound();
        book.observe(PTY, "http://myapp.test:8080/", T0);
        book.reconcile(
            &[WS],
            &[Listener {
                port: 8080,
                pid: Some(7),
            }],
        );
        book.lookup(WS, 8080, Some(7));
        book.observe(PTY, "http://myapp.test:8080/", T0 + 5);

        let found = book.lookup(WS, 8080, Some(9));
        assert!(found.url.is_none());
        assert_eq!(
            found.changes,
            vec![Change {
                owner: WS.to_string(),
                port: 8080
            }]
        );
    }

    /// Two terminals saying the same address: the newest speaker holds the
    /// eviction handle, so the address outlives the first terminal closing.
    #[test]
    fn the_newest_speaker_holds_the_eviction_handle() {
        let mut book = bound();
        book.bind("pty-2", WS, T0);
        book.observe(PTY, "http://myapp.test:8080/", T0);
        book.observe("pty-2", "http://myapp.test:8080/", T0 + 1);

        assert!(book.unbind(PTY).is_empty());
        assert!(origin_for(&mut book, 8080).is_some());
        assert!(!book.unbind("pty-2").is_empty());
        assert!(origin_for(&mut book, 8080).is_none());
    }

    /// One line longer than the whole allowance is cut to its tail — the
    /// bound holds against a single line as it does against many.
    #[test]
    fn one_endless_line_cannot_hold_the_book_open() {
        let mut book = AdvertisedUrls::new();
        let huge = format!("{} http://localhost:5173/", "a".repeat(UNBOUND_CHARS * 2));
        book.observe(PTY, &huge, T0);
        assert!(book.unbound[0].chars <= UNBOUND_CHARS);
        // The banner lives in the tail, so the tail is the right half to keep.
        let changes = book.bind(PTY, WS, T0);
        assert_eq!(
            changes,
            vec![Change {
                owner: WS.to_string(),
                port: 5173
            }]
        );
    }

    /// Another program on the port is not the one that printed the banner.
    #[test]
    fn a_port_changing_hands_drops_the_address_that_came_with_it() {
        let mut book = bound();
        book.observe(PTY, "http://myapp.test:8080/", T0);
        book.reconcile(
            &[WS],
            &[Listener {
                port: 8080,
                pid: Some(7),
            }],
        );
        // First lookup pins the listener.
        assert!(book.lookup(WS, 8080, Some(7)).url.is_some());
        // A different process now answers there.
        let found = book.lookup(WS, 8080, Some(9));
        assert!(found.url.is_none());
        assert_eq!(
            found.changes,
            vec![Change {
                owner: WS.to_string(),
                port: 8080
            }]
        );
    }

    /// A scan can evict a pinned address without anybody looking it up.
    #[test]
    fn a_scan_evicts_a_pinned_address_whose_process_changed() {
        let mut book = bound();
        book.observe(PTY, "http://myapp.test:8080/", T0);
        book.reconcile(
            &[WS],
            &[Listener {
                port: 8080,
                pid: Some(7),
            }],
        );
        book.lookup(WS, 8080, Some(7));
        assert_eq!(
            book.reconcile(
                &[WS],
                &[Listener {
                    port: 8080,
                    pid: Some(9)
                }]
            ),
            vec![Change {
                owner: WS.to_string(),
                port: 8080
            }]
        );
    }

    /// A scan that cannot name the process is missing information, not a
    /// different process.
    #[test]
    fn an_unnamed_process_does_not_unseat_a_pinned_address() {
        let mut book = bound();
        book.observe(PTY, "http://myapp.test:8080/", T0);
        book.reconcile(
            &[WS],
            &[Listener {
                port: 8080,
                pid: Some(7),
            }],
        );
        book.lookup(WS, 8080, Some(7));
        assert!(
            book.reconcile(
                &[WS],
                &[Listener {
                    port: 8080,
                    pid: None
                }]
            )
            .is_empty()
        );
        assert!(origin_for(&mut book, 8080).is_some());
    }

    /// Two sockets on one port make the process ambiguous, so neither is named.
    #[test]
    fn two_listeners_on_one_port_name_no_process() {
        let observed = by_port(&[
            Listener {
                port: 8080,
                pid: Some(7),
            },
            Listener {
                port: 8080,
                pid: Some(9),
            },
            Listener {
                port: 9090,
                pid: Some(3),
            },
        ]);
        assert_eq!(observed[&8080], None);
        assert_eq!(observed[&9090], Some(3));
    }

    /// A row that could not name its process does not erase the name another
    /// row gave — missing information is not a second process.
    #[test]
    fn an_unnamed_row_does_not_erase_a_named_one() {
        let named_first = by_port(&[
            Listener {
                port: 7070,
                pid: Some(4),
            },
            Listener {
                port: 7070,
                pid: None,
            },
        ]);
        assert_eq!(named_first[&7070], Some(4));
        let named_second = by_port(&[
            Listener {
                port: 7070,
                pid: None,
            },
            Listener {
                port: 7070,
                pid: Some(4),
            },
        ]);
        assert_eq!(named_second[&7070], Some(4));
    }

    /// A scan of other workspaces says nothing about this one.
    #[test]
    fn a_scan_that_did_not_look_here_evicts_nothing() {
        let mut book = bound();
        book.observe(PTY, "http://myapp.test:8080/", T0);
        assert!(book.reconcile(&["worktree-2"], &[]).is_empty());
        assert!(book.reconcile(&["worktree-2"], &[]).is_empty());
        assert!(origin_for(&mut book, 8080).is_some());
    }

    /// The connection's ports belong to no one workspace, so the best answer
    /// across all of them is the answer.
    #[test]
    fn the_best_address_across_a_connection_is_found_and_pinned() {
        let mut book = AdvertisedUrls::new();
        book.bind("pty-a", "ws-a", T0);
        book.bind("pty-b", "ws-b", T0);
        book.observe("pty-a", "http://127.0.0.1:8080/", T0);
        book.observe("pty-b", "http://myapp.test:8080/", T0);

        let found = book.best_across(&["ws-a", "ws-b"], 8080, Some(11));
        assert_eq!(
            found.url.map(|url| url.origin).as_deref(),
            Some("http://myapp.test:8080")
        );
        // Only the winner is pinned; the runner-up keeps its own state.
        assert_eq!(
            book.remembered[&("ws-b".to_string(), 8080)].validated_listener,
            Some(11)
        );
        assert_eq!(
            book.remembered[&("ws-a".to_string(), 8080)].validated_listener,
            None
        );
    }

    /// A pinned entry belonging to a departed process is dropped on the way
    /// past, not returned.
    #[test]
    fn the_best_search_evicts_the_pins_it_walks_over() {
        let mut book = AdvertisedUrls::new();
        book.bind("pty-a", "ws-a", T0);
        book.bind("pty-b", "ws-b", T0);
        book.observe("pty-a", "http://myapp.test:8080/", T0);
        book.observe("pty-b", "http://127.0.0.1:8080/", T0);
        book.lookup("ws-a", 8080, Some(11));

        let found = book.best_across(&["ws-a", "ws-b"], 8080, Some(22));
        assert_eq!(
            found.url.map(|url| url.origin).as_deref(),
            Some("http://127.0.0.1:8080")
        );
        assert_eq!(
            found.changes,
            vec![Change {
                owner: "ws-a".to_string(),
                port: 8080
            }]
        );
    }

    /// The book has a ceiling, and the oldest answer is the one that leaves.
    #[test]
    fn the_book_is_bounded_and_drops_the_stalest_first() {
        let mut book = AdvertisedUrls::with_capacity(2);
        book.bind(PTY, WS, T0);
        book.observe(PTY, "http://myapp.test:8001/", T0);
        book.observe(PTY, "http://myapp.test:8002/", T0 + 1);
        let changes = book.observe(PTY, "http://myapp.test:8003/", T0 + 2);

        assert!(changes.contains(&Change {
            owner: WS.to_string(),
            port: 8001
        }));
        assert!(origin_for(&mut book, 8001).is_none());
        assert!(origin_for(&mut book, 8002).is_some());
        assert!(origin_for(&mut book, 8003).is_some());
    }

    /// Saying it again keeps it from being the next one evicted.
    #[test]
    fn a_banner_said_again_refreshes_its_place_in_the_book() {
        let mut book = AdvertisedUrls::with_capacity(2);
        book.bind(PTY, WS, T0);
        book.observe(PTY, "http://myapp.test:8001/", T0);
        book.observe(PTY, "http://myapp.test:8002/", T0 + 1);
        // 8001 speaks again, so 8002 is now the stalest.
        book.observe(PTY, "http://myapp.test:8001/", T0 + 2);
        book.observe(PTY, "http://myapp.test:8003/", T0 + 3);

        assert!(origin_for(&mut book, 8001).is_some());
        assert!(origin_for(&mut book, 8002).is_none());
    }

    /// One address, one announcement.
    #[test]
    fn the_same_port_is_announced_once_per_call() {
        let mut book = bound();
        let changes = book.observe(
            PTY,
            "http://127.0.0.1:5173/ then http://myapp.test:5173/",
            T0,
        );
        assert_eq!(changes.len(), 1);
    }

    /// Dropping one answer by hand says so.
    #[test]
    fn invalidating_an_address_announces_it() {
        let mut book = bound();
        book.observe(PTY, "http://myapp.test:8080/", T0);
        assert_eq!(
            book.invalidate(WS, 8080),
            vec![Change {
                owner: WS.to_string(),
                port: 8080
            }]
        );
        assert!(book.invalidate(WS, 8080).is_empty());
    }
}
