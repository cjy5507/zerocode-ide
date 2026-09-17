//! IDE 이벤트 채널의 속살 — 와이어·토큰·상태·소켓.
//!
//! 사람이 쓰는 문은 [`crate::ide::events`] 하나다. 이 디렉터리는 그 문 뒤에
//! 있는 네 조각이고, 조각마다 한 가지만 안다:
//!
//! | 모듈 | 아는 것 | 정본 |
//! |---|---|---|
//! | [`wire`] | 봉투·오류코드·프레임 `type` | `zerocode-harness/src/lib.rs`, `port-staging/.../serve_protocol.rs` |
//! | [`auth`] | 토큰과 루프백 강제 | `port-staging/.../serve_auth.rs`, `zerocode-lane::validate_loopback_bind` |
//! | [`auth_reload`] | IDE 가 고른 계정의 재적재 | `docs/design/zo-ide-account-oauth.md` §2.3 |
//! | [`state`] | 팬아웃·프롬프트 등록부·명령 큐 | `port-staging/.../socket_permission.rs` 의 responder 맵 |
//! | [`server`] | 소켓과 메서드 배차 | `port-staging/.../serve.rs` 의 dispatch |

pub mod capabilities;
pub mod auth;
pub mod auth_reload;
pub mod server;
pub mod state;
pub mod wire;
