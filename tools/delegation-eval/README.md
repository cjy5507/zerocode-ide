# delegation-eval — 직접 실행 대 선택 위임 (t-73910 A17·A24)

같은 과제를 두 방식으로 돌려, 위임이 단일 워커보다 나은지 숫자로 판정하는 하네스입니다.

| 방식 | 브리핑 |
|---|---|
| `direct` | 한 워커가 끝까지 직접. 다른 에이전트를 부르지 않음 |
| `delegate` | 같은 워커가, 나누는 것이 도움이 될 때만 zerocode-orc로 워커를 부름 |

두 방식의 에이전트·모델·effort는 같습니다(`plan`이 한 값을 둘 다에 씀). 실행 순서는 `--seed`로 섞습니다.

## 이 하네스는 아무것도 띄우지 않습니다

`plan`은 실행 목록과 브리핑을 쓰고, 코디가 칠 `zerocode-orc` 줄을 출력할 뿐입니다. 유료 실행은 사람이
모델·effort·한도를 정한 뒤 코디가 시작합니다. 시험 `tools/tests/test_delegation_eval.py`가 이것을 확인합니다
(가짜 `zerocode-orc`가 한 번도 불리지 않음).

```sh
python3 tools/delegation-eval/eval.py plan --reps 3 --seed 1 --agent claude --model <id> --effort <단> --out <폴더>
python3 tools/delegation-eval/eval.py prepare --out <폴더>        # 실행마다 seed의 새 git 체크아웃
# … 코디가 출력된 줄로 과업을 만들고 워커를 띄움, 끝나면 검증 기록 …
python3 tools/delegation-eval/eval.py accept --out <폴더>         # 인수 명령 + 허용 밖 파일
python3 tools/delegation-eval/eval.py collect --out <폴더> --run <실행> --task <t-…> [--task …]   # 읽기만
python3 tools/delegation-eval/eval.py summarize --out <폴더>
```

## 표가 읽는 것과, 없을 때 말하는 것

| 값 | 어디서 | 없으면 |
|---|---|---|
| 통과 | `accept.json`: rc 0이고 `files` 밖 변경 없음 | 통과 아님. 계획한 실행은 모두 분모에 듦 |
| 벽시계 | `dispatch-*.json`: 첫 시도 시작 → 마지막 검증(`afterReport.verifiedMs`) | 미확인 |
| 재작업 | 같은 곳의 `attempts` 중 첫 번째 뒤의 것 | 미확인 |
| 동시 최대 | 과업마다 마지막 시도의 시작·끝 겹침 | 미확인 |
| 토큰·달러 | `cost-*.json`(창이 내는 TaskCost). 기록이 다 있고 대화가 다 이어졌을 때만 토큰, `usd`가 있을 때만 달러 | 미확인과 그 이유. 0으로 적지 않음 |
| 사람 개입 | 원장에 칸이 없음 | 늘 미확인 |

과업 하나의 비용을 꺼내는 CLI가 아직 없어, `cost-*.json`은 지금 손으로 받아 둡니다. 없으면 달러·토큰은 미확인입니다.

## 과제

`scenarios.json`. 지금은 1×1 표본용 과제 하나(`s01-small-fix`, `seed/s01-small-fix`)만 있습니다. 이 seed는 정말로
인수 시험에 떨어지고, 고치면 붙습니다(시험이 확인). 10×3 평가의 과제 열 개(작은 수정 3, 여러 파일 강한 의존 2,
독립 변경 2, 독립 검증 1, 재시작·알림·복구 2)는 사람 확인 뒤 같은 모양으로 더합니다.

## 사람에게 걸 결정 (코디가 gate-create로)

> 직접 실행과 선택 위임을 비교하는 평가를 돌려도 될까요? 같은 과제 10개를 두 방식으로 3번씩, 모두 60번 실행합니다.
> 모델과 effort는 두 방식이 같습니다.
> 정해 주실 것: (1) 모델·effort, (2) 쓸 수 있는 한도 — 토큰 또는 금액, 그리고 주간 쿼터 몇 %까지,
> (3) 언제 돌릴지(창 실측·착지 줄과 겹치지 않는 시간).
> 결과는 과제별 전후, 중앙값과 가장 느린 값, 실패 과제를 포함한 비용으로 드립니다. 가격을 모르는 모델은 「비용 미확인」으로 적습니다.
> 선택지: 지금 정한 값으로 진행 / 1×1 표본 결과를 보고 다시 정하기 / 이번에는 하지 않기.

1×1 표본(과제 하나를 두 방식으로 한 번씩)도 유료 실행 두 번이라, 돌리기 전에 코디에게 한 번 묻습니다.
