# agent-tool-replay — 에이전트 Jev 도구를 프런티어 모델과 같은 골든에서 재는 하네스 (t-6040)

두 조각이다. `seed.py` 는 **뽑기만** 하고, 계산은 하나도 하지 않는다 — 일치율·Wilson 하한·토큰·지연은
Rust 하네스 한 곳(`tools::smart_router::agent_tool::tests::jev_against_the_frontier_on_the_golden`)이
씨앗을 읽어 두 팔에 같은 질문을 던지며 센다(중복 금지).

| 조각 | 무엇 |
| --- | --- |
| `tools/agent-tool-replay/seed.py` | 세컨드 브레인 `wiki/` 페이지 가운데 주제 태그(11개) 를 **정확히 하나** 단 페이지를 골든으로 뽑는다: 라벨=그 태그, 맥락=제목+본문 머리(2,000자), 선택지=태그 11개와 한 줄 뜻 |
| `zo-ide/crates/tools/src/misc_tools/smart_router/agent_tool/tests.rs` 의 `jev_against_the_frontier_on_the_golden` | 씨앗을 Jev(자리 `decide`, 진짜 문·진짜 엔드포인트)와 프런티어(이 기계 zo 의 메인 턴 모델, 한 번에 한 항목)에 물어 표를 찍는 `#[ignore]` 실측 |
| `tools/tests/test_agent_tool_replay_seed.py` | 씨앗의 상수가 core 표와 같은지(`ConstantsMatchTheirSource`)·뽑기 규칙·계산 없음 |

## 돌리기

```sh
python3 tools/agent-tool-replay/seed.py \
  --vault "$ZEROCODE_SECOND_BRAIN" \
  --out /tmp/agent-tool-replay/seed.json

TYPESAFE_API_KEY=$(security find-generic-password \
    -s dev.zerocode.key.TYPESAFE_API_KEY -a "$USER" -w) \
ZO_AGENT_TOOL_REPLAY_SEED=/tmp/agent-tool-replay/seed.json \
ZO_AGENT_TOOL_REPLAY_MODEL=claude-opus-5 \
ZO_AGENT_TOOL_REPLAY_OUT=/tmp/agent-tool-replay/report.json \
  cargo test -p tools --lib \
  misc_tools::smart_router::agent_tool::tests::jev_against_the_frontier_on_the_golden \
  -- --ignored --nocapture
```

| 손잡이 | 무엇 |
| --- | --- |
| `ZO_AGENT_TOOL_REPLAY_SEED` | 씨앗 파일(필수) |
| `ZO_AGENT_TOOL_REPLAY_MODEL` | 프런티어 팔의 모델 id(필수) — zo 메인 턴이 쓰는 그 모델을 적는다 |
| `ZO_AGENT_TOOL_REPLAY_OUT` | 표를 JSON 으로도 남길 자리(선택) |
| `ZO_AGENT_TOOL_REPLAY_LIMIT` | 몇 항목만(선택, 기본 전부) — 앞에서가 아니라 씨앗 전체에서 **고르게**(k번째마다) 뽑아 라벨 비율을 지킨다; 두 팔은 같은 항목을 받는다 |

## 이 하네스가 지키는 세 가지

1. **같은 질문, 같은 맥락, 같은 선택지.** 두 팔은 씨앗의 `question`·`context`·`options` 를 글자 그대로 받는다.
   Jev 는 자리의 문(`jev_gate`)을 지나 선택지를 `o0…o10` 자리로 받고, 프런티어는 같은 뜻을 목록으로 받아
   선택지 id 한 낱말로 답한다.
2. **숫자는 두 팔 모두 실측이다.** 토큰은 응답의 `usage`, 지연은 호출을 감싼 시계, 비용은 가격표
   (`crates/model-prices`)의 그 모델 행. 일치율은 pooled 원비율과 95 % Wilson 하한만 적는다.
3. **미응답은 미응답으로 센다.** 프런티어 팔은 항목마다 90초 벽 안에서 클라이언트의 재시도(429·529에 최대 6번째 시도까지)를
   기다리고, 벽을 넘기거나 오류로 끝나면 그 항목을 `rate_limited`·`timeout`·`error`로 적고 다음으로 간다 — 같은 계정을
   여러 워커가 쓰는 시간엔 429가 잦다. Jev 팔의 미응답은 자리의 outcome 낱말 그대로다. 표의 마지막 열이 그 집계다.
4. **키는 환경에서.** 하네스는 임시 설정 홈(동의된 워크스페이스 하나, `smart.agentTool: on`)을 세우고
   TypeSafe 키는 `TYPESAFE_API_KEY` 로만 읽는다(사다리 첫 단). 프런티어 팔은 zo 가 그 모델에 쓰는
   자격증명 사다리를 그대로 탄다. 키 값은 어디에도 적히지 않는다.
