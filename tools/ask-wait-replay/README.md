# ask-wait-replay — 질문자가 받는 쪽에 대해 알 수 있던 것을 원장으로 재는 하네스 (t-6740)

두 조각이다. `seed.py` 는 **모으기만** 하고, 규칙은 하나도 갖지 않는다 — 무엇이 알림이 되는지는
`crates/zerocode-core/src/orchestration.rs` 의 `ReceiverNews` 표 한 곳, 「답」의 정의는
`Message::answers` 한 곳이다. 파이썬에 규칙 사본이 생기면 재는 규칙과 나가는 규칙이 갈라진다.

| 조각 | 무엇 |
| --- | --- |
| `tools/ask-wait-replay/seed.py` | 창의 권한 저장소(`~/Library/Application Support/dev.zerocode.app/authority/authority.sqlite`, **읽기 전용 `mode=ro`**)에서 뿌리 질문마다 — 누가 누구에게(주소 머리만), 기다림이 어떻게 끝났는지(`answered`·`closed`·`censored`), 받는 쪽이 워커면 그동안 원장이 그 워커에 대해 적은 사실(턴 끝·멈춤·판정 사유·쿼터 벽·시도 끝남과 그 낱말) — 을 씨앗 한 행으로 |
| `tools/tests/test_ask_wait_replay_seed.py` | 씨앗이 질문 뒤에 적힌 사실만 싣는지, 원장 자신의 줄을 답으로 읽지 않는지, 원장을 한 바이트도 안 움직이는지, 본문이 새지 않는지, 상수가 Rust 원본과 같은지 (`just tools-test`) |

## 돌리기

```sh
python3 tools/ask-wait-replay/seed.py --out /tmp/ask-wait/seed.json      # 요약 표를 찍고 씨앗을 남긴다
python3 tools/ask-wait-replay/seed.py --db <복사본.sqlite> --out …          # 다른 원장
```

## 이 하네스가 지키는 것

1. **질문 당시 알 수 있던 것만.** 받는 쪽의 사실은 원장이 그 사실을 **적은 시각**(`learned_ms`)이 질문 뒤·기다림 끝 전일 때만 싣고, 사실의 제 시각(`fact_ms`)을 따로 둔다. 기다림의 끝은 답이 온 시각, 질문자의 dispatch 가 끝난 시각, 아니면 원장의 마지막 쓰기(오른쪽 censoring — 따로 센다).
2. **답은 원장의 정의로.** 스레드에서 물은 자리의 줄이되 원장 자신의 목소리·원장만 쓰는 종류는 답이 아니다. 오늘의 느슨한 읽기가 답으로 삼았을 줄이 다르면 `misread_answer` 로 센다.
3. **원장은 아무 데도 안 움직인다.** `mode=ro` 로 열고, 씨앗에는 해시 id·주소 머리·낱말·수치만 남는다. 본문은 한 글자도 나가지 않는다.
4. **분모를 나눠 적는다.** 받는 쪽이 워커인 질문 중 「열린 시도가 없던 워커」·「사실이 하나도 안 적힌 워커」·「시도가 먼저 끝난 워커」를 따로 세고, 코디네이터·판 수신자는 `unobservable` 로 둔다 — 원장이 코디네이터의 턴을 적지 않는다는 사실을 수치로 꾸미지 않는다.
