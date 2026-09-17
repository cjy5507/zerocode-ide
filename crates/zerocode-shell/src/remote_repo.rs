//! Where a checkout's remote lives on the web, and the page that opens a
//! review there.
//!
//! A port of the original's `remote-repo.ts` and `manual-review-url.ts`. It
//! exists for the checkouts this window cannot open a review FOR: `gh` speaks
//! to GitHub, and a GitLab or Bitbucket or Azure DevOps remote would otherwise
//! get a create door that simply is not there. The provider's own "new merge
//! request" page is a road, and one link is the whole of it.
//!
//! Three halves of the original have no producer here and are absent rather
//! than guessed. A PROVIDER HINT: the original carries one from a linked
//! review, and it is the only thing that can name a forge whose host cannot
//! be recognised by its spelling. A PUSH TARGET: a fork checkout whose head
//! lives in another repository, which is what the `owner:branch` compare
//! qualifier is for — here the base and the head are one repository. And
//! GITEA, which is self-hosted and therefore reachable ONLY through that
//! hint; a variant nothing can construct is a URL shape no test could drive,
//! so it waits for the hint rather than sitting here unreachable.

use serde::Serialize;

/// The forges whose review pages this module can name.
///
/// The original's `ManualReviewProvider` — its `HostedReviewProvider` minus
/// `unsupported`, which is the absence of a provider rather than one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provider {
    Github,
    Gitlab,
    Bitbucket,
    AzureDevops,
}

/// A remote resolved to the repository it names on the web.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRepo {
    /// `None` when the host is not one of the five. That is NOT the same as
    /// "no review here": a GitHub Enterprise install is an ordinary hostname
    /// nobody can recognise from its spelling alone.
    pub provider: Option<Provider>,
    /// `owner/repository`, decoded — the identity two remotes are compared by.
    pub path: String,
    /// The repository's page, without a trailing slash.
    pub web_base_url: String,
}

/// `origin/fix-login` → `("origin", "fix-login")`.
///
/// A name with no slash, or one at either end, is not an upstream: it is a
/// remote with no branch or a branch with no remote, and either way there is
/// nothing to point at.
#[must_use]
pub fn parse_upstream(name: &str) -> Option<(&str, &str)> {
    let trimmed = name.trim();
    let slash = trimmed.find('/')?;
    if slash == 0 || slash == trimmed.len() - 1 {
        return None;
    }
    Some((&trimmed[..slash], &trimmed[slash + 1..]))
}

/// The branch a ref names, with the remote's own prefixes taken off first.
///
/// The remote's spellings come first on purpose (`refs/remotes/origin/x`,
/// `remotes/origin/x`, `origin/x`) so that a compare base stored as
/// `origin/main` is understood as `main` — the branch a review targets is
/// named without its remote.
#[must_use]
pub fn branch_from_ref(reference: &str, remote_name: Option<&str>) -> Option<String> {
    let trimmed = reference.trim();
    if trimmed.is_empty() {
        return None;
    }
    let owned;
    let prefixes: &[&str] = match remote_name {
        Some(remote) => {
            owned = [
                format!("refs/remotes/{remote}/"),
                format!("remotes/{remote}/"),
                format!("{remote}/"),
            ];
            &[&owned[0], &owned[1], &owned[2]]
        }
        None => &["refs/heads/"],
    };
    for prefix in prefixes {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return (!rest.is_empty()).then(|| rest.to_string());
        }
    }
    for prefix in ["refs/remotes/", "remotes/"] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let slash = rest.find('/')?;
            return (slash > 0).then(|| rest[slash + 1..].to_string());
        }
    }
    if let Some(rest) = trimmed.strip_prefix("refs/heads/") {
        return (!rest.is_empty()).then(|| rest.to_string());
    }
    Some(trimmed.to_string())
}

/// `encodeURIComponent`'s unreserved set, which is what the original encodes
/// path segments and refs with.
///
/// Shared rather than copied: the GitLab reader needs the same set for the
/// search text it puts in a query string, and two spellings of
/// `encodeURIComponent` are two chances to disagree about which byte is safe.
pub(crate) fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

/// Every segment encoded, the separators left alone.
fn encode_path(path: &str) -> String {
    path.split('/')
        .map(encode_component)
        .collect::<Vec<_>>()
        .join("/")
}

/// Compare refs keep `/` and `:` literal.
///
/// The original's reason, verbatim: "GitHub/Gitea compare refs keep '/'
/// (slashed branch names like feature/foo) and ':' (the owner:branch fork
/// qualifier) literal; percent-encoding those separators makes GitHub fail to
/// resolve the branch. Only the segments between them are encoded."
fn encode_compare_ref(reference: &str) -> String {
    reference
        .split(':')
        .map(encode_path)
        .collect::<Vec<_>>()
        .join(":")
}

/// One `%XX` triple, or the text as it stands when it is not one.
fn decode_segment(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' && at + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[at + 1..at + 3]).ok();
            match hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                Some(byte) => {
                    out.push(byte);
                    at += 3;
                    continue;
                }
                None => return value.to_string(),
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    // A decode that is not text is not a decode — `decodeURIComponent` throws
    // there and the original keeps the raw segment.
    String::from_utf8(out).unwrap_or_else(|_| value.to_string())
}

/// `/owner/repo.git/` → `owner/repo`, and nothing shorter than two segments.
///
/// Fewer than two is not a repository path: it is a host with a name after it,
/// and no forge page can be built from one.
fn clean_path(path: &str) -> Option<String> {
    let trimmed = path.trim_start_matches('/').trim_end_matches('/');
    let trimmed = match trimmed.len().checked_sub(4) {
        Some(at) if trimmed[at..].eq_ignore_ascii_case(".git") => &trimmed[..at],
        _ => trimmed,
    };
    let parts: Vec<String> = trimmed
        .split('/')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(decode_segment)
        .collect();
    (parts.len() >= 2).then(|| parts.join("/"))
}

fn provider_for_host(host: &str) -> Option<Provider> {
    let host = host.to_ascii_lowercase();
    match host.as_str() {
        "github.com" | "ssh.github.com" => Some(Provider::Github),
        "gitlab.com" => Some(Provider::Gitlab),
        "bitbucket.org" => Some(Provider::Bitbucket),
        "dev.azure.com" | "ssh.dev.azure.com" => Some(Provider::AzureDevops),
        _ => host
            .ends_with(".visualstudio.com")
            .then_some(Provider::AzureDevops),
    }
}

/// Host and port, without the credentials a remote often carries — the
/// original reads `URL.host`, which never holds userinfo, and a token spliced
/// into a remote must not travel back out as a web address.
fn host_with_port(url: &url::Url) -> String {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    }
}

/// An http(s) remote keeps its own origin — port included, since a
/// self-hosted forge is often not on 443. Anything else (ssh, git) is a
/// transport, not a web address, and its web origin is https on the bare host.
fn web_origin(scheme: &str, host_with_port: &str, hostname: &str) -> String {
    if scheme == "http" || scheme == "https" {
        format!("{scheme}://{host_with_port}")
    } else {
        format!("https://{hostname}")
    }
}

/// `git@host:owner/repo.git` — the scp-like form, which is not a URL.
///
/// The original's expression, read the same way: an optional `user@`, a host
/// with no colon or slash in it, a colon, and the rest as the path.
fn scp_like(trimmed: &str) -> Option<(String, &str)> {
    let (authority, path) = trimmed.split_once(':')?;
    if path.is_empty() || path.contains(char::is_whitespace) {
        return None;
    }
    let host = authority.rsplit_once('@').map_or(authority, |(_, at)| at);
    if host.is_empty() || host.contains('/') || host.contains(char::is_whitespace) {
        return None;
    }
    Some((host.to_ascii_lowercase(), path))
}

/// Azure DevOps names a repository in more shapes than anybody else, and each
/// one is a different arrangement of organization, project and repository.
fn parse_azure_devops(trimmed: &str) -> Option<RemoteRepo> {
    let azure_v3 = |host: &str, path: &str| -> Option<RemoteRepo> {
        if host != "ssh.dev.azure.com" {
            return None;
        }
        let cleaned = clean_path(path)?;
        let parts: Vec<&str> = cleaned.split('/').collect();
        if parts.len() < 4 || !parts[0].eq_ignore_ascii_case("v3") {
            return None;
        }
        let (organization, project, repository) = (parts[1], parts[2], parts[3]);
        Some(RemoteRepo {
            provider: Some(Provider::AzureDevops),
            path: format!("{organization}/{project}/_git/{repository}"),
            web_base_url: format!(
                "https://dev.azure.com/{}/{}/_git/{}",
                encode_path(organization),
                encode_component(project),
                encode_component(repository)
            ),
        })
    };

    if let Some((host, path)) = scp_like(trimmed)
        && let Some(found) = azure_v3(&host, path)
    {
        return Some(found);
    }

    let url = url::Url::parse(trimmed).ok()?;
    let scheme = url.scheme().to_ascii_lowercase();
    if !["http", "https", "ssh", "git+ssh"].contains(&scheme.as_str()) {
        return None;
    }
    let hostname = url.host_str()?.to_ascii_lowercase();
    if let Some(found) = azure_v3(&hostname, url.path()) {
        return Some(found);
    }
    let cleaned = clean_path(url.path())?;
    let parts: Vec<&str> = cleaned.split('/').collect();
    let git_at = parts
        .iter()
        .position(|part| part.eq_ignore_ascii_case("_git"))?;
    if git_at < 1 || git_at + 1 >= parts.len() {
        return None;
    }
    let project = parts[git_at - 1];
    let repository = parts[git_at + 1];
    let prefix = &parts[..git_at - 1];
    // Only `dev.azure.com` puts the organization first in the path; on a
    // `*.visualstudio.com` install the host IS the organization, and an
    // unrecognised host keeps whatever prefix it came with.
    let organization = (hostname == "dev.azure.com")
        .then(|| prefix.first().copied())
        .flatten();
    let web_path: Vec<&str> = match organization {
        Some(organization) if !organization.is_empty() => {
            vec![organization, project, "_git", repository]
        }
        _ => prefix
            .iter()
            .copied()
            .chain([project, "_git", repository])
            .collect(),
    };
    let joined = web_path.join("/");
    let origin = web_origin(&scheme, &host_with_port(&url), &hostname);
    Some(RemoteRepo {
        provider: Some(Provider::AzureDevops),
        web_base_url: format!("{}/{}", origin.trim_end_matches('/'), encode_path(&joined)),
        path: joined,
    })
}

/// A git remote, resolved to the repository page it stands for.
///
/// A local path is not a remote anybody can open: a Windows drive letter and a
/// leading slash are both refused before anything else, the way the original
/// refuses them.
#[must_use]
pub fn parse_remote_repo(remote_url: &str) -> Option<RemoteRepo> {
    let trimmed = remote_url
        .trim()
        .strip_prefix("git+")
        .unwrap_or(remote_url.trim());
    let drive_letter = trimmed
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
        && trimmed.len() >= 3
        && trimmed.as_bytes()[1] == b':'
        && matches!(trimmed.as_bytes()[2], b'\\' | b'/');
    if trimmed.is_empty() || drive_letter || trimmed.starts_with('/') {
        return None;
    }

    if let Some(azure) = parse_azure_devops(trimmed) {
        return Some(azure);
    }

    if !trimmed.contains("://")
        && let Some((host, path)) = scp_like(trimmed)
    {
        let path = clean_path(path)?;
        return Some(RemoteRepo {
            provider: provider_for_host(&host),
            web_base_url: format!("https://{host}/{}", encode_path(&path)),
            path,
        });
    }

    let url = url::Url::parse(trimmed).ok()?;
    let scheme = url.scheme().to_ascii_lowercase();
    if !["git", "http", "https", "ssh"].contains(&scheme.as_str()) {
        return None;
    }
    let path = clean_path(url.path())?;
    let hostname = url.host_str()?.to_ascii_lowercase();
    // `ssh.github.com` is a transport alias for port 443; its web address is
    // the ordinary one.
    let origin = if hostname == "ssh.github.com" {
        "https://github.com".to_string()
    } else {
        web_origin(&scheme, &host_with_port(&url), &hostname)
    };
    Some(RemoteRepo {
        provider: provider_for_host(&hostname),
        web_base_url: format!("{}/{}", origin.trim_end_matches('/'), encode_path(&path)),
        path,
    })
}

/// What the manual review link is built out of.
pub struct ManualReview<'a> {
    /// The compare base, in whatever spelling it was stored.
    pub base_ref: Option<&'a str>,
    pub branch_name: Option<&'a str>,
    /// The remote this repository calls its own (`origin`, usually).
    pub repo_remote_name: Option<&'a str>,
    pub repo_remote_url: Option<&'a str>,
    /// The tracking upstream, `<remote>/<branch>`.
    pub upstream_name: Option<&'a str>,
}

/// The provider's own "open a review" page for this branch, when one can be
/// named at all.
///
/// It answers `None` a great deal, and every one of those is deliberate — a
/// link that lands on "There isn't anything to compare" is worse than no link:
///
/// - no base, no branch, or a detached `HEAD`;
/// - no remote to resolve, or one that resolves to no known forge;
/// - **no upstream**, because without one there is no evidence the branch
///   exists on any remote at all;
/// - an upstream that tracks a DIFFERENT remote than the repository's own,
///   which is a fork this window cannot resolve to a URL — a base-repo
///   compare link would point at a branch that is not there.
#[must_use]
pub fn manual_review_url(input: &ManualReview<'_>) -> Option<String> {
    let base_branch = branch_from_ref(input.base_ref?, input.repo_remote_name)?;
    let local_branch = input.branch_name?.trim();
    if local_branch.is_empty() || local_branch == "HEAD" {
        return None;
    }
    let base_remote_url = input.repo_remote_url?.trim();
    if base_remote_url.is_empty() {
        return None;
    }
    let base_remote_name = input
        .repo_remote_name
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let upstream = input.upstream_name.and_then(parse_upstream);
    let (upstream_remote, upstream_branch) = upstream?;
    if base_remote_name.is_some_and(|base| upstream_remote != base) {
        return None;
    }
    let repo = parse_remote_repo(base_remote_url)?;
    let head_branch = upstream_branch;
    let base = encode_compare_ref(&base_branch);
    let head = encode_compare_ref(head_branch);
    let web = &repo.web_base_url;
    Some(match repo.provider? {
        Provider::Github => format!("{web}/compare/{base}...{head}?expand=1"),
        // The source branch lives in the head repository, which for us is
        // always this one; the original opens the New-MR page there for the
        // fork case, and this is that same expression with one repository.
        Provider::Gitlab => format!(
            "{web}/-/merge_requests/new?{}",
            form_query(&[
                ("merge_request[source_branch]", head_branch),
                ("merge_request[target_branch]", &base_branch),
            ])
        ),
        Provider::Bitbucket => format!(
            "{web}/pull-requests/new?{}",
            form_query(&[("source", head_branch), ("dest", &base_branch)])
        ),
        Provider::AzureDevops => format!(
            "{web}/pullrequestcreate?{}",
            form_query(&[
                ("sourceRef", &format!("refs/heads/{head_branch}")),
                ("targetRef", &format!("refs/heads/{base_branch}")),
            ])
        ),
    })
}

/// `URLSearchParams`' own encoding, which is where the original's query
/// strings come from.
fn form_query(values: &[(&str, &str)]) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in values {
        query.append_pair(key, value);
    }
    query.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_upstream_is_a_remote_and_a_branch_and_nothing_else() {
        assert_eq!(
            parse_upstream("origin/fix-login"),
            Some(("origin", "fix-login"))
        );
        assert_eq!(
            parse_upstream(" origin/feature/deep "),
            Some(("origin", "feature/deep"))
        );
        for lonely in ["", "origin", "/branch", "origin/", "   "] {
            assert_eq!(parse_upstream(lonely), None, "`{lonely}` named an upstream");
        }
    }

    #[test]
    fn a_ref_gives_up_its_remote_before_it_gives_up_its_branch() {
        assert_eq!(
            branch_from_ref("origin/main", Some("origin")).as_deref(),
            Some("main")
        );
        assert_eq!(
            branch_from_ref("refs/remotes/origin/release/1.0", Some("origin")).as_deref(),
            Some("release/1.0")
        );
        // A base stored bare is already the branch.
        assert_eq!(
            branch_from_ref("main", Some("origin")).as_deref(),
            Some("main")
        );
        // Another remote's ref still yields the branch after it.
        assert_eq!(
            branch_from_ref("refs/remotes/upstream/main", Some("origin")).as_deref(),
            Some("main")
        );
        assert_eq!(
            branch_from_ref("refs/heads/main", None).as_deref(),
            Some("main")
        );
        assert_eq!(branch_from_ref("   ", None), None);
    }

    #[test]
    fn a_remote_resolves_to_the_page_its_repository_has() {
        let github = parse_remote_repo("git@github.com:acme/tool.git").expect("scp github");
        assert_eq!(github.provider, Some(Provider::Github));
        assert_eq!(github.path, "acme/tool");
        assert_eq!(github.web_base_url, "https://github.com/acme/tool");

        // The ssh alias is a transport, not an address.
        assert_eq!(
            parse_remote_repo("ssh://git@ssh.github.com:443/acme/tool.git")
                .expect("ssh alias")
                .web_base_url,
            "https://github.com/acme/tool"
        );
        // A self-hosted forge keeps its own origin, port and all.
        let hosted = parse_remote_repo("http://git.example:8080/team/tool.git").expect("hosted");
        assert_eq!(
            hosted.provider, None,
            "an unknown host is not a known forge"
        );
        assert_eq!(hosted.web_base_url, "http://git.example:8080/team/tool");
        // An ssh remote's web origin is https on the bare host — the port it
        // was reached on is not a web port.
        assert_eq!(
            parse_remote_repo("ssh://git@gitlab.com:2222/group/sub/tool.git")
                .expect("gitlab ssh")
                .web_base_url,
            "https://gitlab.com/group/sub/tool"
        );
        assert_eq!(
            parse_remote_repo("https://bitbucket.org/team/tool").map(|one| one.provider),
            Some(Some(Provider::Bitbucket))
        );

        // Local paths are not remotes anybody can open.
        for local in ["/srv/git/tool.git", "C:\\work\\tool", "", "   "] {
            assert!(
                parse_remote_repo(local).is_none(),
                "`{local}` resolved to a web page"
            );
        }
        // One segment is a host with a name after it, not a repository.
        assert!(parse_remote_repo("https://github.com/acme").is_none());
    }

    #[test]
    fn azure_devops_is_read_in_every_shape_it_is_written_in() {
        let expected = "https://dev.azure.com/acme/tools/_git/cli";
        for remote in [
            "git@ssh.dev.azure.com:v3/acme/tools/cli",
            "ssh://git@ssh.dev.azure.com/v3/acme/tools/cli",
            "https://dev.azure.com/acme/tools/_git/cli",
            "https://acme@dev.azure.com/acme/tools/_git/cli",
        ] {
            let found = parse_remote_repo(remote).unwrap_or_else(|| panic!("{remote}"));
            assert_eq!(found.provider, Some(Provider::AzureDevops), "{remote}");
            assert_eq!(found.path, "acme/tools/_git/cli", "{remote}");
            assert_eq!(found.web_base_url, expected, "{remote}");
        }
        // On a `*.visualstudio.com` install the host IS the organization, so
        // the path keeps whatever prefix it came with.
        let old = parse_remote_repo("https://acme.visualstudio.com/tools/_git/cli").expect("vsts");
        assert_eq!(old.path, "tools/_git/cli");
        assert_eq!(
            old.web_base_url,
            "https://acme.visualstudio.com/tools/_git/cli"
        );
    }

    #[test]
    fn the_manual_link_is_the_providers_own_new_review_page() {
        let ask = |remote: &str, upstream: Option<&str>| {
            manual_review_url(&ManualReview {
                base_ref: Some("main"),
                branch_name: Some("fix-login"),
                repo_remote_name: Some("origin"),
                repo_remote_url: Some(remote),
                upstream_name: upstream,
            })
        };
        assert_eq!(
            ask("git@github.com:acme/tool.git", Some("origin/fix-login")).as_deref(),
            Some("https://github.com/acme/tool/compare/main...fix-login?expand=1")
        );
        assert_eq!(
            ask("https://gitlab.com/acme/tool.git", Some("origin/fix-login")).as_deref(),
            Some(
                "https://gitlab.com/acme/tool/-/merge_requests/new?\
                 merge_request%5Bsource_branch%5D=fix-login&\
                 merge_request%5Btarget_branch%5D=main"
            )
        );
        assert_eq!(
            ask("https://bitbucket.org/acme/tool", Some("origin/fix-login")).as_deref(),
            Some("https://bitbucket.org/acme/tool/pull-requests/new?source=fix-login&dest=main")
        );
        assert_eq!(
            ask(
                "https://dev.azure.com/acme/tools/_git/cli",
                Some("origin/fix-login")
            )
            .as_deref(),
            Some(
                "https://dev.azure.com/acme/tools/_git/cli/pullrequestcreate?\
                 sourceRef=refs%2Fheads%2Ffix-login&targetRef=refs%2Fheads%2Fmain"
            )
        );

        // The link names what is ON the remote, not what is checked out here.
        assert_eq!(
            manual_review_url(&ManualReview {
                base_ref: Some("refs/remotes/origin/main"),
                branch_name: Some("local-name"),
                repo_remote_name: Some("origin"),
                repo_remote_url: Some("git@github.com:acme/tool.git"),
                upstream_name: Some("origin/remote-name"),
            })
            .as_deref(),
            Some("https://github.com/acme/tool/compare/main...remote-name?expand=1")
        );
        // A slashed branch keeps its slashes; the segments between are encoded.
        assert_eq!(
            manual_review_url(&ManualReview {
                base_ref: Some("main"),
                branch_name: Some("x"),
                repo_remote_name: Some("origin"),
                repo_remote_url: Some("git@github.com:acme/tool.git"),
                upstream_name: Some("origin/feature/a b"),
            })
            .as_deref(),
            Some("https://github.com/acme/tool/compare/main...feature/a%20b?expand=1")
        );

        // Every refusal, and each one is a link that would land on nothing.
        assert!(
            ask("git@github.com:acme/tool.git", None).is_none(),
            "unpublished"
        );
        assert!(
            ask("git@github.com:acme/tool.git", Some("fork/fix-login")).is_none(),
            "an upstream on another remote is a fork this window cannot resolve"
        );
        assert!(
            ask("/srv/git/tool.git", Some("origin/fix-login")).is_none(),
            "local"
        );
        assert!(
            ask(
                "http://git.example:8080/team/tool.git",
                Some("origin/fix-login")
            )
            .is_none(),
            "an unknown forge has no review page anybody can name"
        );
        assert!(
            manual_review_url(&ManualReview {
                base_ref: Some("main"),
                branch_name: Some("HEAD"),
                repo_remote_name: Some("origin"),
                repo_remote_url: Some("git@github.com:acme/tool.git"),
                upstream_name: Some("origin/main"),
            })
            .is_none(),
            "a detached head has no branch to compare"
        );
    }
}
