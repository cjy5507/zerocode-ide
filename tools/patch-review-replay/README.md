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
| `ZEROCODE_PATCH_REVIEW_REPLAY_LIMIT` | 앞에서부터 이만큼의 검토만 묻는다 (시범용) |
| `ZEROCODE_PATCH_REVIEW_REPLAY_SHOW` | 앞 N개 검토의 보낸 상태와 네 답을 통째로 찍는다 — 사람의 말과 코드가 자기 터미널에만 나간다 |

지출 상한은 시험 안의 상수 `REPLAY_SPEND_CAP_USD`(\$0.20)다: 낸 검토의 입력 토큰 값이 상한에 닿으면 더 묻지 않는다.
동시에 네 개(`REPLAY_IN_FLIGHT`)씩 묻는다 — 와이어의 분당 1,200과는 거리가 멀다.

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
