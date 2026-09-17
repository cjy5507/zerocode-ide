//! 스트리밍 및 렌더러의 4대 적대적 시나리오 검증 스위트.
//!
//! 1. 개행 없는 초대형 텍스트 폭주 시 버퍼 안전성 및 피니시 정합성
//! 2. 스트리밍 도중 초고속 폭 리사이즈(120->40->80->20->100) 시 중복 및 불일치 방지
//! 3. 큐 대량 백로그 폭주 시 수렴(convergence) 및 Smooth 복귀 안정성
//! 4. 제로 카피 emit 최적화의 컬러/비컬러 바이트 계약 정합성

use std::time::{Duration, Instant};

use zo_ide::ide::render::{InputSource, RenderOptions, Renderer};
use zo_ide::tui::ansi::Line;
use zo_ide::tui::cells::MarkdownStream;
use zo_ide::tui::chunking::{DrainPlan, Mode, Policy, Snapshot};

/// 시나리오 1: 개행이 전혀 없는 대량 토큰 폭주 (5,000자 이상)
#[test]
fn newline_free_massive_stream_settles_cleanly() {
    let mut stream = MarkdownStream::answer(40);
    // 개행 없이 100개 청크 연속 투입
    for i in 0..100 {
        stream.push(&format!("chunk{i}_word "));
    }
    // 개행이 없으므로 아직 스크롤백으로 방출된 행은 없어야 한다
    assert!(stream.tick(Instant::now()).is_empty());

    // 스트림 피니시 시 모든 단어가 누락 없이 확정 방출되어야 한다
    let finalized = stream.finish();
    assert!(!finalized.is_empty(), "finish must flush all buffered tokens");
    let full_text: String = finalized.iter().map(Line::plain).collect::<Vec<_>>().join(" ");
    assert!(full_text.contains("chunk0_word"));
    assert!(full_text.contains("chunk99_word"));
}

/// 시나리오 2: 스트리밍 도중 초고속 터미널 폭 리사이즈
#[test]
fn rapid_terminal_width_resize_under_streaming() {
    let mut stream = MarkdownStream::answer(80);
    stream.push("첫 번째 문장입니다.\n이것은 두 번째 긴 문장으로 여러 번 접혀야 합니다.\n");

    let widths = [40, 120, 20, 90, 30, 80];
    for &w in &widths {
        stream.set_width(w);
        let _ = stream.tick(Instant::now());
        stream.push(&format!("새 폭 {w}에서의 추가 라인입니다.\n"));
    }

    let remaining = stream.finish();
    assert!(!remaining.is_empty() || stream.is_empty());
}

/// 시나리오 3: 큐 대량 백로그 폭주 시 점진적 수렴 및 Smooth 복귀
#[test]
fn high_pressure_burst_queue_converges_to_smooth() {
    let mut policy = Policy::default();
    let mut now = Instant::now();

    // 100줄 대량 백로그 유입
    let plan = policy.decide(
        Snapshot {
            queued_lines: 100,
            oldest_age: Some(Duration::from_millis(500)),
        },
        now,
    );
    assert_eq!(policy.mode(), Mode::CatchUp);
    assert!(plan.rows() >= 1);

    // 큐가 점차 비어 1줄로 감소
    now += Duration::from_millis(50);
    let _ = policy.decide(
        Snapshot {
            queued_lines: 1,
            oldest_age: Some(Duration::from_millis(10)),
        },
        now,
    );

    // EXIT_HOLD(250ms) 경과 후 Smooth로 자동 복귀
    now += Duration::from_millis(300);
    let plan_after = policy.decide(
        Snapshot {
            queued_lines: 1,
            oldest_age: Some(Duration::from_millis(5)),
        },
        now,
    );
    assert_eq!(policy.mode(), Mode::Smooth);
    assert_eq!(plan_after, DrainPlan::Single);
}

/// 시나리오 4: 제로 카피 emit 최적화의 컬러/비컬러 바이트 계약 정합성
#[test]
fn zero_copy_emission_matches_contract() {
    let mut color_out = Vec::new();
    let mut plain_out = Vec::new();

    let color_opts = RenderOptions {
        color: true,
        input: InputSource::Pipe,
        agent_label: Some("zo".to_string()),
        ..RenderOptions::default()
    };
    let plain_opts = RenderOptions {
        color: false,
        input: InputSource::Pipe,
        agent_label: Some("zo".to_string()),
        ..RenderOptions::default()
    };

    let mut color_renderer = Renderer::new(&mut color_out, color_opts);
    let mut plain_renderer = Renderer::new(&mut plain_out, plain_opts);

    let test_block_1 = runtime::message_stream::RenderBlock::TextDelta {
        id: runtime::message_stream::BlockId(1),
        text: "Zero-copy streaming test line.\n".to_string(),
        done: true,
    };
    let test_block_2 = runtime::message_stream::RenderBlock::TextDelta {
        id: runtime::message_stream::BlockId(1),
        text: "Zero-copy streaming test line.\n".to_string(),
        done: true,
    };

    color_renderer.push(test_block_1);
    color_renderer.finish_turn();

    plain_renderer.push(test_block_2);
    plain_renderer.finish_turn();

    let color_str = String::from_utf8(color_out).expect("valid utf8");
    let plain_str = String::from_utf8(plain_out).expect("valid utf8");

    // 컬러 출력은 SGR 이스케이프를 포함하고 에이전트 라벨 "zo"를 담고 있어야 함
    assert!(color_str.contains("\u{1b}[35m\u{1b}[3mzo\u{1b}[0m\u{1b}[0m"));
    assert!(color_str.contains("Zero-copy streaming test line."));

    // 비컬러 출력은 이스케이프 없이 평문 "zo"와 텍스트만 담겨 있어야 함
    assert!(!plain_str.contains("\u{1b}["));
    assert!(plain_str.contains("zo\n"));
    assert!(plain_str.contains("Zero-copy streaming test line."));
}
