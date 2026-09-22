# compaction-replay — 관련성 컴팩션 자리를 이 기계의 전사로 재는 하네스 (t-6039)

두 조각이다. `seed.py` 는 **고르기만** 하고, 계산은 하나도 하지 않는다 — 컴팩션 경계는
`runtime::prepare_compaction`, 꼬리는 `preserved_tail_len_for_budget`, 후보·상태·질문·drop 적용은
`runtime::compaction_relevance` 한 곳에만 있고, 하네스가 씨앗의 전사를 그 함수들에 그대로 넘긴다
(중복 금지). 파이썬에 규칙 사본이 생기면 재는 규칙과 나가는 규칙이 갈라진다.

| 조각 | 무엇 |
| --- | --- |
| `tools/compaction-replay/seed.py` | `~/.zo/projects/*/sessions/session-*.jsonl` 중 경계가 설 만큼 긴 전사마다 경로·메시지 수·모델·기록된 컴팩션의 `first_kept_message_index`·볼트 유무를 씨앗 하나로 |
| `zo-ide/crates/tools/src/misc_tools/smart_router/compaction_seat/tests.rs` 의 `the_compactions_this_machine_would_have_made` | 씨앗의 전사마다 제품의 경계 규칙으로 자르고 실제 엔드포인트에 물어 표를 찍는 `#[ignore]` 실측 |
| `tools/tests/test_compaction_replay_seed.py` | 씨앗이 전사만 고르고 사이드카를 버리는지, 상수가 Rust 원본과 같은지 (`just tools-test`) |

## 돌리기

```sh
python3 tools/compaction-replay/seed.py --out /tmp/compaction-replay/seed.json

TYPESAFE_API_KEY=$(security find-generic-password \
    -s dev.zerocode.key.TYPESAFE_API_KEY -a "$USER" -w) \
ZEROCODE_COMPACTION_REPLAY_SEED=/tmp/compaction-replay/seed.json \
  cargo test -p tools --lib \
  compaction_seat::tests::the_compactions_this_machine_would_have_made -- --ignored --nocapture
```

| 손잡이 | 무엇 |
| --- | --- |
| `ZEROCODE_COMPACTION_REPLAY_BUDGET_TOKENS` | 모델의 실제 컴팩션 임계(`auto_compaction_threshold_for_model`) 대신 이 토큰 수에서 자른다 — 실제 임계에 닿은 전사가 적은 기계에서 지점을 늘리는 손잡이. 표에 어느 쪽으로 잘랐는지 적힌다 |

## 지점은 어디인가

전사마다 **하나**다(t-6155 F5). **기록된 지점**이 있으면 그것 — 전사가 실제로 컴팩션한 자리
(`compaction` 레코드의 `first_kept_message_index`)로, 그 앞의 메시지는 볼트(`<id>.vault.jsonl`)에서
되읽는다 — 볼트가 그 앞을 빠짐없이 덮지 않으면 그 전사는 건너뛴다(구멍 난 축은 좌표가 아니다).
없으면 **임계 지점**: 전사의 제품 추정(`estimate_session_tokens`)이 모델의 컴팩션 임계를 처음 넘는
메시지 — 컴팩션이 불렸을 자리. 둘 다 잡던 때는 기록 전사의 임계 지점 블록이 기록 지점 블록의
부분집합이라 같은 도구 결과를 두 번 물었다(24지점 중 20이 겹침). `per_cut` 줄 끝에 전사 id(세션
파일의 stem)가 붙는다.

## 이 하네스가 지키는 세 가지

1. **판정의 입력은 지점 앞뿐이다.** 세션을 지점까지의 메시지로 다시 만들고 거기에
   `prepare_compaction`·`ask_for` 를 건다. 지점 뒤의 줄은 라벨로만 읽는다 — 뺀 블록의 경로를
   `COMPACTION_REGRET_TURNS` 턴 안에 다시 읽었는가(같은 호출을 다시 했는가) — 그것이 사후
   라벨의 정의다.
2. **키는 환경에서, 원장은 아무 데도.** 문은 임시 홈에 서고(`JevDoor::at`), `judge` 는 행을 쓰지
   않는다. 사람의 원장·하루 셈은 움직이지 않는다.
3. **숫자는 pooled 원비율과 지점별 중앙값.** 같은 지점을 여러 번 묻지 않는다. drop % 와 토큰
   절감은 전 지점을 합친 비율 **옆에 지점별 중앙값**을 찍는다 — 한 전사(gpt-5.6-sol 기록 지점)가
   블록의 52%를 들고 있어 pooled −22%가 중앙값 −9.9%였다(t-6155 F5). 후회율은 뺀 블록 중 다시 읽은
   비율에 「후회 안 함」의 Wilson 하한을 붙이고, 그 값은 **하한**이다(t-6155 F10): 같은 호출·같은 경로의
   재읽기만 세고, Grep/Edit로 같은 파일을 다시 만진 것과 철자가 다른 경로는 못 센다. p50/p95 는
   지점마다 한 번 잰 배치 벽(가장 느린 shard)의 분포다.

## 씨앗이 읽지 않는 것

경로·개수·모델·지점뿐이다. 사람이 친 글도 도구가 찍은 출력도 씨앗에 없다 — 전사 본문은
Rust 쪽이 지점에서 직접 읽고, 엔드포인트로 나가는 것은 자리 행이 선언한 머리들뿐이다.
