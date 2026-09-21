//! 이 앱의 이름 하나 — 번들 식별자이자, 이 기계에서 앱의 폴더 이름.
//!
//! 여기 있는 이유는 이 이름이 **경계를 넘기** 때문이다. 창은 이 이름으로
//! 자기 설정·데이터 폴더를 만들고(`zerocode_lane::AppPaths`), 창 밖에서 도는
//! 에이전트는 같은 이름으로 그 폴더를 찾아 창이 고른 계정을 따라간다
//! (t-5777). 한 이름이 두 프로세스에 걸쳐 있으면 그 이름은 이 크레이트의
//! 것이다 — 이 크레이트의 문서가 말하는 그대로.

/// Tauri 번들 식별자, 그래서 앱 데이터 폴더의 이름.
///
/// 플랫폼 뿌리 아래 이 이름의 폴더가 창의 것이다: 맥은
/// `~/Library/Application Support/dev.zerocode.app`, 리눅스는
/// `~/.config/dev.zerocode.app`(설정)와 `~/.local/share/dev.zerocode.app`(데이터).
pub const IDENTIFIER: &str = "dev.zerocode.app";
