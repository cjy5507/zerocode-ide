//! 대화형 프런트 — codex CLI 의 UI/UX 를 그대로.
//!
//! stdin 이 tty 면 이 트리가 화면을 맡는다. 파이프면 손대지 않는다 —
//! `ide::render`/`ide::run_loop` 의 append-only 경로가 그대로 돌고, `codex exec`
//! 패리티 골든은 바이트 하나 안 바뀐다.
//!
//! # 화면 관리 계약
//!
//! - alt-screen 없음. 히스토리 재그리기 없음.
//! - 인라인 뷰포트(하단 5~7줄) + 스크롤 영역/역인덱스 히스토리 삽입
//!   ([`painter`]).
//! - 프레임마다 `ESC[?2026h … ESC[?2026l` 동기화, 프레임 중 커서 숨김.
//! - raw mode 는 이 방식 **안에서** 쓴다(codex 가 같은 패인에서 검증했다).
//!
//! # 모듈 지도
//!
//! | 모듈 | 책임 |
//! |---|---|
//! | [`ansi`] | `Style`/`Span`/`Line` 과 최소 차분 SGR 방출 |
//! | [`agents`] | 현재 세션의 하위 에이전트 개요·메시지·중지 피커 재료 |
//! | [`palette`] | 터미널 기본색 조회·팔레트 상수·지연 OSC 응답 차단 |
//! | [`shimmer`] | Working 글자를 훑는 밝기 웨이브 |
//! | [`wrap`] | 히스토리에 넣기 전 미리 접기 |
//! | [`highlight`] | 코드 담장의 문법 강조(syntect + two-face) |
//! | [`markdown`] | codex 마크다운 규칙(해시 유지 헤딩·리스트 들여쓰기·표) |
//! | [`tables`] | 마크다운 표의 열 폭 배분·정렬·키/값 폴백 |
//! | [`holdback`] | 스트림 원문에서 표를 찾아 꼬리를 붙잡는 스캐너 |
//! | [`chunking`] | 커밋 애니메이션 큐의 케이던스(Smooth ↔ CatchUp) |
//! | [`cells`] | `RenderBlock` → 접두어 붙은 히스토리 셀·스트리밍 두 영역 |
//! | [`fast`] | `/fast` capability, transport variant, and display state |
//! | [`permissions`] | Codex preset rows and zo permission-mode mapping |
//! | [`tools`] | 도구 턴의 셀(`Ran`·`Exploring`·`Edited`·MCP) |
//! | [`thinking`] | reasoning 의 상태 낱말(굵은 제목 또는 마지막 문장)과 접힌 thinking 셀 |
//! | [`folds`] | 도구 셀 경계의 접기 마커(OSC 7788) |
//! | [`composer`] | `›` 한 줄 편집기(UTF-8·붙여넣기 안전) |
//! | [`effort_effect`] | Max·Ultra·Smart 컴포저/푸터 일회성 전환 |
//! | [`slash`] | keep-list 와 컴포저 자동완성 팝업의 재료 |
//! | [`mention`] | `@` 파일·스킬·볼트 페이지 팝업(codex `mentions_v2`) |
//! | [`models`] | 자격증명 있는 provider 의 모델 목록(`/model` 피커) |
//! | [`summary`] | 종료·재개 때 나가는 usage·재개 힌트 두 줄 |
//! | [`question`] | `AskUserQuestion` 오버레이의 재료(codex `request_user_input`) |
//! | [`sessions`] | 최근 세션 목록(`/resume` 피커) |
//! | [`view`] | 뷰포트 레이아웃(상태·컴포저·푸터·다이얼로그) |
//! | [`paths`] | 부팅 카드 경로의 가운데 자르기 |
//! | [`painter`] | 스크롤 영역 삽입과 뷰포트 그리기 |
//! | [`app`] | 상태기계 — 키·블록·턴 |

pub mod ansi;
pub mod agents;
pub mod activity;
pub mod app;
pub mod bell;
pub mod cells;
pub mod clipboard_paste;
pub mod chunking;
pub mod composer;
pub mod diff;
pub mod effort_effect;
pub mod fast;
pub mod folds;
pub mod highlight;
pub mod holdback;
pub mod markdown;
pub mod mention;
pub mod models;
pub mod painter;
mod paint_probe;
pub mod palette;
pub mod pending_input;
pub mod permissions;
pub mod paths;
pub mod question;
pub mod sessions;
pub mod shimmer;
pub mod slash;
pub mod summary;
pub mod strings;
pub mod tables;
pub mod thinking;
pub mod tools;
pub mod transcript;
pub mod tty;
pub mod view;
pub mod wire_model;
pub mod wrap;

pub use app::{TeammateLife, run, run_teammate, run_with_last_message, terminal_is_usable};
