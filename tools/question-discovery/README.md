# question-discovery — 판단 질문을 라벨 있는 원장으로 자동으로 찾는 하네스 (t-6349)

세 조각이다. `seed.py` 는 **세기만** 하고, `loop.py` 는 **제안·적합·판정**만 하며, 묻는 일은 Rust 쪽 묻기 단계 하나가 한다.
행이 무엇인지·상태가 무엇을 싣는지·라벨이 무엇을 말하는지는 자리마다 배포되는 함수의 것이다 — 패치 검토는
`runtime::patch_review::ask_reading`·`state`·`hindsight_of_turn`(`replay_support::replay_points`), 알림은
`zerocode_core::notify_call::ask`·`agreed` — 그리고 요청은 그 자리의 `JEV_USES` 행(`PATCH_REVIEW`·`NOTIFY`)으로 문을
지난다. 파이썬에 규칙 사본이 생기면 재는 규칙과 나가는 규칙이 갈라진다(중복 금지). 어느 원장이 어느 자리인지는
`tools/label-audit/seed.py` 의 읽기를 불러다 쓴다.

| 조각 | 무엇 |
| --- | --- |
| `tools/question-discovery/seed.py` | 후보 표: 계획의 자리 여섯(패치 검토·알림·기억 검색·워커 배치·소환·라우팅)마다 원장 파일·행 수·`agreed` 표식 수·라벨 출처·상태를 되살릴 수 있는지, 그리고 넘겨준 원천 씨앗의 크기 |
| `zo-ide/crates/tools/src/misc_tools/smart_router/question_discovery.rs` 의 `the_questions_of_a_round_asked_of_this_machines_rows` | 라운드 파일이 적은 질문 전부를 **행마다 요청 하나**로 자리의 문을 지나 실제 엔드포인트에 묻고, 답을 숫자로만 라벨·묶음·기준선 옆에 적는 `#[ignore]` 묻기 단계. 빈 질문 목록이면 아무것도 보내지 않고 행만 적는다 |
| `tools/question-discovery/loop.py` | 루프: manifest → 시간순 분할(묶음 경계) → 0라운드(자리의 질문) → 제안자(zo 헤드리스의 프런티어 모델 또는 파일) → 로지스틱 k-겹 → 한 번의 보류 판정 |
| `tools/tests/test_question_discovery.py` | 루프가 보류 라벨을 읽지 않는지, 고치기·빼기는 개발 오차가 내릴 때만 받는지, 라운드의 새 질문이 한 호출에 실리는지, 판정이 한 번뿐인지, 캐시가 재과금을 막는지, 씨앗이 글을 안 읽는지 (`just tools-test`) |

## 돌리기

```sh
python3 tools/patch-review-replay/seed.py --out /tmp/patch-review-replay/seed.json
python3 tools/notify-replay/seed.py --days 30 --out /tmp/notify-replay/seed.json
python3 tools/question-discovery/seed.py --patch-seed /tmp/patch-review-replay/seed.json \
    --notify-seed /tmp/notify-replay/seed.json --out /tmp/question-discovery/candidates.json

# 마른 실행: 행을 세고 manifest만 쓴다 — 키 없이, 요청 0
python3 tools/question-discovery/loop.py run --seat notify --source /tmp/notify-replay/seed.json \
    --sample 50 --workdir /tmp/question-discovery/notify --dry-run

# 유료 실행: 키는 이 명령의 환경에만
TYPESAFE_API_KEY="$(security find-generic-password \
    -s dev.zerocode.key.TYPESAFE_API_KEY -a "$(id -un)" -w)" \
  python3 tools/question-discovery/loop.py run --seat notify --source /tmp/notify-replay/seed.json \
    --sample 50 --workdir /tmp/question-discovery/notify --rounds 1 --cap 300 --proposer zo
```

| 손잡이 | 무엇 |
| --- | --- |
| `--seat` | `patch_review` 또는 `notify` — 판정 시각의 상태를 되살릴 수 있는 두 자리 |
| `--sample` | 라벨 있는 행 중 이만큼을 시간 위에 고르게(머리가 아니라) |
| `--rounds` | 0라운드 뒤 제안 라운드 수의 상한; 개발 오차가 내리지 않는 라운드에서 멈춘다 |
| `--cap` | **전체 실행**에서 예약할 Jev 요청 수(기본 300). 실패·중단으로 결과를 모르는 요청도 센다 |
| `--spend-cap-usd` | 전체 실행의 Jev 입력 비용 상한(기본 $4). 제안자는 `--rounds` 횟수로 제한하며 zo 세션 사용량으로 달러를 별도 추정한다. zo에는 사전 달러 제한 손잡이가 없어 총 달러를 하드 캡으로 보장할 수는 없다 |
| `--proposer` | `zo`(기본, `zo -p --json --model <M> --no-spawn --permission-mode read-only --allowed-tools TodoWrite`, 빈 방 폴더에서 — 파일도 명령도 못 읽는다) · `file:<path>,…`(라운드마다 파일 하나) · `none` |
| `--model` | 제안자의 모델(기본 `claude-opus-5-5`) |
| `--dry-run` | 행 나열과 manifest까지; 요청 0, 키 불필요 |

`--source`와 `--workdir`은 저장소 밖 경로여야 한다. 제안자 zo의 `ZO_CONFIG_HOME`도 작업 폴더 안의 임시 홈으로 강제하고, Rust Jev 문은 자체 임시 홈을 쓴다. 작업 폴더에는 비공개 상태·프롬프트와 zo의 인증 캐시가 있을 수 있으므로 공유하거나 커밋하지 않는다.

작업 폴더에 남는 것: `manifest.json`(대상·라벨 정의/버전·join·수·purge 수·전체 상한·분할 지문·판정 가능 여부), `round-<n>.json`·`rows-<n>.jsonl`
(라운드마다 물은 것과 답), `cache-<identity>.jsonl`(행×질문 캐시 — identity는 자리·원천·질문·모델, 행 목록은 아님), `states.jsonl`(제안자 눈에만 —
문이 통과시킨 상태 그대로, 스크래치), `prompt-<n>.txt`·`proposal-<n>.txt`·`proposer-room-<n>/`(빈 방), `freeze.json`(판정 직전 굳힌 후보·질문·
릿지·분할 지문·seed·`finalEvaluated`), `result.json`, `asker.log`.

## 이 하네스가 지키는 것

1. **manifest가 돈보다 먼저다.** 빈 라운드로 행을 나열해(요청 0) 표본·양성·음성·묶음·분할 지문·상한을 적은 뒤에야 0라운드가 나간다.
   보류에 어느 class 든 `JUDGEABLE_PER_CLASS`(3) 미만이면 판정은 `not_evaluable` 이지 AUC 0/1 이 아니다. 합성 픽스처(시험의 씨앗)는 실측이 아니다.
2. **시간·묶음 누수가 없다.** 판단 시각 뒤의 것은 라벨뿐이다(자리의 함수가 그렇게 짠다). 분할은 시간순 70/30이되 묶음(한 전사의 패치, 한
   판의 알림) 경계에서 자르고, 개발 세트가 가진 묶음 또는 **문이 가린 뒤 동일한 상태 내용**의 뒤 행은 보류에서 **purge** 한다(manifest의 `heldPurged`). k-겹도 이 연결 묶음 단위로 나누고,
   표준화와 릿지 세기(`RIDGES` 격자)는 학습 fold·개발 세트로만 고른다. 제안자가 보는 「가장 틀린·가장 맞은」 행은 개발 세트에서만 고르고,
   제안자는 빈 방에서 도구 없이 답한다 — 첫 파일럿에서 작업 폴더를 읽던 세션은 보류 라벨이 든 행 파일을 열었기에 죽였다.
3. **보류 라벨은 봉인이다.** `HeldOut` 은 판정이 `unseal` 하기 전에 라벨을 내주지 않는다(읽으면 `SealedLabel`). 개발 오차가 나빠진 라운드는 질문 조합에서 되돌린다. 판정은 한 번이고
   `freeze.json` 에 굳힌 후보로 하며, 두 번째 판정과 판정된 폴더의 재실행은 `AlreadyJudged` 다. 최종 점수를 보고 후보를 바꿀 길이 없다.
4. **기준선은 같은 행에서 다시 잰다.** 늘 같은 답(개발 다수 라벨)·오늘 규칙(알림: 늘 울림)·자리의 질문(0라운드 답 위의 로지스틱)·자리 자신의
   판정(`shippedPredicts` — 자리의 리더가 읽은 허용/울림)·크기(패치 바이트·바뀐 줄·대기 판 수)를 보류 행 위에서 AUC·부트스트랩 95% 구간·
   일치율·Wilson 하한으로, 그리고 탐색 대비 **짝 지은 차이와 그 구간**으로 낸다. 옛 수치(54.7%·0.688·≤0.59)는 대입하지 않는다. 「발견 없음」도 정상 산출이다.
5. **연구 산출물까지만.** production 질문·설정·승격 원장은 건드리지 않는다. 넘는 질문이 나와도 카탈로그 2판은 별도 검토 뒤의 제안이다.
   실호출은 기존 문·동의·가림을 쓰고(자리의 행 그대로), 임시 홈에서 나가며, 사람의 원장·하루 셈은 움직이지 않는다. 답은 행마다 (자리·원천·질문·모델)의
   identity(원천 파일 SHA-256 포함) 로 캐시돼 — 다시 돌리든, 같은 manifest에서 행의 부분집합만 다시 쓰든 — 같은 행에 같은 말을 두 번 치르지 않는다. 묻기 직전 예약을 fsync하고 응답을 못 남긴 행은 과금 여부가 불확실하므로 재시작 때 멈춘다. 실패·재시도·제안자 토큰까지
   `result.json` 에 센다.

## 씨앗·행 파일이 읽지 않는 것

`seed.py` 는 원장 행의 `agreed` 와 개수, 원천 씨앗의 크기만 센다. 묻기 단계의 행 파일은 id·시각·묶음 지문·라벨·기준선 숫자·답 숫자·결과 낱말·토큰·벽뿐이다 —
사람의 글도 판 이름도 없다(시험 `the_rows_file_carries_no_words…`). 상태 글은 `states.jsonl` 한 곳에만, 문이 이미 자르고 가린 그대로, 제안자를 위해 남고 깃에 들어가지 않는다.
