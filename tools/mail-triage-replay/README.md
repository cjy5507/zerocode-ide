# mail-triage-replay — 코디네이터 우편 분류의 라벨을 이 기계 원장으로 다시 매기는 하네스 (t-9471)

두 조각이다. `seed.py` 는 **모으기만** 하고, 판정은 하나도 하지 않는다 — 어느 영수증이
코디네이터의 행동인지, 어느 행동이 편지를 처리했는지, 편지가 언제 넘어갔는지, 라벨이 무엇인지는
`zerocode_core::mail_triage`(`Filed::of_parts`·`Mailroom`) 한 곳에만 있고, 재생 시험이 씨앗을
그 함수들에 그대로 넘긴다(중복 금지). 파이썬에 규칙 사본이 생기면 재는 규칙과 창이 쓰는 규칙이
갈라진다.

**새 Jev 요청은 0건이다.** 원장을 다시 읽어 라벨과 종류 규칙(기준선)만 계산한다. 좌석의 실제
답은 창에서 기록 모드가 켜진 뒤에만 생긴다.

| 조각 | 무엇 |
| --- | --- |
| `tools/mail-triage-replay/seed.py` | 창의 권한 저장소(`~/Library/Application Support/dev.zerocode.app/authority/authority.sqlite`, **읽기 전용 `mode=ro`**)에서 원장 **하나**의 실행마다 — 메시지(글 없이, 코어의 `Message` 모양), 시도, 코디네이터가 열어 둔 묶음, 아직 넘어가지 않은 id, 인정된 묶음 — 과 모든 영수증(보낸 세션은 해시, 동사, 시각, `check` 가 본 받은편지함과 넘긴 id, 답에서 원장 id 모양의 값과 훑어보기 쪽 낱말 `mode` 만)을 씨앗 하나로 |
| `crates/zerocode-core/src/mail_triage/tests.rs` 의 `the_mail_this_machine_would_have_labeled` | 씨앗의 편지마다 배포되는 라벨 규칙으로 다시 매기고, 종류 규칙과 코디네이터가 기각한 규칙(인정=처리)을 같은 편지 위에 나란히 세는 `#[ignore]` 실측 |
| `tools/tests/test_mail_triage_replay_seed.py` | 씨앗이 편지의 글·답의 산문·재시도 이름·세션 원문·폴더 경로를 싣지 않는지, 원장 하나만 읽는지, 저장소를 한 바이트도 안 움직이는지, 상수가 Rust 원본과 같은지 (`just tools-test`) |

## 돌리기

```sh
python3 tools/mail-triage-replay/seed.py --out /tmp/mail-triage/today.json          # 오늘(이 기계 자정부터)
python3 tools/mail-triage-replay/seed.py --days 7 --out /tmp/mail-triage/week.json  # 최근 7일

ZEROCODE_MAIL_TRIAGE_REPLAY_SEED=/tmp/mail-triage/today.json \
  cargo test -p zerocode-core --lib \
  mail_triage::tests::the_mail_this_machine_would_have_labeled \
  -- --ignored --nocapture
```

| 손잡이 | 무엇 |
| --- | --- |
| `--db` / `--ledger-id` | 다른 저장소, 원장이 여럿인 저장소(없으면 거절, 종료 코드 2) |
| `--from-ms` / `--days` | 라벨을 매길 편지가 쓰인 시각의 시작(기본은 이 기계의 오늘 자정). 행동은 창 밖의 것도 읽는다 |

## 라벨 (코디네이터 결정 m-10067)

- **행동**은 코디네이터 세션이 `--retry-request` 로 남긴 영수증이다 — 좌석의 세션, 그리고 실행의
  목소리(`run:<id>`)로 편지를 쓴 적이 있는 세션. 받은편지함의 부기(`check`, 그리고 그것이 싣는
  인정)는 **행동이 아니다**: 우편 안내가 묶음마다 곧바로 인정하게 하므로(09-26: 편지 172통이 든 묶음
  107개, 다음 `check` 까지 p50 11 s·p90 33 s, 107개 중 106개가 빈 받은편지함) 인정은 「읽었다」일 뿐
  편지가 무엇을 요구했는지 말하지 않는다.
- **처리**는 편지가 다루는 것을 처음 이름 댄 행동이다 — 질문은 그 답(`Run::answer_to`)만, 작업 완료
  보고는 그 작업을 이름 대는 행동(`task-update`·`worker-start`·그 작업을 단 편지)만, 나머지는 그
  스레드·작업·작업자·시도를 이름 대는 행동. 이름은 영수증 답의 원장 id(`taskId`·`workerId`·
  `messageId`/`questionId`·`dispatchId`·`retryOf`)와 코디네이터가 쓴 메시지의 스레드·작업·받는 이로 읽는다.
- **시작**은 넘겨준 때다: 창이 본 열린 묶음의 도장 → 그 편지를 넘긴 `check` 영수증 → 편지가 쓰인 때(행에
  `start` 로 표식).
- 세 번째 행동 안에 처리되면 `answer_now`, 그 뒤면 `can_wait`, 서른 번의 행동 동안 없으면 `no_need`
  (K·R 은 `mail_triage.rs` 의 상수와 그 근거). 같은 종류·보낸 이·작업자·작업의 더 새 편지가 처리 전에
  오면 `superseded`, 하루가 지나도 행동이 모자라면 `unhandled` — 둘 다 비교하지 않는다.

## 읽지 않는 것

편지의 본문·제목·짐, 영수증의 재시도 이름·지문, 답의 산문(작업 제목, 다른 에이전트가 쓴 답),
세션의 원문, 실행·작업자·원장 자신이 아닌 주소의 머리 뒤(`@worktree:` 뒤의 폴더 경로, 판 이름).
재생의 출력은 수와 낱말뿐이다.
