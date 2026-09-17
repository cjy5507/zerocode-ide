//! 이벤트 채널의 공유 비밀 — `port-staging/zo-cli/src/serve_auth.rs` 방식.
//!
//! 두 가지만 옮겼다: **루프백 강제**와 **상수 시간 비교**.
//!
//! `zo serve` 는 루프백이 아닌 주소도 토큰만 있으면 열어 줬지만, 패인 채널은
//! 그 문을 아예 닫는다. 이 채널은 한 사람의 워크스테이션에서 창과 패인 사이만
//! 잇는 것이고, 저쪽에서 오는 것은 툴을 돌릴 수 있는 승인이다 —
//! `zerocode-lane::validate_loopback_bind` 가 세션 서버에 대해 내리는 것과
//! 같은 판정을 여기서도 내린다.
//!
//! 토큰은 하네스와 **같은 변수**([`TOKEN_ENV`])에서 읽는다. 하네스
//! `Client::connect` 가 토큰을 안 받으면 이 변수를 스스로 읽으므로
//! (`zerocode-harness/src/lib.rs`), 창과 패인이 같은 값을 물려받는 한
//! 양쪽 다 배선이 필요 없다.

use std::net::SocketAddr;

/// 공유 비밀이 실린 환경변수. 하네스 `TOKEN_ENV` 와 같은 철자여야 한다 —
/// 다르게 쓰면 하네스가 토큰 없이 붙어 `-32002` 를 맞는다.
pub const TOKEN_ENV: &str = "ZO_SERVE_TOKEN";

/// 채널이 요구하는 토큰(없으면 무인증).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenPolicy {
    expected: Option<String>,
}

impl TokenPolicy {
    /// 값이 있고 공백만이 아닐 때에만 토큰으로 친다 — 공백뿐인 값이
    /// 우연히 맞아 떨어지는 비밀이 되면 안 된다. 원문은 다듬지 않는다:
    /// 창과 패인이 같은 변수를 읽고 바이트로 맞춰야 하기 때문이다.
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(match std::env::var(TOKEN_ENV) {
            Ok(value) if !value.trim().is_empty() => Some(value),
            _ => None,
        })
    }

    /// 토큰을 직접 준다(테스트·명시 배선용).
    #[must_use]
    pub fn new(expected: Option<String>) -> Self {
        Self { expected }
    }

    /// 정책이 쥔 토큰을 꺼낸다 — 채널 설정으로 옮길 때.
    #[must_use]
    pub fn into_token(self) -> Option<String> {
        self.expected
    }

    /// 토큰이 걸려 있는가.
    #[must_use]
    pub fn is_guarded(&self) -> bool {
        self.expected.is_some()
    }

    /// 요청 하나를 판정한다. 무인증 채널은 전부 통과.
    #[must_use]
    pub fn authorize(&self, provided: Option<&str>) -> bool {
        let Some(expected) = self.expected.as_deref() else {
            return true;
        };
        provided.is_some_and(|candidate| constant_time_eq(expected.as_bytes(), candidate.as_bytes()))
    }
}

/// 바인드 주소가 루프백인지. 아니면 채널을 열지 않는다.
///
/// # Errors
///
/// 주소를 파싱할 수 없거나 루프백이 아니면 사람이 읽는 사유를 돌려준다.
pub fn resolve_loopback_bind(bind: &str) -> Result<SocketAddr, String> {
    let addr: SocketAddr = bind
        .parse()
        .map_err(|error| format!("--events-bind {bind}: 주소를 읽을 수 없습니다 ({error}). 127.0.0.1:PORT 형식이어야 합니다."))?;
    if !addr.ip().is_loopback() {
        return Err(format!(
            "--events-bind {bind}: 이벤트 채널은 루프백에만 붙습니다. 이 소켓으로 오는 것은 \
             도구 실행을 여는 승인이라, 다른 기계가 닿을 수 있는 주소에는 열지 않습니다."
        ));
    }
    Ok(addr)
}

/// 어디서 첫 글자가 갈리는지와 무관한 시간으로 비교한다 — 길이는 새어도
/// 되지만(공유 비밀 관례) 내용은 아니다.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unguarded_channel_lets_everything_through() {
        let policy = TokenPolicy::new(None);
        assert!(!policy.is_guarded());
        assert!(policy.authorize(None));
        assert!(policy.authorize(Some("anything")));
    }

    #[test]
    fn a_guarded_channel_takes_only_the_exact_secret() {
        let policy = TokenPolicy::new(Some("s3cret".to_string()));
        assert!(policy.is_guarded());
        assert!(policy.authorize(Some("s3cret")));
        assert!(!policy.authorize(None));
        assert!(!policy.authorize(Some("wrong")));
        // 접두사도 접미사도 아니다.
        assert!(!policy.authorize(Some("s3cre")));
        assert!(!policy.authorize(Some("s3cretX")));
    }

    #[test]
    fn only_loopback_binds_are_accepted() {
        assert!(resolve_loopback_bind("127.0.0.1:0").is_ok());
        assert!(resolve_loopback_bind("[::1]:8788").is_ok());
        assert!(resolve_loopback_bind("0.0.0.0:8788").is_err());
        assert!(resolve_loopback_bind("192.168.1.10:8788").is_err());
        // 포트 없는 주소·호스트이름은 받지 않는다 — 패인 채널은 창이 고른
        // 번호로 열리고, 이름 풀이가 끼면 어디에 열렸는지 흐려진다.
        assert!(resolve_loopback_bind("127.0.0.1").is_err());
        assert!(resolve_loopback_bind("localhost:8788").is_err());
    }
}
