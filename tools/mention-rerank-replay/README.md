# mention-rerank-replay — 의도 재순위 자리를 이 기계의 프롬프트로 재는 하네스 (t-6042)

두 조각이다. `seed.py` 는 **고르기만** 하고, 계산은 하나도 하지 않는다 — 페이지는
`runtime::file_search::run`, 물음은 `smart_router::mention_rerank::judge` 한 곳에만 있고,
하네스가 씨앗의 프롬프트를 그 함수들에 그대로 넘긴다(중복 금지). 파이썬에 규칙 사본이
생기면 재는 규칙과 나가는 규칙이 갈라진다.

| 조각 | 무엇 |
| --- | --- |
| `tools/mention-rerank-replay/seed.py` | zo 세션(`~/.zo/projects/*/sessions`)과 Claude Code 세션(`~/.claude/projects`)의 사람 프롬프트 중 **그 작업 트리의 파일을 상대 경로로 부른** 것마다 전사 경로·줄·cwd·부른 경로를 씨앗 한 행으로 |
| `zo-ide/crates/tools/src/misc_tools/smart_router/mention_rerank/tests.rs` 의 `the_pages_this_machine_would_have_reranked` | 씨앗의 프롬프트마다 제품의 퍼지 검색으로 한 페이지를 만들고 실제 엔드포인트에 물어 표를 찍는 `#[ignore]` 실측 |
| `tools/tests/test_mention_rerank_replay_seed.py` | 씨앗이 사람 프롬프트만 고르고 낱말을 싣지 않는지, 상수가 Rust 원본과 같은지 (`just tools-test`) |

## 돌리기

```sh
python3 tools/mention-rerank-replay/seed.py --out /tmp/mention-rerank-replay/seed.json

TYPESAFE_API_KEY=$(security find-generic-password \
    -s dev.zerocode.key.TYPESAFE_API_KEY -a "$USER" -w) \
ZEROCODE_MENTION_REPLAY_SEED=/tmp/mention-rerank-replay/seed.json \
  cargo test -p tools --release --lib \
  mention_rerank::tests::the_pages_this_machine_would_have_reranked -- --ignored --nocapture
```

| 손잡이 | 무엇 |
| --- | --- |
| `ZEROCODE_MENTION_REPLAY_QUERY_CHARS` | 친 글자로 삼는 파일 이름 머리의 길이(기본 3). 사람이 실제로 몇 자 치고 골랐는지는 전사에 없으므로 이 하네스의 **가정**이고, 표에 적힌다 |
| `ZEROCODE_MENTION_REPLAY_LIMIT` | 앞에서부터 이만큼의 프롬프트만 |

## 라벨은 어디서 오는가

프롬프트가 부른 파일이 「사람이 고른 행」이다. 퍼지 첫 페이지(여덟 행)에 그 파일이 있으면
비교 한 건: 퍼지 1위가 그 파일이면 퍼지 적중, Jev 1위가 그 파일이면 Jev 적중. 페이지에 없으면
`notOffered` 로 세고 비교하지 않는다 — 자리가 내밀지 않은 행은 자리에 대해 아무것도 말하지 않는다.

## 이 하네스가 지키는 세 가지

1. **판정의 입력은 프롬프트뿐이다.** 의도는 부른 경로를 뺀 프롬프트 글, 친 글자는 파일 이름
   머리, 페이지는 지금 트리에서 퍼지 검색이 낸 첫 여덟 행이다. 파일 본문·전사의 다른 줄은
   읽지 않는다. 의도에서 부른 파일은 경로와 이름을 **부분 문자열로** 뺀다(`intent_of`, t-6155 F4:
   낱말 단위로 빼던 때는 줄 범위·절대 경로·백틱 속 이름이 남아 22/136건이 답을 실었다). 표의
   「intent residue」 줄이 이름 잔존 건수(0이어야)와 어간 잔존 건수를 찍는다.
2. **키는 환경에서, 원장은 아무 데도.** 문은 임시 홈에 서고(`JevDoor::at`), `judge` 는 행을 쓰지
   않는다. 사람의 원장·하루 셈은 움직이지 않는다.
3. **숫자는 pooled 원비율과 Wilson 하한.** 같은 (cwd, 경로, 의도)는 한 번만 묻는다. 적중률은
   전 비교를 합친 비율이고, p50/p95 는 물음마다 한 번 잰 왕복의 분포다.

## 씨앗이 읽지 않는 것

경로·줄 번호·cwd·부른 경로뿐이다. 사람이 친 글은 씨앗에 없다 — Rust 쪽이 그 줄에서 직접
읽고, 엔드포인트로 나가는 것은 자리 행이 선언한 머리들뿐이다.

## 이 하네스의 한계

- 친 글자는 가정이다(파일 이름 앞 N자). 사람이 디렉터리를 치거나 다른 조각을 쳤다면 페이지가
  다르다.
- 페이지는 지금 트리에서 낸다. 프롬프트가 쓰인 뒤 트리가 움직였으면 그때의 페이지와 다르다.
- `/resume` 목록의 라벨은 오프라인 표본이 없다 — 어느 세션을 골랐는지는 창 로그에 있어도
  그때 친 검색어가 없다. 그 표면은 shadow 라벨(고른 행==Jev 1위)이 원장에 쌓이면서 잰다.
