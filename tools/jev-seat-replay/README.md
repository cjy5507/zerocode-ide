# jev-seat-replay — 한 원장 사본 위에서 zo 두 개의 좌석 판정을 한 표로 (t-9427)

좌석 판정을 바꾸는 과업의 전/후 표만 만든다. 채점·판정은 하나도 하지 않는다 — 숫자는 시험할 zo 바이너리의
`zo jev summary --json` 이 사본을 읽고 낸 것이라, 같은 사본 위의 두 바이너리는 바뀐 것만큼만 다르다. 라벨 규칙을
새로 매겨 보는 일은 `tools/label-audit`(배포되는 라벨 함수로 원장을 다시 채점), 한 좌석의 문답 재생은
`tools/{notify,summon,routing,command-guard}-replay` 몫이다.

**Jev 요청 0건, 사람의 원장 쓰기 0건.** 사본은 읽기 전용이고, zo는 `ZO_CONFIG_HOME`·`ZO_HOME`·`HOME` 을 사람의
홈 밖으로 돌린 채 돈다.

| 조각 | 무엇 |
| --- | --- |
| `replay.py snapshot` | 사용 표가 이름 붙인 좌석 원장(이름은 바이너리에게 묻는다 — 빈 홈에서 `zo jev summary --json` 이 좌석마다 `ledger` 를 말한다)을 `<zo 홈>/jev/` 와 각 프로젝트의 `projects/<slug>/state/smart-router/` 에서, 설정 파일은 `smart` 블록만 `<out>/.zo` 로 복사하고 쓰기를 막는다 |
| `replay.py table` | 바이너리마다 사본으로 요약을 돌려 좌석·바이너리별로 서 있는 자리·판정(줄)·창·호출·p50/p95·표식(하한)·비교 안 됨(이유별, t-9556)·기준선·주간 답·다음 판정까지를 마크다운 표 하나로, 그 아래 **적용선 표**(t-9468): 좌석의 bands가 고정한 선(전)과 라벨이 그은 선(후, 없으면 이유)에서 각각 적용 몫·적용분 오류율·기준선 오류율·남긴 몫의 오류율, 그리고 제품이 읽은 표의 선 |
| `replay.py thresholds` | 바이너리에게 좌석마다 라벨이 그은 선을 물어(`calibration.row`) 그 좌석 원장 옆의 표 파일(이름은 바이너리가 `thresholdsFile`로 말한다)에 둘 행을 보인다. **`--write`일 때만** 쓰고, 임시 파일을 옆에 쓴 뒤 rename, `--home` 밖에는 쓰지 않는다 — 사본에 돌리면 사본 안에만 쓴다(읽기 전용 폴더는 쓰는 동안만 열고 다시 닫는다) |
| `tools/tests/test_jev_seat_replay.py` | 사본이 이름 붙인 원장과 `smart` 블록만 가져가는지, 사람의 파일을 안 바꾸는지, 사본이 쓰기 불가인지, 프로젝트 폴더를 zo의 slug 규칙으로만 찾는지, 표가 바이너리의 판정·이유별 비교 안 됨·적용선 숫자를 그대로 싣는지, 표 파일이 원장 옆에만·홈 안에만·rename으로 쓰이는지 (`just tools-test`) |

## 돌리기

```sh
# 전·후 바이너리를 만든 뒤(각 체크아웃의 zo-ide 에서 cargo build -p zo-ide --bin zo)
python3 tools/jev-seat-replay/replay.py snapshot --zo <before-zo> \
    --project <프로젝트 경로> --out <사본 폴더>
python3 tools/jev-seat-replay/replay.py table --snapshot <사본 폴더> --project <프로젝트 경로> \
    --zo before=<before-zo> --zo after=<after-zo> [--seat recall --seat placement ...] [--json <out.json>]
```

| 손잡이 | 무엇 |
| --- | --- |
| `--zo-home` (snapshot) | 읽을 zo 홈(기본 `~/.zo`) |
| `--project` | 프로젝트 경로. snapshot 은 여러 번 줄 수 있고, 폴더는 zo의 slug(경로의 끝 80자 + 16자리 해시)로 하나만 찾는다 — 둘이거나 없으면 거절 |
| `--seat` (table) | 표에 넣을 좌석(기본: 어느 바이너리든 원장을 찾은 좌석) |
| `--json` (table) | 표의 행을 JSON 으로도 |

선을 제품이 읽게 하려면(사본에서 먼저 보고, 사람의 홈에는 그 뒤에 명시적으로):

```sh
python3 tools/jev-seat-replay/replay.py thresholds --zo <after-zo> --home <사본 폴더>/.zo --project <프로젝트 경로>          # 볼 뿐
python3 tools/jev-seat-replay/replay.py thresholds --zo <after-zo> --home <사본 폴더>/.zo --project <프로젝트 경로> --write  # 사본 안에 쓴다
```

## 읽는 법

- 판정·창·표식 숫자는 행의 개수라 시계와 무관하다. 주간 답 수는 명령을 돌린 시각 기준이다.
- 적용선 표의 「전」은 좌석 bands의 고정 선(`act_from_permille`)이고, 알림·배치·멈춤·소환의 단계는 표가 없으면 이 선을 읽지 않고 답마다 적용한다(표가 생기기 전 동작 그대로). 「후」의 선은 라벨이 이 사본에서 그은 것이고, 제품은 `thresholds`가 원장 옆에 쓴 행을 — 그 좌석이 지금 묻는 루브릭의 행만 — 읽는다.
- 좌석의 루브릭(말 또는 라벨)이 바뀐 바이너리는 사본의 옛 루브릭 행을 읽지 않는다(t-6877). 그 좌석의 「후」가 `too_few_rows 0/n`
  이면 새 루브릭 행이 아직 없다는 뜻이고, 새 라벨이 옛 행에 무엇이라 했을지는 `tools/label-audit` 가 잰다.
