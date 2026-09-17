# 터미널 충실도 자 (ZeroCode 판 ↔ Terminal.app)

「같은 화면이 Terminal.app보다 색이 다르고 흐리거나 깨져 보인다」를 숫자로 가르는 자.
같은 바이트를 두 터미널에 띄우고, 창 단위 캡처를 셀 좌표로 잘라 잰다.

## 한 곳씩

| 파일 | 하는 일 |
|---|---|
| `layout.py` | 픽스처 **한 표**. 터미널에 보낼 바이트(`fixture_bytes`)와 분석기가 읽는 탐침(`probes`)이 같은 걸음에서 나온다. 96×22(Terminal.app 기본 120×30, 창이 워커 탭에서 가르는 판 96×24에 둘 다 들어감), 절대 위치만(개행 없음 → 스크롤 없음) |
| `show.sh` | 이 터미널에 픽스처를 띄우고 Enter까지 붙잡는다. 그 전에 `env-<이름>.txt`(크기·TERM·COLORTERM·TERM_PROGRAM)를 적는다 |
| `fidelity.py` | `fixture`·`palette-zerocode`·`palette-terminal-app`·`analyze`·`synth` |
| `probe_page.py` | 픽스처를 창의 터미널 DOM(`section.terminal > pre.term > div.term-row > span`, 렌더러의 클래스 이름)으로, `ui/tokens.css`·`ui/shell.css`를 링크해서. 변형 표 `VARIANTS` |
| `Probe.swift` | 창과 같은 호스팅(투명 창·HUD 재질·`drawsBackground` false)의 WKWebView로 프로브 페이지 한 장을 띄워 `screencapture -l`로 찍고 끝난다 |

## 재는 것

- **격자**: 첫·끝 행의 자홍/초록 교대 배경, 첫·끝 열의 파랑/노랑 교대 배경(xterm 큐브 모서리 — 어느 256색 팔레트에서도 0/255)으로 열·행 경계를 기기 픽셀 소수점까지. 셀 폭·높이의 평균·소수부·정수 패턴.
- **색**: 캡처를 제 ICC 프로필에서 Display P3로(ColorSync 프로필), 기대색은 그 터미널이 정의하는 공간에서(CSS·xterm = sRGB, Terminal.app ANSI = device RGB → 캡처의 디스플레이 프로필, Terminal.app 글자·배경 = Generic RGB) P3로 옮겨 CIEDE2000. 행: ANSI 16 배경·글자, xterm 16–255, Claude Code 다크 테마 16색의 truecolor 길과 256색 길(chalk `rgbToAnsi256`), 회색 램프.
- **글리프**: 흰 글자/검은 바탕(`38;5;231;48;5;16` — 테마와 무관)과 기본색에서 `|`의 줄기 폭(주사선마다 커버리지 합), `-`의 막대 두께, `H`의 잉크 양(em²), 가장자리 부분 픽셀 비율(0.1–0.9), 굵게.
- **선**: 긴 세로 `│` 9행과 표 왼쪽 선의 끊긴 주사선 수, 둥근 상자 윗선의 끊긴 열 수.
- **정렬**: 한글 줄·대체 글리프 줄(SF Mono에 없는 ⏺✻❯⎿⠋✶ …)의 `|`가 ASCII 기준 줄보다 밀린 셀 수.
- **이음매**: 같은 배경 두 행 사이 주사선의 ΔE00 > 2.3.
- **dim·bold**: dim 휘도비, bold 색 변화.

## 분석기 자기 검증

```sh
PY="uv run --offline --no-project --python 3.12 --with numpy --with pillow python"
python3 tools/terminal-fidelity/fidelity.py palette-zerocode > output/terminal-fidelity/palette-zerocode.json
$PY tools/terminal-fidelity/fidelity.py synth --out output/terminal-fidelity/selftest/synth.png
$PY tools/terminal-fidelity/fidelity.py analyze output/terminal-fidelity/selftest/manifest.json --out output/terminal-fidelity/selftest
```

합성 캡처(정수 격자·sRGB)는 ΔE00 0.00·정렬 0셀이어야 한다. 결함을 넣은 사본(색 +12, 2 px 이음매,
5 px 밀림)은 ΔE00 중앙 2.35·이음매 2 주사선·0.263셀(= 5/19)로 되찾아야 한다(2026-09-17 확인).

## GUI 단계 (사람 화면을 움직인다 — 허가 뒤에만)

1. 창: Computer Use는 창 자신을 누르지 않으므로(`app_blocked`) 워커 탭에서
   `tmux split-window -v -P -F '#{pane_id}' -- sh <워크트리>/tools/terminal-fidelity/show.sh zerocode-15 <out>`로
   판을 열고 `tmux select-pane -t <id>`. 창 캡처는 앞 탭만 찍으므로 사람이 그 탭을 앞에 둘 때
   `screencapture -x -o -l <창 id>`(앞에 오는 순간은 캡처를 반복해 표식이 보일 때 잡는다). 끝나면 `tmux kill-pane`.
2. Terminal.app: `open -a Terminal <show.sh를 부르는 .command>`, 캡처; `zerocode-computer hotkey --app com.apple.Terminal --key cmd+=` ×3으로 15 pt에서 다시(창만 바뀌고 프로필은 그대로). Return으로 끝내고 `quit`.
   `env-terminal-app-12.txt`의 `COLORTERM`이 Claude Code가 256색으로 도는지를 말한다
   (`output/terminal-fidelity/evidence/claude-code-apple-terminal-256.txt`).
3. 프로브: `swiftc -O tools/terminal-fidelity/Probe.swift -o <scratch>/terminal-probe`, 변형마다
   `probe_page.py --variant <v>` → `terminal-probe <page> <워크트리> <png>`(키 초점을 가져가지 않는 창 하나, 약 2초).
   변형 없는 `inherit`이 실제 창 캡처와 같은 숫자를 내야 나머지 변형이 뜻을 가진다.
4. 매니페스트(`{"captures":[{name,png,scale,font_px,palette}], "pairs":[[a,b]]}`)로 `analyze`.
