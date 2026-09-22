# patch-review-replay — 패치 검토 자리를 이 기계의 zo 전사로 재는 하네스 (t-6203)

두 조각이다. `seed.py` 는 **고르기만** 하고, 계산은 하나도 하지 않는다 — 무엇을 묻는지(사람의 말·hunk·증거의 꼬리·경로 지문)는
`runtime::patch_review::ask_for`, 패치가 어떻게 됐는지는 `runtime::patch_review::hindsight_of_turn`, 판정은 `verdict` 한 곳에만 있고,
하네스가 씨앗의 전사를 그 함수들에 그대로 넘긴다(중복 금지). 파이썬에 규칙 사본이 생기면 재는 규칙과 나가는 규칙이 갈라진다.
어느 파일이 전사인지도 `tools/compaction-replay/seed.py` 의 읽기를 불러다 쓴다.

| 조각 | 무엇 |
| --- | --- |
| `tools/patch-review-replay/seed.py` | `~/.zo/projects/*/sessions/session-*.jsonl` 중 성공한 편집 결과가 있는 전사마다 경로·메시지 수·모델·편집 결과 수·볼트 유무를 씨앗 하나로 |
| `zo-ide/crates/tools/src/misc_tools/smart_router/patch_review/tests.rs` 의 `the_patches_this_machine_wrote_reviewed_in_hindsight` | 씨앗의 전사마다 편집 지점에서 제품의 함수로 묻고 실제 엔드포인트에 보내, 그 뒤 턴으로 라벨을 붙여 표를 찍는 `#[ignore]` 실측 |
| `tools/tests/test_patch_review_replay_seed.py` | 씨앗이 전사만 고르고 글을 싣지 않는지, 상수가 Rust 원본과 같은지 (`just tools-test`) |

## 돌리기

```sh
python3 tools/patch-review-replay/seed.py --out /tmp/patch-review-replay/seed.json

TYPESAFE_API_KEY=$(security find-generic-password \
    -s dev.zerocode.key.TYPESAFE_API_KEY -a "$USER" -w) \
ZEROCODE_PATCH_REVIEW_REPLAY_SEED=/tmp/patch-review-replay/seed.json \
  cargo test -p tools --lib \
  patch_review::tests::the_patches_this_machine_wrote_reviewed_in_hindsight -- --ignored --nocapture
```

| 손잡이 | 무엇 |
| --- | --- |
| `ZEROCODE_PATCH_REVIEW_REPLAY_STATE` | 과업을 어떻게 읽을지 — `v1`(기본, 자리의 것) · `v2a` · `v2b` · `v2c` (`runtime::patch_review::TaskReading`, 아래) |
| `ZEROCODE_PATCH_REVIEW_REPLAY_LIMIT` | 씨앗의 패치 중 대략 이만큼을 고르게 뽑아 묻는다 (`div_ceil` 간격) |
| `ZEROCODE_PATCH_REVIEW_REPLAY_SHOW` | 앞 N개 검토의 보낸 상태와 네 답을 통째로 찍는다 — 사람의 말과 코드가 자기 터미널에만 나간다 |
| `ZEROCODE_PATCH_REVIEW_REPLAY_OUT` | 검토마다 JSON 한 줄(읽기·`judged` 지문·과업 글자 수·답·판정·사후·벽)을 이 파일에 — 글도 경로도 없다 |

지출 상한은 시험 안의 상수 `REPLAY_SPEND_CAP_USD`다: 과업 읽기 넷을 한 표본에서 견주는 비교 전체의 상한
`REPLAY_COMPARISON_CAP_USD`(\$0.30)를 읽기 수로 나눈 몫(\$0.075)이 한 판의 상한이고, 낸 검토의 입력 토큰 값이 거기 닿으면 더 묻지 않는다.
동시에 네 개(`REPLAY_IN_FLIGHT`)씩 묻는다 — 와이어의 분당 1,200과는 거리가 멀다.

## 과업 읽기 — 같은 패치, 다른 `/state/task` (t-6232)

질문 넷·패치·증거·경로 지문은 그대로 두고, 무엇을 「과업」으로 읽는지만 바꾼다. 읽기는 모두
`runtime::patch_review::task_by` 한 함수(자리가 묻는 `ask_for` 는 그 `v1`)이고, 재생은 `ask_reading` 으로 같은 패치를 묻는다.

| 낱말 | 과업 |
| --- | --- |
| `v1` | 사람의 최근 말 하나 — 자리가 오늘 보내는 것 |
| `v2a` | 사람의 최근 말 셋(`PERSONS_RECENT_MESSAGES`), 말한 순서대로 |
| `v2b` | 사람의 최근 말, 그 뒤 모델이 편집 전에 한 가장 최근 말(계획·설명) |
| `v2c` | 그 뒤 모델이 할 일 도구(`TodoWrite`)로 적은 가장 최근 계획, 열린 항목 먼저 — 없거나 다 끝났으면 `v2b` |

여러 글을 잇는 읽기는 스스로 `PATCH_REVIEW_TASK_CHAR_CAP` 안에 맞춘다 — 가장 최근 글부터 맞추고 각 글은 남은 자리의 머리를
가진다(문은 과업의 머리를 자르므로, 먼저 붙여 넣은 긴 브리핑이 「진행」을 밀어내지 않게). 어느 읽기든 사람의 말이 없으면
묻지 않으므로 네 읽기는 **같은 패치**를 묻는다: 한 씨앗·한 `LIMIT` 의 네 판은 같은 표본이고, 머리줄의 `sample` 지문이 그것을 보인다.
한 판은 표본 위에서 네 읽기가 무엇을 읽었는지(`v1`·`v2b` 와 달랐던 수, 글자 수 p50/p90)도 묻지 않고 찍는다.

## 이 하네스가 지키는 네 가지

1. **판정의 입력은 편집 결과 앞뿐이다.** 검토는 그 편집 결과 메시지 **앞의** 메시지와 편집 자신의 결과(패치)로만 만든다
   (`ask_for(&history[..at], …)`). 그 뒤의 줄은 라벨로만 읽는다([[a-replay-row-sees-only-what-the-ledger-knew-at-its-own-clock]]).
2. **그 시각에 있던 것은 그대로 본다.** microcompact 가 나중에 지운 도구 결과는 세션의 볼트·스냅샷에서 `tool_use_id` 로 되살린다
   (`runtime::heal_cleared_tool_results`) — 살아 있던 자리가 봤을 증거를 재생도 본다. 표에 되살리지 못한 자리표시 수가 찍힌다.
3. **키는 환경에서, 원장은 아무 데도.** 문은 임시 홈에 서고(`JevDoor::at`), `judge` 는 행을 쓰지 않는다. 사람의 원장·하루 셈은 움직이지 않는다.
4. **숫자는 pooled 원비율과 Wilson 하한.** 같은 패치를 여러 번 묻지 않는다. 일치율 옆에 **늘 허용하는 판독기**(모든 패치를 permit)의
   같은 행 위 일치율을 찍는다 — 자리가 그 기준선을 넘지 못하면 신호가 없는 것이고, 기본은 shadow 로 둔다.

## 라벨 — 사후에 패치가 어떻게 됐나

편집이 쓴 줄(추가된 줄과 줄이 지워진 자리, 반개구간)을 그 뒤 같은 파일의 편집들을 따라 옮기며 본다.

- **후회(regret)**: 창 안의 편집이 그 줄을 건드렸다 — 고친 것을 다시 고쳤거나 되돌렸다.
- **섰다(receipt)**: 그 턴의 마지막 편집 뒤에 검사 모양 bash 가 0으로 끝났다(r43 영수증, verified-state 의 분류기 그대로). 영수증이 먼저 판정한다.
- **섰다(window)**: `PATCH_REVIEW_REGRET_TURNS`(5) 턴이 조용히 지났다.
- **열림(open)**: 전사가 창보다 먼저 끝났다 — 채점하지 않는다.

`agreed` 는 판정이 그것을 불렀는가다: `permit` 이 선 패치, `proposal_only` 가 후회한 패치. 턴은 사람의 말에서 시작한다
(`persons_turns` — `[zo:…]` 로 여는 하네스의 글은 그 턴을 잇는다). 한계: bash 의 `git checkout`/`git restore` 로 되돌린 것은
편집 도구의 결과가 아니라서 세지 못한다 — 후회율은 **하한**이다.

## 씨앗이 읽지 않는 것

경로·개수·모델뿐이다. 사람이 친 글도 도구가 찍은 출력도 씨앗에 없다 — 전사 본문은 Rust 쪽이 직접 읽고, 엔드포인트로 나가는 것은
자리 행(`PATCH_REVIEW.sends`)이 선언한 것뿐이다.
