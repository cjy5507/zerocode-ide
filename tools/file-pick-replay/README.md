# file-pick-replay — 코드 요청의 파일 후보를 최근 편집 기준선과 재생

하네스는 두 조각입니다. `seed.py`는 최근 zo 전사 위치와 작업 폴더, 모델, 메시지 수, 성공한 편집 결과 수를 고릅니다. 씨앗에는 기기별 위치가 있으므로 개인 스크래치패드에만 둡니다. 사람의 요청, 도구 출력, 편집한 파일 경로는 씨앗에 복사하지 않습니다. Rust의 무시 시험이 각 전사를 읽어 실제 사용자 턴의 요청, 그 시각까지의 최근 편집, 검색 결과와 이미 있는 코드그래프 연결로 후보를 만들고, 제품의 `FilePickJudge`와 같은 질문·문·기록 코드를 씁니다.

| 조각 | 역할 |
| --- | --- |
| `tools/file-pick-replay/seed.py` | 기본 최근 7일의 전사에서 성공한 파일 편집 결과가 있는 세션을 고름 |
| `zo-ide/crates/tools/src/misc_tools/smart_router/file_pick/tests.rs`의 `the_file_pick_this_machine_would_have_shown` | 턴마다 한 번 묻고, 실제 편집 파일로 라벨을 기록한 뒤 원시 점수 top-3, 확신 선을 통과한 후보, 최근 편집 순서, 후보 천장, 요청 수, 토큰 비용, p50 왕복을 출력 |
| `tools/tests/test_file_pick_replay_seed.py` | 씨앗이 요청 문장과 도구 출력·편집 파일 경로를 저장하지 않는지 확인 |

## 실행

1. `seed.py`로 씨앗을 만듭니다. 전사와 작업 폴더 위치가 들어가므로 결과를 개인 스크래치패드에 둡니다.
2. 전역 `settings.json`을 임시 `ZO_CONFIG_HOME`에 복사합니다. 기존 Jev 사용 설정과 워크스페이스 동의를 그대로 읽어야 하며, 테스트가 사람의 원장에 쓰지 않도록 임시 홈을 사용합니다.
3. `TYPESAFE_API_KEY`가 설정된 명령에서 `cargo test -p tools --lib file_pick::tests::the_file_pick_this_machine_would_have_shown -- --ignored --nocapture`를 실행합니다. 시험은 50회 요청 상한과 비용 상한을 적용하고, 출력·보고서에는 요청이나 파일 경로를 싣지 않습니다.

재생은 요청을 그 시각에 보였던 후보와 비교합니다. 라벨은 같은 턴에서 실제 수정된 파일들이고, 기준선은 그 전에 있던 최근 편집 순서입니다. 후보 목록에 수정 파일이 없던 경우는 모델 불일치가 아니라 후보 천장 미스로 따로 셉니다. 아무 파일도 수정하지 않은 턴은 비교에서 제외합니다.
