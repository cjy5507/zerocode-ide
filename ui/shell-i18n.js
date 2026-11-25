/* ---- 다국어 / localisation ----
 *
 * Orca's own arrangement, measured out of its bundle rather than invented:
 * i18next with `defaultNS: "translation"`, dotted keys, `{{name}}`
 * interpolation, a `supportedLngs` list and a fallback
 * (`I18nProvider-4EBrmTGg.js`). Its picker offers five languages by their
 * native names, and its settings key for the row is
 * `settings.appearance.language.title` with a `.system` option.
 *
 * One thing here is deliberately not i18next's default shape: the Korean
 * stays at the CALL SITE as the fallback argument, which is i18next's own
 * `defaultValue` (it is in its option list). Korean is this product's source
 * language, so a missing catalog entry renders the real sentence rather than
 * a bare key — and the window's text stays readable to anyone reading the
 * code, which a file of `t("sidebar.newWorktree")` calls is not. */
const LOCALES = [
  { code: "system", name: "시스템 설정" },
  { code: "ko", name: "한국어" },
  { code: "en", name: "English" },
  { code: "ja", name: "日本語" },
  { code: "zh", name: "中文（简体）" },
  { code: "es", name: "Español" },
];

/* The treatments, in the order the picker offers them.
 *
 * Unlike the languages these ARE translated: `다크` names a treatment, not a
 * language, so a person reading the window in Japanese should read it in
 * Japanese. The names have to agree with what the backend will store
 * (`THEMES`, crates/zerocode-shell/src/main.rs) or the picker offers a code
 * that is refused on the way out. */
const THEMES = [
  { code: "system", key: "settings.appearance.theme.system", name: "시스템 설정" },
  { code: "dark", key: "settings.appearance.theme.dark", name: "다크" },
  { code: "light", key: "settings.appearance.theme.light", name: "라이트" },
];

/* Korean is the source, so it has no entry: `t` falls through to the literal
 * written where the string is used. */


const CATALOG = {
  ko: {
    "board.graph.scopeCopy": "누가 시작했고, 어떤 결과를 기다리는지 확인하세요.",
    "board.graph.gestureHint": "카드와 연결선을 눌러보세요",
    "board.orbit.view": "관계 보기",
    "board.orbit.orbit": "입체",
    "board.orbit.cards": "카드",
    "board.orbit.summary": "에이전트 {{count}}개 · 작업 중 {{working}} · 확인 필요 {{attention}}",
    "board.orbit.rings": "끌면 돌고 놓으면 미끄러져 서며, 두 번 누르면 처음 자리로 — 캐릭터나 이름표를 누르면 상세가 열립니다",
    "board.orbit.hint": "캐릭터나 이름표를 누르면 상세가 열립니다",
    "board.orbit.liveInCards": "실시간 조율은 카드 보기에서만 보입니다",
    "board.orbit.now": "지금 흐름",
    "board.orbit.keyRail": "지시 흐름 →",
    "board.orbit.keyMail": "← 우편 회신",
    "board.orbit.home": "본진",
    "board.orbit.particle": "이|가",
    "board.orbit.glanceAttention": "{{name}}{{subject}} 확인을 기다립니다.",
    "board.orbit.glanceFailed": "{{name}}{{subject}} 실패했습니다.",
    "board.orbit.glanceDone": "{{name}} 완료 — 결과가 {{parent}}에게 돌아갑니다.",
    "board.orbit.glanceAll": "{{name}}{{subject}} 작업 {{count}}개에 지시를 보냈고, 모두 작업 중입니다.",
    "board.orbit.glanceSome": "{{name}}{{subject}} 작업 {{count}}개에 지시를 보냈고, {{working}}개가 작업 중입니다.",
    "board.orbit.glancePlain": "에이전트 {{count}}개가 워크스페이스 {{workspaces}}곳에 앉아 있습니다.",
    "board.orbit.glanceRead": "왼쪽 → 오른쪽으로 읽습니다 · 빛의 색 = 상태 · 움직이는 빛 = 오가는 지시와 우편 · 대기 {{idle}}",
    "board.orbit.glLost": "그래픽 장치가 입체 그림을 놓아 카드로 보여 줍니다 — 돌아오면 다시 섭니다",
    "board.orbit.noGl": "이 창에서는 입체를 그릴 수 없어 카드로 보여 줍니다",
    "board.graph.openInspector": "선택 상세 열기",
    "board.graph.scopeEmpty": "이 범위에 표시할 실행이 없습니다. 전체 실행에서 다른 작업을 확인하세요.",
    "board.graph.previousAttempt": "이전 시도",
    "board.graph.allExecutions": "전체 실행",
    "board.graph.scopeCount": "{{shown}} / {{total}} 에이전트",
    "board.graph.taskPending": "선행 조건 대기",
    "board.graph.taskReady": "실행 대기",
    "board.graph.taskDispatched": "배차됨",
    "board.graph.taskCompleted": "과업 완료",
    "board.graph.taskFailed": "과업 실패",
    "board.graph.taskBlocked": "결정 대기",
    "board.graph.factUnknown": "확인되지 않음",
    "board.graph.endpointOutside": "현재 보드에 없는 실행의 관계",
    "board.graph.mailEvidence": "현재 표시된 실행 사이에 원장이 기록한 메시지입니다. 수신 여부와 응답 완료는 서로 다른 사실입니다.",
    "board.graph.messageCount": "메시지 수",
    "board.graph.unreadCount": "미확인 수",
    "board.graph.lastMessageTime": "마지막 메시지",
    "board.graph.messageId": "메시지 ID",
    "board.graph.sender": "발신 주소",
    "board.graph.recipient": "수신 주소",
    "board.graph.dependencyEvidence": "후속 과업에 등록된 선행 조건입니다. 이 연결만으로 에이전트가 현재 멈춰 있거나 검증이 끝났다고 판단하지 않습니다.",
    "board.graph.relationCount": "등록된 관계 수",
    "board.graph.taskCreated": "후속 과업 생성",
    "board.graph.dependencyDetailUnknown": "이 스냅샷에는 과업별 선행 조건의 상세 상태가 없습니다.",
    "board.graph.upstreamTask": "선행 과업",
    "board.graph.downstreamTask": "후속 과업",
    "board.graph.upstreamNotShown": "선행 과업의 실행은 현재 보드에 없습니다. 과업의 상태는 원장 기록으로 확인합니다.",
    "board.graph.lineageEvidence": "판이 보고한 실제 부모 관계입니다. 작업의 선행 조건이나 실행 순서와는 구분합니다.",
    "board.graph.containmentEvidence": "프로젝트와 실제 체크아웃에 속한 실행을 나타냅니다.",
    "board.graph.ackRecord": "확인했어요",
    "board.graph.openExecution": "실행 열기",
    "board.graph.selectedRelation": "선택한 관계",
    "board.graph.relationHint": "연결의 방향과 근거를 확인하세요.",
    "board.graph.noRecordedRelations": "현재 스냅샷에 표시할 관계가 없습니다.",
    "board.graph.model": "모델",
    "board.graph.taskId": "과업 ID",
    "board.graph.workerId": "워커 ID",
    "board.graph.dispatchId": "배차 ID",
    "board.graph.attemptStarted": "시도 시작",
    "board.graph.retryOf": "이전 시도",
    "board.graph.scope": "범위",
    "board.graph.lineage": "계보",
    "board.graph.cardDetails": "카드 정보 펼치기",
    "board.graph.detailTabs": "선택 상세 보기",
    "board.graph.executionDetails": "실행 정보",
    "board.batch.head": "묶음 통합",
    "board.batch.off": "꺼짐: 열린 묶음 {{count}}개",
    "board.batch.chipChosen": "고름 · 쌓기 기다림",
    "board.batch.chipStacking": "쌓는 중",
    "board.batch.chipStackStopped": "쌓기 멈춤",
    "board.batch.chipStackAbandoned": "쌓기 끊김",
    "board.batch.chipStacked": "쌓음 · 게이트 기다림",
    "board.batch.chipGating": "게이트 중",
    "board.batch.chipGreen": "초록",
    "board.batch.chipRed": "빨강",
    "board.batch.chipSearching": "범인 찾는 중",
    "board.batch.chipCulprit": "범인: {{task}}",
    "board.batch.chipPromoting": "올리는 중",
    "board.batch.chipPromoteStopped": "올리기 멈춤",
    "board.batch.chipPromoted": "main에 올림",
    "board.batch.chipCancelled": "취소됨",
    "board.batch.actPromote": "main으로 올리기",
    "board.batch.actBisect": "범인 찾기",
    "board.batch.actDropCulprit": "범인 빼고 다시 쌓기",
    "board.batch.actDropTask": "{{task}} 빼고 다시 쌓기",
    "board.batch.actRestack": "새 main 위에 다시 쌓기",
    "board.batch.actCancel": "취소",
    "board.batch.heldLine": "{{acts}}: {{why}}",
    "board.batch.heldPromoting": "올리는 중 — 끝나야 결과가 정해집니다",
    "board.batch.heldStacking": "쌓는 중",
    "board.batch.heldNoStack": "쌓은 것이 없음",
    "board.batch.heldRed": "게이트가 빨강 — 범인을 찾거나 다시 쌓으세요",
    "board.batch.heldNoGreen": "끝의 초록 게이트가 아직 없음",
    "board.batch.heldSearching": "범인 찾는 중",
    "board.batch.heldCulpritFound": "범인을 이미 찾았음",
    "board.batch.heldNothingLeft": "빼고 나면 남는 과업이 없음",
    "board.batch.heldSending": "보내는 중…",
    "board.batch.stopConflict": "S{{step}} {{task}}에서 충돌 — 파일 {{total}}개: {{files}}",
    "board.batch.stopGitSilent": "S{{step}} {{task}}에서 git이 한도 안에 답하지 않음",
    "board.batch.stopGitRefused": "S{{step}} {{task}}에서 git이 거절함",
    "board.batch.stopPublishRefused": "쌓은 끝을 통합 ref로 게시하지 못함 — 다시 쌓기",
    "board.batch.stopAbandoned": "쌓기가 끊김: 다시 쌓기",
    "board.batch.searchNext": "다음 게이트: S{{next}} · 구간 S{{lo}}–S{{hi}}",
    "board.batch.searchSpan": "구간 S{{lo}}–S{{hi}} · 다음 게이트를 고르는 중",
    "board.batch.culpritLine": "범인 {{task}} — S{{step}}에서 빨강이 됨",
    "board.batch.culpritMainFirst": "main 자체가 초록인지 먼저 확인하세요",
    "board.batch.stopMainMoved": "main이 움직였어요({{from}} → {{to}}). 새 main 위에 다시 쌓기",
    "board.batch.stopMainMovedPlain": "main이 움직였어요. 새 main 위에 다시 쌓기",
    "board.batch.stopUnfinished": "main 체크아웃에 고치다 만 파일 {{total}}개: {{files}}. 정리하고 다시 누르세요",
    "board.batch.stopTwoCheckouts": "main이 체크아웃 여러 곳에 나가 있어요 — 한 곳만 남기고 다시 누르세요",
    "board.batch.stopPromoteRefused": "git이 빨리감기를 거절했어요 — 아무것도 바뀌지 않았습니다",
    "board.batch.stopPromoteSilent": "git이 한도 안에 답하지 않았어요 — 아무것도 바뀌지 않았습니다",
    "board.batch.stopUnknownState": "main의 상태를 알 수 없어요 — 손으로 확인하세요",
    "board.batch.whoPerson": "사람",
    "board.batch.whoCoordinator": "코디",
    "board.batch.whoApp": "앱",
    "board.batch.chosen": "고른 쪽: {{who}} · {{at}}",
    "board.batch.pressedPromote": "올리기를 누른 쪽: {{who}} · {{at}}",
    "board.batch.pressedSearch": "범인 찾기를 누른 쪽: {{who}}",
    "board.batch.promotedBy": "올린 쪽: {{who}} · {{at}}",
    "board.batch.settledLater": "다음 실행이 확인함",
    "board.batch.cancelledBy": "취소한 쪽: {{who}}",
    "board.batch.facts": "세대 {{generation}} · {{branch}} {{base}}",
    "board.batch.factsStacked": "세대 {{generation}} · {{branch}} {{base}} → 끝 {{tip}}",
    "board.batch.overlaps": "양쪽이 고친 파일 {{count}}개",
    "board.batch.droppedLine": "빠진 과업: {{tasks}}",
    "board.batch.droppedTask": "{{task}}({{why}})",
    "board.batch.droppedCulprit": "범인",
    "board.batch.droppedAsked": "빼 달라고 함",
    "board.batch.droppedOnMain": "이미 main에 있음",
    "board.batch.askKeep": "그대로 두기",
    "board.batch.askPromoteTitle": "main으로 올릴까요?",
    "board.batch.askPromoteBody": "{{branch}} 가지를 묶음 {{batch}}의 끝 {{tip}}까지 빨리감기합니다 — 과업 {{count}}개가 한 번에 올라갑니다. 새 커밋은 생기지 않고, 그 가지의 체크아웃 파일도 함께 바뀝니다.",
    "board.batch.askBisectTitle": "범인을 찾을까요?",
    "board.batch.askBisectBody": "끝의 게이트가 빨강입니다. 쌓은 차례를 반씩 나눠 다시 게이트해 처음 빨강이 되는 과업을 찾습니다 — 과업 {{count}}개면 게이트가 {{gates}}번쯤 더 듭니다.",
    "board.batch.askDropCulpritTitle": "범인을 빼고 다시 쌓을까요?",
    "board.batch.askDropTaskTitle": "{{task}} 빼고 다시 쌓을까요?",
    "board.batch.askDropBody": "{{task}} 없이 나머지 과업 {{count}}개를 지금 {{branch}} 위에 {{generation}}번째 세대로 다시 쌓습니다. 빠진 과업의 가지는 그대로 남습니다.",
    "board.batch.askRestackTitle": "새 main 위에 다시 쌓을까요?",
    "board.batch.askRestackBody": "과업 {{count}}개를 지금 {{branch}} 위에 {{generation}}번째 세대로 다시 쌓습니다. 지금 세대의 게이트 결과는 쓰지 않습니다.",
    "board.batch.askCancelTitle": "묶음을 취소할까요?",
    "board.batch.askCancelBody": "이 묶음({{batch}})을 닫고 이 세대의 게시에 묘비를 세웁니다. 과업의 가지와 워커 체크아웃은 그대로 둡니다.",
    "board.batch.askCancelYes": "묶음 취소",
    "board.batch.saidSending": "보내는 중…",
    "board.batch.saidPromoted": "눌렀습니다 — main에 올림",
    "board.batch.saidPromoting": "아직 올리는 중 — git이 끝나면 적습니다",
    "board.batch.saidStacking": "눌렀습니다 — 다시 쌓는 중",
    "board.batch.saidStacked": "눌렀습니다 — 다시 쌓음 · 게이트 기다림",
    "board.batch.saidSearching": "눌렀습니다 — 범인 찾기를 시작함",
    "board.batch.saidCulprit": "눌렀습니다 — 범인을 찾음",
    "board.batch.saidCancelled": "눌렀습니다 — 취소됨",
    "board.batch.saidStopped": "멈춤 — {{why}}",
    "board.batch.saidRefused": "안 됨 — {{why}}",
    "board.batch.saidDone": "눌렀습니다",
    "board.batch.settingLabel": "묶음 통합 큐",
    "board.batch.settingDesc": "준비된 과업 여럿을 main 위에 함께 쌓아 한 번 게이트하고, 초록이면 main을 빨리감기합니다. 이 저장소에만 적용됩니다.",
    "board.batch.settingLocked": "macOS에서만 켤 수 있습니다 — 다른 플랫폼에서는 아직 시험되지 않았습니다.",
    "crash.settingsTitle": "멈춤 감시와 진단",
    "crash.settingsHint": "창의 응답이 늦어지면 진단을 남깁니다. 자료는 이 컴퓨터에만 보관합니다.",
    "crash.watchdog": "창 멈춤 감시",
    "crash.firstMs": "진단 기록 임계값 (ms)",
    "crash.secondMs": "상태바 알림 임계값 (ms)",
    "crash.title": "지난 실행의 진단",
    "crash.crumbs": "마지막 활동",
    "crash.bundleHint": "진단 폴더에는 활동 기록, 빌드 정보와 환경 요약이 담깁니다. 토큰·쿠키·이메일은 제외합니다. 필요한 곳에 직접 첨부하세요.",
    "crash.bundle": "진단 번들 만들기",
    "crash.openLog": "로그 열기",
    "crash.killed": "지난 창이 정상 종료 기록 없이 닫혔습니다.",
    "crash.hang": "지난 실행에서 창의 응답이 늦어졌습니다.",
    "crash.panic": "지난 실행에서 내부 오류가 발생했습니다.",
    "crash.ledger": "기록 당시 워커 {{workers}}개 · 배차 {{dispatches}}개",
    "crash.noCrumbs": "강제 종료는 메모리의 활동 기록을 남기지 못할 수 있습니다.",
    "crash.hangBadge": "창이 {{seconds}}초 멈췄었습니다",
    "crash.logFailed": "로그를 열지 못했습니다.",
    "crash.creating": "진단 폴더를 만들고 있습니다…",
    "crash.created": "진단 폴더를 열었습니다. 필요한 곳에 직접 첨부하세요.",
    "crash.bundleFailed": "진단 폴더를 만들지 못했습니다. 다시 시도해 주세요.",
    "crash.filed": "과업 {{task}} 으로 올렸습니다",

    "file.searchCase": "대소문자 구분",
    "file.searchWord": "단어 단위",
    "file.searchWordShort": "단어",
    "file.searchRegex": "정규식",
    "file.searchInclude": "포함할 경로",
    "file.searchExclude": "제외할 경로",
    "file.searchPresets": "프리셋",
    "file.searchReset": "필터 초기화",
    "file.searchInvalidRegex": "정규식이 올바르지 않습니다. 괄호와 반복 기호를 확인하세요.",
    "file.searchInvalidGlob": "경로 패턴이 올바르지 않습니다. 와일드카드와 괄호를 확인하세요.",
    "file.searchTimeout": "검색 시간 한도에 도달했습니다. 포함할 경로를 좁혀 주세요.",
    "file.searchFailed": "검색하지 못했습니다. 경로와 필터를 확인하세요.",
    "tree.dragPreview": "{{count}}개 → {{folder}}",
    "tree.dragConflict": "같은 이름 또는 이동할 수 없는 경로",
    "tree.dragChecking": "확인 중…",
    "tree.undo": "되돌리기",
    "tree.redo": "다시 실행",
    "tree.conflict": "그 자리에 다른 파일이 있어 작업하지 않았습니다.",
    "tree.changed": "파일이 다른 파일로 바뀌었거나 없어져 작업하지 않았습니다.",
    "tree.emptyHistory": "이 워크트리에 되돌릴 작업이 없습니다.",
    "tree.invalidMove": "이 경로로는 파일을 옮길 수 없습니다.",
    "tree.tooMany": "선택한 파일 수가 작업 한도를 넘었습니다.",
    "tree.failed": "파일 작업을 마치지 못했습니다. 경로와 권한을 확인하세요.",
    "tree.rollbackFailed": "일부 파일을 되돌리지 못했습니다. 되돌리기로 남은 파일을 복원하세요.",
    "tree.undoMove": "옮김 되돌리기",
    "tree.undoRename": "이름 변경 되돌리기",
    "tree.undoTrash": "삭제 되돌리기",
    "tree.undoCreate": "만들기 되돌리기",
    "tree.undoDuplicate": "복제 되돌리기",
    "tree.originAgent": "에이전트 · {{agent}} · {{pane}}",
    "tree.originHuman": "사람",
    "tree.follow": "에이전트 따라가기",
    "tree.followTip": "에이전트가 읽고 쓰는 파일로 트리를 펼칩니다",
    "tree.git.conflict": "충돌",
    "tree.git.deleted": "삭제됨",
    "tree.git.modified": "수정됨",
    "tree.git.added": "추가됨",
    "tree.git.untracked": "추적 안 됨",
    "tree.headTip": "{{branch}} — {{upstream}}보다 {{ahead}}개 앞서고 {{behind}}개 뒤져 있습니다",
    "tree.label": "파일 트리",
    "tree.git.ignored": "무시됨",
    "tree.touch.read": "에이전트가 읽는 중",
    "tree.touch.write": "에이전트가 쓰는 중",
    "tree.touch.commit": "에이전트가 커밋함",
    "tree.writing.file": "에이전트가 지금 쓰는 중",
    "tree.writing.within": "안의 파일에 에이전트가 쓰는 중",
    "sourceControl.revealInTree": "파일 트리에서 보기",
    "tree.menu.pinRoot": "트리 루트로 고정",
    "tree.unpin": "고정 풀기",
    "tree.agentHere": "에이전트 작업 폴더",
    "tree.vcs.running": "{{op}} 진행 중",
    "worktree.landOverlap": "{{ref}}과 겹침 {{count}}",
    "worktree.landTipOverlap": "저장 안 한 변경 가운데 {{count}}개 파일은 갈라진 뒤 {{ref}}에서도 바뀌었습니다: {{files}}",
    "worktree.landTipOverlapMore": " 외 {{count}}개",
    "worktree.landTipOverlapScope": "파일 단위 겹침입니다 — 같은 줄을 고쳤는지는 보지 않습니다",
    "worktree.landTipOverlapNone": "저장 안 한 변경 가운데 갈라진 뒤 {{ref}}에서도 바뀐 파일은 없습니다",
    "worktree.landTipOverlapUnknown": "저장 안 한 변경이 {{ref}}에서 바뀐 파일과 겹치는지는 읽지 못했습니다",
    "skills.nativeNotice": "동봉된 스킬은 감지한 에이전트에 직접 설치합니다.",
    "skills.bundledItem": "{{name}} 동봉본 설치",
    "skills.bundledSync": "동봉본 설치 / 갱신",
    "skills.copyCurrent": "최신",
    "skills.copyStale": "낡음",
    "skills.copyEdited": "직접 고침",
    "skills.copyMissing": "없음",
    "skills.copyCount": "{{state}} {{count}}곳",
    "skills.rewrite": "고쳐 쓰기",
    "skills.rewriteTip": "없거나 낡은 사본만 이 빌드의 글로 씁니다. 직접 고친 사본은 그대로 둡니다.",

    "skills.resumeOnboarding":"온보딩으로 돌아가기",
    "skills.outdated": "오래된 사본 · 최신 버전 {{latest}}",
    "skills.plan_prepared": "명령 준비됨 · 터미널에서 Enter로 실행하세요.",
    "skills.plan_changed": "다시 읽기 완료 · 설치 파일의 변경을 확인했습니다.",
    "skills.plan_unchanged": "파일 변경 없음 · 명령이 아직 실행 대기 중일 수 있습니다.",
    "skills.plan_failed": "설치 명령의 오류를 확인하세요.",
    "skills.title": "스킬·MCP",
    "skills.search": "스킬 검색",
    "skills.reload": "다시 읽기",

    "skills.filtered": "{{shown}} / {{count}}",
    "skills.none": "찾은 스킬이 없습니다",
    "skills.noneMatch": "조건에 맞는 스킬이 없습니다",

    "skills.reveal": "파일 위치 열기",


    "skills.library": "에이전트 라이브러리",
    "skills.loading": "스킬을 읽는 중…",
    "skills.bundle": "번들 설치",
    "skills.root_all": "모든 위치",
    "skills.root_home": "전역",
    "skills.root_repo": "프로젝트",
    "skills.root_plugin": "플러그인",
    "skills.usage": "최근 7일 {{count}}회 · {{last}} · {{agents}}",
    "skills.unused": "최근 7일 사용 증거 없음",
    "skills.conflict": "내용 충돌",
    "skills.requiredMissing": "필수 스킬 {{count}}개 설치 필요",

    "skills.requiredItem": "{{name}} 설치 → {{agents}}",
    "skills.capped": "검색 한도에 도달했습니다. 설정의 skills 정책에서 검색 범위를 확인하세요.",
    "skills.detail": "스킬 상세",
    "skills.variant": "스킬 사본",
    "skills.unversioned": "버전 없음",
    "skills.install": "설치…",
    "skills.update": "갱신 명령 준비",
    "skills.evidence": "사용 증거",
    "skills.invalidPlan": "설치 대상과 HTTPS 저장소 주소를 확인하세요.",
    "skills.noBundleSkills": "출력에서 스킬을 찾지 못했습니다. 명령 결과를 확인하세요.",
    "skills.selectTargets": "스킬과 에이전트를 하나 이상 선택하세요.",
    "skills.fileTooLarge": "상세 보기 한도보다 큰 파일입니다. 파일 위치에서 확인하세요.",
    "skills.outsideRoot": "검색한 스킬 위치 안의 파일만 열 수 있습니다.",
    "skills.notFound": "스킬이 이동되었거나 삭제되었습니다. 다시 읽어 주세요.",
    "skills.homeUnavailable": "사용자 홈 경로를 찾을 수 없습니다."
},
};

/* The other four catalogs live one language to a file. The window used to
 * scan all four at boot: 2.2 MB of source for a person who reads one language —
 * Korean, the source, reads none of them. VS Code's language packs and the
 * lazy dictionaries measured in open Tauri/Electron apps take the same shape:
 * the active locale arrives before the first frame, and the rest stay on disk
 * until somebody chooses them. */
const I18N_CATALOG_FILES = Object.freeze({
  en: "./shell-i18n-en.js",
  ja: "./shell-i18n-ja.js",
  zh: "./shell-i18n-zh.js",
  es: "./shell-i18n-es.js",
});
const catalogLoads = new Map();

/* A locale file's only call. It puts its object where readers have always
 * found it, so the rest of the window does not care whether the words were in
 * the first script or arrived later. */
function registerI18nCatalog(code, words) {
  Object.defineProperty(CATALOG, code, { configurable: true, enumerable: true, writable: true, value: words });
  return words;
}

function catalogLoaded(code) {
  return typeof CATALOG[code] === "object" && CATALOG[code] !== null;
}

/* Load one catalog exactly once. A missing file is not a boot failure: the
 * call sites' Korean fallback is the sentence the person can still read, and
 * the next language choice may try again. */
function ensureCatalog(code) {
  if (!Object.hasOwn(I18N_CATALOG_FILES, code) || catalogLoaded(code)) return Promise.resolve(true);
  if (catalogLoads.has(code)) return catalogLoads.get(code);
  const loading = new Promise((resolve) => {
    const script = document.createElement("script");
    const finish = (arrived) => {
      if (!arrived) catalogLoads.delete(code);
      resolve(arrived);
    };
    script.src = I18N_CATALOG_FILES[code];
    script.onload = () => finish(catalogLoaded(code));
    script.onerror = () => finish(false);
    document.head.appendChild(script);
  });
  catalogLoads.set(code, loading);
  return loading;
}

function localeCatalogCode(code, system = systemLocale) {
  return code === "system" ? system : code;
}

function localeCatalogReady(code, system = systemLocale) {
  const active = localeCatalogCode(code, system);
  return !Object.hasOwn(I18N_CATALOG_FILES, active) || catalogLoaded(active);
}

function ensureLocaleCatalog(code, system = systemLocale) {
  const active = localeCatalogCode(code, system);
  if (!Object.hasOwn(I18N_CATALOG_FILES, active)) return Promise.resolve(true);
  return ensureCatalog(active);
}

/* The browser suites still read `CATALOG.en` synchronously in dozens of
 * places. Production never uses this door; the harness marks itself, and only
 * then does the first read pull exactly that catalog across the harness's own
 * local server. A suite that never asks pays nothing, and a suite that asks
 * keeps the synchronous contract it was written against. */
function loadCatalogSynchronouslyForTests(code) {
  if (window.__ZEROCODE_I18N_SYNC_FOR_TESTS__ !== true || !Object.hasOwn(I18N_CATALOG_FILES, code)) return false;
  try {
    const request = new XMLHttpRequest();
    request.open("GET", I18N_CATALOG_FILES[code], false);
    request.send();
    if (request.status < 200 || request.status >= 300) return false;
    window.eval(request.responseText);
  } catch {
    return false;
  }
  const held = Object.getOwnPropertyDescriptor(CATALOG, code);
  return typeof held?.value === "object" && held.value !== null;
}

if (window.__ZEROCODE_I18N_SYNC_FOR_TESTS__ === true) {
  for (const code of Object.keys(I18N_CATALOG_FILES)) {
    Object.defineProperty(CATALOG, code, {
      configurable: true,
      enumerable: true,
      get() {
        loadCatalogSynchronouslyForTests(code);
        return Object.getOwnPropertyDescriptor(CATALOG, code)?.value;
      },
    });
  }
}


/* The locale in force, and the one the operating system asked for. Both come
 * from the backend at boot: the window never guesses, because `system` has to
 * resolve to something real before the first frame is painted. */
let locale = "ko";
let systemLocale = "ko";

/* Which catalog `t` should read. `system` is not a catalog — it is a pointer
 * at whatever the OS said, and an OS that said nothing we ship lands on
 * Korean, which is the source. */
function activeCatalog() {
  return CATALOG[activeLocaleCode()] ?? null;
}

/* The language code in force, for the browser's own formatters (a list's
 * 「및」·「、」, a date) to speak the same language as `t`. */
function activeLocaleCode() {
  return locale === "system" ? systemLocale : locale;
}

/* One string, in the language in force.
 *
 * `fallback` is the Korean written at the call site and it is not optional —
 * a key with no catalog entry has to render the real sentence, never a bare
 * `sidebar.newSession` on somebody's screen. Interpolation is `{{name}}`,
 * which is what Orca's i18next uses. */
function t(key, fallback, vars) {
  const catalog = activeCatalog();
  const text = (catalog && catalog[key]) || fallback;
  if (!vars) return text;
  // `name in vars` rather than `Object.hasOwn`: this runs inside whichever
  // webview the platform gives us, and a method missing there would throw on
  // every interpolated string — which is most of the window's chrome.
  return text.replace(/\{\{(\w+)\}\}/g, (whole, name) =>
    name in vars ? String(vars[name]) : whole,
  );
}
