//! Why a fetch failed, when the answer changes what happens next.
//!
//! A port of the original's `src/main/git/fetch-error-classification.ts`. It
//! exists for one caller — resolving a pull request's head before a workspace
//! is cut from it — and for the reason that file states in its own comment:
//! keeping what is already on disk after a failed fetch "is only safe when the
//! fetch plainly died in transport. A deleted PR head, auth failure, or
//! stale-relay method-not-found must surface, or the caller checks out a
//! dead/unauthorized tip."
//!
//! The list is an ALLOWLIST, not a blocklist, and that direction is the whole
//! safety of it: an unrecognised sentence is [`FetchRefusal::Fatal`], so a
//! failure mode nobody has seen yet refuses instead of quietly serving
//! yesterday's commit.

/// What a non-zero `git fetch` was.
///
/// Three, because the caller does three different things. The original's two
/// predicates (`isMissingRemoteRefGitError`, `isTransientReviewHeadFetchError`)
/// are asked in this order and the second begins by denying the first — one
/// enum says the same thing without letting a caller ask them out of order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchRefusal {
    /// The remote has no such ref. The branch is not where we looked — for a
    /// pull request that means the head lives in another repository.
    MissingRef,
    /// The transport died. The only branch on which what is already on disk is
    /// still the last thing this remote is known to have said.
    Transient,
    /// Everything else — an expired token, a repository we may not read, a
    /// protocol the other end refused. Not an answer.
    Fatal,
}

/// The remote answered, and what it said was "no such ref".
///
/// Both spellings: git changed the apostrophe form between versions and the
/// original matches both (`fetch-error-classification.ts:6-9`).
const MISSING_REF_MARKS: [&str; 2] = ["could not find remote ref", "couldn't find remote ref"];

/// Sentences that mean the transport died rather than the request being
/// answered — the original's `TRANSIENT_FETCH_ERROR_PATTERNS`
/// (`fetch-error-classification.ts:17-31`), which covers both raw git stderr
/// and the relay's normalized messages.
const TRANSIENT_MARKS: [&str; 13] = [
    "timed out",
    "timeout",
    "operation was aborted",
    "network error",
    "network is unreachable",
    "could not resolve host",
    "temporary failure in name resolution",
    "connection refused",
    "connection reset",
    "connection closed",
    "early eof",
    "remote end hung up",
    // A 5xx from a smart-http remote is the server failing, not us.
    "the requested url returned error: 5",
];

/// Read what git wrote to standard error.
///
/// Missing-ref is asked FIRST and wins outright: "could not find remote ref
/// …: connection reset" would otherwise read as transient and soft-keep a ref
/// for a branch that is gone. The original enforces the same precedence by
/// making its transient predicate return `false` for a missing ref before it
/// looks at anything else (`fetch-error-classification.ts:34-36`).
#[must_use]
pub fn classify(said: &str) -> FetchRefusal {
    let normalized = said.to_lowercase();
    if MISSING_REF_MARKS
        .iter()
        .any(|mark| normalized.contains(mark))
    {
        return FetchRefusal::MissingRef;
    }
    if TRANSIENT_MARKS.iter().any(|mark| normalized.contains(mark)) {
        return FetchRefusal::Transient;
    }
    FetchRefusal::Fatal
}

/// git's first sentence, for a message a person reads.
///
/// The original trims the same way everywhere it surfaces one of these
/// (`pr-start-point.ts:172`, `:224`) — a fetch failure is a paragraph of
/// remote helper noise whose first line is the part that says what happened.
#[must_use]
pub fn first_line(said: &str) -> &str {
    said.lines().next().unwrap_or("").trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_that_is_not_there_is_not_a_broken_connection() {
        for said in [
            "fatal: couldn't find remote ref refs/heads/feature/login",
            "fatal: Could not find remote ref refs/heads/main",
        ] {
            assert_eq!(classify(said), FetchRefusal::MissingRef);
        }
        // 그리고 두 낱말이 한 문장에 같이 있어도 없는 것이 이긴다 — 없는
        // 브랜치를 "전송이 죽었다"로 읽으면 지워진 PR head 자리에 어제의
        // 커밋을 내주게 된다.
        assert_eq!(
            classify("fatal: couldn't find remote ref x\nfatal: connection reset by peer"),
            FetchRefusal::MissingRef
        );
    }

    #[test]
    fn every_measured_transport_death_is_one() {
        for said in [
            "fatal: unable to access 'https://x/': Failed to connect: Connection refused",
            "ssh: connect to host github.com port 22: Operation timed out",
            "fatal: unable to access 'https://x/': Could not resolve host: github.com",
            "error: RPC failed; curl 92 HTTP/2 stream 0 was not closed cleanly\nfatal: early EOF",
            "fatal: The remote end hung up unexpectedly",
            "fatal: unable to access 'https://x/': The requested URL returned error: 502",
            "Network error. Check your connection.",
        ] {
            assert_eq!(classify(said), FetchRefusal::Transient, "{said}");
        }
    }

    #[test]
    fn a_sentence_nobody_listed_is_refused_rather_than_forgiven() {
        // 허용 목록의 방향이 이 축의 안전 전부다. 처음 보는 실패는 **거절**이고,
        // 그래서 만료된 토큰이 어제의 커밋을 잘라 주지 않는다.
        for said in [
            "remote: Invalid username or password.\nfatal: Authentication failed for 'https://x/'",
            "remote: Permission to a/b.git denied to c.",
            "fatal: repository 'https://x/' not found",
            "",
        ] {
            assert_eq!(classify(said), FetchRefusal::Fatal, "{said}");
        }
    }

    #[test]
    fn the_sentence_a_person_reads_is_the_first_one() {
        assert_eq!(
            first_line("fatal: Authentication failed\nremote: see docs"),
            "fatal: Authentication failed"
        );
        assert_eq!(first_line("  spaced  \nrest"), "spaced");
        assert_eq!(first_line(""), "");
    }
}
