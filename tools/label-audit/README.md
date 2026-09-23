# label-audit — 모든 Jev 자리의 라벨을 새 규칙으로 다시 매기는 하네스 (t-6342)

두 조각이다. `seed.py` 는 **모으기만** 하고, 판정은 하나도 하지 않는다. 표식은
원장이 이미 적어 둔 `agreed`(옛 규칙)와, 하네스가 행의 사실을 배포되는 함수에 그대로
넘겨 얻은 새 표식이다 — `stall_cause::mark`·`worker_placement::mark`·
`notify_call::agreed`·`rerank_shadow::mark`·`step_effort::move_mark`·
`orchestration::runs_model`/`native_agent`, 기준선은 각 자리의 `baseline_mark`,
확신도 구간은 `summary::band_tally`·`confidence_curve`. 파이썬에 규칙 사본이 생기면
재는 규칙과 나가는 규칙이 갈라진다(중복 금지).

**새 Jev 요청은 0건이다.** 원장을 다시 읽을 뿐, 아무것도 다시 묻지 않는다.

| 조각 | 무엇 |
| --- | --- |
| `tools/label-audit/seed.py` | 창 자리 원장(`<zo 홈>/jev/*.jsonl`)과 zo 자리 원장(모든 프로젝트의 `state/smart-router/*.jsonl`)을 두 묶음으로, 창 블랙박스(`window-errors.log(.1)`)의 무대 선언과 워커 터미널을 숫자로 줄여 씨앗 하나로 |
| `zo-ide/crates/tools/src/misc_tools/smart_router/jev_summary/label_audit_tests.rs` | 씨앗의 행마다 새 규칙으로 다시 매겨 자리별 표(전/후 일치·하한·비교 안 됨·기준선·향상·불일치 수·요청 수 전/후)를 찍는 `#[ignore]` 실측, 그리고 작은 고정 씨앗으로 같은 셈을 지키는 시험 |
| `tools/tests/test_label_audit_seed.py` | 씨앗이 두 집의 원장을 섞지 않는지, 블랙박스를 숫자로만 줄이는지 (`just tools-test`) |

## 돌리기

```sh
python3 tools/label-audit/seed.py --out /tmp/label-audit/seed.json

cd zo-ide
ZEROCODE_LABEL_AUDIT_SEED=/tmp/label-audit/seed.json \
ZEROCODE_LABEL_AUDIT_OUT=/tmp/label-audit/audit.json \
  cargo test -p tools --lib the_labels_this_machine_holds_graded_again \
  -- --ignored --nocapture
```

| 손잡이 | 무엇 |
| --- | --- |
| `--zo-home` (seed) | zo 홈(기본 `~/.zo`) |
| `--black-box` (seed, 여러 번) | 블랙박스 파일(기본 창 지원 폴더의 `window-errors.log.1`·`window-errors.log`) |
| `ZEROCODE_LABEL_AUDIT_OUT` | 자리별 숫자를 JSON으로(곡선·구간 포함) |

## 이 하네스가 지키는 것

1. **이름 하나로 자리를 정하지 않는다.** 창 자리와 zo 자리는 다른 집에 원장을 둔다. zo의
   걸음 사고 깊이는 제 원장을 갖기 전에 창 사고 깊이 자리의 이름(`step-effort.jsonl`)으로
   적었다. 그래서 씨앗은 두 묶음이고, 하네스는 zo 기록자가 파일을 두는 자리(각 기록자의
   `*_FILE` 이 그 자리의 `ledger`)만 프로젝트 묶음에서 읽는다.
2. **요청은 늘지 않는다.** 자리마다 「새 규칙이면 같은 사건에서 보냈을 요청 수」가 원장의
   수보다 크면 실측이 실패한다.
3. **모르는 것은 모른다고 센다.** 옛 배치 라벨에는 「봤나」가 없다. 블랙박스의 무대 선언으로
   되살릴 수 있는 행만 되살리고(초점은 블랙박스에 없어 무대 절반만 본다), 나머지는
   `sight_unknown` 으로 비교에서 뺀다. 방 고침만의 효과는 「모두 봤다고 칠 때」로 따로 적는다.
4. **추정은 추정으로 적는다.** 후보를 좁힌 소환의 새 답은 다시 물어야 안다. 하네스는 옛 답의
   확률을 남은 후보 안에서 견준 기울기만 따로 적고, 표식에는 넣지 않는다.

## 읽지 않는 것

씨앗에는 원장 행(식별자·수·지문·답)과 블랙박스의 시각·터미널 번호만 들어간다. 사람의 글,
알림 본문, 화면 내용은 원장에 없고 씨앗에도 없다.
