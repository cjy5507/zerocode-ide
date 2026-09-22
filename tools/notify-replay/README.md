# notify-replay — 알림 판정 자리를 이 기계의 전사로 재는 하네스 (t-6043)

두 조각이다. `seed.py` 는 **모으기만** 하고, 계산은 하나도 하지 않는다 — 질문은
`zerocode_core::notify_call::ask`, 오늘의 규칙은 `Call::today`, 라벨의 표식은
`notify_call::agreed` 한 곳에만 있고, 하네스가 씨앗의 행을 그 함수들에 그대로 넘긴다
(중복 금지). 파이썬에 규칙 사본이 생기면 재는 규칙과 나가는 규칙이 갈라진다.

| 조각 | 무엇 |
| --- | --- |
| `tools/notify-replay/seed.py` | 창의 Claude 전사(`~/Library/Application Support/dev.zerocode.app/.claude/projects`·`~/.claude/projects`)에서 벨이 울렸을 순간마다 — 턴 끝(`finished`)·사람이 멈춤(`stopped`)·`AskUserQuestion`/`ExitPlanMode`(`needs input`) — 그 순간 벨이 알았던 사실과 라벨을 씨앗 하나로 |
| `crates/zerocode-shell/src/notify_call/tests.rs` 의 `the_calls_this_machine_would_have_made` | 씨앗의 행마다 배포되는 질문 그대로 실제 엔드포인트에 묻고 오늘 규칙 vs 자리의 표를 찍는 `#[ignore]` 실측 |
| `tools/tests/test_notify_replay_seed.py` | 씨앗이 사람의 손과 원장의 타이핑을 가르는지, 판정 시각 뒤를 라벨 말고는 읽지 않는지, 상수가 Rust 원본과 같은지 (`just tools-test`) |

## 돌리기

```sh
python3 tools/notify-replay/seed.py --out /tmp/notify-replay/seed.json

TYPESAFE_API_KEY=$(security find-generic-password \
    -s dev.zerocode.key.TYPESAFE_API_KEY -a "$USER" -w) \
ZEROCODE_NOTIFY_REPLAY_SEED=/tmp/notify-replay/seed.json \
ZEROCODE_NOTIFY_REPLAY_RUNS=2 \
  cargo test -p zerocode-shell --bin zerocode-shell \
  notify_call::tests::the_calls_this_machine_would_have_made -- --ignored --nocapture
```

| 손잡이 | 무엇 |
| --- | --- |
| `ZEROCODE_NOTIFY_REPLAY_RUNS` | 행마다 몇 번 물을지(기본 1). 둘째 pass부터는 근거가 아니라 같은 물음에 같은 답을 하는지의 반복성이다 |
| `ZEROCODE_NOTIFY_REPLAY_ROWS` | 오래된 순으로 이만큼만 |
| `ZEROCODE_NOTIFY_REPLAY_LANES` | 동시에 묻는 수(기본 4) |
| `--days` (seed) | 최근 며칠의 전사만(기본 14) |

## 이 하네스가 지키는 세 가지

1. **행보다 미래를 보지 않는다.** 행마다 벨이 그 시각에 알았던 것만 싣는다 — 사건의 낱말,
   판 이름, 그 시각 **이전** 마지막 손으로 읽은 참석(`NOTIFY_ATTENDANCE_WINDOW_MS`), 알림
   본문 한 줄, 그 판의 앞선 알림들(각각의 반응은 그 알림의 1분이 이 시각 전에 닫혔을 때만),
   그 순간 물음에 서 있던 다른 판 수. 라벨(1분 안의 손)은 답을 받은 **뒤에만** 읽는다.
2. **키는 환경에서, 원장은 아무 데도.** 문은 임시 홈에 서고(`Wire::at`), 사람의
   `notify-call.jsonl`·하루 셈은 움직이지 않는다.
3. **숫자는 pass별 Wilson 하한.** 「사람이 반응한 행을 끊기로 맞춘 비율」과 자리의 표식
   (`agreed`)은 pass마다 한 번씩 물은 값에만 Wilson 하한을 붙이고, 반복 표본은 합쳐 원비율만
   본다. 오늘 규칙(모두 끊기)의 표식도 같은 라벨로 같은 줄에 찍는다.

## 씨앗이 읽는 것과 읽지 않는 것

사람의 손은 전사가 `promptSource: typed` 로 적은 프롬프트 중 원장이 친 것(`<pasted_content`
소환·`orchestration message` 포인터)이 아닌 것이다. 훅이 끼운 줄(`isMeta`·`system`·`sdk`)은
누구의 손도 아니다. 씨앗은 알림 본문 한 줄(`NOTIFY_WORDS_CHAR_CAP`)과 판 이름(경로의
마지막 마디)을 싣는다 — 보고서에는 수만 적는다. zo 판의 `PushNotification` 은 아직 씨앗에
없다: zo 전사는 다른 모양이고, 창의 푸시 길이 그 자리에서 판정되는 것은 이 하네스와 무관하게
시험으로 핀돼 있다.

## 라벨의 한계

전사의 「사람의 손」은 창의 `term_key`/`term_paste` 와 같은 사실이 아니라 그 근사다: 탭을
눌러 읽기만 한 반응은 전사에 없고, 원장이 친 답을 사람의 손으로 잘못 읽을 자리는 위 두 표식으로
막았다. 자리 밖(away)이면서 반응이 없는 행은 표식이 없다 — 없던 사람이 돌아설 수는 없다.
