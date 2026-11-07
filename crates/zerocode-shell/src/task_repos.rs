//! Where the tasks page's repositories live (2026-11-06).
//!
//! The page used to judge "is there a GitHub source" by the one checkout that
//! was active when the status was read, and asked GitLab only about that
//! checkout — so a person standing in a folder project (`work`, ten company
//! checkouts) was told there was no source and asked to sign in to gitlab.com.
//! This reads every repository the sidebar catalog reaches (the catalogued
//! ones and the ones directly inside a catalogued folder, see
//! `catalog_repository_roots`) and sorts them by where their remote lives:
//! GitHub, a GitLab instance (each one its own row, with whether `glab` is
//! signed in to it), or somewhere this page has no source for.
//!
//! Nothing here reads a credential. Hosts come from remote URLs with any
//! userinfo dropped (`gh::remote_host`), and the only network call is one
//! unauthenticated `GET /api/v4/version` to a host that no name, no CLI and no
//! list could place — asked once per host per run.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

/// One remote, read into the facts the page needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteParts {
    /// The host, lowercase, without userinfo or port.
    pub(crate) host: String,
    /// The port of a web remote (`http://host:8080/…`) — the instance's own
    /// address. An ssh remote's port is the ssh daemon's, not the web's, so it
    /// is never kept.
    pub(crate) port: Option<u16>,
    /// `https` or `http` for a web remote; `None` for ssh and the scp form.
    pub(crate) scheme: Option<&'static str>,
    /// The repository's path on that host — `group/sub/name`, no `.git`.
    pub(crate) slug: String,
}

impl RemoteParts {
    /// The instance's web address — what `glab --hostname` and a browser use.
    pub(crate) fn address(&self) -> String {
        match self.port {
            Some(port) => format!("{}:{port}", self.host),
            None => self.host.clone(),
        }
    }
}

/// A host without a `:port` tail, lowercase — how two spellings of one
/// machine (an ssh remote and `glab`'s `host:8443`) are recognised as one.
pub(crate) fn bare_host(host: &str) -> String {
    let host = host.trim().to_ascii_lowercase();
    match host.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            name.to_string()
        }
        _ => host,
    }
}

/// Each remote `git remote -v` lists, with its fetch URL, in git's order —
/// one git process for every remote a repository has.
pub(crate) fn fetch_remotes(listed: &str) -> Vec<(String, String)> {
    let mut remotes: Vec<(String, String)> = Vec::new();
    for line in listed.lines() {
        let mut fields = line.split_whitespace();
        let (Some(name), Some(url), Some("(fetch)")) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if !remotes.iter().any(|(held, _)| held == name) {
            remotes.push((name.to_string(), url.to_string()));
        }
    }
    remotes
}

/// The fetch URL this page reads a repository by: the remote its catalog row
/// names (`git_config::primary_remote_name` — `upstream`, then `origin`, then
/// the first by name). Git lists remotes by name, so "the first one listed"
/// would read a person's `fork` before the company's `upstream`.
pub(crate) fn chosen_fetch_url(listed: &str) -> Option<String> {
    let remotes = fetch_remotes(listed);
    let picked = zerocode_core::git_config::primary_remote_name(
        remotes.iter().map(|(name, _)| name.as_str()),
    )?;
    remotes
        .iter()
        .find(|(name, _)| name == picked)
        .map(|(_, url)| url.clone())
}

/// The host of the scp form without a user (`gitlab.acme.test:group/app.git`),
/// which `gh::remote_host` leaves alone. A dotted name before the colon and no
/// slash in it — a Windows drive (`C:\work`) has neither.
fn userless_scp_host(url: &str) -> Option<String> {
    let (name, path) = url.trim().split_once(':')?;
    let plausible =
        name.contains('.') && !name.contains(['/', '\\', '@']) && !path.starts_with("//");
    plausible.then(|| crate::gh::remote_host(&format!("git@{name}:{path}")))?
}

/// A remote URL in any of the shapes git accepts — `https://host/a/b.git`,
/// `ssh://git@host:2222/a/b.git`, `git@host:a/b.git` — or `None` for a local
/// path, which has no host to ask.
pub(crate) fn remote_parts(url: &str) -> Option<RemoteParts> {
    let host = crate::gh::remote_host(url).or_else(|| userless_scp_host(url))?;
    let trimmed = url.trim();
    let scheme = if trimmed.starts_with("https://") {
        Some("https")
    } else if trimmed.starts_with("http://") {
        Some("http")
    } else {
        None
    };
    let (authority, path) = match trimmed.split_once("://") {
        Some((_, rest)) => rest.split_once('/').unwrap_or((rest, "")),
        None => trimmed.split_once(':').unwrap_or(("", "")),
    };
    let port = scheme.and_then(|_| {
        let host_port = authority.rsplit_once('@').map_or(authority, |(_, at)| at);
        host_port.rsplit_once(':')?.1.parse::<u16>().ok()
    });
    let slug = path
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_matches('/')
        .trim_end_matches(".git")
        .trim_matches('/')
        .to_string();
    Some(RemoteParts {
        host,
        port,
        scheme,
        slug,
    })
}

/// Which source a host belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Forge {
    GitHub,
    GitLab,
    /// Bitbucket, Azure and the like — this page has no source for them.
    Other,
}

/// Hosts known not to be GitLab — never probed.
const NOT_GITLAB: &[&str] = &[
    "bitbucket.org",
    "dev.azure.com",
    "visualstudio.com",
    "codeberg.org",
    "sr.ht",
    "gitee.com",
];

/// The source a host belongs to by its name and the two CLIs' own lists, or
/// `None` when nothing places it (the caller may then ask the host itself).
pub(crate) fn forge_by_name(
    host: &str,
    github_hosts: &[String],
    gitlab_hosts: &[String],
) -> Option<Forge> {
    let host = bare_host(host);
    let listed = |hosts: &[String]| hosts.iter().any(|known| bare_host(known) == host);
    if host == "github.com" || listed(github_hosts) {
        return Some(Forge::GitHub);
    }
    if host == "gitlab.com" || host.contains("gitlab") || listed(gitlab_hosts) {
        return Some(Forge::GitLab);
    }
    let under = |known: &str| host == known || host.ends_with(&format!(".{known}"));
    if NOT_GITLAB.iter().any(|known| under(known)) {
        return Some(Forge::Other);
    }
    None
}

/// One repository on the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SourceRepo {
    /// The repository root — what the list commands are asked about.
    pub(crate) path: String,
    /// The folder's name, as the sidebar would say it.
    pub(crate) name: String,
    /// Where it lives on its host.
    pub(crate) slug: String,
}

impl SourceRepo {
    pub(crate) fn at(root: &Path, parts: &RemoteParts) -> Self {
        Self {
            path: root.to_string_lossy().into_owned(),
            name: root.file_name().map_or_else(
                || root.to_string_lossy().into_owned(),
                |name| name.to_string_lossy().into_owned(),
            ),
            slug: parts.slug.clone(),
        }
    }
}

/// One GitLab instance and the repositories that live on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GitlabHost {
    /// The machine, without a port — one row however its remotes spell it.
    pub(crate) host: String,
    /// Its web address (`host` or `host:port`, from a web remote that named a
    /// port) — what `glab auth login --hostname` and the token page use.
    pub(crate) address: String,
    /// What `glab auth login --api-protocol` should say: `http` when any of
    /// its remotes is plain http (a company server without TLS), else `https`.
    pub(crate) api_protocol: &'static str,
    /// What `--git-protocol` should say: `ssh` when any of its remotes is
    /// ssh — the person's pushes already go that way — else the web scheme.
    pub(crate) git_protocol: &'static str,
    /// Whether `glab` says it is signed in to this host.
    pub(crate) signed_in: bool,
    pub(crate) repos: Vec<SourceRepo>,
}

/// A host the page has no source for, and how many repositories live there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OtherHost {
    pub(crate) host: String,
    pub(crate) count: usize,
}

/// The whole answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct TaskSources {
    pub(crate) github: Vec<SourceRepo>,
    pub(crate) gitlab: Vec<GitlabHost>,
    pub(crate) other: Vec<OtherHost>,
}

/// Sort the repositories into their sources. `probed` answers, by address,
/// the hosts nothing else placed — absent means "not GitLab". `signed_in` is
/// every host `glab` names as signed in (any spelling, port or none): `glab`
/// calls a machine signed out while ANY of its configured hosts is, so the
/// machine-wide flag cannot say which instance is usable. Order is the
/// catalog's.
pub(crate) fn group(
    repos: Vec<(SourceRepo, RemoteParts)>,
    github_hosts: &[String],
    gitlab_hosts: &[String],
    signed_in: &[String],
    probed: &HashMap<String, bool>,
) -> TaskSources {
    let mut sources = TaskSources::default();
    for (repo, parts) in repos {
        let forge = forge_by_name(&parts.host, github_hosts, gitlab_hosts).unwrap_or_else(|| {
            if probed.get(&parts.address()).copied().unwrap_or(false) {
                Forge::GitLab
            } else {
                Forge::Other
            }
        });
        match forge {
            Forge::GitHub => sources.github.push(repo),
            Forge::GitLab => {
                let at = match sources
                    .gitlab
                    .iter()
                    .position(|held| held.host == parts.host)
                {
                    Some(at) => at,
                    None => {
                        sources.gitlab.push(GitlabHost {
                            host: parts.host.clone(),
                            address: parts.host.clone(),
                            api_protocol: "https",
                            git_protocol: "https",
                            signed_in: signed_in.iter().any(|host| bare_host(host) == parts.host),
                            repos: Vec::new(),
                        });
                        sources.gitlab.len() - 1
                    }
                };
                let row = &mut sources.gitlab[at];
                if parts.port.is_some() {
                    row.address = parts.address();
                }
                match parts.scheme {
                    Some("http") => {
                        row.api_protocol = "http";
                        if row.git_protocol != "ssh" {
                            row.git_protocol = "http";
                        }
                    }
                    Some(_) => {}
                    None => row.git_protocol = "ssh",
                }
                row.repos.push(repo);
            }
            Forge::Other => match sources
                .other
                .iter_mut()
                .find(|held| held.host == parts.host)
            {
                Some(held) => held.count += 1,
                None => sources.other.push(OtherHost {
                    host: parts.host,
                    count: 1,
                }),
            },
        }
    }
    sources
}

/// How long one host gets to answer the GitLab question.
const PROBE_BUDGET: Duration = Duration::from_millis(2500);

/// What hosts said, by address, for this run.
static PROBED: Mutex<Option<HashMap<String, bool>>> = Mutex::new(None);

fn remembered(address: &str) -> Option<bool> {
    PROBED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .and_then(|held| held.get(address).copied())
}

/// How long a host that answered nothing is left unasked: long enough that
/// the task page's reads do not each wait out the probe again, short enough
/// that joining the network is noticed within a minute.
const UNANSWERED_FOR: Duration = Duration::from_secs(60);

/// When each host last answered nothing, by address, for this run.
static UNANSWERED: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

fn note_unanswered(address: &str, at: Instant) {
    UNANSWERED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .insert(address.to_string(), at);
}

fn unanswered_lately(address: &str, now: Instant) -> bool {
    UNANSWERED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .and_then(|held| held.get(address).copied())
        .is_some_and(|at| now.saturating_duration_since(at) < UNANSWERED_FOR)
}

fn remember(address: &str, gitlab: bool) {
    PROBED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .insert(address.to_string(), gitlab);
}

/// Whether an answer is GitLab's: `GET /api/v4/version` is 401 without a
/// token (200 on an instance anyone may read), and the body is GitLab's JSON
/// — `{"message":"401 Unauthorized"}` or `{"version":…}`.
pub(crate) fn reads_like_gitlab(status: u16, body: &str) -> bool {
    matches!(status, 200 | 401 | 403)
        && body.trim_start().starts_with('{')
        && (body.contains("\"message\"") || body.contains("\"version\""))
}

/// Ask a host nothing else placed whether it is a GitLab. A web remote is
/// asked on its own scheme; an ssh remote on https, then http. No redirect is
/// followed (a login page is not an answer) and nothing is sent but the GET.
pub(crate) async fn answers_like_gitlab(address: &str, scheme: Option<&'static str>) -> bool {
    if let Some(known) = remembered(address) {
        return known;
    }
    if unanswered_lately(address, Instant::now()) {
        return false;
    }
    let Ok(client) = reqwest::Client::builder()
        .timeout(PROBE_BUDGET)
        .redirect(reqwest::redirect::Policy::none())
        .build()
    else {
        return false;
    };
    let schemes: &[&str] = match scheme {
        Some("http") => &["http"],
        Some(_) => &["https"],
        None => &["https", "http"],
    };
    let mut answered = None;
    for scheme in schemes {
        let Ok(answer) = client
            .get(format!("{scheme}://{address}/api/v4/version"))
            .send()
            .await
        else {
            continue;
        };
        let status = answer.status().as_u16();
        let body = answer.text().await.unwrap_or_default();
        answered = Some(reads_like_gitlab(status, &body));
        break;
    }
    // A host that answered nothing (the VPN not up yet, the office network
    // not joined) is not known to be anything: it is asked again once
    // UNANSWERED_FOR has passed, so joining the network is found within a
    // minute and the reads in between do not each wait out the probe.
    match answered {
        Some(gitlab) => remember(address, gitlab),
        None => note_unanswered(address, Instant::now()),
    }
    answered.unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review of v1.1.128 (t-97404 P2): a host that answered nothing — the
    /// company GitLab with the VPN down — is left unasked for a while, so every
    /// read of the task page does not wait out the 2.5 s probe again.
    #[test]
    fn a_host_that_answered_nothing_is_left_unasked_for_a_minute() {
        let at = Instant::now();
        let address = "unanswered-probe.test";
        note_unanswered(address, at);
        assert!(unanswered_lately(address, at + Duration::from_secs(1)));
        assert!(!unanswered_lately(
            address,
            at + UNANSWERED_FOR + Duration::from_secs(1)
        ));
        assert!(!unanswered_lately("another-host.test", at));
    }

    /// Review of v1.1.128 (P3-G): a host that answered nothing is not known to
    /// be anything — it is never remembered as "not a GitLab" for the run, only
    /// left unasked for a while — and a host that answered is remembered. Both
    /// against 127.0.0.1: a port nobody listens on, and one that answers as a
    /// GitLab without a token does.
    #[tokio::test]
    async fn only_an_answer_is_remembered_and_silence_is_left_for_a_while() {
        let silent = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
            format!(
                "127.0.0.1:{}",
                listener.local_addr().expect("its port").port()
            )
        };
        assert!(!answers_like_gitlab(&silent, Some("http")).await);
        assert_eq!(
            remembered(&silent),
            None,
            "a host that answered nothing was remembered for the whole run"
        );
        assert!(unanswered_lately(&silent, Instant::now()));

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        let answering = format!(
            "127.0.0.1:{}",
            listener.local_addr().expect("its port").port()
        );
        let server = std::thread::spawn(move || {
            use std::io::{Read as _, Write as _};
            let (mut stream, _) = listener.accept().expect("the probe");
            let mut asked = [0_u8; 1024];
            let _ = stream.read(&mut asked);
            let body = r#"{"message":"401 Unauthorized"}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        });
        assert!(answers_like_gitlab(&answering, Some("http")).await);
        server.join().expect("the fake GitLab");
        assert_eq!(remembered(&answering), Some(true));
        assert!(!unanswered_lately(&answering, Instant::now()));
    }

    #[test]
    fn a_remote_is_read_in_every_shape_git_accepts() {
        let web = remote_parts("https://github.com/acme/app.git").expect("https");
        assert_eq!(
            (web.host.as_str(), web.port, web.scheme, web.slug.as_str()),
            ("github.com", None, Some("https"), "acme/app")
        );
        let plain = remote_parts("http://192.0.2.14/ai/search-index.git").expect("http");
        assert_eq!(
            (plain.host.as_str(), plain.scheme, plain.slug.as_str()),
            ("192.0.2.14", Some("http"), "ai/search-index")
        );
        let ssh =
            remote_parts("ssh://git@gitlab.acme.test:2222/mobile/ios/app.git").expect("ssh url");
        assert_eq!(
            (ssh.host.as_str(), ssh.port, ssh.scheme, ssh.slug.as_str()),
            ("gitlab.acme.test", None, None, "mobile/ios/app"),
            "an ssh port is the daemon's, not the instance's address"
        );
        let scp = remote_parts("git@gitlab.acme.test:acme/card.git").expect("scp");
        assert_eq!(
            (scp.host.as_str(), scp.scheme, scp.slug.as_str()),
            ("gitlab.acme.test", None, "acme/card")
        );
        assert_eq!(remote_parts("../local/repo"), None);
    }

    #[test]
    fn the_fetch_url_of_each_remote_is_read_from_one_listing() {
        let listed = "origin\thttps://github.com/acme/app.git (fetch)\n\
                      origin\thttps://github.com/acme/app.git (push)\n\
                      upstream\tgit@gitlab.acme.test:mobile/app.git (fetch)\n\
                      upstream\tgit@gitlab.acme.test:mobile/app.git (push)\n";
        assert_eq!(
            fetch_remotes(listed),
            vec![
                (
                    "origin".to_string(),
                    "https://github.com/acme/app.git".to_string()
                ),
                (
                    "upstream".to_string(),
                    "git@gitlab.acme.test:mobile/app.git".to_string()
                ),
            ]
        );
        assert!(fetch_remotes("").is_empty());
        assert!(fetch_remotes("origin\thttps://github.com/acme/app.git (push)\n").is_empty());
    }

    /// Review of v1.1.128 (P3-B): `git remote -v` lists remotes by name, so a
    /// repository whose remotes are `upstream` (the company's) and `fork` (a
    /// person's copy) lists `fork` first. The listings below are in that real
    /// order; the page reads the repository the catalog row names —
    /// `upstream`, then `origin`, then the first by name.
    #[test]
    fn a_repository_is_read_by_the_remote_its_catalog_row_names() {
        let fork_and_upstream = "fork\thttps://github.com/dev/app.git (fetch)\n\
                                 fork\thttps://github.com/dev/app.git (push)\n\
                                 upstream\thttps://github.com/acme/app.git (fetch)\n\
                                 upstream\thttps://github.com/acme/app.git (push)\n";
        assert_eq!(
            chosen_fetch_url(fork_and_upstream).as_deref(),
            Some("https://github.com/acme/app.git"),
            "the person's fork was read instead of the company repository"
        );
        let fork_and_origin = "fork\thttps://github.com/dev/app.git (fetch)\n\
                               origin\thttps://github.com/acme/app.git (fetch)\n";
        assert_eq!(
            chosen_fetch_url(fork_and_origin).as_deref(),
            Some("https://github.com/acme/app.git")
        );
        let origin_and_upstream = "origin\thttps://github.com/dev/app.git (fetch)\n\
                                   upstream\thttps://github.com/acme/app.git (fetch)\n";
        assert_eq!(
            chosen_fetch_url(origin_and_upstream).as_deref(),
            Some("https://github.com/acme/app.git"),
            "the page read another repository than the catalog row names"
        );
        let neither = "mirror\thttps://github.com/acme/mirror.git (fetch)\n\
                       zeta\thttps://github.com/acme/zeta.git (fetch)\n";
        assert_eq!(
            chosen_fetch_url(neither).as_deref(),
            Some("https://github.com/acme/mirror.git")
        );
        assert_eq!(chosen_fetch_url(""), None);
    }

    #[test]
    fn the_scp_form_without_a_user_is_placed_and_a_windows_drive_is_not() {
        let userless = remote_parts("gitlab.acme.test:group/app.git").expect("userless scp");
        assert_eq!(
            (
                userless.host.as_str(),
                userless.scheme,
                userless.slug.as_str()
            ),
            ("gitlab.acme.test", None, "group/app")
        );
        assert_eq!(remote_parts("C:\\work\\repo"), None);
        assert_eq!(remote_parts("/srv/git/app.git"), None);
    }

    #[test]
    fn a_web_remote_keeps_its_port_and_drops_its_userinfo() {
        let ported = remote_parts("http://dev@192.0.2.20:8080/team/api.git?x=1")
            .expect("a ported web remote");
        assert_eq!(
            (ported.host.as_str(), ported.port, ported.slug.as_str()),
            ("192.0.2.20", Some(8080), "team/api")
        );
        assert_eq!(ported.address(), "192.0.2.20:8080");
        assert_eq!(bare_host("GitLab.Acme.Test:8443"), "gitlab.acme.test");
        assert_eq!(bare_host("gitlab.acme.test"), "gitlab.acme.test");
    }

    #[test]
    fn a_host_is_placed_by_its_name_and_the_clis_lists_before_anyone_asks_it() {
        let none: Vec<String> = Vec::new();
        assert_eq!(
            forge_by_name("github.com", &none, &none),
            Some(Forge::GitHub)
        );
        assert_eq!(
            forge_by_name("ghe.acme.test", &["ghe.acme.test".into()], &none),
            Some(Forge::GitHub)
        );
        assert_eq!(
            forge_by_name("gitlab.com", &none, &none),
            Some(Forge::GitLab)
        );
        assert_eq!(
            forge_by_name("gitlab.acme.test", &none, &none),
            Some(Forge::GitLab)
        );
        assert_eq!(
            forge_by_name("192.0.2.14", &none, &["192.0.2.14:8443".into()]),
            Some(Forge::GitLab)
        );
        assert_eq!(
            forge_by_name("bitbucket.org", &none, &none),
            Some(Forge::Other)
        );
        assert_eq!(
            forge_by_name("ssh.bitbucket.org", &none, &none),
            Some(Forge::Other)
        );
        assert_eq!(
            forge_by_name("192.0.2.14", &none, &none),
            None,
            "an unplaced host must be asked, not guessed"
        );
    }

    fn repo(path: &str, url: &str) -> (SourceRepo, RemoteParts) {
        let parts = remote_parts(url).expect("a remote");
        (SourceRepo::at(Path::new(path), &parts), parts)
    }

    #[test]
    fn repositories_are_sorted_into_sources_and_a_gitlab_host_learns_its_protocols() {
        let repos = vec![
            repo("/projects/zerocode", "https://github.com/acme/zerocode.git"),
            repo(
                "/projects/work/search-index",
                "http://192.0.2.14/ai/search-index.git",
            ),
            repo(
                "/projects/work/registry",
                "http://gitlab.acme.test/common/registry.git",
            ),
            repo(
                "/projects/work/ios",
                "ssh://git@gitlab.acme.test:2222/mobile/ios/app.git",
            ),
            repo("/projects/work/card", "https://bitbucket.org/acme/card.git"),
            repo("/projects/work/core", "https://bitbucket.org/acme/core.git"),
        ];
        let probed = HashMap::from([("192.0.2.14".to_string(), true)]);
        let sources = group(repos, &[], &[], &["gitlab.acme.test".into()], &probed);
        assert_eq!(
            sources
                .github
                .iter()
                .map(|one| one.name.as_str())
                .collect::<Vec<_>>(),
            ["zerocode"]
        );
        assert_eq!(sources.gitlab.len(), 2);
        let ip = &sources.gitlab[0];
        assert_eq!(
            (
                ip.host.as_str(),
                ip.api_protocol,
                ip.git_protocol,
                ip.signed_in
            ),
            ("192.0.2.14", "http", "http", false)
        );
        let named = &sources.gitlab[1];
        assert_eq!(
            (
                named.host.as_str(),
                named.api_protocol,
                named.git_protocol,
                named.signed_in
            ),
            ("gitlab.acme.test", "http", "ssh", true)
        );
        assert_eq!(
            named
                .repos
                .iter()
                .map(|one| one.slug.as_str())
                .collect::<Vec<_>>(),
            ["common/registry", "mobile/ios/app"]
        );
        assert_eq!(
            sources.other,
            vec![OtherHost {
                host: "bitbucket.org".into(),
                count: 2
            }]
        );
    }

    /// glab names a self-managed instance with its web port; the ssh remote of
    /// the same machine has none. They are one host, signed in, at the port.
    #[test]
    fn a_ported_instance_is_one_row_signed_in_under_either_spelling() {
        let repos = vec![
            repo(
                "/projects/work/ios",
                "ssh://git@gitlab.acme.test:2222/mobile/ios/app.git",
            ),
            repo(
                "/projects/work/web",
                "https://gitlab.acme.test:8443/mobile/web.git",
            ),
        ];
        let sources = group(
            repos,
            &[],
            &[],
            &["gitlab.acme.test:8443".into()],
            &HashMap::new(),
        );
        assert_eq!(sources.gitlab.len(), 1);
        let row = &sources.gitlab[0];
        assert_eq!(
            (row.address.as_str(), row.signed_in, row.repos.len()),
            ("gitlab.acme.test:8443", true, 2)
        );
    }

    #[test]
    fn an_unplaced_host_that_did_not_answer_like_gitlab_is_not_offered_a_gitlab_sign_in() {
        let sources = group(
            vec![repo("/projects/work/x", "https://git.acme.test/x/y.git")],
            &[],
            &[],
            &[],
            &HashMap::new(),
        );
        assert!(sources.gitlab.is_empty());
        assert_eq!(
            sources.other,
            vec![OtherHost {
                host: "git.acme.test".into(),
                count: 1
            }]
        );
    }

    #[test]
    fn only_gitlab_s_own_json_reads_as_gitlab() {
        assert!(reads_like_gitlab(401, "{\"message\":\"401 Unauthorized\"}"));
        assert!(reads_like_gitlab(
            200,
            "{\"version\":\"17.9.1\",\"revision\":\"abc\"}"
        ));
        assert!(!reads_like_gitlab(404, "{\"message\":\"404 Not Found\"}"));
        assert!(!reads_like_gitlab(401, "<html>Sign in</html>"));
        assert!(!reads_like_gitlab(302, ""));
    }
}
