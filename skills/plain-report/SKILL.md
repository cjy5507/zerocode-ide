---
name: plain-report
description: 사람이 읽을 보고서·요약·완료 알림·worker_done 요약을 짧고 쉬운 글로 쓰는 규칙(한국어·English). 구조·흐름을 설명하는 보고에는 그림을 같이 둔다. 코드·커밋 메시지·작업 중 대화에는 쓰지 않는다. Use it when you write a report, summary or notice that a person will read.
invocation: auto
keywords: [보고서 작성, 작업 보고, 결과 보고, 완료 보고, status report, completion notice, write a report, plain language]
---

# 쉬운 글로 보고하기

ZeroCode에서 사람이 읽을 글을 쓰기 전에 이 규칙을 읽는다. 사람이 읽는 글은 보고서, 요약, 완료 알림, `worker_done` 요약, 그리고 브리핑에 쓰는 답이다.
규칙은 ASD-STE100(통제된 기술 영어)의 핵심을 80% 수준으로 옮겼다. 매 턴이 아니라 이런 글을 쓸 때만 읽는다.

## 한국어 규칙

1. 결론을 맨 앞에 쓴다. 첫 문장이 "무엇이 어떻게 됐다"를 말한다.
2. 한 문장에는 사실 하나, 지시 하나만 둔다. 한 문장은 60자 안에서 끝낸다.
3. 능동으로 쓴다. 누가 했는지 주어로 보이게 한다.
4. 숫자에는 단위를 붙이고, 바뀐 것은 전 → 후로 쓴다 (7.4초 → 2.1초).
5. 한 뜻에는 한 단어만 쓴다. 쉬운 단어를 고른다.
6. 코드베이스 안에서만 통하는 비유를 쓰지 않는다. 아래 목록대로 바꿔 쓴다.
7. 직역투를 쓰지 않는다. 영어 문장을 옮긴 말투(`~하는 것이다`, `~에 의해`, `~을 진행한다`)는 동사로 바로 쓴다.
8. 코드·명령·경로는 백틱으로 하나씩, 필요할 때만 쓴다.
9. 긴 내용은 파일에 쓰고 경로를 한 번만 적는다. 요약에는 바뀐 것, 확인한 방법, 남은 것만 쓴다.

바꿔 쓰는 말 (왼쪽을 쓰지 않고 오른쪽으로 쓴다):

<!-- plain-rules:ko:begin -->
| 쓰지 않는다 | 이렇게 쓴다 |
| --- | --- |
| Jev 자리 | Jev 기능 |
| 자리 나는 대로 | 여유가 생기면 |
| 규칙 표 | 규칙 |
| 박자 | 주기 |
| 진입점 문 | 진입점·경로 |
| 낱말 | 설정값 또는 단어 |
| 손을 든다 | 적용하지 않음 |
| held | 적용하지 않음 |
| 표만 돈다 | 기록만 한다 |
| ~하는 것이다 | 서술어로 바로 끝낸다 (~한다) |
| ~에 의해 | 하는 쪽을 주어로 쓴다 (X가 처리한다) |
| ~를 통해 | ~로 / ~해서 |
| ~에 대한 | 동사로 풀어 쓴다 (~을 확인한다) |
| ~를 진행한다 | 동사로 바로 쓴다 (확인을 진행한다 → 확인한다) |
| ~에 있어서 | ~에서 / ~할 때 |
| ~로 인해 | ~때문에 |
| 보여진다 | 이중 피동을 풀어 쓴다 (보여진다 → 보인다) |
| ~라고 할 수 있다 | 단정해서 쓴다 |
| ~할 필요가 있다 | ~해야 한다 |
<!-- plain-rules:ko:end -->

## English rules

1. Put the result first. The first sentence says what happened.
2. Write one fact or one instruction in each sentence. Use 25 words or fewer in a description and 20 or fewer in a step.
3. Use the active voice. Say who does the action.
4. Use one word for one meaning, and choose the simple word.
5. Keep the articles (a, an, the). Do not stack more than three nouns. Do not chain -ing words.
6. Give every number a unit. Write a change as before → after (7.4 s → 2.1 s).
7. Do not use names that only the codebase understands (seat, door, beat, a table for a rule list). Say what the thing does.
8. Write a long answer to a file and give its path once.

Replace these (use the right side):

<!-- plain-rules:en:begin -->
| Do not write | Write |
| --- | --- |
| utilize | use |
| leverage | use |
| facilitate | help / allow |
| commence | start |
| prior to | before |
| subsequent to | after |
| in order to | to |
| a number of | several (or the exact number) |
| in the event that | if |
| at this point in time | now |
| with regard to | about |
| due to the fact that | because |
| is able to | can |
| it should be noted that | delete it and state the fact |
| there is a need to | must / needs to |
| is performed | name who acts and use an active verb |
| perform a | use the verb itself (analyze, check, test) |
<!-- plain-rules:en:end -->

## 그림 / Diagrams

구조·흐름·관계를 설명하는 보고는 글을 늘리지 말고 그림을 같이 둔다. Markdown 보고서에는 mermaid 코드 블록(`flowchart`, `sequenceDiagram`)을 넣는다. HTML 아티팩트로 만들 때는 `artifact-diagramming` 스킬을 읽고 인라인 SVG로 그린다.
If a report explains a structure, a flow or a relation, add a diagram. In Markdown, use a mermaid `flowchart`. In an HTML artifact, use an inline SVG (see the `artifact-diagramming` skill).

## 점검 / Check

ZeroCode 창은 `worker_done` 요약과 보고서를 보여 줄 때 글 점검 배지를 붙인다. 평균 문장 길이, 바꿀 말의 수, 직역투의 수를 센 것이며 글을 거절하지 않는다. 배지가 걸린 곳은 위 목록으로 바꿔 쓴다.
The ZeroCode window shows a writing badge on a `worker_done` summary and on a report. The badge counts the mean sentence length, the words to replace and the stilted phrases. It only counts. It never refuses a text.
