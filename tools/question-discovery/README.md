# question-discovery — 판단 질문을 라벨 있는 원장으로 자동으로 찾는 하네스 (t-6349)

## 검토한 Jev 응답으로 질문 비교하기

`review.py`는 `zo jev export`의 입력·응답과 명시적인 검토 결과를 읽는다. 같은 입력은 작업 공간이나 질문 문구가 달라도 개발/보류 양쪽에 놓지 않는다. 한 실행은 기능·루브릭·응답 모델·검토 출처를 하나씩 고정한다. 개발용 파일에만 정답을 내주고, 최종 비교는 후보를 고정한 뒤 한 번만 연다. 같은 입력의 복수 응답으로 좋은 결과를 고르는 후보 파일은 거절한다.

```sh
zo jev export --seat compaction --json > /tmp/jev-reviewed.json
python3 tools/question-discovery/review.py prepare /tmp/jev-reviewed.json /tmp/jev-study \
  --seat compaction --rubric 2 --model jev-1.13.0 --reviewer human
```

`development.json`을 보고 후보 파일을 작성한다. 후보는 질문의 시작 문구만 바꾸며 타입·선택지·입력은 그대로 비교한다. 예를 들어 `/tmp/jev-candidate.json`은 아래 모양이다. `from`은 실제 개발 사례의 질문에 있는 문구여야 한다.

```json
{
  "schemaVersion": 1,
  "seat": "compaction",
  "baseRubricVersion": 2,
  "rubricVersion": 3,
  "model": "jev-1.13.0",
  "rewrites": [{"from": "The conversation needs a smaller context.", "to": "Judge which original tool results the remaining task still requires."}]
}
```

```sh
python3 tools/question-discovery/review.py requests /tmp/jev-study \
  --candidate /tmp/jev-candidate.json > /tmp/jev-development-requests.json
```

요청 파일을 실행하는 단계는 기존 Jev 설정·작업 공간 동의·`smart.jev.labelDrafts`가 허용할 때만 보낸다. 출력 홈은 새 경로여야 한다. 예시는 개발 요청이며, **유료 API 요청**이 나간다. 키는 기존 설정으로 찾는다. 에이전트를 새로 실행하지 않는다.

```sh
ZEROCODE_JEV_REVIEW_PLAN=/tmp/jev-development-requests.json \
ZEROCODE_JEV_REVIEW_OUTPUT=/tmp/jev-development-records \
cargo test --manifest-path zo-ide/Cargo.toml -p tools --lib \
  review_replay::replay_frozen_review_requests -- --ignored --nocapture --test-threads=1
```

기본 한도는 요청 32회·예상 API 비용 $4이다. `ZEROCODE_JEV_REVIEW_CAP`(최대 300)과 `ZEROCODE_JEV_REVIEW_SPEND_USD`(최대 $4)로 낮추거나 요청 수를 지정한다. 호출당 재시도는 없고, 비용이 미확정이거나 예약량을 넘으면 다음 요청을 중지한다. 가격의 발표일·사용량·예상 비용은 출력 홈의 `replay.jsonl`에 남는다. 이 비용은 연구 실행의 영수증이며 운영 기능의 승격 원장에 섞지 않는다.

기존 검토 CLI를 출력 홈에 대면 재생한 사례를 읽고 결과를 기록할 수 있다. `outcome`의 `--reviewer`는 실제 근거의 출처로 지정한다. 재생 단계는 정답을 만들지 않는다.

```sh
ZO_CONFIG_HOME=/tmp/jev-development-records zo jev review --json
# 각 사례를 확인하고 zo jev outcome <case-id> --question <id> --correct ... --reviewer ... --note ... 로 기록한다.
ZO_CONFIG_HOME=/tmp/jev-development-records zo jev export --json > /tmp/jev-development-results.json
python3 tools/question-discovery/review.py evaluate /tmp/jev-study /tmp/jev-development-results.json \
  --development --candidate /tmp/jev-candidate.json
python3 tools/question-discovery/review.py freeze /tmp/jev-study --candidate /tmp/jev-candidate.json
python3 tools/question-discovery/review.py requests /tmp/jev-study --heldout > /tmp/jev-heldout-requests.json
```

보류 요청도 **별도의 새 출력 홈**으로 재생·검토·내보낸 뒤 `evaluate`를 `--development` 없이 실행한다. 고정 전 응답, 누락된 검토, 바뀐 원본·모델·후보는 최종 점수로 받지 않는다. 같은 보류 사례를 다른 연구에서 다시 쓰면 새로운 독립 증거가 아니다.

```sh
python3 tools/question-discovery/review.py evaluate /tmp/jev-study /tmp/jev-heldout-results.json
python3 tools/question-discovery/review.py result /tmp/jev-study
```

`ready_for_code_review`는 검토한 사례에서 입력 묶음별 짝 비교가 개선을 지지했다는 뜻이다. 운영 적용·전체 트래픽 정확도·시간이나 비용 절감을 뜻하지 않는다. 운영 질문을 바꿀 때는 고정된 문구와 다음 루브릭 버전을 코드에 반영하고 기능 검사와 시간·비용 비교를 함께 통과시킨다. 되돌릴 때도 기존 루브릭 판정 기록과 버전 경계를 유지한다.

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
| `tools/tests/test_question_discovery.py` | 루프가 보류 라벨을 읽지 않는지, 고치기·빼기는 개발 오차가 내릴 때만 받는지, 라운드의 새 질문이 한 호출에 실리는지, 판정이 한 번뿐인지, 캐시가 재과금을 막는지, 청구가 불확실하면 상한 유무와 상관없이 다음 구매·판정이 라벨 개봉 전에 멈추는지, 직접 부른 판정도 `run()`과 같은 평가 자격을 지키는지, 씨앗이 글을 안 읽는지 (`just tools-test`) |

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
| `--seat` | `notify`는 시간순 평가 가능. `patch_review`는 실제 판정 시각이 없어 목록만 열거하며 유료·평가 경로는 막는다 |
| `--sample` | 라벨 있는 행 중 이만큼을 시간 위에 고르게(머리가 아니라) |
| `--rounds` | 0라운드 뒤 제안 라운드 수의 상한; 개발 오차가 내리지 않는 라운드에서 멈춘다 |
| `--cap` | **전체 실행**의 wire 송신 상한(기본 `REQUEST_CAP` 300). 묻기 단계는 재시도를 끄고 직렬 송신하며, 실패·중단으로 결과를 모르는 요청도 센다 |
| `--spend-cap-usd` | **전체 실행**의 Jev 달러 상한(기본 `SPEND_CAP_USD` $4 — 묻기 단계가 한 번의 호출에 거는 천장과 같은 수이고, 시험이 둘을 한 쌍으로 묶는다). 송신 전 입력 바이트와 프레이밍 여유를 비용으로 예약한다. `Search`에 상한을 주지 않아도 「상한 없음」이 아니라 이 기본값이며 manifest의 `caps.spendUsd`가 쓰인 수를 적는다. 묻기 단계에 남은 금액 없이 넘어간 라운드는 $0을 받는다. 서버가 예약보다 많이 청구할 가능성 때문에 절대 청구 상한은 검증되지 않았다. 제안자는 `--rounds` 횟수로 제한하며 zo 세션 사용량으로 달러를 별도 추정한다 |
| `--proposer` | `zo`(기본, `zo -p --json --model <M> --no-spawn --permission-mode read-only --allowed-tools TodoWrite`, 빈 방 폴더에서 — 파일도 명령도 못 읽는다) · `file:<path>,…`(라운드마다 파일 하나) · `none` |
| `--model` | 제안자의 모델(기본 `claude-opus-5-5`) |
| `--dry-run` | 행 나열과 manifest까지; 요청 0, 키 불필요 |

`--source`와 `--workdir`은 저장소 밖 경로여야 한다. 제안자 zo의 `ZO_CONFIG_HOME`도 작업 폴더 안의 임시 홈으로 강제하고, Rust Jev 문은 자체 임시 홈을 쓴다. 작업 폴더에는 비공개 상태·프롬프트와 zo의 인증 캐시가 있을 수 있으므로 공유하거나 커밋하지 않는다.

완료된 평가를 재현할 때는 Python `loop.read_result(workdir)`로 읽는다. 이 함수는 참조 입력의 현재 digest, 동결된 후보의 평가 ID, 결과 파일 SHA-256을 확인하고 저장 결과만 반환한다. 새 `Search.judge()` 호출은 이미 존재하는 `freeze.json`을 원자적으로 거절한다.
`Search.judge()`를 직접 부르는 길(외부 검증처럼 개발·보류 세트를 호출자가 짜는 경우)도 `run()`과 같은 두 판단을 **라벨을 열기 전에** 먼저 거친다 — 평가 자격(`unevaluable`: 시간순 자리인가, 행 id 중복, 보류 행이 개발 행과 묶음(세션·판·같은 상태)을 공유하는가 — `split`의 purge가 보장하는 것을 호출자가 짠 세트에도 —, 양쪽의 class 수, 개발 묶음 수)이 안 되면 `NotEvaluable`, 청구가 불확실하면(`_unsettled`) `UnsettledBill`. 어느 쪽이든 `freeze.json`·`result.json`을 쓰지 않고 manifest의 `finalEvaluated`도 그대로다.

작업 폴더에 남는 것: `manifest.json`(대상·라벨 정의/버전·join·수·purge 수·전체 상한·분할 지문·판정 가능 여부), `round-<n>.json`·`rows-<n>.jsonl`
(라운드마다 물은 것과 답), `cache-<identity>.jsonl`(행×질문 캐시 — identity는 자리·원천·질문·모델, 행 목록은 아님), `states.jsonl`(제안자 눈에만 —
문이 통과시킨 상태 그대로, 스크래치), `prompt-<n>.txt`·`proposal-<n>.txt`·`proposer-room-<n>/`(빈 방), `freeze.json`(판정 직전 굳힌 후보·질문·
릿지·분할 지문·seed·`finalEvaluated`), `result.json`, `asker.log`.

## 이 하네스가 지키는 것

1. **manifest가 돈보다 먼저다.** 빈 라운드로 행을 나열해(요청 0) 표본·양성·음성·묶음·분할 지문·상한을 적은 뒤에야 0라운드가 나간다.
   평가 자격은 함수 하나(`unevaluable`)다 — manifest의 `judgeable`, `run()`의 유료 라운드 전 검사, `judge()`의 라벨 개봉 전 검사가 모두 그것을 읽는다.
   개발·보류 어느 쪽이든 어느 class 가 `JUDGEABLE_PER_CLASS`(3) 미만이거나 개발 묶음이 `FOLDS`(5) 미만이면 판정은 `not_evaluable` 이지 AUC 0/1 이 아니다. 합성 픽스처(시험의 씨앗)는 실측이 아니다.
2. **알림의 시간·묶음 누수를 막는다.** 판단 시각 뒤의 것은 라벨뿐이다(자리의 함수가 그렇게 짠다). 분할은 시간순 70/30이되 묶음(한 전사의 패치, 한
   판의 알림) 경계에서 자르고, 개발 세트가 가진 묶음 또는 **문이 가린 뒤 동일한 상태 내용**의 뒤 행은 보류에서 **purge** 한다(manifest의 `heldPurged`). k-겹도 이 연결 묶음 단위로 나누고,
   표준화와 릿지 세기(`RIDGES` 격자)는 학습 fold·개발 세트로만 고른다. 제안자가 보는 「가장 틀린·가장 맞은」 행은 개발 세트에서만 고르고,
   제안자는 빈 방에서 도구 없이 답한다 — 첫 파일럿에서 작업 폴더를 읽던 세션은 보류 라벨이 든 행 파일을 열었기에 죽였다. patch_review의 `at`은 세션 생성 시각일 뿐 patch 판단 시각이 아니다. 해당 cohort는 `byTime:false`로 적고(`TIME_ORDERED`) 유료·평가하지 않는다 — `run()`뿐 아니라 직접 부른 `judge()`도, class 수가 충분해도, 라벨을 열기 전에 거절하고, 직접 부른 `round_zero()`·`step()`의 유료 질문은 행을 예약하기 전에 거절한다(보내지도 않을 질문의 예약이 불확실한 청구로 남지 않게).
3. **보류 라벨은 봉인이다.** `HeldOut` 은 판정이 `unseal` 하기 전에 라벨을 내주지 않는다(읽으면 `SealedLabel`). 개발 오차가 나빠진 라운드는 질문 조합에서 되돌린다. 판정은 한 번이고
   `freeze.json` 에 굳힌 후보로 하며, 두 번째 판정과 판정된 폴더의 재실행은 `AlreadyJudged` 다. 최종 점수를 보고 후보를 바꿀 길이 없다.
4. **기준선은 같은 행에서 다시 잰다.** 늘 같은 답(개발 다수 라벨)·오늘 규칙(알림: 늘 울림)·자리의 질문(0라운드 답 위의 로지스틱)·자리 자신의
   판정(`shippedPredicts` — 자리의 리더가 읽은 허용/울림)·크기(패치 바이트·바뀐 줄·대기 판 수)를 보류 행 위에서 AUC·부트스트랩 95% 구간·
   일치율·Wilson 하한으로, 그리고 탐색 대비 **짝 지은 차이와 그 구간**으로 낸다. 옛 수치(54.7%·0.688·≤0.59)는 대입하지 않는다. 「발견 없음」도 정상 산출이다.
5. **연구 산출물까지만.** production 질문·설정·승격 원장은 건드리지 않는다. 넘는 질문이 나와도 카탈로그 2판은 별도 검토 뒤의 제안이다.
   실호출은 기존 문·동의·가림을 쓰고(자리의 행 그대로), 임시 홈에서 나가며, 사람의 원장·하루 셈은 움직이지 않는다. 답은 행마다 (자리·원천·질문·모델)의
   identity(원천 파일과 참조 전사 전체 바이트 SHA-256 포함) 로 캐시돼 — 다시 돌리든, 같은 manifest에서 행의 부분집합만 다시 쓰든 — 같은 행에 같은 말을 두 번 치르지 않는다. 묻기 직전 예약을 fsync하고 응답을 못 남긴 행은 과금 여부가 불확실하므로 재시작 때 멈춘다. 새 행에는 wire 요청 수·재시도·비용 미확정·예약 초과 여부가 남는다.
   청구가 불확실한 것이 하나라도 있으면 — 응답을 못 남긴 예약, 비용 미확정·예약 초과 행, wire 영수증 없는 행, 답이 저장되지 않은 제안 청구(`proposal-<n>.pending`) — 달러 상한을 적었든 안 적었든 다음 송신·다음 제안 구매·최종 판정(라벨 개봉 전)이 모두 같은 판단(`_unsettled`)에서 `UnsettledBill`로 멈추고, 재시작도 그 자리에서 멈춘다(재송신 0). 사람이 청구 기록을 대사하기 전에는 풀리지 않는다. `askerSchemaVersion=4` 이전 캐시는 wire 영수증이 없으므로 비용 미확정으로 취급한다. 과거 파일럿의 298은 논리 행이며 wire 요청 수는 미측정이다. 과거 $0.026577은 저장된 응답 토큰 기준 추정치다.

## 씨앗·행 파일이 읽지 않는 것

`seed.py` 는 원장 행의 `agreed` 와 개수, 원천 씨앗의 크기만 센다. 묻기 단계의 행 파일은 id·시각·묶음 지문·라벨·기준선 숫자·답 숫자·결과 낱말·토큰·벽뿐이다 —
사람의 글도 판 이름도 없다(시험 `the_rows_file_carries_no_words…`). 상태 글은 `states.jsonl` 한 곳에만, 문이 이미 자르고 가린 그대로, 제안자를 위해 남고 깃에 들어가지 않는다.


## 프로젝트 규칙의 실행과 원문 검증

`zo jev rules compile <definition.json> --cwd <project>`는 검토한 규칙 정의를
현재 원문의 정확한 줄과 SHA-256에 묶어 보관하고 선택한다. 정의는
`schemaVersion: 1`, `rules` 배열이며, 각 규칙은 `id`,
`source: {path, sha256, firstLine, lastLine, text}`, `paths`, `check`를 갖는다.
비어 있는 `sha256`만 현재 파일에서 채운다. `check.kind`는 `model`
(`question` 필요), `deterministic` (`checks_after_edits`,
`no_contradicted_completion`, `worktree_edits` 중 `check`), `deferred`
(`reason` 필요)다. 정의를 준비하는 것은 전역 Jev 스위치·동의·기능 모드를
바꾸지 않는다. 현재 선택은 `zo jev rules show`, 이전 정의로의 복귀는
`zo jev rules activate <definition-id>`로 확인하고 실행한다.

기존 Jev 설정의 프로젝트 규칙 점검이 켜져 있고 해당 폴더가 송신에 동의한
경우에만 완료 턴을 검사한다. 기록만 모드는 판단을 남기며, On은 다음 턴의
문맥에 제한된 관찰을 한 번 전달한다. 정의가 없으면 질문하지 않는다.
판단은 자동 수정이나 새로운 권한이 아니며 사람의 명시적 지시가 우선한다.

하위 `AGENTS.md`, `AGENTS.override.md`, `CLAUDE.md`, `GEMINI.md`는
그 디렉터리 안에서만 적용된다. 실제 편집 경로의 상위 디렉터리에 정의가
다루지 않은 지침 파일이 있으면, 재정의 전까지 해당 턴을 보류한다.
송신 전·알림 보관 전·전달 전에 이를 확인하므로, 늦게 도착한 판단도 새
하위 지침을 무시할 수 없다. 원문에 없는 정책을 자동 추정하지 않으며,
등록한 규칙 이외의 의무까지 모두 검사했다고 해석해서는 안 된다.
검사 통과 근거가 없거나 하위 지침의 예외를 코드로 해석할 수 없는 경우는
`unknown`이다. 편집 경로는 디렉터리와 적용 규칙이 같은 것끼리 묶어 대표 경로를 남긴다.
같은 영역의 수백 파일을 다루는 수정도 이 묶음으로 검사하며, 최대 32개
서로 다른 지침 범위의 상위 디렉터리 128개만 확인하며
저장소 전체를 매 턴 탐색하지 않는다.

검토 연구의 독립 묶음은 입력 문자열이 아니라 캡처 시점의 실제 세션·과업
출처(`originGroup`)다. 같은 출처의 다른 턴과, 서로 다른 출처의 같은 입력은
연결된 한 묶음으로 나누고 채점한다. 출처가 없는 이전 기록은 계속 검토하고
내보낼 수 있지만, 독립 표본으로 간주하거나 보류 평가의 합격 근거로 쓰지
않는다. 세션 이름은 로컬에서 해시로 묶으며 Jev 요청에는 추가하지 않는다.

활성 검토 기록의 512건 한도는 보관 명령으로 관리한다.
`zo jev archive <새로운-보관파일> --seat <기능> --rubric-version <판>`은
선택한 기록과 모든 검토자의 결과를 새 비공개 JSON 파일에 완전히 저장하고
동기화한 뒤 활성 공간을 비운다. 기본값은 모든 질문을 검토한 기록만이며,
미검토 기록까지 옮기려면 `--include-unreviewed`를 명시한다. 기존 파일은
덮어쓰지 않고, 보관에 실패하면 원본을 지우지 않는다. 손상된 기록은 그대로
두고 개수를 보고한다. 보관파일은 공개 체크아웃 밖의 지속적으로 보관할
위치에 두며, 같은 연구 도구가 읽는 `zo jev export` 형식이다.
