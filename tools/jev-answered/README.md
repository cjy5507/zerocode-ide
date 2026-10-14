# jev-answered — 좌석마다 판정 요청이 답을 받은 비율을 공급자 폴더와 시간대별로 (t-55423)

판정 공급자를 고치는 과업의 전/후 표를 만든다. 원장을 읽기만 하고, 한 줄에서 꺼내는 것은 좌석(원장 파일 이름), 공급자
폴더, 시각(`at`), 결과 단어(`outcome`)뿐이다. 결과가 없는 줄(라벨, 나중에 덧붙인 기록)은 요청으로 세지 않는다.
단어 모양이 아닌 결과 값은 `outcome_not_a_word` 하나로 세어, 행의 다른 글이 표에 실리지 않게 한다.

**Jev 요청 0건, 홈 쓰기 0건.** 쓰는 파일은 `--json` 이 이름 붙인 것 하나뿐이다.

| 조각 | 무엇 |
| --- | --- |
| `answered.py` | `<zo 홈>/jev/` 와 각 프로젝트의 `projects/<slug>/state/smart-router/` 에서, 폴더 자체(Jev 공급자, 표에는 `(jev)`)와 `providers/<이름>/` 마다 원장을 읽어 좌석·자리(창/프로젝트 합계)·공급자 폴더·시간대별로 요청 수, 답한 수, 답한 비율, 실패 단어(많은 순)를 마크다운 표 하나로 낸다. 시각을 읽을 수 없는 줄은 표 아래에 좌석별로 센다 |
| `tools/tests/test_jev_answered.py` | 결과가 있는 줄만 세는지, 폴더·시간대별로 나누는지, 결과 단어 말고는 아무것도 싣지 않는지, 홈을 그대로 두는지, 키와 폴더 이름이 Rust와 같은지 (`just tools-test`) |

폴더 이름(`jev`, `state/smart-router`, `providers`)은 `tools/jev-seat-replay/replay.py` 의 상수를 그대로 쓴다.

## 돌리기

```sh
python3 tools/jev-answered/answered.py \
    --window "before=2026-10-14T19:10..2026-10-15T07:10" \
    --window "after=2026-10-15T09:30..2026-10-15T11:30" \
    [--project <프로젝트 경로>] [--home <zo 홈>] [--json <out.json>]
```

| 손잡이 | 무엇 |
| --- | --- |
| `--window` | `이름=시작..끝`. 시각은 로컬 `YYYY-MM-DDTHH:MM` 이나 epoch 밀리초, 비운 끝은 열림. 시작은 넣고 끝은 뺀다. 없으면 `all` 하나 |
| `--project` | 읽을 프로젝트(여러 번 가능). zo 의 slug 규칙으로 폴더 하나를 찾고, 없거나 둘이면 거절한다. 없으면 모든 프로젝트를 합친다 |
| `--home` | 읽을 zo 홈(기본 `~/.zo`) |
| `--json` | 표의 행을 JSON 으로도 |
