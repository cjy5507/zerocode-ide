//! What `~/.ssh/config` already says about a person's machines.
//!
//! The SSH pane exists because somebody has already written this file, and
//! Orca reads it without being asked — a pane that made them type the same
//! host a second time is a pane they open once. This module is the READER
//! and stops there: it turns the file into plain host records. What becomes
//! of them — which are new, which drifted, which one a person deleted and
//! meant it — is [`crate::ssh_store::sync_from_config`]'s decision.
//!
//! The grammar honored here is OpenSSH's, narrowed to what a settings row can
//! hold rather than approximated: keywords are case-insensitive, a line whose
//! first word starts with `#` is a comment and a `#` anywhere else is DATA (a
//! `ProxyCommand` may carry one), `Host` opens a block, `Match` closes one and
//! is otherwise ignored entirely, and `Include` splices another file in at the
//! point it appears. A pattern holding `*`, `?` or `!` names a CLASS of
//! machines rather than one, so it is dropped: this file draws cards, and a
//! card has to be dialable.
//!
//! Nothing here is ever logged, and the one failure it reports carries the
//! path it could not read and not one byte of what was in it.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::ssh_store::DEFAULT_PORT;

/// A hand-written config is a few kilobytes, and an `Include` glob that pulls
/// in a directory of them is still meant to be read in one go. A megabyte
/// across every file is far past the largest real one and far short of a
/// paste that would make opening this pane stall.
const MAX_TOTAL_BYTES: u64 = 1_048_576;
/// `Include` is a fan-out and a directory glob is the shape that fans widest.
const MAX_FILES: usize = 64;
/// OpenSSH's own nesting limit for `Include`.
const MAX_DEPTH: usize = 16;
/// The store keeps 64 targets. Reading a few times that is enough to notice a
/// file this window will never import in full, and cheap enough to stop at.
const MAX_HOSTS: usize = 256;

/// One `Host` block, resolved for one concrete alias.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SshConfigHost {
    /// The alias exactly as written. This is what the system `ssh` binary
    /// will be handed: the file's own name for a machine outranks every field
    /// beside it, which is why it lands in `config_host` rather than `host`.
    pub(crate) alias: String,
    pub(crate) hostname: String,
    pub(crate) port: u16,
    pub(crate) username: String,
    pub(crate) identity_file: String,
    pub(crate) proxy_command: String,
    pub(crate) jump_host: String,
    /// Read because the block says them, kept because reading this file a
    /// second time would mean a second parser. They deliberately do NOT reach
    /// `SshTarget`: it has no column for any of them, and a column nothing
    /// fills is a column the next slice has to migrate. 1-g55c — the
    /// connection lifecycle — is where an agent socket, a GSSAPI answer and
    /// `ProxyUseFdpass` (which forces the system `ssh` transport) matter.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) identity_agent: String,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) identities_only: Option<bool>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) gssapi_authentication: Option<bool>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) proxy_use_fdpass: Option<bool>,
}

impl Default for SshConfigHost {
    fn default() -> Self {
        Self {
            alias: String::new(),
            hostname: String::new(),
            // A block that never says `Port` means 22, and saying so here
            // keeps every reader of this struct from having to know that.
            port: DEFAULT_PORT,
            username: String::new(),
            identity_file: String::new(),
            proxy_command: String::new(),
            jump_host: String::new(),
            identity_agent: String::new(),
            identities_only: None,
            gssapi_authentication: None,
            proxy_use_fdpass: None,
        }
    }
}

/// A `Host` block while it is still open. Every slot is an `Option` because
/// the rule is FIRST-wins — OpenSSH takes the earliest value it is given for
/// a keyword — and "not said yet" is what an empty string cannot express.
#[derive(Debug, Default)]
struct Block {
    aliases: Vec<String>,
    hostname: Option<String>,
    port: Option<u16>,
    username: Option<String>,
    identity_file: Option<String>,
    proxy_command: Option<String>,
    jump_host: Option<String>,
    identity_agent: Option<String>,
    identities_only: Option<bool>,
    gssapi_authentication: Option<bool>,
    proxy_use_fdpass: Option<bool>,
}

/// The file this window imports from.
///
/// A missing file is an empty list rather than a failure: most machines have
/// no `~/.ssh/config` at all, and the pane runs this every time it opens.
pub(crate) fn read_user_config() -> Result<Vec<SshConfigHost>, SshConfigError> {
    let Some(home) = dirs::home_dir() else {
        // A platform that will not say where home is has no `~/.ssh/config`
        // to read, which is the same answer as an empty one.
        return Ok(Vec::new());
    };
    read_config(&home.join(".ssh").join("config"), &home)
}

/// The reader proper. `home` is passed rather than asked for so a test can
/// point the whole grammar — `~` expansion, relative `Include`, the lot — at
/// a directory of its own.
pub(crate) fn read_config(path: &Path, home: &Path) -> Result<Vec<SshConfigHost>, SshConfigError> {
    let mut reader = Reader::new(home);
    match slurp(path, MAX_TOTAL_BYTES) {
        Ok(content) => {
            reader.remember(path);
            reader.spend(&content);
            reader.read_lines(&content, 0);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(SshConfigError {
                path: path.to_path_buf(),
            });
        }
    }
    // The last block is still open at end of file, and it is a block like any
    // other — the machines in it are exactly the ones a config that ends on
    // its most recent entry describes.
    reader.close_block();
    Ok(reader.hosts)
}

struct Reader<'a> {
    home: &'a Path,
    budget: u64,
    files: usize,
    seen: HashSet<PathBuf>,
    named: HashSet<String>,
    hosts: Vec<SshConfigHost>,
    /// `None` before the first `Host` — a directive there belongs to no
    /// machine this pane could draw — and after a `Match`.
    block: Option<Block>,
}

impl<'a> Reader<'a> {
    fn new(home: &'a Path) -> Self {
        Self {
            home,
            budget: MAX_TOTAL_BYTES,
            files: 0,
            seen: HashSet::new(),
            named: HashSet::new(),
            hosts: Vec::new(),
            block: None,
        }
    }

    /// Mark a file as read. Identity is the CANONICAL path, so two names for
    /// one file — a link, a `./` in the middle — are one entry in the cycle
    /// guard rather than two.
    fn remember(&mut self, path: &Path) {
        self.files += 1;
        self.seen
            .insert(fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
    }

    fn spend(&mut self, content: &str) {
        self.budget = self.budget.saturating_sub(content.len() as u64);
    }

    fn read_lines(&mut self, content: &str, depth: usize) {
        for raw in content.lines() {
            // `lines()` already drops a trailing `\r`, which is what makes a
            // config written on Windows the same config here.
            let line = raw.trim();
            // Only a line that BEGINS with `#` is a comment. A `#` further
            // along is part of the value — OpenSSH reads it that way, and a
            // `ProxyCommand` is exactly where one shows up.
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((keyword, rest)) = split_directive(line) else {
                continue;
            };
            let keyword = keyword.to_ascii_lowercase();
            match keyword.as_str() {
                "host" => {
                    self.close_block();
                    self.block = Some(Block {
                        // Several concrete patterns on one line are several
                        // machines; a pattern that matches a class is not one
                        // of them and is dropped where it stands.
                        aliases: split_openssh_arguments(rest)
                            .into_iter()
                            .filter(|pattern| !pattern.contains(['*', '?', '!']))
                            .collect(),
                        ..Block::default()
                    });
                }
                // A `Match` is a rule about a connection being made, and this
                // pane is not making one. It closes the block before it, and
                // everything under it is skipped until the next `Host`.
                "match" => self.close_block(),
                "include" => {
                    for argument in split_openssh_arguments(rest) {
                        self.read_include(&argument, depth);
                    }
                }
                _ => {
                    if let Some(block) = self.block.as_mut() {
                        apply(block, &keyword, rest, self.home);
                    }
                }
            }
        }
    }

    fn read_include(&mut self, argument: &str, depth: usize) {
        if depth >= MAX_DEPTH {
            return;
        }
        for path in self.include_paths(argument) {
            if self.files >= MAX_FILES || self.budget == 0 {
                return;
            }
            let canonical = match fs::canonicalize(&path) {
                Ok(found) => found,
                Err(_) => continue,
            };
            // A file that includes the file including it is a loop, and a
            // config that names the same file twice has already been read.
            if self.seen.contains(&canonical) {
                continue;
            }
            // A directory, a socket, a file past the budget, a file that is
            // not UTF-8: an include that cannot be read is skipped in silence
            // rather than taking the whole import down. The person who wrote
            // the line is not the person opening this pane.
            let Ok(content) = slurp(&path, self.budget) else {
                continue;
            };
            self.remember(&path);
            self.spend(&content);
            self.read_lines(&content, depth + 1);
        }
    }

    /// Every file one `Include` argument names.
    fn include_paths(&self, argument: &str) -> Vec<PathBuf> {
        // `%d`, `%u`, `%h` and the rest resolve against the connection being
        // made. This reader is not making one, and a path guessed from the
        // wrong token would read some other file — so it reads none.
        if argument.contains('%') {
            return Vec::new();
        }
        let expanded = expand_home(argument, self.home);
        let named = Path::new(&expanded);
        // OpenSSH resolves a relative `Include` in a user config against
        // `~/.ssh`, never against this process's working directory.
        let named = if named.is_absolute() {
            named.to_path_buf()
        } else {
            self.home.join(".ssh").join(named)
        };
        let Some(name) = named.file_name().and_then(|name| name.to_str()) else {
            return Vec::new();
        };
        if !name.contains('*') {
            return vec![named];
        }
        let Some(parent) = named.parent() else {
            return Vec::new();
        };
        // Only the last component may glob. A directory glob would mean
        // walking a tree, and this reader reads a directory it was named.
        if parent.to_string_lossy().contains('*') {
            return Vec::new();
        }
        let Ok(entries) = fs::read_dir(parent) else {
            return Vec::new();
        };
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|held| matches_glob(name, held))
            })
            .map(|entry| entry.path())
            .collect();
        // A directory hands back its entries in whatever order it stored
        // them; two machines reading one config have to agree on the answer.
        found.sort();
        found
    }

    /// Close the open block and write out one host per concrete alias in it.
    fn close_block(&mut self) {
        let Some(block) = self.block.take() else {
            return;
        };
        for alias in block.aliases {
            if self.hosts.len() >= MAX_HOSTS {
                return;
            }
            // The same alias in two blocks is one machine described twice,
            // and OpenSSH keeps the first value it was given.
            if !self.named.insert(alias.clone()) {
                continue;
            }
            self.hosts.push(SshConfigHost {
                alias,
                hostname: block.hostname.clone().unwrap_or_default(),
                port: block.port.unwrap_or(DEFAULT_PORT),
                username: block.username.clone().unwrap_or_default(),
                identity_file: block.identity_file.clone().unwrap_or_default(),
                proxy_command: block.proxy_command.clone().unwrap_or_default(),
                jump_host: block.jump_host.clone().unwrap_or_default(),
                identity_agent: block.identity_agent.clone().unwrap_or_default(),
                identities_only: block.identities_only,
                gssapi_authentication: block.gssapi_authentication,
                proxy_use_fdpass: block.proxy_use_fdpass,
            });
        }
    }
}

/// One directive, applied to the block it was written in.
fn apply(block: &mut Block, keyword: &str, rest: &str, home: &Path) {
    match keyword {
        "hostname" => first_wins(&mut block.hostname, first_argument(rest)),
        "port" => {
            // A value that is not a port is not an answer, so it does not
            // take the first-wins slot away from a later line that is one.
            if block.port.is_none() {
                block.port = first_argument(rest)
                    .and_then(|value| value.parse::<u16>().ok())
                    .filter(|port| *port > 0);
            }
        }
        "user" => first_wins(&mut block.username, first_argument(rest)),
        // LAST wins, and the first argument only. OpenSSH gathers every
        // `IdentityFile` line into a list it offers in order; a settings row
        // has one field, and the last line is what the file most recently
        // meant. `~` is expanded here because the row is read back on a
        // machine that may not be this one's home.
        "identityfile" => {
            if let Some(value) = first_argument(rest) {
                block.identity_file = Some(expand_home(&value, home));
            }
        }
        // Verbatim, to the end of the line: this is a command, `%h` and
        // quotes and all, and the system `ssh` binary is what reads it.
        "proxycommand" => first_wins(
            &mut block.proxy_command,
            (!rest.is_empty()).then(|| rest.to_string()),
        ),
        "proxyjump" => first_wins(&mut block.jump_host, first_argument(rest)),
        "identityagent" => first_wins(&mut block.identity_agent, first_argument(rest)),
        "identitiesonly" => first_wins(&mut block.identities_only, parsed_yes_no(rest)),
        "gssapiauthentication" => {
            first_wins(&mut block.gssapi_authentication, parsed_yes_no(rest));
        }
        "proxyusefdpass" => first_wins(&mut block.proxy_use_fdpass, parsed_yes_no(rest)),
        // Every other directive is a real one this pane has nowhere to put.
        // Dropping it silently is the point: an "unsupported keys" report
        // would be a list of things a person configured on purpose.
        _ => {}
    }
}

/// A value that is not there does not fill the slot, so a directive nobody
/// could read leaves the keyword still unanswered for the line after it.
fn first_wins<T>(slot: &mut Option<T>, value: Option<T>) {
    if slot.is_none() {
        *slot = value;
    }
}

fn first_argument(rest: &str) -> Option<String> {
    split_openssh_arguments(rest).into_iter().next()
}

/// `yes` or `no`, which is the whole vocabulary OpenSSH accepts for these.
/// `true`/`1` are not synonyms there and are not synonyms here.
fn parsed_yes_no(rest: &str) -> Option<bool> {
    match first_argument(rest)?.to_ascii_lowercase().as_str() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

/// A line's keyword and everything after it.
///
/// OpenSSH lets the two be parted by whitespace, by `=`, or by both, so
/// `Port 22`, `Port=22` and `Port = 22` are one line written three ways. A
/// line with no separator at all is a keyword with no arguments — `Match`
/// alone still closes the block it follows.
fn split_directive(line: &str) -> Option<(&str, &str)> {
    let end = line
        .find(|glyph: char| glyph.is_whitespace() || glyph == '=')
        .unwrap_or(line.len());
    let (keyword, rest) = line.split_at(end);
    let rest = rest.trim_start_matches(|glyph: char| glyph.is_whitespace() || glyph == '=');
    (!keyword.is_empty()).then_some((keyword, rest))
}

/// One line's arguments, split the way OpenSSH's `strdelim` splits them:
/// whitespace parts them, a double quote groups them.
///
/// The only escape is `\"`, and it is the only one on purpose. OpenSSH's own
/// splitter has no escape at all, and a Windows `IdentityFile
/// C:\Users\me\.ssh\id_ed25519` — or a UNC `\\host\share` — is a path, not a
/// run of escapes. Reading every backslash as one would quietly delete the
/// separators out of every Windows key path in the file.
pub(crate) fn split_openssh_arguments(line: &str) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    // Tracked apart from `current` so an explicitly empty `""` is an
    // argument rather than nothing at all.
    let mut started = false;
    let mut quoted = false;
    let mut glyphs = line.chars().peekable();
    while let Some(glyph) = glyphs.next() {
        match glyph {
            '\\' if glyphs.peek() == Some(&'"') => {
                glyphs.next();
                current.push('"');
                started = true;
            }
            '"' => {
                quoted = !quoted;
                started = true;
            }
            _ if glyph.is_whitespace() && !quoted => {
                if started {
                    arguments.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            _ => {
                current.push(glyph);
                started = true;
            }
        }
    }
    if started {
        arguments.push(current);
    }
    arguments
}

/// `*` and nothing else. `?` and `[…]` are read as the literal characters
/// they are: a config that needs them is rarer than this reader growing a
/// second grammar, and a name that does not match is an include that is
/// skipped rather than a file read by mistake.
fn matches_glob(pattern: &str, name: &str) -> bool {
    // `glob(3)` does not let a leading `*` reach a dotfile, and neither does
    // the shell somebody tested their `Include` line in.
    if name.starts_with('.') && !pattern.starts_with('.') {
        return false;
    }
    let segments: Vec<&str> = pattern.split('*').collect();
    let Some((head, tail)) = segments.split_first() else {
        return false;
    };
    let Some(last) = tail.last() else {
        return pattern == name;
    };
    let Some(mut rest) = name.strip_prefix(*head) else {
        return false;
    };
    for segment in &tail[..tail.len() - 1] {
        let Some(at) = rest.find(*segment) else {
            return false;
        };
        rest = &rest[at + segment.len()..];
    }
    rest.ends_with(*last)
}

/// `~` is the shell's, not the file system's: OpenSSH expands a LEADING one
/// against the home directory and leaves `~other` alone, because resolving
/// somebody else's home is a lookup this window does not make.
fn expand_home(value: &str, home: &Path) -> String {
    if value == "~" {
        return home.to_string_lossy().into_owned();
    }
    // The second spelling is what OpenSSH for Windows writes.
    for prefix in ["~/", "~\\"] {
        if let Some(rest) = value.strip_prefix(prefix) {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    value.to_string()
}

/// Read a file the USER owns, bounded.
///
/// A link is followed on purpose. `~/.ssh/config -> ~/dotfiles/ssh/config` is
/// how the person with a config worth importing keeps it, and refusing that
/// would refuse exactly them — this is not one of the application's own
/// artifacts, where [`crate::durable_file::open_plain_file`]'s link-refusing
/// open is the rule. What is still refused is a path that is not a regular
/// file at the end — a fifo would be a read that never returns — and a file
/// past the budget. No error raised here carries a byte of what was read.
fn slurp(path: &Path, limit: u64) -> io::Result<String> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    if metadata.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "over the read budget",
        ));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "over the read budget",
        ));
    }
    String::from_utf8(bytes).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not UTF-8"))
}

/// The one failure worth reporting: a config that is THERE and would not be
/// read. It carries the path and nothing else — an error line built out of a
/// parse failure is the fastest way to print somebody's `~/.ssh/config` into
/// a log file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SshConfigError {
    path: PathBuf,
}

impl fmt::Display for SshConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ssh_config_unreadable: {}", self.path.display())
    }
}

impl std::error::Error for SshConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("test parent");
        }
        fs::write(path, content).expect("test file");
    }

    /// Read a config written into a temporary home, so `~`, a relative
    /// `Include` and the file itself all point at the same invented machine.
    fn read(home: &Path, content: &str) -> Vec<SshConfigHost> {
        let path = home.join(".ssh").join("config");
        write(&path, content);
        read_config(&path, home).expect("the config reads")
    }

    fn named<'a>(hosts: &'a [SshConfigHost], alias: &str) -> &'a SshConfigHost {
        hosts
            .iter()
            .find(|host| host.alias == alias)
            .unwrap_or_else(|| panic!("`{alias}` is not in the parsed config"))
    }

    #[test]
    fn arguments_are_split_on_whitespace_and_grouped_by_quotes() {
        assert_eq!(split_openssh_arguments("  web  build   "), ["web", "build"]);
        assert_eq!(
            split_openssh_arguments("\"my server\" plain"),
            ["my server", "plain"]
        );
        // The one escape, and the one that must NOT be: a Windows key path
        // and a UNC share come back with every separator they arrived with.
        assert_eq!(
            split_openssh_arguments(r#""say \"hi\"" C:\Users\me\.ssh\id_ed25519 \\host\share"#),
            [
                "say \"hi\"",
                r"C:\Users\me\.ssh\id_ed25519",
                r"\\host\share",
            ]
        );
        // An explicitly empty argument is an argument; nothing at all is not.
        assert_eq!(split_openssh_arguments("\"\""), [""]);
        assert!(split_openssh_arguments("   ").is_empty());
    }

    #[test]
    fn a_line_parts_at_whitespace_an_equals_or_both() {
        for line in ["Port 2222", "Port=2222", "Port = 2222", "Port\t=\t2222"] {
            let (keyword, rest) = split_directive(line).expect("a directive");
            assert_eq!(
                (keyword.to_ascii_lowercase(), rest),
                ("port".to_string(), "2222"),
                "`{line}` did not part into a keyword and its argument"
            );
        }
        // A keyword alone still names itself, which is what lets a bare
        // `Match` close the block above it.
        assert_eq!(split_directive("Match"), Some(("Match", "")));
    }

    #[test]
    fn one_host_line_names_every_concrete_machine_on_it() {
        let home = tempfile::tempdir().expect("temp home");
        let hosts = read(
            home.path(),
            concat!(
                "# a comment line, and one that is not a comment below\n",
                "Host web build !offsite gpu-* dev?\n",
                "  HostName 10.0.0.5\n",
                "  User deploy\n",
            ),
        );
        assert_eq!(
            hosts
                .iter()
                .map(|host| host.alias.as_str())
                .collect::<Vec<_>>(),
            ["web", "build"],
            "a wildcard or negated pattern became a card"
        );
        // Both carry the block's directives — one block, two machines.
        assert!(hosts.iter().all(|host| host.hostname == "10.0.0.5"
            && host.username == "deploy"
            && host.port == DEFAULT_PORT));
    }

    #[test]
    fn a_match_block_is_ignored_and_closes_the_host_above_it() {
        let home = tempfile::tempdir().expect("temp home");
        let hosts = read(
            home.path(),
            concat!(
                "Host web\n",
                "  HostName 10.0.0.5\n",
                "Match host bastion exec \"true\"\n",
                "  User root\n",
                "  ProxyCommand nc %h %p\n",
                "Host build\n",
                "  User deploy\n",
            ),
        );
        assert_eq!(
            hosts
                .iter()
                .map(|host| host.alias.as_str())
                .collect::<Vec<_>>(),
            ["web", "build"],
            "a Match block became a machine"
        );
        assert_eq!(named(&hosts, "web").username, "", "Match leaked upward");
        assert_eq!(
            named(&hosts, "build").proxy_command,
            "",
            "a directive under Match reached the next block"
        );
    }

    #[test]
    fn the_first_answer_wins_except_the_key_where_the_last_one_does() {
        let home = tempfile::tempdir().expect("temp home");
        let hosts = read(
            home.path(),
            concat!(
                "Host web\n",
                "  User deploy\n",
                "  User someone-else\n",
                "  Port not-a-port\n",
                "  Port 2222\n",
                "  Port 2200\n",
                "  IdentityFile ~/.ssh/first_key\n",
                "  IdentityFile \"~/.ssh/second key\" ~/.ssh/ignored\n",
                "  ProxyCommand cloudflared access ssh --hostname %h # not a comment\n",
                "  ProxyJump bastion.example.test\n",
                "  IdentityAgent ~/.1password/agent.sock\n",
                "  IdentitiesOnly yes\n",
                "  GSSAPIAuthentication no\n",
                "  ProxyUseFdpass yes\n",
            ),
        );
        let web = named(&hosts, "web");
        assert_eq!(web.username, "deploy", "a second User overwrote the first");
        assert_eq!(
            web.port, 2222,
            "an unparseable Port either won or blocked the one that follows it"
        );
        // Last-wins, first argument only, `~` expanded against this home.
        assert_eq!(
            web.identity_file,
            home.path()
                .join(".ssh")
                .join("second key")
                .to_string_lossy(),
            "the identity file is not the last one named"
        );
        assert_eq!(
            web.proxy_command, "cloudflared access ssh --hostname %h # not a comment",
            "the proxy command was cut short of the end of its line"
        );
        assert_eq!(web.jump_host, "bastion.example.test");
        // Read and kept for 1-g55c, which is the slice with somewhere to put
        // them. The agent socket stays as written: `~` is expanded for the
        // key path because a settings row is read back elsewhere, and these
        // four are not read back anywhere yet.
        assert_eq!(
            (
                web.identity_agent.as_str(),
                web.identities_only,
                web.gssapi_authentication,
                web.proxy_use_fdpass,
            ),
            (
                "~/.1password/agent.sock",
                Some(true),
                Some(false),
                Some(true)
            )
        );
    }

    #[test]
    fn an_include_is_read_in_place_globbed_and_never_twice() {
        let home = tempfile::tempdir().expect("temp home");
        let ssh = home.path().join(".ssh");
        // A relative include resolves against `~/.ssh`, a `*` matches inside
        // one directory, and a dotfile stays out of a `*` the way glob(3)
        // keeps it out.
        write(
            &ssh.join("config.d").join("10-web"),
            "Host web\n  Port 2222\n",
        );
        write(
            &ssh.join("config.d").join("20-build"),
            "Host build\n  User deploy\n",
        );
        write(&ssh.join("config.d").join(".hidden"), "Host hidden\n");
        // The cycle: this file includes the file that includes it.
        write(
            &ssh.join("loop"),
            "Include ~/.ssh/config\nHost tail\n  HostName 10.0.0.9\n",
        );
        let hosts = read(
            home.path(),
            concat!(
                "Include config.d/*\n",
                "Include ~/.ssh/loop\n",
                // Percent tokens resolve against a connection nobody is
                // making here, so the line is skipped rather than guessed.
                "Include %d/.ssh/never\n",
                "Host web\n",
                "  User overwritten-by-nobody\n",
            ),
        );
        assert_eq!(
            hosts
                .iter()
                .map(|host| host.alias.as_str())
                .collect::<Vec<_>>(),
            ["web", "build", "tail"],
            "an include was read out of order, twice, or not at all"
        );
        let web = named(&hosts, "web");
        assert_eq!(web.port, 2222, "the included block did not land");
        assert_eq!(
            web.username, "",
            "a later block reopened an alias the include already named"
        );
    }

    #[test]
    fn a_missing_or_empty_config_is_an_empty_list_and_not_a_failure() {
        let home = tempfile::tempdir().expect("temp home");
        let missing = home.path().join(".ssh").join("config");
        assert_eq!(read_config(&missing, home.path()), Ok(Vec::new()));
        assert!(read(home.path(), "").is_empty());
        // A directive with no `Host` above it belongs to no machine, and a
        // file of them still describes none.
        assert!(read(home.path(), "User deploy\nPort 2222\n").is_empty());
        // What IS a failure: a config that is there and will not be read.
        let directory = home.path().join(".ssh").join("as-a-directory");
        fs::create_dir_all(&directory).expect("test directory");
        let refused =
            read_config(&directory, home.path()).expect_err("a directory is not a config");
        assert!(
            refused.to_string().starts_with("ssh_config_unreadable:"),
            "the failure did not name itself: {refused}"
        );
    }

    #[test]
    fn a_glob_matches_the_way_the_shell_that_was_tested_in_does() {
        assert!(matches_glob("*", "10-web"));
        assert!(matches_glob("*.conf", "web.conf"));
        assert!(matches_glob("10-*-web", "10-staging-web"));
        assert!(!matches_glob("*.conf", "web.conf.bak"));
        assert!(!matches_glob("*", ".hidden"));
        assert!(matches_glob(".*", ".hidden"));
        // No second grammar: `?` is the character it looks like.
        assert!(!matches_glob("web?", "webs"));
        assert!(matches_glob("web?", "web?"));
    }
}
