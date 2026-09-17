//! 원격 저장소 하나를 받아 오는 일에서 **git을 부르지 않고** 답할 수 있는 부분.
//!
//! 클론은 네트워크지만, 클론을 둘러싼 판단 넷은 그렇지 않다: 어떤 이름의
//! 디렉터리가 생기는가, 진행률 한 줄이 무슨 뜻인가, 실패했을 때 사람에게
//! 보여 줄 문장은 어느 줄인가, 그리고 그 문장에 비밀번호가 섞여 있지는
//! 않은가. 넷 다 문자열 하나가 들어오고 값 하나가 나가는 순수 함수이고,
//! 그래서 네트워크 없이 표 하나로 시험된다 — [`workitem`](crate::workitem)이
//! 스마트 입력에 대해 하는 것과 같은 자리다.
//!
//! 계약은 셋이다:
//!
//!   1. **이름은 URL이 정하고, 이 함수만 정한다.** 창이 미리 보여 주는 이름과
//!      실제로 만들어지는 디렉터리 이름이 두 규칙에서 나오면 언젠가 갈라지고,
//!      갈라진 날의 증상은 "미리 보기와 다른 폴더가 생겼다"이다.
//!   2. **자격 증명은 화면에 닿기 전에 지운다.** `https://user:token@host/…`를
//!      그대로 실은 오류 한 줄은 스크린샷에도, 버그 리포트에도, 로그에도
//!      남는다. 지우는 자리는 git이 말한 문장을 **읽는 곳** 하나다.
//!   3. **모르면 조용하다.** 진행률로 안 읽히는 줄은 진행률이 아니고
//!      ([`read_progress`]가 [`None`]), `fatal:`/`error:` 줄이 없는 표준
//!      오류는 보여 줄 문장이 없는 것이다([`clone_failure`]가 [`None`]).
//!      추측한 문장보다 없는 문장이 낫다.

use serde::Serialize;

/// 디렉터리 이름으로 쓸 수 없는 낱말. 셋 다 "이름"이 아니라 **경로 문법**이라,
/// 통과시키면 부모 밖으로 걸어 나가는 대상 경로가 만들어진다.
const NOT_A_NAME: [&str; 3] = ["", ".", ".."];

/// `git clone --progress`가 표준 오류에 적는 한 순간.
///
/// Orca의 `emitCloneProgressFromText`가 방송하는 것과 같은 두 값이다 — 단계
/// 이름과 퍼센트. 바이트 수·속도는 싣지 않는다: 진행 막대 하나가 읽는 것은
/// 그 둘뿐이고, 나머지는 그리지 않을 값을 경계 너머로 나르는 일이다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CloneStep {
    /// `Receiving objects`처럼 git이 부르는 단계 이름, 접두 없이.
    pub phase: String,
    /// 0..=100.
    pub percent: u8,
}

/// 이 URL을 클론하면 생기는 디렉터리 이름. 이름을 못 읽으면 [`None`].
///
/// Orca `deriveCloneRepoNameFromUrl`과 같은 자리이고, 같은 네 가지를 벗긴다:
/// 질의·조각, 끝의 `/`, `.git` 접미, 그리고 scp 형식(`git@host:owner/repo.git`)의
/// 호스트 부분.
///
/// **경로 문법은 이름이 아니다.** `.`·`..`·구분자를 품은 결과는 [`None`]으로
/// 돌아간다 — 그것이 "부모 폴더 밖으로 클론되는 경로"를 만들 수 있는 유일한
/// 입력이고, 거절은 여기서 한 번만 하면 된다. 앞이 `-`인 이름도 마찬가지로
/// 거절한다: git의 인자 자리에서 그것은 이름이 아니라 플래그로 읽힌다.
#[must_use]
pub fn clone_dir_name(url: &str) -> Option<String> {
    let url = url.trim();
    // 질의와 조각은 경로가 아니다. 붙어 있으면 마지막 마디를 오염시킨다.
    let url = url.split(['?', '#']).next().unwrap_or(url);
    // 끝의 `/`는 마디가 아니라 마침표다. 붙어 있으면 마지막 마디가 빈 문자열이
    // 되고, 빈 이름은 부모 디렉터리 그 자체를 가리킨다.
    let url = url.trim_end_matches('/');
    // 스킴은 이름의 재료가 아니다. 떼어 두는 이유는 아래 한 줄 때문이다 —
    // 스킴을 남겨 두면 `https://github.com`에도 마디가 둘로 보인다.
    let rest = url.split_once("://").map_or(url, |(_, after)| after);
    // `:`도 가르는 이유가 scp 형식이다 — `git@github.com:owner/repo.git`에는
    // `/`가 하나뿐이라 `/`만으로는 호스트가 이름에 남는다.
    let tail = rest.rsplit(['/', ':']).next()?;
    // 마디가 하나뿐이면 그것은 호스트이지 저장소가 아니다. `https://github.com/`은
    // 주소이지 클론할 것이 아니고, 이 한 줄이 없으면 `github.com`이라는 폴더가
    // 생긴다.
    if tail == rest {
        return None;
    }
    let name = tail.strip_suffix(".git").unwrap_or(tail);
    is_dir_name(name).then(|| name.to_string())
}

/// 이 낱말은 **부모 안의 한 마디**인가.
///
/// 파생한 이름도 사람이 고쳐 적은 이름도 이 문을 지난다. 문이 하나인 것이
/// 요점이다 — 미리 보기는 통과시키고 실제 만들기는 거절하는(또는 그 반대의)
/// 두 규칙이 이 기능의 유일한 위험한 실패이고, 그것은 부모 폴더 밖에 디렉터리를
/// 만드는 것으로 나타난다.
#[must_use]
pub fn is_dir_name(name: &str) -> bool {
    !NOT_A_NAME.contains(&name)
        && !name.starts_with('-')
        // `:`까지 막는 이유는 윈도우의 드라이브 문자다. 유닉스에서는 쓸 수 있는
        // 글자지만, 여기서 통과시킬 이유가 없다.
        && !name.contains(['/', '\\', '\0', ':'])
}

/// 한 줄에서 자격 증명을 지운다.
///
/// `scheme://user:token@host/…`의 **`@` 앞부분만** 별표로 바꾼다. 호스트도
/// 경로도 남는데, 그 둘은 사람이 무엇을 클론하려 했는지 읽는 데 필요하고
/// 비밀이 아니다. scp 형식(`git@host:…`)에는 `://`가 없으므로 손대지 않는다 —
/// 거기 있는 `git`은 비밀번호가 아니라 사용자 이름이다.
#[must_use]
pub fn scrub_credentials(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find("://") {
        let (head, tail) = rest.split_at(at + 3);
        out.push_str(head);
        // 권한부(authority)는 다음 구분자까지다. `@`를 그 밖에서 찾으면 경로나
        // 뒤따르는 문장 속의 `@`를 자격 증명으로 오해한다.
        let end = tail
            .find(|ch: char| ch == '/' || ch == '?' || ch == '#' || ch.is_whitespace())
            .unwrap_or(tail.len());
        let (authority, after) = tail.split_at(end);
        match authority.rfind('@') {
            Some(mark) => {
                out.push_str("***");
                out.push_str(&authority[mark..]);
            }
            None => out.push_str(authority),
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// git이 표준 오류에 남긴 것 중 **사람에게 보여 줄 마지막 한 줄**.
///
/// 끝에서부터 `fatal:`/`error:`로 시작하는 줄을 찾는다(Orca
/// `getGitCloneFailureMessage`와 같은 규칙). 끝에서부터인 이유는 git이 원인을
/// 마지막에 적기 때문이다 — 앞쪽 `error:`들은 대개 그 원인의 증상이다.
///
/// 돌아오는 문장은 이미 [`scrub_credentials`]를 지났다. 지우는 자리를 읽는
/// 곳 하나로 둔 것이 이 함수의 값이다.
#[must_use]
pub fn clone_failure(stderr: &str) -> Option<String> {
    stderr
        // `--progress`는 `\r`로 한 줄을 덮어 쓰므로 그것도 줄바꿈으로 센다.
        .split(['\n', '\r'])
        .map(str::trim)
        .rev()
        .find(|line| line.starts_with("fatal:") || line.starts_with("error:"))
        .map(scrub_credentials)
}

/// `--progress`가 방금 뱉은 덩어리에서 읽어 낸 한 순간. 진행률이 아니면 [`None`].
///
/// 덩어리로 받는 것이 요점이다. git은 한 줄을 `\r`로 몇십 번이고 덮어 쓰므로
/// 파이프에서 읽힌 한 덩어리에는 같은 단계의 순간이 여러 개 들어 있다 — 지금
/// 그릴 것은 그중 **마지막**이고, 앞의 것들은 이미 지나간 화면이다.
#[must_use]
pub fn read_progress(chunk: &str) -> Option<CloneStep> {
    let last = chunk
        .split(['\r', '\n'])
        .rev()
        .find(|part| part.contains('%'))?;
    let head = &last[..last.find('%')?];
    // 퍼센트 바로 앞의 숫자 뭉치. 바이트 인덱스를 문자 경계에서 얻는 이유는
    // 단계 이름이 ASCII라는 보장이 없기 때문이다.
    let digits_at = head
        .char_indices()
        .rev()
        .take_while(|(_, ch)| ch.is_ascii_digit())
        .last()
        .map(|(at, _)| at)?;
    let percent: u8 = head[digits_at..].parse().ok()?;
    if percent > 100 {
        return None;
    }
    // `remote: Counting objects:  45%`처럼 접두가 붙는다. 마지막 `:` 뒤가
    // 단계의 이름이다.
    let phase = head[..digits_at]
        .trim_end()
        .trim_end_matches(':')
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .trim();
    if phase.is_empty() {
        return None;
    }
    Some(CloneStep {
        phase: phase.to_string(),
        percent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// URL 하나가 디렉터리 이름 하나를 정한다 — 그리고 정할 수 없으면 말한다.
    #[test]
    fn a_url_names_the_directory_it_would_create() {
        for (url, named) in [
            ("https://github.com/owner/repo.git", Some("repo")),
            ("https://github.com/owner/repo", Some("repo")),
            ("https://github.com/owner/repo/", Some("repo")),
            ("https://github.com/owner/repo.git/", Some("repo")),
            ("  https://github.com/owner/repo.git  ", Some("repo")),
            ("https://github.com/owner/repo?tab=readme", Some("repo")),
            ("https://github.com/owner/repo#anchor", Some("repo")),
            // scp 형식 — `/`가 하나뿐이라 `:`도 가르지 않으면 호스트가 남는다.
            ("git@github.com:owner/repo.git", Some("repo")),
            ("git@github.com:repo.git", Some("repo")),
            ("ssh://git@github.com/owner/repo.git", Some("repo")),
            ("file:///srv/git/repo.git", Some("repo")),
            // 이름이 아니라 경로 문법인 것들.
            ("https://github.com/owner/..", None),
            ("https://github.com/owner/.", None),
            ("", None),
            ("   ", None),
            ("https://github.com/", None),
            // 호스트뿐인 주소는 클론할 것이 아니다 — 이 줄이 없으면
            // `github.com`이라는 폴더가 생긴다.
            ("https://github.com", None),
            ("repo.git", None),
            // git의 인자 자리에서 플래그로 읽히는 이름.
            ("https://github.com/owner/-rf", None),
        ] {
            assert_eq!(
                clone_dir_name(url).as_deref(),
                named,
                "`{url}` derived the wrong directory name"
            );
        }
    }

    /// 파생한 이름은 언제나 **한 마디**다 — 부모 밖으로 걸어 나가지 못한다.
    #[test]
    fn a_derived_name_is_one_segment_and_cannot_walk_out_of_its_parent() {
        for url in [
            "https://github.com/owner/repo.git",
            "git@github.com:group/sub/repo.git",
            "https://example.com/a/b/c/d/deep.git",
        ] {
            let named = clone_dir_name(url).expect("named");
            assert!(
                is_dir_name(&named),
                "`{url}` derived `{named}`, which is not a single directory name"
            );
        }
    }

    /// 미리 보기와 실제 만들기가 같은 문을 지난다.
    ///
    /// 이름을 판정하는 규칙이 둘이 되면 한쪽만 통과시키는 이름이 생기고, 그
    /// 이름은 부모 폴더 밖에 디렉터리를 만든다.
    #[test]
    fn the_one_gate_a_name_passes_is_the_same_gate_the_derived_name_passed() {
        for good in ["repo", "my.repo", "a_b-c", ".dotted", "한글이름"] {
            assert!(
                is_dir_name(good),
                "`{good}` was refused as a directory name"
            );
        }
        for bad in ["", ".", "..", "-rf", "a/b", "a\\b", "C:", "a\0b"] {
            assert!(
                !is_dir_name(bad),
                "`{bad}` passed as a directory name, so it can leave its parent"
            );
        }
        // 그리고 파생한 이름은 예외 없이 그 문을 지난 것이다.
        assert_eq!(
            clone_dir_name("https://example.com/owner/-rf"),
            None,
            "a derived name skipped the gate the typed one has to pass"
        );
    }

    /// 비밀번호는 화면에 닿지 않는다 — 호스트와 경로는 닿는다.
    #[test]
    fn a_credential_never_reaches_the_sentence_a_person_reads() {
        assert_eq!(
            scrub_credentials("fatal: could not read https://joe:ghp_secret@github.com/o/r.git"),
            "fatal: could not read https://***@github.com/o/r.git"
        );
        // 사용자 이름만 있어도 지운다. 사람 이름도 남의 화면에 실릴 것은 아니다.
        assert_eq!(
            scrub_credentials("https://joe@example.com/x"),
            "https://***@example.com/x"
        );
        // 자격 증명이 없으면 한 글자도 바뀌지 않는다.
        assert_eq!(
            scrub_credentials("fatal: repository 'https://github.com/o/r.git' not found"),
            "fatal: repository 'https://github.com/o/r.git' not found"
        );
        // scp 형식에는 `://`가 없다 — 거기 `git@`은 사용자 이름이다.
        assert_eq!(
            scrub_credentials("fatal: git@github.com:o/r.git: no such repo"),
            "fatal: git@github.com:o/r.git: no such repo"
        );
        // 한 줄에 둘이면 둘 다.
        assert_eq!(
            scrub_credentials("https://a:b@one.example/x and https://c:d@two.example/y"),
            "https://***@one.example/x and https://***@two.example/y"
        );
        // 경로 속의 `@`는 자격 증명이 아니다.
        assert_eq!(
            scrub_credentials("https://example.com/@scope/pkg"),
            "https://example.com/@scope/pkg"
        );
    }

    /// 실패는 git이 **마지막에** 말한 것으로 보고된다 — 그리고 씻겨서 나온다.
    #[test]
    fn the_failure_a_person_sees_is_gits_last_word_with_the_secret_taken_out() {
        let said = "Cloning into 'repo'...\n\
                    error: RPC failed\n\
                    fatal: could not read Username for 'https://joe:tok@github.com'\n";
        assert_eq!(
            clone_failure(said).as_deref(),
            Some("fatal: could not read Username for 'https://***@github.com'")
        );
        // `fatal:`이 없으면 `error:`가 답이다.
        assert_eq!(
            clone_failure("error: unable to write\n").as_deref(),
            Some("error: unable to write")
        );
        // 보여 줄 줄이 없으면 없다고 말한다 — 지어내지 않는다.
        assert_eq!(clone_failure("Cloning into 'repo'...\ndone.\n"), None);
        assert_eq!(clone_failure(""), None);
    }

    /// 진행률 한 줄은 단계와 퍼센트다 — 그리고 아닌 줄은 진행률이 아니다.
    #[test]
    fn progress_is_a_phase_and_a_percent_and_nothing_else_is() {
        let step = read_progress("Receiving objects:  42% (94/222), 1.00 MiB\r").expect("read");
        assert_eq!(step.phase, "Receiving objects");
        assert_eq!(step.percent, 42);
        // git이 붙이는 `remote:` 접두는 단계의 이름이 아니다.
        let remote = read_progress("remote: Counting objects:  45% (100/222)\r").expect("read");
        assert_eq!(remote.phase, "Counting objects");
        assert_eq!(remote.percent, 45);
        // 한 덩어리에 여러 순간이 들어 있으면 마지막이 지금이다.
        let latest = read_progress(
            "Receiving objects:  10% (1/10)\rReceiving objects:  90% (9/10)\rReceiving objects: 100% (10/10)\r",
        )
        .expect("read");
        assert_eq!(latest.percent, 100);
        // 진행률이 아닌 줄.
        assert_eq!(read_progress("Cloning into 'repo'...\n"), None);
        assert_eq!(read_progress(""), None);
        // 퍼센트 앞에 숫자가 없으면 진행률이 아니다.
        assert_eq!(read_progress("percent: %\n"), None);
        // 단계 이름이 없는 퍼센트도 아니다.
        assert_eq!(read_progress("  42%\n"), None);
        // 100을 넘는 값은 git이 적은 것이 아니다.
        assert_eq!(read_progress("Receiving objects: 420%\n"), None);
    }
}
