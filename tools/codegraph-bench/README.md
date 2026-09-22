# codegraph 인덱스 실측 하네스 (t-5970)

zo 도구 `find_symbol`·`find_references`·`file_outline` 뒤의 인덱스(`zo-ide/crates/codegraph`)가
이 저장소에서 무엇을 치르는지 재는 자. 인덱스를 바꾸는 커밋은 이 표의 전/후를 싣는다.

## 무엇을 재나

| 단계 | 한 프로세스가 하는 일 | 표의 줄 |
|---|---|---|
| `build` | 빈 캐시 디렉터리에서 `CodeGraph::load_or_build` | 첫 인덱스 (s)·최대 RSS·캐시 크기(디렉터리 안 파일 전부 — 사이드카 포함) |
| `load` | 있는 캐시로 `load_or_build`, 이어서 `find_references` 한 번 | 캐시 로드 (ms)·로드 뒤 상주 RSS·첫 질의 |
| `query` | 로드된 인덱스에 도구 질의 셋을 이름마다 20번 | `find_references`·`find_symbol`·`file_outline` p50/p95, 변화 없는 신선도 확인 |
| `edit` | 파일 하나 저장→갱신(×5), 파일 하나 추가→갱신·삭제→갱신(×5) — 갱신이 그 변화를 봤는지 질의로 확인 | 저장·추가·삭제 뒤 갱신 p50 |

- 각 단계는 크레이트의 **공개 API만** 부른다(도구가 부르는 그대로) — 그래서 같은 하네스가 API 뒤의
  어떤 인덱스든 잰다. 질의 한 번의 시간에는 그 질의가 치르는 신선도 확인이 들어 있다.
- 최대 RSS는 `/usr/bin/time`(macOS `-l`, Linux `-v`), 상주 RSS는 그 순간의 `ps -o rss=`.
- 이름·파일·횟수는 `run.py` 머리의 상수 한 곳: 참조가 가장 많은 이름 다섯(최악의 답), 정의가 가장
  많은 이름 다섯, 저장하는 파일(`crates/zerocode-core/src/second_brain_graph.rs`, 1.7k 줄).

## 다시 돌리기

```sh
# 재는 트리를 커밋으로 못박는다 — 전과 후가 같은 트리를 읽게
tools/codegraph-bench/run.py --scratch <빈 디렉터리> --rev <sha> --label "전" --out before.json
# 인덱스를 바꾼 뒤, 같은 --rev 로
tools/codegraph-bench/run.py --scratch <빈 디렉터리> --rev <sha> --label "후" --out after.json --compare before.json
```

`--scratch`는 매번 비우고 다시 채운다: `git archive <rev>` 스냅샷과 캐시가 그 안에만 생긴다(실제
`~/.zo/projects/*/state/codegraph`는 건드리지 않는다). 벤치는 릴리즈 프로필(zo가 나가는 프로필)로
한 번 빌드된다.

## 바쁜 기계에서 전/후 재기

다른 빌드가 도는 기계에서는 몇 분 사이에 같은 수가 두 배로 흔들린다. 그래서 전과 후를 **번갈아**
재고 같은 라벨끼리 합친다: 전 커밋을 스크래치에 `git archive`로 풀어 벤치를 따로 빌드하고
(`cargo bench -p codegraph --bench index_cost --no-run`), `--binary`로 그 실행 파일을 돌린다.
같은 라벨의 실행들은 한 열로 합쳐져 모든 수가 전체 표본의 중앙값이 되고, 표 머리에 실행 수와
그동안의 부하(1분 평균)가 같이 찍힌다.

```sh
for i in 1 2 3; do
  tools/codegraph-bench/run.py --scratch <s> --rev <sha> --binary <전 벤치> --label 전 --out v1-$i.json
  tools/codegraph-bench/run.py --scratch <s> --rev <sha> --binary <후 벤치> --label 후 --out v2-$i.json
done
tools/codegraph-bench/run.py --no-run --compare v1-1.json … --compare v2-3.json   # 표만
```
