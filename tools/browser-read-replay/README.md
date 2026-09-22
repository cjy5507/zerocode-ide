# browser-read-replay — 브라우저 read 소음 자리를 이미 일어난 행과 실제 페이지로 재는 하네스 (t-6041)

세 조각이다. 어느 것도 **계산하지 않는다** — 블록을 나누는 규칙은 창의 페이지 스크립트
(`crates/zerocode-shell/src/cmd/browser.rs`의 `BROWSER_READ_BLOCKS_BODY`) 한 곳, 묻고 읽고
접는 규칙은 `zerocode_core::browser_read` 한 곳에만 있고, 하네스는 그 둘을 그대로 부른다.

| 조각 | 무엇 |
| --- | --- |
| `gather.mjs` | 공개 페이지 열 장(`pages.txt`)을 Chromium으로 열어 **창의 페이지 스크립트 그대로**로 블록을 잘라 씨앗 하나로 |
| `seed.py` | 원장 행(`~/.zo/jev/browser-read.jsonl`)의 read 행과 그 뒤 누름이 쓴 라벨 행을 read 이름으로 이어 씨앗 하나로 — 뽑기만 |
| `crates/zerocode-shell/src/browser_read/tests.rs`의 `the_pages_that_were_gathered` | gather 씨앗을 실제 엔드포인트에 물어 페이지마다 전/후 글자 수·블록·접힌 수·p50/p95·토큰·비용을 찍는 `#[ignore]` 실측 |
| `tools/tests/test_browser_read_replay_seed.py` | 씨앗이 제 시각 뒤의 라벨을 보지 않는지, 상수 사본이 Rust와 같은지 (`just tools-test`) |

## 돌리기

```sh
# 1. 페이지 열 장을 창의 커터로 자른다 (Playwright 는 저장소의 node_modules 에서 빌린다).
NODE_PATH=/path/to/node_modules node tools/browser-read-replay/gather.mjs --out /tmp/browser-read/seed.json

# 2. 실제 엔드포인트에 묻는다. 키는 환경에서 — 어떤 파일에도 남지 않는다.
TYPESAFE_API_KEY=$(security find-generic-password -s dev.zerocode.key.TYPESAFE_API_KEY -a "$USER" -w) \
ZEROCODE_BROWSER_READ_SEED=/tmp/browser-read/seed.json \
  cargo test -p zerocode-shell --bin zerocode-shell \
  browser_read::tests::the_pages_that_were_gathered -- --ignored --nocapture

# 3. 이미 일어난 read 행과 라벨을 씨앗으로.
python3 tools/browser-read-replay/seed.py --ledger ~/.zo/jev/browser-read.jsonl --out /tmp/browser-read/rows.json
```

## 이 하네스가 지키는 것

1. **키는 환경에서.** 실측은 임시 zo 홈으로 나가 사람의 원장·하루 셈을 건드리지 않는다.
2. **행보다 미래를 보지 않는다.** 라벨은 그 read 뒤에 쓰인 것만 잇고, read 보다 오래된 라벨은
   씨앗을 내보내지 않고 종료 코드 1 로 끝난다.
3. **숫자는 pooled 원비율과 pass 별 Wilson 하한만.** 같은 페이지를 여러 번 물은 답은 독립 표본이
   아니다. 표는 합계 옆에 **페이지별 중앙값**과 **봇 벽 페이지를 뺀 합계**를 함께 찍는다(t-6155 F6):
   `gather.mjs` 가 상태 코드 400 이상이거나 흔한 벽 제목인 페이지를 `wall: true` 로 표시한다 — 벽의
   낱말은 그 페이지 자체라 접어도 페이지를 접은 것이 아니다.
4. **골든 오탐은 사람이 읽는다.** 실측은 접힌 블록마다 경로와 글 머리를 찍으므로, 본문 블록이
   접혔는지는 그 목록을 읽어 판정한다 — 자동 채점기가 없다.
5. **라벨은 둘.** 접힌 뒤 같은 페이지에서 click/type 이 닿은 자리(접힌 블록 안이면 `agreed:false`)와,
   같은 페이지를 `read --full` 로 다시 통째로 읽은 것(`verb: read_full`, 늘 `agreed:false` — 접기의
   가장 흔한 후회, t-6155 F6). 접은 것이 없는 판(shadow)의 `--full` 은 라벨이 아니다.
6. **행에 주소는 호스트까지.** read 행은 `host` 와 경로의 지문(`pathFingerprint`, `jev::fingerprint_of`)만
   싣는다(t-6155 F8) — 경로에는 티켓 번호·사용자 id·서명 URL 조각이 올 수 있고, 라벨 대조는 메모리의
   URL 로 하므로 행에 경로가 있을 이유가 없다.
