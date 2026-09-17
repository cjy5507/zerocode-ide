/* ---- the document surfaces ---- */

/* Every tab kind that `docHost` builds a view for, which is every kind except
 * the two painted from their own processes (`term` and `lane`).
 *
 * Named once because two places walk it — the host builder and the stage's
 * hide-everything pass — and the second of those used to carry its own list.
 * A view missing from that list is never told to hide, so it stays on screen
 * with nothing addressing it. */
/* 스테이지가 그릴 수 있는 문서 표면 전부 — 한 항목에 한 종류.
 *
 * 예전에는 같은 사실이 세 군데에 흩어져 적혀 있었다: 종류의 목록,
 * 템플릿을 마크업에서 찾을지 여기서 지을지, 그리고 누가 그리는지. 셋을
 * 손으로 맞춰야 하는 한 새 표면은 언제나 어느 하나에서 빠지고, 증상은 둘
 * 중 하나다 — 숨으라는 말을 듣지 못해 탭이 떠난 뒤에도 남는 판(보드가
 * 그랬다), 아니면 보이라는 말을 듣지 못해 새카맣게 열리는 판.
 *
 * 후자가 `worker`였다. 판은 지어졌고 `hidden`으로 태어났는데 이 목록에
 * 없어서 `updateStage`의 표면 순회가 한 번도 그것을 집어 들지 않았다 —
 * 즉 열리기는 하나 절대 보이지 않는 판. "코덱스 창이 안 보임", "아직도
 * 안 보이는데", 그리고 헬퍼 행을 눌렀을 때의 검은 화면이 전부 한 뿌리의
 * 같은 증상이었고, 그때마다 고친 것은 탭을 앞에 여는 일이었지 판을 보이게
 * 하는 일이 아니었다.
 *
 * `build` 마크업이 아니라 이 창이 짓는 표면(index.html은 다른 세션의 것).
 *        없으면 `<kind>-view`를 마크업에서 손으로 집어 온다.
 * `paint` `paintDoc`이 부를 화가. 없는 종류는 자기 이벤트로 그린다 —
 *         보관함이 그렇다(목록이 도착할 때 스스로 그린다).
 * `wire`  템플릿이 처음 지어질 때 한 번 거는 배선. */
/* ---- 토큰 사용량 화면 (P1 사용량 ⑧) --------------------------------------
 *
 * 요금 게이지는 "얼마나 남았나"를 답한다. 이 화면은 다른 질문에 답한다 —
 * "어디로 갔나". 아무도 그 숫자를 보고하지 않으므로 벤더가 남긴 전사에서
 * 직접 세며(`claude_token_usage`), 세는 일의 어려움은 백엔드가 이미 짊어졌다
 * (같은 메시지가 여러 번 내려앉아 순진한 합계의 45.3%가 중복이었다).
 *
 * 금액은 **추정치**다. 구독은 토큰당 청구가 아니므로 이 숫자는 "같은 일을
 * API로 했다면" 이고, 화면이 그렇게 말한다(Orca도 자기 숫자에 같은 주장을
 * 단다 — `ClaudeUsagePane.tsx:224`). */
/* 이 화면이 셀 줄 아는 장부들, 그리고 각자의 낱말.
 *
 * 이름은 상태 바의 USAGE_PROVIDERS와 같은 관용 — 고유명사라 번역표를 타지
 * 않는다. 그 아래가 이 표의 값어치다: 두 벤더가 같은 것을 다른 낱말로 세고
 * (claude는 메시지, codex는 턴), 캐시를 다른 자리에 싣는다 — claude의 캐시
 * 읽기는 입력과 나란한 제 항목이고, codex의 캐시 입력은 입력의 부분집합이다.
 * 그래서 같은 카드가 두 벤더에서 다른 산수여야 한다. 화가를 둘로 나누는 대신
 * 이 표가 다름을 갖는다 — 갈라진 화가는 한쪽에만 들어가는 고침을 부른다. */
const TOKEN_PROVIDERS = [
  {
    id: "claude",
    name: "Claude",
    command: "claude_token_usage",
    /* 한 줄의 공통 모양. 벤더의 낱말을 여기서 한 번만 옮긴다. */
    day: (row) => ({
      date: row.date,
      model: row.model,
      project: row.project,
      label: row.label ?? null,
      ours: row.ours === true,
      turns: row.messages ?? 0,
      cold: row.zero_cache_read ?? 0,
      input: row.input ?? 0,
      output: row.output ?? 0,
      cacheRead: row.cache_read ?? 0,
      cacheWrite: row.cache_create ?? 0,
      reasoning: 0,
      cost: row.cost_usd ?? null,
    }),
    session: (row) => ({
      session: row.session,
      at: row.last_at,
      date: row.last_date,
      model: row.model ?? null,
      branch: row.branch ?? null,
      label: row.label ?? null,
      places: row.places ?? 0,
      oursLabel: row.ours_label ?? null,
      oursPlaces: row.ours_places ?? 0,
      ours: row.ours === true,
      turns: row.messages ?? 0,
      oursTurns: row.ours_messages ?? 0,
      tokens: tokenBuckets(row.tokens),
      oursTokens: tokenBuckets(row.ours_tokens),
      cost: row.cost_usd ?? null,
      oursCost: row.ours_cost_usd ?? null,
    }),
    /* 캐시 재사용률 — 읽어 온 몫이 "새로 보냈을 입력 + 읽어 온 몫"에서
     * 차지하는 비율(Orca ClaudeUsagePane.tsx:234의 그 식). */
    reuse: (sum) =>
      sum.input + sum.cacheRead > 0 ? sum.cacheRead / (sum.input + sum.cacheRead) : null,
    /* 막대 한 칸을 이루는 조각들. 서로 겹치지 않는 넷이라 그대로 쌓는다. */
    segments: (day) => [
      { key: "input", word: t("tokens.input", "입력"), value: day.input },
      { key: "output", word: t("tokens.output", "출력"), value: day.output },
      { key: "cache-read", word: t("tokens.cacheRead", "캐시 읽기"), value: day.cacheRead },
      { key: "cache-write", word: t("tokens.cacheWrite", "캐시 쓰기"), value: day.cacheWrite },
    ],
    /* 표의 숫자 열 — 모델별·위치별·날짜별이 같은 다섯 칸을 쓴다. */
    columns: () => [
      { word: t("tokens.input", "입력"), of: (row) => tokenCount(row.input) },
      { word: t("tokens.output", "출력"), of: (row) => tokenCount(row.output) },
      { word: t("tokens.cacheRead", "캐시 읽기"), of: (row) => tokenCount(row.cacheRead) },
      { word: t("tokens.cacheWrite", "캐시 쓰기"), of: (row) => tokenCount(row.cacheWrite) },
      { word: t("tokens.cost", "비용"), of: (row) => tokenCost(row.cost) },
    ],
    countWord: () => t("tokens.messageCount", "메시지"),
    /* 최근 대화 표에서 낱말로 서는 열이 하나 더 있다 — 브랜치. */
    recentWords: 1,
    /* 최근 대화 표의 열. claude만 브랜치를 안다 — 롤아웃은 브랜치를 적지 않는다. */
    recent: (mine) => [
      { word: t("tokens.branch", "브랜치"), of: (one) => one.branch ?? "—" },
      { word: t("tokens.messageCount", "메시지"), of: (one) => tokenCount(tokenSide(one, mine).turns) },
      { word: t("tokens.input", "입력"), of: (one) => tokenShort(tokenSide(one, mine).input) },
      { word: t("tokens.output", "출력"), of: (one) => tokenShort(tokenSide(one, mine).output) },
      {
        word: t("tokens.cache", "캐시"),
        of: (one) => tokenShort(tokenSide(one, mine).cacheRead + tokenSide(one, mine).cacheWrite),
      },
      { word: t("tokens.cost", "비용"), of: (one) => tokenCost(tokenSide(one, mine).cost) },
    ],
    /* 이 판이 세우는 카드 여덟 장, Orca의 순서 그대로. */
    cards: (slice) => [
      { glyph: "upload", word: t("tokens.inputTokens", "입력 토큰"), said: tokenShort(slice.sum.input) },
      { glyph: "download", word: t("tokens.outputTokens", "출력 토큰"), said: tokenShort(slice.sum.output) },
      { glyph: "ft-database", word: t("tokens.cacheRead", "캐시 읽기"), said: tokenShort(slice.sum.cacheRead) },
      { glyph: "hard-drive", word: t("tokens.cacheWrite", "캐시 쓰기"), said: tokenShort(slice.sum.cacheWrite) },
      { glyph: "orbit", word: t("tokens.cacheReuse", "캐시 재사용률"), said: tokenPercent(slice.reuse) },
      { glyph: "circle-dashed", word: t("tokens.coldMessages", "캐시 미사용 메시지"), said: tokenPercent(slice.cold) },
      {
        glyph: "columns",
        word: t("tokens.sessionsMessages", "대화 / 메시지"),
        said: `${tokenCount(slice.sessions.length)} / ${tokenCount(slice.sum.turns)}`,
      },
      { glyph: "book", word: t("tokens.estimatedCost", "추정 비용"), said: tokenCost(slice.sum.cost) },
    ],
    /* 카드 밑 한 줄 — 재사용률이 무엇의 비율인지. 백틱은 쓰지 않는다
     * (no_user_facing_string_is_written_in_markdown). */
    note: () => t("tokens.cacheReuseNote", "캐시 재사용률은 캐시 읽기 ÷ (입력 + 캐시 읽기)로 셈합니다"),
    counted: (held) =>
      t("tokens.counted", "파일 {{files}}개에서 기록 {{records}}개를 읽어 고유 {{messages}}개", {
        files: tokenCount(held.files),
        records: tokenCount(held.records),
        messages: tokenCount(held.messages),
      }),
    undated: (count) => t("tokens.undated", "날짜가 없어 빠진 메시지 {{count}}개", { count }),
    /* 이 장부가 아예 비었는가 — 구간·범위 이전의 물음이다. */
    bare: (held) => !held || (held.records ?? 0) === 0,
  },
  {
    id: "codex",
    name: "Codex",
    command: "codex_token_usage",
    day: (row) => ({
      date: row.date,
      model: row.model,
      project: row.project,
      label: row.label ?? null,
      ours: row.ours === true,
      turns: row.turns ?? 0,
      cold: row.zero_cached_input ?? 0,
      input: row.input ?? 0,
      output: row.output ?? 0,
      // Codex의 캐시는 입력의 부분집합이다 — 쌓을 때 빼야 하는 몫.
      cacheRead: row.cached_input ?? 0,
      cacheWrite: 0,
      reasoning: row.reasoning_output ?? 0,
      cost: row.cost_usd ?? null,
    }),
    session: (row) => ({
      session: row.session,
      at: row.last_at,
      date: row.last_date,
      model: row.model ?? null,
      branch: null,
      label: row.label ?? null,
      places: row.places ?? 0,
      oursLabel: row.ours_label ?? null,
      oursPlaces: row.ours_places ?? 0,
      ours: row.ours === true,
      turns: row.turns ?? 0,
      oursTurns: row.ours_turns ?? 0,
      tokens: tokenBuckets(row.tokens),
      oursTokens: tokenBuckets(row.ours_tokens),
      cost: row.cost_usd ?? null,
      oursCost: row.ours_cost_usd ?? null,
    }),
    // 캐시가 입력의 일부이므로 분모는 입력 하나다. 카드로는 서지 않고, 이
    // 값이 필요한 곳이 하나 더 늘면 여기 한 자리에서 답한다.
    reuse: (sum) => (sum.input > 0 ? sum.cacheRead / sum.input : null),
    /* 세 조각이 정확히 `입력 + 출력`이 된다. 추론 출력은 이미 출력 안에 든
     * 몫이라 쌓지 않는다 — 쌓으면 한 번 쓴 토큰을 두 번 그리게 되고, 그건
     * 스캐너가 존재하는 이유인 그 오류다(codex_tokens.rs `CodexTokens`). */
    segments: (day) => [
      { key: "cache-read", word: t("tokens.cachedInput", "캐시 입력"), value: day.cacheRead },
      {
        key: "input",
        word: t("tokens.uncachedInput", "신규 입력"),
        value: Math.max(0, day.input - day.cacheRead),
      },
      { key: "output", word: t("tokens.output", "출력"), value: day.output },
    ],
    columns: () => [
      { word: t("tokens.input", "입력"), of: (row) => tokenCount(row.input) },
      { word: t("tokens.cachedInput", "캐시 입력"), of: (row) => tokenCount(row.cacheRead) },
      { word: t("tokens.output", "출력"), of: (row) => tokenCount(row.output) },
      { word: t("tokens.reasoning", "추론 출력"), of: (row) => tokenCount(row.reasoning) },
      { word: t("tokens.cost", "비용"), of: (row) => tokenCost(row.cost) },
    ],
    countWord: () => t("tokens.turnCount", "턴"),
    recentWords: 0,
    recent: (mine) => [
      { word: t("tokens.turnCount", "턴"), of: (one) => tokenCount(tokenSide(one, mine).turns) },
      { word: t("tokens.input", "입력"), of: (one) => tokenShort(tokenSide(one, mine).input) },
      { word: t("tokens.cachedInput", "캐시 입력"), of: (one) => tokenShort(tokenSide(one, mine).cacheRead) },
      { word: t("tokens.output", "출력"), of: (one) => tokenShort(tokenSide(one, mine).output) },
      { word: t("tokens.cost", "비용"), of: (one) => tokenCost(tokenSide(one, mine).cost) },
    ],
    cards: (slice) => [
      { glyph: "upload", word: t("tokens.inputTokens", "입력 토큰"), said: tokenShort(slice.sum.input) },
      { glyph: "download", word: t("tokens.outputTokens", "출력 토큰"), said: tokenShort(slice.sum.output) },
      { glyph: "ft-database", word: t("tokens.cachedInput", "캐시 입력"), said: tokenShort(slice.sum.cacheRead) },
      { glyph: "memory", word: t("tokens.reasoning", "추론 출력"), said: tokenShort(slice.sum.reasoning) },
      { glyph: "circle-dashed", word: t("tokens.coldTurns", "캐시 미사용 턴"), said: tokenPercent(slice.cold) },
      {
        glyph: "columns",
        word: t("tokens.sessionsTurns", "대화 / 턴"),
        said: `${tokenCount(slice.sessions.length)} / ${tokenCount(slice.sum.turns)}`,
      },
      { glyph: "book", word: t("tokens.estimatedCost", "추정 비용"), said: tokenCost(slice.sum.cost) },
    ],
    note: () =>
      t(
        "tokens.reasoningNote",
        "추론 출력은 이미 출력 안에 들어 있고, 금액은 입력·캐시 입력·출력으로만 셈합니다",
      ),
    counted: (held) =>
      t("tokens.countedCodex", "파일 {{files}}개에서 이벤트 {{events}}개를 읽어 중복 {{folded}}개 접음", {
        files: tokenCount(held.files),
        events: tokenCount(held.events),
        folded: tokenCount(held.folded),
      }),
    undated: (count) => t("tokens.undatedTurns", "날짜가 없어 빠진 턴 {{count}}개", { count }),
    bare: (held) => !held || (held.events ?? 0) === 0,
  },
];

/* 화면이 물어볼 수 있는 창(窓)들 — 전부 "오늘부터 뒤로 N일"이다.
 *
 * 접미 구간이라는 것이 설계의 전부다: 한 대화가 구간에 걸치는지를 마지막
 * 활동 하루만 보고 정확히 답할 수 있고(Orca도 같은 이유로 그렇게 한다 —
 * claude-usage-scope-filters.ts:32), 그래서 스캔 한 번의 결과를 디스크를 다시
 * 걷지 않고 네 구간 전부로 자를 수 있다. */
const TOKEN_RANGES = [
  { id: "7d", days: 7, key: "tokens.range7d", word: "최근 7일" },
  { id: "30d", days: 30, key: "tokens.range30d", word: "최근 30일" },
  { id: "90d", days: 90, key: "tokens.range90d", word: "최근 90일" },
  { id: "all", days: null, key: "tokens.rangeAll", word: "전체 기간" },
];

/* 어디까지를 볼 것인가. `ours`는 이 창이 아는 워크스페이스에서 일어난 것만 —
 * 백엔드가 행마다 답해 둔 `ours` 한 칸이 그 판정 전부다. */
const TOKEN_SCOPES = [
  { id: "ours", key: "tokens.scopeOurs", word: "이 앱의 워크스페이스만" },
  { id: "all", key: "tokens.scopeAll", word: "이 기기 전체" },
];

const TOKENS_RANGE_DEFAULT = "30d";
const TOKENS_SCOPE_DEFAULT = "ours";

/* 위치 표에 세우는 줄 수. 나머지는 한 줄로 접히되 사라지지는 않는다 —
 * Orca는 다섯 줄에서 잘라 버려서 표의 합이 자기 헤드라인과 다르다
 * (UsageBreakdownSection.tsx:44). */
const TOKENS_ROLLUP_ROWS = 8;

/* 최근 대화 표의 길이(Orca `limit = 12`, claude-usage-session-rows.ts:14). */
const TOKENS_SESSION_ROWS = 10;

/* 막대 몇 개를 그릴 것인가 — 그리고 그 축은 데이터가 아니라 달력이다.
 * Orca는 "존재하는 마지막 열 줄"을 그려서(ClaudeUsageDailyChart.tsx:50) 사흘을
 * 건너뛴 두 날이 나란히 선다. 쉰 날도 칸을 갖는 것이 이 차트와 저 차트의 차이다. */
const TOKENS_CHART_DAYS = 30;
const TOKENS_CHART_STEP = 10;
const TOKENS_CHART_BAR = 8;
const TOKENS_CHART_HEIGHT = 120;
/* 0이 아닌 조각은 결코 보이지 않는 높이로 그려지지 않는다. */
const TOKENS_CHART_MIN_BAR = 1;
/* 쓴 것이 없는 날이 남기는 자국. 없음도 그려야 없음이 보인다. */
const TOKENS_CHART_BASELINE = 1;

/* 카드와 막대의 큰 수를 줄이는 자리들. 표는 여기 오지 않는다. */
const TOKENS_SHORT_TIERS = [
  [1_000_000_000, "B"],
  [1_000_000, "M"],
  [1_000, "k"],
];

function tokenProvider(id) {
  return TOKEN_PROVIDERS.find((one) => one.id === id) ?? TOKEN_PROVIDERS[0];
}

/* 세션 행이 싣고 온 토큰 묶음을 이 화면의 낱말로. 두 벤더가 서로 다른 이름의
 * 같은 자리를 쓰므로 여기서 한 번만 옮긴다. */
function tokenBuckets(held) {
  return {
    input: held?.input ?? 0,
    output: held?.output ?? 0,
    cacheRead: held?.cache_read ?? held?.cached_input ?? 0,
    cacheWrite: held?.cache_create ?? 0,
    reasoning: held?.reasoning_output ?? 0,
  };
}

/* 지금 보고 있는 범위가 고르는 절반. 대화 한 줄은 두 벌의 숫자를 들고 온다 —
 * 전체와, 그중 우리 워크스페이스에서 일어난 몫. */
function tokenSide(one, mine) {
  const held = mine ? one.oursTokens : one.tokens;
  return {
    input: held.input,
    output: held.output,
    cacheRead: held.cacheRead,
    cacheWrite: held.cacheWrite,
    reasoning: held.reasoning,
    turns: mine ? one.oursTurns : one.turns,
    cost: mine ? one.oursCost : one.cost,
  };
}

function buildTokensView() {
  const root = document.createElement("section");
  root.className = "file-view tokens-view";
  // The first leaf's copy answers to this name, exactly as the surfaces the
  // markup declares do — `docHost` strips it from every clone after.
  root.id = "tokens-view";
  root.hidden = true;
  const head = document.createElement("header");
  head.className = "tokens-head";
  const name = document.createElement("span");
  name.className = "tokens-name";
  name.dataset.i18n = "tokens.title";
  name.textContent = t("tokens.title", "토큰 사용량");
  const pick = document.createElement("div");
  pick.className = "tokens-pick";
  for (const provider of TOKEN_PROVIDERS) {
    const one = document.createElement("button");
    one.type = "button";
    one.className = "btn tokens-provider";
    one.dataset.provider = provider.id;
    one.textContent = provider.name;
    pick.appendChild(one);
  }
  const note = document.createElement("span");
  note.className = "tokens-note";
  note.dataset.i18n = "tokens.estimate";
  note.textContent = t("tokens.estimate", "추정치 · API 환산");
  const gap = document.createElement("span");
  gap.className = "tokens-gap";
  // 필터는 사이드바의 표시 메뉴와 같은 문법으로 연다 — 이 창에 메뉴는 한
  // 종류뿐이고, 두 번째 문법은 두 번째로 배워야 하는 것이다.
  //
  // 지어지는 판은 로드 시점 낱말을 굽는다(보드 메모 6) — 그래서 이 단추는
  // 키를 입고 다니고, docHost가 판을 붙일 때 applyLocale이 갈아입힌다.
  const sift = document.createElement("button");
  sift.type = "button";
  sift.className = "btn tokens-filter";
  sift.dataset.i18nAria = "tokens.filters";
  sift.dataset.i18nTitle = "tokens.filters";
  sift.setAttribute("aria-label", t("tokens.filters", "표시 범위"));
  sift.dataset.tip = t("tokens.filters", "표시 범위");
  sift.innerHTML = icon("sliders");
  const again = document.createElement("button");
  again.type = "button";
  again.className = "btn tokens-refresh";
  again.dataset.i18n = "tokens.refresh";
  again.textContent = t("tokens.refresh", "다시 읽기");
  head.append(name, pick, note, gap, sift, again);
  const body = document.createElement("div");
  body.className = "tokens-body";
  root.append(head, body);
  return root;
}

/* 큰 수는 자릿점으로, 돈은 두 자리로 — 로케일의 것이 아니라 이 창의 것이다.
 * 숫자 표기를 로케일에 맡기면 같은 화면이 언어마다 다른 폭으로 흔들린다. */
function tokenCount(count) {
  return String(count).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

function tokenCost(dollars) {
  if (dollars === null || dollars === undefined) return "—";
  // 1센트 밑을 두 자리로 자르면 전부 $0.00이 된다 — 쓴 적이 있는데 안 썼다고
  // 말하는 숫자다. Orca도 같은 문턱에서 네 자리로 내려간다
  // (usage-formatters.ts:12).
  const digits = dollars > 0 && dollars < 0.01 ? 4 : 2;
  return `$${dollars.toFixed(digits).replace(/\B(?=(\d{3})+(?!\d)\.)/g, ",")}`;
}

/* 카드와 막대에서만 줄인다 — 표는 정확한 자릿수를 보러 가는 곳이다. */
function tokenShort(count) {
  for (const [floor, mark] of TOKENS_SHORT_TIERS) {
    if (count >= floor) return `${(count / floor).toFixed(1)}${mark}`;
  }
  return tokenCount(count);
}

function tokenPercent(share) {
  return share === null || share === undefined ? "—" : `${Math.round(share * 100)}%`;
}

/* 어느 달력 하루인가. `toISOString`은 여기서 쓸 수 없다 — 그건 UTC이고, 행은
 * 부르는 쪽의 오프셋으로 철해졌으므로 UTC 열쇠는 잘라내는 자리를 하루까지
 * 밀어 버린다. */
function tokenDayKey(at) {
  const month = String(at.getMonth() + 1).padStart(2, "0");
  const day = String(at.getDate()).padStart(2, "0");
  return `${at.getFullYear()}-${month}-${day}`;
}

/* `key`로부터 `back`일 앞선 달력 하루. 월·연을 넘는 셈은 Date가 한다. */
function tokenDayShift(key, back) {
  const [year, month, day] = key.split("-").map(Number);
  return tokenDayKey(new Date(year, month - 1, day - back));
}

/* 표 한 장. `numericFrom`부터가 숫자 칸이다 — 오른쪽 정렬과 탭 숫자는 자릿수를
 * 견주라고 있는 것이고, 낱말을 그렇게 세우면 브랜치 이름이 오른쪽 끝에 매달린다. */
function tokensTable(head, rows, numericFrom = 1) {
  const table = document.createElement("table");
  table.className = "tokens-table";
  const top = document.createElement("thead");
  const topRow = document.createElement("tr");
  head.forEach((cell, at) => {
    const th = document.createElement("th");
    th.textContent = cell;
    if (at >= numericFrom) th.className = "tokens-num";
    topRow.appendChild(th);
  });
  top.appendChild(topRow);
  const trunk = document.createElement("tbody");
  for (const row of rows) {
    const tr = document.createElement("tr");
    const cells = Array.isArray(row) ? row : row.cells;
    if (!Array.isArray(row) && row.tip) tr.dataset.tip = row.tip;
    cells.forEach((cell, at) => {
      const td = document.createElement("td");
      td.textContent = cell;
      if (at >= numericFrom) td.className = "tokens-num";
      tr.appendChild(td);
    });
    trunk.appendChild(tr);
  }
  table.append(top, trunk);
  return table;
}

/* ---- 하나의 문 --------------------------------------------------------------
 *
 * 화면의 모든 숫자가 여기서 나온다 — 카드도, 차트도, 네 표도. 같은 부분집합에서
 * 나오지 않으면 "이번 주 대화 3개"와 "최근 대화 일곱 줄"이 한 화면 안에서 서로를
 * 반박한다. 스캔의 통짜 합계(`models`·`total`·`cost_usd`·`unpriced`)는 이 화면
 * 어디에서도 읽지 않는다: 그것들은 코퍼스 전체의 사실이고, 걸러진 화면에 그대로
 * 실으면 카드와 표가 다른 기간을 말한다.
 *
 * 자르기는 창에서 한다. 디스크는 세 번만 걷는다 — 처음 열 때, 아직 안 읽은
 * 장부로 옮길 때, 그리고 "다시 읽기". 세 부분 열쇠(날짜·모델·자리)가 있는 이유가
 * 이것이고, 옆에 선 프로바이더 단추가 이미 같은 약속을 하고 있다. */
function tokensSlice(held, provider, range, scope) {
  const span = TOKEN_RANGES.find((one) => one.id === range) ?? TOKEN_RANGES[0];
  const cutoff = span.days === null
    ? null
    : tokenDayShift(tokenDayKey(new Date()), span.days - 1);
  const mine = scope === "ours";
  const days = (held?.days ?? [])
    .map(provider.day)
    .filter((row) => (cutoff === null || row.date >= cutoff) && (!mine || row.ours));
  const sessions = (held?.sessions ?? [])
    .map(provider.session)
    .filter((one) => (cutoff === null || one.date >= cutoff) && (!mine || one.ours));
  const sum = {
    input: 0, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0,
    turns: 0, cold: 0, cost: null,
  };
  const byModel = new Map();
  const byPlace = new Map();
  const unpriced = new Set();
  for (const row of days) {
    sum.input += row.input;
    sum.output += row.output;
    sum.cacheRead += row.cacheRead;
    sum.cacheWrite += row.cacheWrite;
    sum.reasoning += row.reasoning;
    sum.turns += row.turns;
    sum.cold += row.cold;
    // 값이 없는 것과 0인 것을 가른다: 요율을 모르는 모델만 있는 구간은 "$0"이
    // 아니라 "—"다(Orca hasAnyBillableCost와 같은 주장).
    if (row.cost !== null) sum.cost = (sum.cost ?? 0) + row.cost;
    else unpriced.add(row.model);
    for (const [into2, key, word] of [
      [byModel, row.model, row.model],
      [byPlace, row.project, row.label],
    ]) {
      const into = into2.get(key)
        ?? { key, word, turns: 0, input: 0, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0, cost: null };
      into.turns += row.turns;
      into.input += row.input;
      into.output += row.output;
      into.cacheRead += row.cacheRead;
      into.cacheWrite += row.cacheWrite;
      into.reasoning += row.reasoning;
      if (row.cost !== null) into.cost = (into.cost ?? 0) + row.cost;
      // 자리의 이름은 처음 본 것이 정본이다 — 같은 열쇠는 같은 자리다.
      if (into.word === null && word !== null && word !== undefined) into.word = word;
      into2.set(key, into);
    }
  }
  // 스캐너가 쓰는 그 무게로 정렬한다 — 창과 백엔드가 같은 순서로 읽도록.
  const heaviest = (rows) => [...rows.values()].sort((left, right) =>
    (right.input + right.output + right.cacheRead + right.cacheWrite)
      - (left.input + left.output + left.cacheRead + left.cacheWrite)
    || String(left.key).localeCompare(String(right.key)));
  const models = heaviest(byModel);
  const places = heaviest(byPlace);
  return {
    days,
    sessions,
    sum,
    models,
    places,
    unpriced: [...unpriced].sort(),
    reuse: provider.reuse(sum),
    // 캐시를 하나도 읽지 않은 턴의 비율. 턴이 없으면 비율도 없다.
    cold: sum.turns > 0 ? sum.cold / sum.turns : null,
    empty: days.length === 0,
  };
}

/* 카드가 쓰는 글리프. `icon()`은 문자열을 돌려주고 이 길은 그릴 때마다
 * 지나가므로, 여기서는 노드를 짓는다 — 그리는 길에서 innerHTML은 쓰지 않는다. */
function iconNode(name) {
  const mark = document.createElementNS(GRAPH_NS, "svg");
  mark.setAttribute("class", "icon");
  mark.setAttribute("aria-hidden", "true");
  const use = document.createElementNS(GRAPH_NS, "use");
  use.setAttribute("href", `#i-${name}`);
  mark.appendChild(use);
  return mark;
}

function tokensCards(provider, slice) {
  const cards = document.createElement("div");
  cards.className = "tokens-cards";
  for (const one of provider.cards(slice)) {
    const card = document.createElement("div");
    card.className = "tokens-card";
    const mark = document.createElement("span");
    mark.className = "tokens-card-mark";
    mark.appendChild(iconNode(one.glyph));
    const body = document.createElement("div");
    body.className = "tokens-card-body";
    const said = document.createElement("span");
    said.className = "tokens-card-value";
    said.textContent = one.said;
    const word = document.createElement("span");
    word.className = "tokens-card-label";
    word.textContent = one.word;
    body.append(said, word);
    card.append(mark, body);
    cards.appendChild(card);
  }
  return cards;
}

/* 날짜별 막대 — 축은 달력이다.
 *
 * 하루를 건너뛴 두 날이 나란히 서면 그림이 거짓말을 한다. 그래서 자리는 데이터가
 * 아니라 달력이 정하고, 아무것도 쓰지 않은 날은 바닥에 자국 하나를 남긴다. */
function tokensChart(provider, slice) {
  const byDay = new Map();
  for (const row of slice.days) {
    const held = byDay.get(row.date)
      ?? { date: row.date, input: 0, output: 0, cacheRead: 0, cacheWrite: 0 };
    held.input += row.input;
    held.output += row.output;
    held.cacheRead += row.cacheRead;
    held.cacheWrite += row.cacheWrite;
    byDay.set(row.date, held);
  }
  const dates = [...byDay.keys()].sort();
  const last = dates[dates.length - 1];
  const first = [
    tokenDayShift(last, TOKENS_CHART_DAYS - 1),
    dates[0],
  ].sort().pop();
  const columns = [];
  for (let day = first; day <= last; day = tokenDayShift(day, -1)) {
    const held = byDay.get(day) ?? { date: day, input: 0, output: 0, cacheRead: 0, cacheWrite: 0 };
    const segments = provider.segments(held).filter((one) => one.value > 0);
    columns.push({
      date: day,
      segments,
      total: segments.reduce((sum, one) => sum + one.value, 0),
    });
  }
  let tallest = 1;
  for (const column of columns) {
    if (column.total > tallest) tallest = column.total;
  }
  const figure = document.createElement("figure");
  figure.className = "tokens-chart";
  const scale = document.createElement("div");
  scale.className = "tokens-chart-scale";
  const peak = document.createElement("span");
  peak.className = "tokens-chart-peak";
  peak.textContent = tokenShort(tallest);
  scale.appendChild(peak);
  const plot = document.createElementNS(GRAPH_NS, "svg");
  plot.setAttribute("class", "tokens-chart-plot");
  plot.setAttribute("role", "img");
  plot.setAttribute("preserveAspectRatio", "none");
  plot.setAttribute(
    "viewBox",
    `0 0 ${columns.length * TOKENS_CHART_STEP} ${TOKENS_CHART_HEIGHT}`,
  );
  // 막대는 장식이다. 이 그림의 접근 가능한 원본은 바로 아래 날짜별 표이고,
  // 거기에는 같은 숫자가 전부 글자로 서 있다.
  plot.setAttribute(
    "aria-label",
    t("tokens.chartAria", "최근 {{days}}일 일별 토큰 사용량", { days: columns.length }),
  );
  columns.forEach((column, at) => {
    const left = at * TOKENS_CHART_STEP + (TOKENS_CHART_STEP - TOKENS_CHART_BAR) / 2;
    if (column.total === 0) {
      const bare = document.createElementNS(GRAPH_NS, "rect");
      bare.setAttribute("class", "tokens-chart-empty");
      bare.setAttribute("x", String(left));
      bare.setAttribute("y", String(TOKENS_CHART_HEIGHT - TOKENS_CHART_BASELINE));
      bare.setAttribute("width", String(TOKENS_CHART_BAR));
      bare.setAttribute("height", String(TOKENS_CHART_BASELINE));
      plot.appendChild(bare);
    }
    let stacked = 0;
    for (const one of column.segments) {
      const high = Math.max(
        TOKENS_CHART_MIN_BAR,
        Math.round((one.value / tallest) * TOKENS_CHART_HEIGHT),
      );
      const bar = document.createElementNS(GRAPH_NS, "rect");
      bar.setAttribute("class", `tokens-chart-seg tokens-chart-seg--${one.key}`);
      bar.setAttribute("x", String(left));
      bar.setAttribute("y", String(TOKENS_CHART_HEIGHT - stacked - high));
      bar.setAttribute("width", String(TOKENS_CHART_BAR));
      bar.setAttribute("height", String(high));
      plot.appendChild(bar);
      stacked += high;
    }
    // 하나의 판정 사각형이 칸 전체를 덮는다 — 1px 조각을 겨누게 하지 않는다.
    const hit = document.createElementNS(GRAPH_NS, "rect");
    hit.setAttribute("class", "tokens-chart-hit");
    hit.setAttribute("x", String(at * TOKENS_CHART_STEP));
    hit.setAttribute("y", "0");
    hit.setAttribute("width", String(TOKENS_CHART_STEP));
    hit.setAttribute("height", String(TOKENS_CHART_HEIGHT));
    hit.dataset.tip = t("tokens.chartDay", "{{date}} · 토큰 {{total}}", {
      date: column.date,
      total: tokenShort(column.total),
    });
    plot.appendChild(hit);
  });
  const axis = document.createElement("div");
  axis.className = "tokens-chart-days";
  for (const day of [columns[0]?.date ?? "", columns[columns.length - 1]?.date ?? ""]) {
    const said = document.createElement("span");
    // 연도는 서른 개의 막대 밑에서 서른 번 같은 말을 한다 — 월·일만 남긴다.
    said.textContent = day.slice(5);
    axis.appendChild(said);
  }
  const key = document.createElement("ol");
  key.className = "tokens-legend";
  for (const one of provider.segments({ input: 0, output: 0, cacheRead: 0, cacheWrite: 0 })) {
    const item = document.createElement("li");
    const dot = document.createElement("span");
    dot.className = `tokens-swatch tokens-swatch--${one.key}`;
    dot.setAttribute("aria-hidden", "true");
    const word = document.createElement("span");
    word.textContent = one.word;
    item.append(dot, word);
    key.appendChild(item);
  }
  figure.append(scale, plot, axis, key);
  return figure;
}

/* 자리에 붙일 이름. 백엔드는 낱말이 없는 자리를 `null`로 두고 창이 말한다 —
 * 한 언어로 구운 문장이 네 언어의 창에 닿지 않도록(usage_places 모듈 주석). */
function tokenPlaceWord(word) {
  return word ?? t("tokens.placeUnknown", "위치 없음");
}

/* 위치 표 — 여덟 줄과, 나머지를 합쳐 든 한 줄.
 *
 * 꼬리를 그냥 자르면 표의 합이 바로 위 카드와 달라진다. Orca는 다섯 줄에서
 * 자르고 나머지를 말하지 않는다(UsageBreakdownSection.tsx:44); 여기서는 접되
 * 잃지 않는다. */
function tokensRollupRows(provider, rows, name) {
  const columns = provider.columns();
  const head = [name, provider.countWord(), ...columns.map((one) => one.word)];
  const shown = rows.slice(0, TOKENS_ROLLUP_ROWS);
  const body = shown.map((row) => [
    tokenPlaceWord(row.word),
    tokenCount(row.turns),
    ...columns.map((one) => one.of(row)),
  ]);
  const rest = rows.slice(TOKENS_ROLLUP_ROWS);
  if (rest.length > 0) {
    const folded = rest.reduce((sum, row) => ({
      turns: sum.turns + row.turns,
      input: sum.input + row.input,
      output: sum.output + row.output,
      cacheRead: sum.cacheRead + row.cacheRead,
      cacheWrite: sum.cacheWrite + row.cacheWrite,
      reasoning: sum.reasoning + row.reasoning,
      cost: row.cost === null ? sum.cost : (sum.cost ?? 0) + row.cost,
    }), {
      turns: 0, input: 0, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0, cost: null,
    });
    body.push([
      t("tokens.placeRest", "그 외 {{count}}곳", { count: tokenCount(rest.length) }),
      tokenCount(folded.turns),
      ...columns.map((one) => one.of(folded)),
    ]);
  }
  return { head, body };
}

/* 최근 대화 표. 한 줄이 어디에 있었다고 말할 것인가 — 한 자리에 머문 대화는
 * 그 자리의 이름을, 옮겨 다닌 대화는 몇 곳을 더 거쳤는지까지. 가장 무거운
 * 자리 하나만 적으면 나머지가 조용히 사라진다. */
function tokensSessionRows(provider, sessions, mine) {
  const columns = provider.recent(mine);
  // 마지막 활동·위치·모델은 언제나 낱말이고, 그 뒤로 벤더가 낱말을 하나 더
  // 둘 수 있다(claude의 브랜치). 숫자는 그다음부터다.
  const numericFrom = 3 + (provider.recentWords ?? 0);
  const head = [
    t("tokens.lastActive", "마지막 활동"),
    t("tokens.place", "위치"),
    t("tokens.model", "모델"),
    ...columns.map((one) => one.word),
  ];
  const body = sessions.slice(0, TOKENS_SESSION_ROWS).map((one) => {
    const named = tokenPlaceWord(mine ? one.oursLabel : one.label);
    const spread = mine ? one.oursPlaces : one.places;
    const more = t("tokens.placeMore", "외 {{count}}곳", { count: tokenCount(spread - 1) });
    const place = spread > 1 ? `${named} · ${more}` : named;
    return {
      // 표의 칸은 날짜별 표와 같은 낱말을 쓰고, 정확한 시각은 줄의 툴팁이 든다.
      tip: new Date(one.at).toLocaleString(),
      cells: [
        one.date,
        place,
        one.model ?? "—",
        ...columns.map((column) => column.of(one)),
      ],
    };
  });
  return { head, body, numericFrom };
}

function tokensSection(word) {
  const name = document.createElement("h3");
  name.className = "tokens-section";
  name.textContent = word;
  return name;
}

function tokensQuiet(word) {
  const said = document.createElement("p");
  said.className = "tokens-quiet";
  said.textContent = word;
  return said;
}

function paintTokensView(tab) {
  const host = docHost(tab.pane, "tokens");
  // ⌘F 바가 이 판에서 죽어 있던 자리 — 바는 `dataset.tab`으로 자기 탭을
  // 찾는다(buildDocFindBar), 그리고 이 판은 그 이름을 달아 준 적이 없었다.
  host.dataset.tab = tab.id;
  const body = host.querySelector(".tokens-body");
  const provider = tokenProvider(tab.provider);
  const held = tab.scans[provider.id];
  host.querySelector(".tokens-refresh").disabled = Boolean(tab.reading);
  host.querySelector(".tokens-filter").disabled = Boolean(tab.reading);
  // 전환 중에도 어느 장부를 보고 있는지는 항상 표시된다 — 읽는 동안 누르는
  // 것만 막는다(두 스캔이 한 판을 두고 경주하면 늦게 온 쪽이 이긴 척한다).
  for (const button of host.querySelectorAll(".tokens-provider")) {
    button.classList.toggle("is-active", button.dataset.provider === tab.provider);
    button.disabled = Boolean(tab.reading);
  }
  body.replaceChildren();
  if (tab.reading) {
    body.appendChild(tokensQuiet(t("tokens.reading", "전사를 읽는 중…")));
    return;
  }
  // 언제 읽었고 무엇을 보고 있는가 — 한 줄로. 상대시각은 다음 그리기까지
  // 늙는다(타이머는 달지 않는다); 정확한 시각은 이 줄의 툴팁이 든다.
  const status = document.createElement("p");
  status.className = "tokens-status";
  const when = tab.readAt
    ? t("tokens.readAgo", "{{when}} 전에 읽음", { when: agoWord(tab.readAt, Date.now()) })
    : t("tokens.readNever", "아직 읽지 않음");
  const scopeWord = TOKEN_SCOPES.find((one) => one.id === tab.scope) ?? TOKEN_SCOPES[0];
  const rangeWord = TOKEN_RANGES.find((one) => one.id === tab.range) ?? TOKEN_RANGES[0];
  // 낱말을 먼저 꺼내 두고 잇는다 — 템플릿 구멍 바로 뒤에 `t(` 가 붙으면
  // 호출을 세는 게이트가 앞 이름과 붙여 읽는다(`whent`).
  const said = t(scopeWord.key, scopeWord.word);
  const spanned = t(rangeWord.key, rangeWord.word);
  status.textContent = `${when} · ${said} · ${spanned}`;
  if (tab.readAt) status.dataset.tip = new Date(tab.readAt).toLocaleString();
  body.appendChild(status);
  // 세 가지 빈 상태는 서로 다른 사실이고 사람이 할 일도 다르다: 읽을 전사가
  // 아예 없는 것, 이 범위에 없는 것, 이 구간에 없는 것.
  if (provider.bare(held)) {
    body.appendChild(tokensQuiet(t("tokens.empty", "읽을 전사가 없습니다")));
    return;
  }
  const slice = tokensSlice(held, provider, tab.range, tab.scope);
  if (slice.empty) {
    body.appendChild(tokensQuiet(
      tab.scope === "ours"
        ? t(
          "tokens.emptyScope",
          "이 앱의 워크스페이스에는 읽을 것이 없습니다 — 범위를 이 기기 전체로 바꿔 보세요",
        )
        : t("tokens.emptyRange", "이 기간에는 읽을 것이 없습니다"),
    ));
    return;
  }
  body.append(tokensCards(provider, slice), tokensQuiet(provider.note()));
  body.appendChild(tokensQuiet(provider.counted(held)));
  // 숨기지 않는 세 가지: 요율을 모르는 모델, 날짜가 없어 빠진 기록, 그리고
  // 목록에서 잘려 나간 오래된 대화.
  if (slice.unpriced.length) {
    body.appendChild(tokensQuiet(
      t("tokens.unpriced", "요율 미상이라 금액에서 빠짐: {{models}}", {
        models: slice.unpriced.join(", "),
      }),
    ));
  }
  if (held.undated > 0) {
    body.appendChild(tokensQuiet(provider.undated(tokenCount(held.undated))));
  }
  const dropped = (held.sessions_seen ?? 0) - (held.sessions?.length ?? 0);
  if (dropped > 0) {
    body.appendChild(tokensQuiet(
      t("tokens.sessionsLeftOut", "오래된 대화 {{count}}개는 목록에서 뺐습니다", {
        count: tokenCount(dropped),
      }),
    ));
  }
  body.append(
    tokensSection(t("tokens.dailyChart", "일별 추이")),
    tokensChart(provider, slice),
  );
  const byModel = tokensRollupRows(provider, slice.models, t("tokens.model", "모델"));
  body.append(
    tokensSection(t("tokens.byModel", "모델별")),
    tokensTable(byModel.head, byModel.body),
  );
  const byPlace = tokensRollupRows(provider, slice.places, t("tokens.place", "위치"));
  body.append(
    tokensSection(t("tokens.byPlace", "위치별")),
    tokensTable(byPlace.head, byPlace.body),
  );
  const recent = tokensSessionRows(provider, slice.sessions, tab.scope === "ours");
  body.append(
    tokensSection(t("tokens.recent", "최근 대화")),
    tokensTable(recent.head, recent.body, recent.numericFrom),
  );
  const columns = provider.columns();
  body.append(
    tokensSection(t("tokens.byDay", "날짜별")),
    tokensTable(
      [t("tokens.date", "날짜"), t("tokens.model", "모델"), ...columns.map((one) => one.word)],
      slice.days.map((row) => [row.date, row.model, ...columns.map((one) => one.of(row))]),
      2,
    ),
  );
}

/* 화면을 열고, 없으면 읽어 온다.
 *
 * 오프셋은 창이 넘긴다 — 하루는 사람의 단위이고 어느 자정을 말하는지는 이쪽만
 * 안다. `getTimezoneOffset`은 UTC 기준 서쪽이 양수라 부호를 뒤집는다.
 * 프로바이더마다 자기 칸에 내려앉으므로 전환은 다시 읽기가 아니다. */
async function readTokenUsage(tab) {
  const provider = tokenProvider(tab.provider);
  tab.reading = true;
  if (tab.id === activeTabId) paintTokensView(tab);
  try {
    tab.scans[provider.id] = await invoke(provider.command, {
      offsetMinutes: -new Date().getTimezoneOffset(),
    });
    tab.readAt = Date.now();
  } catch (error) {
    tab.scans[provider.id] = null;
    showError(error);
  }
  tab.reading = false;
  if (tabs.some((one) => one.id === tab.id)) paintTokensView(tab);
}

function openTokensTab() {
  const held = tabs.find((tab) => tab.id === "tokens");
  if (held) {
    setActiveTab(held.id);
    return;
  }
  const tab = {
    id: "tokens",
    kind: "tokens",
    provider: "claude",
    range: TOKENS_RANGE_DEFAULT,
    scope: TOKENS_SCOPE_DEFAULT,
    reading: true,
    scans: {},
    readAt: null,
  };
  openTab(tab);
  void readTokenUsage(tabs.find((one) => one.id === "tokens") ?? tab);
}

/* 구간과 범위는 저장되지 않는다.
 *
 * 이 창의 규칙은 사이드바 표시 선택 옆에 적혀 있다(35377): 재시작을 건너 무엇이
 * **있는지**를 바꾸는 것만 저장을 얻고, 기본값으로 돌아오는 시야는 잃은 것이
 * 아니다. 게다가 이 탭은 `STAGE_STORED_KINDS`에 없어 재시작을 건너지 못한다 —
 * 살아남지 못하는 판의 필터를 저장하면 필터가 자기가 거르던 것보다 오래 산다.
 * Orca도 같은 두 필드를 저장하지 않는다(usage-provider-slices.ts). */
function setTokensRange(tab, id) {
  if (tab.range === id) return;
  tab.range = id;
  paintTokensView(tab);
}

function setTokensScope(tab, id) {
  if (tab.scope === id) return;
  tab.scope = id;
  paintTokensView(tab);
}

/* 필터 메뉴 — 범위 하나와 구간 하나. 고른 뒤에도 열려 있다: 7일과 30일을
 * 견주러 온 사람이 메뉴를 두 번 열 이유가 없다(Orca가 자기 라디오마다
 * `onSelect: preventDefault`를 다는 이유). */
function openTokensFilters(tab, anchor) {
  const bounds = anchor.getBoundingClientRect();
  openSidebarMenu(bounds.right, bounds.bottom + 6, [
    { caption: t("tokens.scope", "범위") },
    {
      segment: TOKEN_SCOPES.map((one) => ({ value: one.id, label: t(one.key, one.word) })),
      value: tab.scope,
      pick: (id) => setTokensScope(tab, id),
    },
    { caption: t("tokens.range", "기간") },
    {
      segment: TOKEN_RANGES.map((one) => ({ value: one.id, label: t(one.key, one.word) })),
      value: tab.range,
      pick: (id) => setTokensRange(tab, id),
    },
  ], anchor);
}

/* 판이 처음 붙을 때 한 번 — 전환·필터·다시 읽기가 이 판의 세 손짓이다.
 * 템플릿은 복제되므로(docHost) 청취자는 build가 아니라 여기 얹는다. */
function wireTokensView(host) {
  wireDocFind(host);
  host.addEventListener("click", (event) => {
    const tab = tabs.find((one) => one.id === "tokens");
    if (!tab || tab.reading) return;
    const picked = event.target.closest(".tokens-provider");
    if (picked) {
      if (picked.dataset.provider === tab.provider) return;
      tab.provider = picked.dataset.provider;
      // 이미 읽어 둔 장부는 그대로 보여 준다 — 전환은 시선을 옮기는 것이지
      // 디스크를 다시 걷는 것이 아니다. 다시 걷기는 "다시 읽기"의 일이다.
      if (tab.scans[tab.provider]) paintTokensView(tab);
      else void readTokenUsage(tab);
      return;
    }
    const sifting = event.target.closest(".tokens-filter");
    if (sifting) {
      openTokensFilters(tab, sifting);
      return;
    }
    if (event.target.closest(".tokens-refresh")) void readTokenUsage(tab);
  });
}

/* ---- 목차 (1-g37) ----
 *
 * Orca의 `MarkdownTableOfContentsPanel`에서 재어 온 숫자들. 들여쓰기는 12px에서
 * 시작해 단계마다 12px씩, 목차는 200px보다 좁아지지 않고 판의 절반보다 넓어지지
 * 않으며, 처음 서는 너비는 240px. 수준 단추의 5는 접을 수준이 아니라 "전부
 * 펼침"이다(Orca의 TOC_EXPAND_ALL_LEVEL). 문서 템플릿이 로드 중에
 * 지어지므로(`docTemplates` 고리), 이 수치들은 그보다 먼저 서 있어야 한다. */
const MD_TOC_INDENT_BASE = 12;
const MD_TOC_INDENT_STEP = 12;
const MD_TOC_MIN_WIDTH = 200;
const MD_TOC_DEFAULT_WIDTH = 240;
const MD_TOC_EXPAND_ALL = 5;

/* ---- 프리뷰에서 찾기 (1-g38) ----
 *
 * 질의의 크기는 브라우저 찾기가 재는 그 2KB와 같다 — 같은 사람이 같은 상자에
 * 같은 것을 친다. 하이라이트 두 이름은 등록부(CSS.highlights)의 열쇠고, 이
 * 창의 것이므로 우리 이름을 쓴다. 목차 수치와 같은 자리에 서는 이유도 같다:
 * 문서 템플릿이 로드 중에 지어진다. */
const DOC_SEARCH_QUERY_MAX_BYTES = 2 * 1024;
const DOC_SEARCH_HIGHLIGHT = "mdview-search-match";
const DOC_SEARCH_ACTIVE_HIGHLIGHT = "mdview-search-active-match";

/* ---- 표 (1-g39) ----
 *
 * Orca의 `CsvViewer`에서 재어 온 수치들. 줄 하나는 28px로 고정이고 — 그 고정이
 * 창 넓히기(virtualization)를 산수 한 줄로 만든다 — 화면 밖으로 열두 줄을 더
 * 그려 두어 스크롤이 빈 칸을 보이지 않게 한다. 칸의 너비는 글자 수 × 7px에
 * 여백 24px을 더해 80~320px 사이로 죈다: 폭을 재려고 백만 줄을 훑을 수는 없으니
 * 머리줄과 앞의 200줄만 본다. 냄새 맡기(구분자 추론)가 읽는 앞부분도 64KiB까지다.
 * 목차 수치와 같은 자리에 서는 이유도 같다: 문서 템플릿이 로드 중에 지어진다. */
const CSV_ROW_HEIGHT = 28;
const CSV_OVERSCAN = 12;
const CSV_MIN_COL_PX = 80;
const CSV_MAX_COL_PX = 320;
const CSV_ROW_NUMBER_COL_PX = 48;
const CSV_CHAR_PX = 7;
const CSV_CELL_PADDING_PX = 24;
const CSV_WIDTH_SAMPLE_ROWS = 200;
const CSV_SNIFF_LIMIT = 64 * 1024;
/* 바이트 순서 표시(U+FEFF). 첫 글자로 서 있으면 그것은 글자가 아니라 표식이다 —
 * 남겨 두면 첫 칸의 이름이 눈에 보이지 않게 달라진다. */
const CSV_BOM = 65279;
/* 빈칸으로 치는 코드 단위 — 탭, 세로 탭, 폼 피드, 공백, 줄바꿈 없는 공백. */
const CSV_BLANKS = new Set([9, 11, 12, 32, 160]);

/* Opening the same file twice is one tab, not two — and it re-reads, so the
 * second open is never a stale copy of the first. */
/* ---- where this window has been (titlebar ← →) ----
 *
 * Orca's arrows walk `worktreeNavHistory`: ONE stack holding workspaces and
 * the two page views — 작업 and 자동화 — deduplicated against the entry under
 * the cursor, capped at 50, and with entries whose workspace has since died
 * skipped rather than scrubbed (store-BgJxB0hr.js:30138-30252). Files are
 * not in it: opening a document never moves these arrows. Walking back and
 * then going somewhere NEW drops the forward half, which is what every
 * history in every application does. */
const navHistory = [];
let navHistoryAt = -1;
/* True while an arrow is being followed, so the activation it causes does
 * not record itself — Orca's `isNavigatingHistory`. */
let navigatingHistory = false;
const MAX_NAV_HISTORY = 50;
/* True while one page closes only because the other is opening — 작업↔자동화
 * 직행 — so the close is not read as a return to the terminal. */
let crossingPages = false;

/* A place that can still be gone to. The two views always can; a workspace
 * only while the catalog still lists it. */
function navEntryLive(entry) {
  if (entry === "tasks" || entry === "automations" || entry === "space") return true;
  return projects.some((project) =>
    project.worktrees.some((worktree) => worktree.path === entry));
}

function recordNavVisit(entry) {
  if (navigatingHistory) return;
  if (navHistory[navHistoryAt] === entry) return;
  navHistory.splice(navHistoryAt + 1);
  navHistory.push(entry);
  if (navHistory.length > MAX_NAV_HISTORY) {
    navHistory.splice(0, navHistory.length - MAX_NAV_HISTORY);
  }
  navHistoryAt = navHistory.length - 1;
  paintHistoryButtons();
}

function nextLiveNavIndex(step) {
  for (let at = navHistoryAt + step; at >= 0 && at < navHistory.length; at += step) {
    if (navEntryLive(navHistory[at])) return at;
  }
  return null;
}

function paintHistoryButtons() {
  el("nav-back").disabled = nextLiveNavIndex(-1) === null;
  el("nav-forward").disabled = nextLiveNavIndex(1) === null;
}

async function walkNavHistory(step) {
  const to = nextLiveNavIndex(step);
  if (to === null) return;
  const entry = navHistory[to];
  navigatingHistory = true;
  try {
    if (entry === "tasks") {
      setTaskOpen(true);
      navHistoryAt = to;
    } else if (entry === "automations") {
      setAutoOpen(true);
      navHistoryAt = to;
    } else if (entry === "space") {
      setSpaceOpen(true);
      navHistoryAt = to;
    } else if (await activateWorktree(entry)) {
      navHistoryAt = to;
    }
    // A refused activation — unsaved documents held the move — leaves the
    // cursor where it was, so the arrow can be pressed again.
  } finally {
    navigatingHistory = false;
  }
  paintHistoryButtons();
}

el("nav-back").addEventListener("click", () => walkNavHistory(-1));
el("nav-forward").addEventListener("click", () => walkNavHistory(1));

async function openFile(path, opts = {}) {
  // Already open with edits in it: show that, do not re-read. The second
  // open would otherwise hand the tab the text on disk and the person's
  // typing would be gone, from a click that only meant "go there".
  const held = tabs.find((tab) => tab.id === `file:${path}`);
  if (isDirty(held)) {
    setActiveTab(held.id);
    return;
  }
  let opened;
  try {
    opened = await invoke("read_text_file", {
      path,
      ...(opts.allowMissing && { allowMissing: true }),
    });
  } catch (error) {
    showError(`${path}: ${error}`);
    return;
  }
  const mode = renderedAs(path);
  // Review notes are part of the first Markdown frame. Painting them a beat
  // later makes the document jump and briefly lies about the note count.
  if (mode === "markdown") await refreshDiffNotes();
  markFirstRun("opened_file");
  // `draft` and `saved` are set rather than left off: re-opening a tab merges
  // onto the one that is there, and an old draft left behind would be shown
  // as edits to a file that has since been re-read.
  openTab({
    id: `file:${path}`,
    kind: "file",
    path,
    text: opened.text,
    draft: opened.text,
    // What the file was when it was read. A save hands this back, and the
    // backend refuses if the file has moved since — which is the whole reason
    // an editor is safe to leave open while an agent works in the checkout.
    version: opened.version,
    stale: false,
    gone: opened.missing === true,
    saved: false,
    mode,
    preview: Boolean(opts.preview),
  });
  // A caller that named a line meant that line — a terminal printed
  // `src/foo.rs:42` and somebody clicked it. Asked for AFTER the tab exists,
  // because the editor is built from the tab and there is nothing to move
  // until there is one.
  if (opts.line !== undefined) revealLine(`file:${path}`, opts.line);
}

/* Put a 1-based line of an open document under the reader's eye.
 *
 * The editor is not necessarily built yet — `openTab` paints on its own beat —
 * so this waits a frame the way `restoreEditorPlace` does, and gives up
 * quietly if the tab moved on. A line past the end of the file clamps rather
 * than throws: the output naming it may be older than the file. */
function revealLine(id, line) {
  requestAnimationFrame(() => {
    const tab = tabs.find((held) => held.id === id);
    if (!tab || tab.id !== activeTabId) return;
    const held = editorViews.get(tab.pane);
    if (!held || held.showing !== tab.id) return;
    const editor = held.editor;
    const wanted = Math.min(Math.max(1, line), editor.state.doc.lines);
    const at = editor.state.doc.line(wanted).from;
    editor.dispatch({
      selection: { anchor: at },
      effects: window.CM6.EditorView.scrollIntoView(at, { y: "center" }),
    });
  });
}

/* Which of Orca's viewers a path belongs to.
 *
 * By extension, never by content. A file called `notes.md` that happens to
 * open with a comma is a markdown file, and a viewer that guessed otherwise
 * would be arguing with the person who named it. */
const IMAGE_EXTENSIONS = new Set([
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "avif",
  "bmp",
  "ico",
  "svg",
]);

function extensionOf(path) {
  const name = basename(path);
  const at = name.lastIndexOf(".");
  return at <= 0 ? "" : name.slice(at + 1).toLowerCase();
}

function isImagePath(path) {
  return IMAGE_EXTENSIONS.has(extensionOf(path));
}

/* How a text file is drawn when it is opened. `text` is every other file —
 * the plain line list this viewer has always been. */
function renderedAs(path) {
  const extension = extensionOf(path);
  if (extension === "md" || extension === "markdown") return "markdown";
  if (extension === "csv" || extension === "tsv") return "csv";
  if (extension === "ipynb") return "notebook";
  // Orca's own two (store-BgJxB0hr.js:39114-39115), and its rich/source toggle
  // comes with them (MERMAID_VIEW_MODES, editor-panel-file-mode-ZhV2p83l.js:43).
  if (extension === "mmd" || extension === "mermaid") return "mermaid";
  return "text";
}

/* Orca's `ImageViewer`, as a tab of its own.
 *
 * Its own kind rather than a mode of the file viewer: there is no text to
 * fall back to, so a surface that could show either would spend half its
 * code answering a question that has one answer here. */
async function openImage(path, opts = {}) {
  let image;
  try {
    image = await invoke("read_image_file", { path });
  } catch (error) {
    showError(`${path}: ${error}`);
    return;
  }
  openTab({ id: `file:${path}`, kind: "image", path, image, preview: Boolean(opts.preview) });
}

/* The one door every "show me this file" goes through, so the tree, quick
 * open and the reopen stack cannot disagree about what a `.png` is. */
function openPath(path, opts = {}) {
  if (isImagePath(path)) return openImage(path, opts);
  // 표로 태어난 파일은 표로 연다(1-g39). 무엇이 표인지는 이미 `renderedAs`가
  // 확장자로 답하고 있으므로 두 번째 목록을 두지 않는다 — 원본은 표의 툴바에
  // 있는 문이 연다.
  if (renderedAs(path) === "csv") return openCsv(path, opts);
  // 노트북도 제 판으로 연다(1-g40) — 어느 확장자가 무엇인지는 여전히
  // `renderedAs` 한 곳만 안다.
  if (renderedAs(path) === "notebook") return openIpynb(path, opts);
  return openFile(path, opts);
}

function paintImageView(tab) {
  const view = docHost(tab.pane, "image");
  const shown = view.querySelector(".image-shown");
  const source = `data:${tab.image.mime};base64,${tab.image.data}`;
  // Only when it differs. Every leaf repaints on every stage change, and
  // re-assigning the same URL makes the webview decode the picture again.
  if (shown.getAttribute("src") !== source) shown.src = source;
  // The alt text is the file's name because that is what this image is: a
  // file being looked at, not an illustration with a meaning to convey.
  shown.alt = basename(tab.path);
  view.querySelector(".file-view-path").textContent = tab.path;
  view.querySelector(".file-view-note").textContent = t("file.imageSize", "{{size}} KB", {
    size: (tab.image.bytes / 1000).toFixed(1),
  });
}

/* One viewer element, repainted from whichever file tab is active. Holding a
 * DOM tree per tab would keep every file ever opened resident. */
function paintFileView(tab) {
  // The leaf that holds this tab, not the window's first one.
  const view = docHost(tab.pane, "file");
  view.dataset.tab = tab.id;
  view.dataset.path = tab.path;
  const body = view.querySelector(".file-body");
  body.replaceChildren();
  // Two bodies, one at a time. Markdown and CSV are drawn as what they are;
  // `원본` shows the source instead — and the source is the editor, because a
  // file you can read but not correct is a file you leave the window to fix.
  const showing = tab.mode !== "text" && !tab.raw ? tab.mode : "source";
  const editor = view.querySelector(".file-edit");
  body.hidden = showing === "source";
  editor.hidden = !body.hidden;
  body.className = `file-body file-body--${showing}`;
  const snapshot = tab.artifact?.snapshot;
  paintMdToolbar(view, tab, snapshot ? "snapshot" : showing);
  if (snapshot) {
    body.hidden = false;
    editor.hidden = true;
    if (showing === "markdown") paintMarkdown(body, snapshot.text);
    else {
      const source = document.createElement("pre");
      source.textContent = snapshot.text;
      body.appendChild(source);
    }
  } else if (showing === "markdown") {
    paintMarkdown(body, tab.text);
    // A markdown file can hold diagrams too, which is Orca's `MermaidBlock`
    // inside its rich markdown rather than a viewer of its own.
    paintMarkdownDiagrams(body);
    paintMarkdownReviewTools(body, tab);
  } else if (showing === "csv") paintTable(body, tab.text, extensionOf(tab.path) === "tsv");
  else if (showing === "notebook") paintNotebook(body, tab.text);
  else if (showing === "mermaid") paintMermaid(body, tab.text);
  else paintEditor(view, tab);

  const mode = view.querySelector(".file-view-mode");
  mode.hidden = tab.mode === "text";
  mode.textContent = tab.raw ? t("file.rendered", "미리보기") : t("file.source", "원본");
  // Assigned rather than added: these nodes outlive the repaint, so a
  // listener added here would stack up one per view of the file.
  mode.onclick = () => {
    tab.raw = !tab.raw;
    paintFileView(tab);
  };
  // Same reason as the mode button: this surface is cloned for the second
  // leaf, and a clone carries no listeners.
  const reload = view.querySelector(".file-view-reload");
  reload.dataset.tip = t("file.reload", "디스크에서 다시 읽기");
  reload.onclick = () => {
    if (tab.artifact) {
      tab.artifact.snapshot = null;
      tab.artifact.version = null;
    }
    rereadFile(tab);
  };
  view.querySelector(".file-view-path").textContent = tab.path;
  // 갤러리에서 연 문서의 머리띠(t-3233 §5) — 다른 파일에는 서지 않는다. 머리의
  // 첫 줄로: 파일 뷰의 격자는 줄이 넷으로 못 박혀 있어(머리·찾기·툴바·몸)
  // 다섯째 줄을 세우기보다 머리가 두 줄로 접힌다. 머리띠는 이 파일이 지은
  // 노드라 마크업이 아닌 머리에게 묻는다.
  const head = view.querySelector(".file-view-head");
  let strip = head.querySelector(".artifact-strip");
  if (!tab.artifact) {
    strip?.remove();
  } else {
    if (!strip) {
      strip = artifactStripNode();
      head.prepend(strip);
    }
    paintArtifactStrip(strip, tab, (version) => {
      // A snapshot is a read-only view. The working file keeps its buffer,
      // stamp and undo history while an older version is on screen.
      if (version.path === tab.artifact.path) {
        tab.artifact.snapshot = null;
        if (stillShowing(tab)) paintFileView(tab);
        return;
      }
      invoke("read_text_file", { path: version.path })
        .then((opened) => {
          if (tab.artifact?.version !== version.n) return;
          tab.artifact.snapshot = { text: opened.text, path: version.path };
          if (stillShowing(tab)) paintFileView(tab);
        })
        .catch((error) => showError(String(error)));
    });
  }
  paintSaveNote(tab);
  body.scrollTop = 0;
}

/* ---- 마크다운 서식 툴바 — Orca's RichMarkdownToolbar (1-g21) ----
 *
 * The original rides a tiptap document; our markdown surface edits the
 * SOURCE in CodeMirror, so every button is a text transform instead of a
 * rich-document command: block buttons rewrite the selected lines' prefixes,
 * inline buttons wrap or unwrap the selection. Buttons, order, grouping and
 * the 28px chrome are the original's, measured (RichMarkdownToolbar.tsx,
 * rich-markdown-editor.css). Active lamps follow the caret the way tiptap's
 * `isActive` does — the block from the caret's line, the inline marks from
 * what surrounds the caret. */

// 한 줄의 '블록'을 말하는 무늬들. 물을 때는 그 하나, 걷을 때는 아래 한 벌.
// todo가 ul보다 먼저다 — `- [ ]`는 글머리 목록의 무늬로도 읽히므로.
const MD_BLOCK_MARKS = {
  h1: /^#\s+/,
  h2: /^#{2}\s+/,
  h3: /^#{3}\s+/,
  h4: /^#{4}\s+/,
  h5: /^#{5}\s+/,
  todo: /^[-*+]\s+\[[ xX]\]\s+/,
  ul: /^[-*+]\s+(?!\[[ xX]\]\s)/,
  ol: /^\d+[.)]\s+/,
  quote: /^>\s?/,
};
// 어떤 블록이든 벗기는 한 벌 — 새 옷을 입히기 전에 서 있던 무늬를 걷는다.
const MD_ANY_BLOCK = /^(#{1,6}\s+|[-*+]\s+\[[ xX]\]\s+|[-*+]\s+|\d+[.)]\s+|>\s?)/;

function mdBlockPrefix(kind, index) {
  if (kind === "ol") return `${index + 1}. `;
  return {
    p: "", h1: "# ", h2: "## ", h3: "### ", h4: "#### ", h5: "##### ",
    ul: "- ", todo: "- [ ] ", quote: "> ",
  }[kind];
}

/* 고른 줄들이 전부 그 블록을 이미 입고 있으면 벗기고(→ 본문), 아니면
 * 입힌다 — tiptap의 toggle과 같은 판정, 소스 위에서. */
function mdToggleBlock(editor, kind) {
  const { state } = editor;
  const range = state.selection.main;
  const first = state.doc.lineAt(range.from).number;
  const last = state.doc.lineAt(range.to).number;
  const lines = [];
  for (let at = first; at <= last; at += 1) lines.push(state.doc.line(at));
  const asks = MD_BLOCK_MARKS[kind];
  const worn = kind !== "p" && lines.every((line) => asks.test(line.text));
  const changes = lines.map((line, index) => ({
    from: line.from,
    to: line.to,
    insert: (worn ? "" : mdBlockPrefix(kind, index)) + line.text.replace(MD_ANY_BLOCK, ""),
  }));
  editor.dispatch({ changes, userEvent: "input.format" });
  editor.focus();
}

const MD_INLINE_MARKS = { bold: "**", italic: "*", strike: "~~" };

/* 선택을 두른 표가 이미 서 있으면 걷고, 없으면 두른다. 빈 선택은 표 한 쌍을
 * 심고 캐럿을 그 사이에 둔다. 이탤릭의 `*` 하나가 볼드의 `**` 속을 제
 * 것으로 오판하지 않게, 바깥 판정은 두 겹을 먼저 살핀다. */
function mdToggleInline(editor, kind) {
  const mark = MD_INLINE_MARKS[kind];
  const width = mark.length;
  const { state } = editor;
  const range = state.selection.main;
  const picked = state.doc.sliceString(range.from, range.to);
  const before = state.doc.sliceString(Math.max(0, range.from - width), range.from);
  const after = state.doc.sliceString(range.to, Math.min(state.doc.length, range.to + width));
  const beyond = kind === "italic" &&
    state.doc.sliceString(Math.max(0, range.from - 2), range.from) === "**" &&
    state.doc.sliceString(range.to, Math.min(state.doc.length, range.to + 2)) === "**";
  if (picked.startsWith(mark) && picked.endsWith(mark) && picked.length >= width * 2) {
    // 표까지 집어 골랐다 — 안쪽만 남긴다.
    editor.dispatch({
      changes: { from: range.from, to: range.to, insert: picked.slice(width, picked.length - width) },
      userEvent: "input.format",
    });
  } else if (before === mark && after === mark && !beyond) {
    editor.dispatch({
      changes: [
        { from: range.from - width, to: range.from, insert: "" },
        { from: range.to, to: range.to + width, insert: "" },
      ],
      userEvent: "input.format",
    });
  } else {
    editor.dispatch({
      changes: { from: range.from, to: range.to, insert: mark + picked + mark },
      selection: { anchor: range.from + width, head: range.to + width },
      userEvent: "input.format",
    });
  }
  editor.focus();
}

/* Orca의 링크 버블·이미지 픽커 자리 — 소스 위에서는 마크다운 그 자체를 심고
 * 주소 자리를 골라 준다: 다음 타이핑이 곧 주소다. */
function mdInsertLink(editor, image) {
  const { state } = editor;
  const range = state.selection.main;
  const picked = state.doc.sliceString(range.from, range.to);
  const label = picked || (image ? "" : t("md.linkText", "링크 텍스트"));
  const lead = `${image ? "!" : ""}[${label}](`;
  const url = "https://";
  editor.dispatch({
    changes: { from: range.from, to: range.to, insert: `${lead}${url})` },
    selection: { anchor: range.from + lead.length, head: range.from + lead.length + url.length },
    userEvent: "input.format",
  });
  editor.focus();
}

/* 줄 중간에서 불려도 블록은 제 줄에서 시작한다 — 앞에 줄바꿈 하나. */
function mdBlockDoor(state, from) {
  return from === 0 || state.doc.sliceString(from - 1, from) === "\n" ? "" : "\n";
}

function mdInsertCodeBlock(editor) {
  const { state } = editor;
  const range = state.selection.main;
  const picked = state.doc.sliceString(range.from, range.to);
  const door = mdBlockDoor(state, range.from);
  editor.dispatch({
    changes: { from: range.from, to: range.to, insert: `${door}\`\`\`\n${picked}\n\`\`\`\n` },
    // 캐럿은 펜스 뒤 — 언어 이름이 서는 자리.
    selection: { anchor: range.from + door.length + 3 },
    userEvent: "input.format",
  });
  editor.focus();
}

/* Orca More 메뉴의 '접는 섹션'(tiptap details) — 마크다운이 접을 줄 아는
 * 유일한 말은 HTML의 details라, 소스에는 그 말 그대로 선다. */
function mdInsertDetails(editor) {
  const { state } = editor;
  const range = state.selection.main;
  const picked = state.doc.sliceString(range.from, range.to);
  const title = t("md.detailsTitle", "제목");
  const lead = `${mdBlockDoor(state, range.from)}<details>\n<summary>`;
  editor.dispatch({
    changes: { from: range.from, to: range.to, insert: `${lead}${title}</summary>\n\n${picked}\n</details>\n` },
    selection: { anchor: range.from + lead.length, head: range.from + lead.length + title.length },
    userEvent: "input.format",
  });
  editor.focus();
}

/* 버튼의 차림표 — Orca의 줄과 칸막이 그대로: ¶ H1 H2 H3 | B I S | 목록 셋 |
 * 인용 링크 이미지 ⋯. B·I·S는 원본처럼 글자다. */
function mdToolbarActs() {
  return [
    { act: "p", glyph: "pilcrow", label: t("md.body", "본문") },
    { act: "h1", glyph: "heading-1", label: t("md.h1", "제목 1") },
    { act: "h2", glyph: "heading-2", label: t("md.h2", "제목 2") },
    { act: "h3", glyph: "heading-3", label: t("md.h3", "제목 3") },
    null,
    { act: "bold", said: "B", label: t("md.bold", "굵게") },
    { act: "italic", said: "I", label: t("md.italic", "기울임") },
    { act: "strike", said: "S", label: t("md.strike", "취소선") },
    null,
    { act: "ul", glyph: "list", label: t("md.ul", "글머리 목록") },
    { act: "ol", glyph: "list-ordered", label: t("md.ol", "번호 목록") },
    { act: "todo", glyph: "list-todo", label: t("md.todo", "체크리스트") },
    null,
    { act: "quote", glyph: "quote", label: t("md.quote", "인용") },
    { act: "link", glyph: "link", label: t("md.link", "링크") },
    { act: "image", glyph: "image", label: t("md.image", "이미지") },
    { act: "more", glyph: "more", label: t("md.more", "다른 블록") },
  ];
}

function buildMdToolbar(host) {
  for (const row of mdToolbarActs()) {
    if (row === null) {
      const sep = document.createElement("div");
      sep.className = "md-toolbar-sep";
      host.appendChild(sep);
      continue;
    }
    const button = document.createElement("button");
    button.type = "button";
    button.className = "md-toolbar-button";
    button.dataset.act = row.act;
    button.dataset.tip = row.label;
    button.setAttribute("aria-label", row.label);
    if (row.glyph) button.innerHTML = icon(row.glyph);
    else button.textContent = row.said;
    host.appendChild(button);
  }
  // ⋯의 접이 — Orca 드롭다운의 넷: H4, H5, 코드 블록, 접는 섹션.
  const pop = document.createElement("div");
  pop.className = "md-more-pop";
  pop.hidden = true;
  for (const row of [
    { act: "h4", glyph: "heading-4", label: t("md.h4", "제목 4") },
    { act: "h5", glyph: "heading-5", label: t("md.h5", "제목 5") },
    { act: "codeblock", glyph: "code", label: t("md.codeBlock", "코드 블록") },
    { act: "details", glyph: "chevron", label: t("md.details", "접는 섹션") },
  ]) {
    const pick = document.createElement("button");
    pick.type = "button";
    pick.className = "note-pop-row md-more-row";
    pick.dataset.act = row.act;
    pick.innerHTML = icon(row.glyph);
    const name = document.createElement("span");
    name.className = "note-pop-name";
    name.textContent = row.label;
    pick.appendChild(name);
    pop.appendChild(pick);
  }
  host.appendChild(pop);
}

function runMdToolbarAct(view, tab, act, host) {
  const pop = host.querySelector(".md-more-pop");
  if (act === "more") {
    pop.hidden = !pop.hidden;
    return;
  }
  if (pop) pop.hidden = true;
  const editor = editorShowing(tab);
  if (!editor) return;
  if (act === "bold" || act === "italic" || act === "strike") mdToggleInline(editor, act);
  else if (act === "link") mdInsertLink(editor, false);
  else if (act === "image") mdInsertLink(editor, true);
  else if (act === "codeblock") mdInsertCodeBlock(editor);
  else if (act === "details") mdInsertDetails(editor);
  else mdToggleBlock(editor, act);
  paintMdToolbarActive(view);
}

/* 이 판의 툴바를 세운다 — 마크다운의 원본(편집) 면에서만. 버튼은 한 번
 * 지어지고(복제 판의 죽은 마크업 포함), 듣기는 매 그림마다 대입된다 —
 * 이 창의 모든 복제 표면과 같은 규칙. */
function paintMdToolbar(view, tab, showing) {
  const host = view.querySelector(".md-toolbar");
  if (!host) return;
  const held = tab.mode === "markdown" && showing === "source";
  host.hidden = !held;
  if (!held) return;
  if (host.childElementCount === 0) buildMdToolbar(host);
  host.onclick = (event) => {
    const button = event.target.closest?.(".md-toolbar-button, .md-more-row");
    if (button) runMdToolbarAct(view, tab, button.dataset.act, host);
  };
  // 포커스가 에디터에서 버튼으로 새지 않게 — 원본의 onMouseDown 그대로.
  host.onmousedown = (event) => event.preventDefault();
  paintMdToolbarActive(view);
}

/* tiptap의 isActive를 소스에서: 블록은 캐럿 줄의 무늬로, 인라인은 캐럿
 * 왼쪽에 홀수 번 선 표로 판정한다 — 정밀한 파서라기보다, 원본과 같은
 * 자리에서 켜지는 등이다. */
function paintMdToolbarActive(view) {
  const host = view.querySelector(".md-toolbar");
  if (!host || host.hidden || host.childElementCount === 0) return;
  const tab = tabs.find((one) => one.id === view.dataset.tab);
  const editor = tab && editorShowing(tab);
  if (!editor) return;
  const head = editor.state.selection.main.head;
  const line = editor.state.doc.lineAt(head);
  let block = "p";
  for (const [kind, asks] of Object.entries(MD_BLOCK_MARKS)) {
    if (asks.test(line.text)) {
      block = kind;
      break;
    }
  }
  const left = line.text.slice(0, head - line.from);
  const insideMark = (mark) => {
    const stood = mark === "*"
      ? left.replaceAll("**", "").split("*").length - 1
      : left.split(mark).length - 1;
    return stood % 2 === 1;
  };
  const on = {
    // ¶ 등은 없다 — 모든 본문 줄에서 켜져 있는 등은 등이 아니라 소음이고,
    // 원본도 ¶에는 불을 달지 않았다.
    h1: block === "h1", h2: block === "h2", h3: block === "h3",
    ul: block === "ul", ol: block === "ol", todo: block === "todo",
    quote: block === "quote",
    bold: insideMark("**"), italic: insideMark("*"), strike: insideMark("~~"),
  };
  for (const button of host.querySelectorAll(".md-toolbar-button")) {
    button.classList.toggle("is-active", on[button.dataset.act] === true);
  }
}

// 바깥 클릭이 ⋯의 접이를 닫는다 — 팔레트의 dismissable과 같은 판정을 이
// 작은 접이 하나가 통째로 빌릴 이유는 없어서, 한 줄로.
document.addEventListener("pointerdown", (event) => {
  if (event.target.closest?.(".md-toolbar")) return;
  for (const pop of document.querySelectorAll(".md-more-pop:not([hidden])")) pop.hidden = true;
});

/* ---- the editor ----
 *
 * Orca's `EditorPanel` spot, and now the same working surface it has.
 *
 * WHAT ORCA ACTUALLY RUNS, measured off `asar-1.4.164` rather than assumed:
 * Monaco 0.55.1, 15 MB of editor on disk, +22 MB of main-thread heap, five
 * worker kinds — including an 11 MB TypeScript 5.9.3 isolate that starts for
 * every `.ts` tab. And with all of that: `defineTheme` is called zero times,
 * `MonacoLspClient` is bundled and never constructed, `setDiagnosticsOptions`
 * turns off all three diagnostic channels, `setExtraLibs` is never called, and
 * the minimap is off by default. What the person actually gets is syntax
 * highlighting, find and replace, folding, multi-cursor, bracket matching and
 * an external-change banner.
 *
 * That list is exactly what `ui/vendor/cm6.js` does, in 1.38 MB on one thread
 * with no worker and no `blob:` URL — so the CSP stayed `default-src 'self'`.
 * The optional minimap below reads this same CodeMirror document and scroll
 * owner. It is sampled to a fixed ceiling instead of introducing another
 * editor model, worker or runtime dependency.
 *
 * WHAT DID NOT CHANGE is the model underneath. The tab still holds `draft`,
 * which starts as what was read and is what gets written, and `text` still
 * holds the last thing that was on disk — so "has this changed" is still a
 * comparison rather than a flag somebody has to remember to clear. Every
 * keystroke in CodeMirror is copied back into `draft`, which is what keeps
 * `isDirty`, the strip's mark, ⌘S and the restored-draft record all reading
 * the same one fact.
 *
 * ONE EDITOR PER LEAF, not per tab — the same rule the file surface itself
 * follows. What is per TAB is the document STATE: switching away parks a
 * tab's `EditorState` on the tab and switching back hands it straight to the
 * leaf, so the undo stack, the selection and the fold state survive a tab
 * switch. That is the one place we are ahead of the textarea this replaces,
 * which lost all three every time (`input.value` re-assignment). */

/* The ramp, as a highlight style. Every colour is a `var()` into
 * `ui/tokens.css`, so the treatment switch repaints the editor with the rest
 * of the window and there is no second theme table to keep in step. Orca
 * picks `vs-dark` or `vs` with a boolean and paints its chrome to match
 * Monaco; this goes the other way. */
let editorHighlight = null;

function editorHighlightStyle() {
  if (editorHighlight) return editorHighlight;
  const { HighlightStyle, tags } = window.CM6;
  editorHighlight = HighlightStyle.define([
    {
      tag: [
        tags.keyword,
        tags.controlKeyword,
        tags.definitionKeyword,
        tags.moduleKeyword,
        tags.operatorKeyword,
        tags.modifier,
        tags.self,
        tags.null,
      ],
      color: "var(--syntax-keyword)",
    },
    {
      tag: [tags.string, tags.special(tags.string), tags.regexp, tags.character],
      color: "var(--syntax-string)",
    },
    { tag: [tags.number, tags.integer, tags.float, tags.bool], color: "var(--syntax-number)" },
    {
      tag: [tags.comment, tags.lineComment, tags.blockComment, tags.docComment],
      color: "var(--syntax-comment)",
      fontStyle: "italic",
    },
    {
      tag: [tags.function(tags.variableName), tags.function(tags.propertyName), tags.macroName],
      color: "var(--syntax-function)",
    },
    {
      tag: [tags.typeName, tags.className, tags.namespace, tags.definition(tags.typeName)],
      color: "var(--syntax-type)",
    },
    {
      tag: [tags.variableName, tags.propertyName, tags.definition(tags.variableName)],
      color: "var(--syntax-variable)",
    },
    {
      tag: [tags.constant(tags.variableName), tags.standard(tags.variableName), tags.atom],
      color: "var(--syntax-constant)",
    },
    { tag: [tags.operator, tags.derefOperator, tags.compareOperator], color: "var(--syntax-operator)" },
    { tag: [tags.punctuation, tags.separator, tags.bracket], color: "var(--syntax-punctuation)" },
    { tag: [tags.tagName, tags.angleBracket], color: "var(--syntax-tag)" },
    { tag: [tags.attributeName, tags.attributeValue], color: "var(--syntax-attribute)" },
    { tag: [tags.meta, tags.annotation, tags.processingInstruction], color: "var(--syntax-meta)" },
    { tag: [tags.link, tags.url], color: "var(--syntax-link)", textDecoration: "underline" },
    { tag: tags.heading, color: "var(--syntax-function)", fontWeight: "600" },
    { tag: tags.emphasis, fontStyle: "italic" },
    { tag: tags.strong, fontWeight: "600" },
    { tag: tags.strikethrough, textDecoration: "line-through" },
    { tag: tags.invalid, color: "var(--syntax-invalid)" },
  ]);
  return editorHighlight;
}

/* The editor's furniture, in the same currency. Written here rather than in
 * `ui/shell.css` because CodeMirror's own base rules are injected at runtime
 * and a stylesheet rule would have to out-specify them by hand; a theme
 * extension is placed after them by construction. */
let editorTheme = null;

function editorThemeExtension() {
  if (editorTheme) return editorTheme;
  editorTheme = window.CM6.EditorView.theme({
    "&": {
      height: "100%",
      backgroundColor: "var(--editor-surface)",
      color: "var(--ink-chalk)",
      // The terminal's own text size plus the persisted ⌘± level — Orca's
      // editor base IS terminalFontSize (editor-font-zoom.ts:21,
      // useIpcEvents.ts:3164), one setting for every monospace surface.
      // The token stands in only until the first settings snapshot lands.
      fontSize: "var(--editor-font-size, var(--text-xs))",
    },
    ".cm-scroller": {
      fontFamily: "var(--editor-font-family, var(--font-mono))",
      lineHeight: "1.5",
    },
    ".cm-content": { padding: "var(--space-3) 0", caretColor: "var(--editor-cursor)" },
    ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--editor-cursor)" },
    // One colour on every layer CodeMirror paints a selection with, and the
    // focused selector is shaped to WIN. CodeMirror's base theme paints its
    // stock focused colour on `&light.cm-focused > .cm-scroller >
    // .cm-selectionLayer .cm-selectionBackground` (#d7d4f0) — with `&light`
    // expanding to its light class, five classes deep. Our old
    // `&.cm-focused .cm-selectionBackground` was three, and a shorter
    // selector loses on specificity whatever its order: the visible selection
    // in a dark window was CodeMirror's pale lavender for months while our
    // token sat unapplied (adversarial probe 2026-09-09, `editor-selection.mjs`).
    // Mirroring the base theme's exact chain ties it at five, and a user theme
    // is mounted after the base theme, so the tie is ours. (`&light`/`&dark`
    // are base-theme-only placeholders — a user theme may not name them.) The
    // `::selection` pair below is what the browser paints over the glyphs on
    // top of these layers.
    "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground": {
      backgroundColor: "var(--editor-selection)",
    },
    ".cm-content ::selection, .cm-line ::selection": {
      backgroundColor: "var(--editor-selection)",
      // The solid selection carries the terminal's own selection foreground,
      // so selected prose stays legible the way Codex CLI's does.
      color: "var(--term-selection-fg)",
    },
    ".cm-activeLine": { backgroundColor: "var(--editor-line-band)" },
    ".cm-gutters": {
      backgroundColor: "var(--editor-surface)",
      color: "var(--editor-gutter-ink)",
      border: "0",
      borderRight: "var(--rule-width) solid var(--edge-rule)",
    },
    ".cm-activeLineGutter": {
      backgroundColor: "var(--editor-line-band)",
      color: "var(--editor-gutter-ink-active)",
    },
    ".cm-foldGutter .cm-gutterElement": { color: "var(--editor-fold-ink)", cursor: "pointer" },
    ".cm-foldPlaceholder": {
      padding: "0 var(--space-2)",
      border: "var(--rule-width) solid var(--edge-rule)",
      borderRadius: "var(--radius-sm)",
      background: "var(--surface-raised)",
      color: "var(--ink-mist)",
    },
    // An outline rather than a fill: a filled bracket at the caret reads as a
    // selection, and there is already one of those on screen.
    "&.cm-focused .cm-matchingBracket, .cm-matchingBracket": {
      backgroundColor: "transparent",
      outline: "var(--rule-width) solid var(--editor-bracket)",
    },
    "&.cm-focused .cm-nonmatchingBracket, .cm-nonmatchingBracket": {
      backgroundColor: "transparent",
      outline: "var(--rule-width) solid var(--syntax-invalid)",
    },
    // The find bar's own colours, and they are the window's existing search
    // pair rather than a new one — `--markdown-search-match` already means
    // "a hit" everywhere else this window shows one.
    ".cm-selectionMatch": {
      backgroundColor: "color-mix(in srgb, var(--markdown-search-match) 26%, transparent)",
    },
    ".cm-searchMatch": {
      backgroundColor: "color-mix(in srgb, var(--markdown-search-match) 26%, transparent)",
    },
    ".cm-searchMatch.cm-searchMatch-selected": {
      backgroundColor: "color-mix(in srgb, var(--markdown-search-match-active) 40%, transparent)",
    },
    ".cm-tooltip": {
      border: "var(--rule-width) solid var(--edge-rule)",
      borderRadius: "var(--radius-sm)",
      background: "var(--surface-raised)",
      color: "var(--ink-chalk)",
      fontFamily: "var(--font-ui)",
      fontSize: "var(--text-2xs)",
    },
    ".cm-tooltip-autocomplete > ul > li[aria-selected]": {
      background: "var(--row-selected)",
      color: "var(--ink-chalk)",
    },
  });
  return editorTheme;
}

const editorWrapCompartment = new window.CM6.Compartment();
const diffWrapCompartment = new window.CM6.Compartment();
let editingPrefs = null;
let editingPrefsSpec = null;
let systemFontSuggestions = [];
let systemFontSuggestionsPromise = null;

/* CodeMirror deliberately renders plain document text rather than anchors.
 * Find only the http(s) token under a primary-modifier click; the setting's
 * Shift rule is then decided by `routeHttpLink`, not duplicated here. */
function editorHttpLinkAt(view, event) {
  if (!hasPrimaryModifier(event)) return null;
  const position = view.posAtCoords({ x: event.clientX, y: event.clientY });
  if (position === null) return null;
  const line = view.state.doc.lineAt(position);
  const offset = position - line.from;
  const links = /\bhttps?:\/\/[^\s<>"'\x60]+/gi;
  for (let match = links.exec(line.text); match !== null; match = links.exec(line.text)) {
    const href = match[0].replace(/[\])},.;!?]+$/, "");
    if (offset >= match.index && offset < match.index + href.length) return href;
  }
  return null;
}

/* What every document is opened with. The language is baked into the state
 * rather than swapped through a compartment, because a state is built per tab
 * anyway — a compartment would be a second way to say the same thing. */
function editorExtensions(tab) {
  const CM = window.CM6;
  return [
    CM.lineNumbers(),
    CM.highlightActiveLineGutter(),
    CM.highlightActiveLine(),
    CM.highlightSpecialChars(),
    CM.foldGutter(),
    CM.history(),
    CM.drawSelection(),
    CM.dropCursor(),
    CM.rectangularSelection(),
    CM.crosshairCursor(),
    CM.indentOnInput(),
    CM.bracketMatching(),
    CM.closeBrackets(),
    CM.autocompletion(),
    CM.highlightSelectionMatches(),
    CM.search(),
    findWash().field,
    // Multi-cursor, which is one facet and a modifier-click away rather than
    // a feature: Orca gets it from Monaco's standalone contributions, we get
    // it from here.
    CM.EditorState.allowMultipleSelections.of(true),
    // Orca's file editor defaults `editorWordWrap` to ON (`=== false ? "off"
    // : "on"`), and the textarea this replaces was pinned `wrap="off"` — a
    // long line meant a horizontal scrollbar under every other line.
    editorWrapCompartment.of(editingPrefs?.editor_word_wrap ? CM.EditorView.lineWrapping : []),
    // Words already in the file, which is the only completion source with
    // nothing behind it. Worth saying plainly: Orca's is bigger on paper and
    // barely bigger in practice — its TypeScript worker is never given a
    // tsconfig or a node_modules (`setExtraLibs` call count: zero), so it
    // completes from `lib.d.ts` and the one open file.
    CM.EditorState.languageData.of(() => [{ autocomplete: CM.completeAnyWord }]),
    // NOT `searchKeymap`. ⌘F belongs to this window's own chord table, which
    // opens the bar below; binding it here as well would open CodeMirror's
    // panel AND our bar on one keystroke.
    // 마크다운의 세 화음 — 문서에는 서식이고 코드에는 없는 말이라 여기서만.
    tab.mode === "markdown"
      ? CM.keymap.of([
          { key: "Mod-b", run: (editor) => (mdToggleInline(editor, "bold"), true) },
          { key: "Mod-i", run: (editor) => (mdToggleInline(editor, "italic"), true) },
          { key: "Mod-Shift-x", run: (editor) => (mdToggleInline(editor, "strike"), true) },
        ])
      : [],
    CM.keymap.of([
      ...CM.closeBracketsKeymap,
      ...CM.defaultKeymap,
      ...CM.historyKeymap,
      ...CM.foldKeymap,
      ...CM.completionKeymap,
      CM.indentWithTab,
    ]),
    editorThemeExtension(),
    CM.syntaxHighlighting(editorHighlightStyle()),
    // Nothing claims this extension — a plain-text file, and every file whose
    // extension is not in the vendor's table.
    window.CM6.languageFor(tab.path) ?? [],
    CM.EditorView.domEventHandlers({
      click(event, view) {
        const href = editorHttpLinkAt(view, event);
        if (href === null) return false;
        event.preventDefault();
        routeHttpLink(href, event);
        return true;
      },
    }),
    CM.EditorView.updateListener.of(onEditorUpdate),
  ];
}

/* The live editors, one per split leaf, keyed by the leaf the way every other
 * per-leaf surface in this window is. `showing` is the id of the tab whose
 * document is mounted right now, which is how an update knows whose typing it
 * is carrying. */
const editorViews = new Map();

/* Orca's minimap is a file-editor overview, not a second document model. This
 * controller reads the mounted CodeMirror state and its one scroll owner. The
 * trace is sampled to a fixed ceiling so a generated file cannot turn one
 * preference into thousands of DOM nodes or unbounded paint work. */
const EDITOR_MINIMAP_MAX_SAMPLES = 320;
const EDITOR_MINIMAP_COLUMNS = 80;
const EDITOR_MINIMAP_INDENT_COLUMNS = 12;
const EDITOR_MINIMAP_TAB_COLUMNS = 2;
const EDITOR_MINIMAP_KEY_STEP = 0.1;

function editorMinimapLineGeometry(text) {
  let first = 0;
  let indent = 0;
  while (first < text.length && (text[first] === " " || text[first] === "\t")) {
    indent += text[first] === "\t" ? EDITOR_MINIMAP_TAB_COLUMNS : 1;
    first += 1;
  }
  const content = text.slice(first).trimEnd();
  if (!content) return null;
  const x = Math.min(EDITOR_MINIMAP_INDENT_COLUMNS, indent);
  const width = Math.max(2, Math.min(EDITOR_MINIMAP_COLUMNS - x, content.length));
  return { x, width };
}

function refreshEditorMinimapViewport(held) {
  const minimap = held?.minimap;
  if (!minimap || minimap.root.hidden) return;
  const scroller = held.editor.scrollDOM;
  const scrollHeight = Math.max(scroller.clientHeight, scroller.scrollHeight);
  const maximum = Math.max(0, scrollHeight - scroller.clientHeight);
  const height = scrollHeight > 0 ? Math.min(1, scroller.clientHeight / scrollHeight) : 1;
  const travel = 1 - height;
  const top = maximum > 0 ? (scroller.scrollTop / maximum) * travel : 0;
  minimap.window.style.top = `${top * 100}%`;
  minimap.window.style.height = `${height * 100}%`;
  minimap.root.setAttribute("aria-valuemax", String(Math.round(maximum)));
  minimap.root.setAttribute("aria-valuenow", String(Math.round(scroller.scrollTop)));
}

function refreshEditorMinimapDocument(held) {
  const minimap = held?.minimap;
  if (!minimap || minimap.root.hidden) return;
  const document = held.editor.state.doc;
  const samples = Math.min(document.lines, EDITOR_MINIMAP_MAX_SAMPLES);
  const commands = [];
  for (let index = 0; index < samples; index += 1) {
    const lineNumber = Math.min(
      document.lines,
      Math.floor((index * document.lines) / Math.max(1, samples)) + 1,
    );
    const geometry = editorMinimapLineGeometry(document.line(lineNumber).text);
    if (geometry) commands.push(`M${geometry.x} ${index + 0.5}h${geometry.width}`);
  }
  minimap.svg.setAttribute("viewBox", `0 0 ${EDITOR_MINIMAP_COLUMNS} ${Math.max(1, samples)}`);
  minimap.trace.setAttribute("d", commands.join(""));
  refreshEditorMinimapViewport(held);
}

function seekEditorMinimap(held, clientY) {
  const minimap = held?.minimap;
  if (!minimap) return;
  const bounds = minimap.root.getBoundingClientRect();
  if (bounds.height <= 0) return;
  const ratio = Math.min(1, Math.max(0, (clientY - bounds.top) / bounds.height));
  const scroller = held.editor.scrollDOM;
  scroller.scrollTop = ratio * Math.max(0, scroller.scrollHeight - scroller.clientHeight);
  refreshEditorMinimapViewport(held);
}

function createEditorMinimap(held, host) {
  const namespace = "http://www.w3.org/2000/svg";
  const root = document.createElement("div");
  root.className = "file-minimap";
  root.tabIndex = 0;
  root.setAttribute("role", "scrollbar");
  root.setAttribute("aria-orientation", "vertical");
  root.setAttribute("aria-valuemin", "0");

  const svg = document.createElementNS(namespace, "svg");
  svg.classList.add("file-minimap-code");
  svg.setAttribute("preserveAspectRatio", "none");
  svg.setAttribute("aria-hidden", "true");
  const trace = document.createElementNS(namespace, "path");
  trace.classList.add("file-minimap-trace");
  svg.appendChild(trace);
  const viewport = document.createElement("span");
  viewport.className = "file-minimap-window";
  viewport.setAttribute("aria-hidden", "true");
  root.append(svg, viewport);

  held.minimap = { root, svg, trace, window: viewport, pointer: null, resize: null };
  root.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    event.preventDefault();
    held.minimap.pointer = event.pointerId;
    root.setPointerCapture?.(event.pointerId);
    root.focus({ preventScroll: true });
    seekEditorMinimap(held, event.clientY);
  });
  root.addEventListener("pointermove", (event) => {
    if (held.minimap?.pointer === event.pointerId) seekEditorMinimap(held, event.clientY);
  });
  const finishPointer = (event) => {
    if (held.minimap?.pointer !== event.pointerId) return;
    held.minimap.pointer = null;
    if (root.hasPointerCapture?.(event.pointerId)) root.releasePointerCapture(event.pointerId);
  };
  root.addEventListener("pointerup", finishPointer);
  root.addEventListener("pointercancel", finishPointer);
  root.addEventListener("keydown", (event) => {
    const scroller = held.editor.scrollDOM;
    const maximum = Math.max(0, scroller.scrollHeight - scroller.clientHeight);
    const smallStep = Math.max(1, scroller.clientHeight * EDITOR_MINIMAP_KEY_STEP);
    let next = null;
    if (event.key === "ArrowUp") next = scroller.scrollTop - smallStep;
    else if (event.key === "ArrowDown") next = scroller.scrollTop + smallStep;
    else if (event.key === "PageUp") next = scroller.scrollTop - scroller.clientHeight;
    else if (event.key === "PageDown") next = scroller.scrollTop + scroller.clientHeight;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = maximum;
    if (next === null) return;
    event.preventDefault();
    scroller.scrollTop = Math.min(maximum, Math.max(0, next));
    refreshEditorMinimapViewport(held);
  });

  host.appendChild(root);
  held.minimap.resize = new ResizeObserver(() => refreshEditorMinimapViewport(held));
  held.minimap.resize.observe(host);
}

function syncEditorMinimap(held) {
  const host = held?.editor.dom.parentElement;
  if (!host?.classList.contains("file-edit")) return;
  const enabled = editingPrefs?.editor_minimap_enabled === true;
  if (enabled && !held.minimap) createEditorMinimap(held, host);
  host.classList.toggle("has-minimap", enabled);
  if (!held.minimap) return;
  held.minimap.root.hidden = !enabled;
  held.minimap.root.setAttribute("aria-label", t("settings.editing.minimap", "미니맵"));
  if (enabled) refreshEditorMinimapDocument(held);
}

function editorEntryOf(view) {
  for (const held of editorViews.values()) if (held.editor === view) return held;
  return null;
}

/* The editor in front of this tab, or null when the tab is not the one its
 * leaf is showing. Every caller that wants a caret, a scroll position or a
 * search has to ask this rather than reach for a leaf's editor directly:
 * a background tab's leaf is showing somebody else's document. */
function editorShowing(tab) {
  const held = editorViews.get(tab?.pane);
  return held && held.showing === tab.id ? held.editor : null;
}

/* Browser spelling belongs only to Markdown documents. CodeMirror owns one
 * contenteditable node per visible leaf, so the preference follows the tab
 * seated in that leaf instead of becoming a blanket attribute on every file
 * editor. This is the native browser/OS checker Orca enables in its rich
 * Markdown editor; no dictionary or second spellchecking engine lives here. */
function syncMarkdownSpellcheck(held) {
  if (!held) return;
  const tab = tabs.find((one) => one.id === held.showing);
  const enabled =
    tab?.kind === "file" &&
    tab.mode === "markdown" &&
    editingPrefs?.rich_markdown_spellcheck_enabled !== false;
  held.editor.contentDOM.setAttribute("spellcheck", enabled ? "true" : "false");
}

/* A leaf folded away. The element is about to be removed from the stage, and
 * an EditorView that is dropped without `destroy()` leaves its DOM listeners
 * and its measure loop behind — the exact shape of leak `dropTermView` exists
 * to prevent for shells. */
function dropEditorView(group) {
  const held = editorViews.get(group);
  if (!held) return;
  held.minimap?.resize?.disconnect();
  held.minimap?.root.remove();
  held.editor.destroy();
  editorViews.delete(group);
}

function editorStateFor(tab) {
  const CM = window.CM6;
  const seen = scrollCache.get(scrollKeyOf(tab));
  // Clamped: the file on disk may be shorter than it was when the eye was
  // last here, and a selection past the end throws rather than degrading.
  const at = Math.min(seen?.caret ?? 0, tab.draft.length);
  return CM.EditorState.create({
    doc: tab.draft,
    selection: { anchor: at },
    extensions: editorExtensions(tab),
  });
}

/* One debounce owner for both editable document surfaces. A timer belongs to
 * the tab, not the mounted editor: background tabs keep their draft and a
 * split switching surfaces must not move the pending write to another file. */
function cancelAutoSave(tab) {
  if (!tab) return;
  tab.autoSaveGeneration = (tab.autoSaveGeneration ?? 0) + 1;
  if (tab.autoSaveTimer != null) window.clearTimeout(tab.autoSaveTimer);
  tab.autoSaveTimer = null;
}

function canAutoSave(tab) {
  return editingPrefs?.editor_auto_save === true &&
    tabs.includes(tab) &&
    (tab.kind === "file" || (tab.kind === "diff" && tab.version != null)) &&
    isDirty(tab) &&
    !tab.stale &&
    !tab.gone;
}

async function runAutoSave(tab, generation) {
  // A manual save already in flight owns the optimistic file version. Wait
  // for its exact answer, then decide again from the current draft instead of
  // racing it with the old version.
  while (tab.saveInFlight) await tab.saveInFlight;
  if (tab.autoSaveGeneration !== generation || !canAutoSave(tab)) return;
  await saveFile(tab);
}

function scheduleAutoSave(tab) {
  cancelAutoSave(tab);
  if (!canAutoSave(tab)) return;
  const delay = editingPrefs.editor_auto_save_delay_ms;
  if (!Number.isInteger(delay)) return;
  const generation = tab.autoSaveGeneration;
  const timer = window.setTimeout(() => {
    if (tab.autoSaveTimer === timer) tab.autoSaveTimer = null;
    void runAutoSave(tab, generation);
  }, delay);
  tab.autoSaveTimer = timer;
}

function rescheduleAutoSaves() {
  for (const tab of tabs) scheduleAutoSave(tab);
}

/* Everything one keystroke has to move. Split out of the update handler
 * because a save, a reload and a restore all arrive as document changes too,
 * and only this one is the person. */
function editorTyped(tab, typed) {
  const was = isDirty(tab);
  tab.draft = typed;
  tab.saved = false;
  // Typing is the strongest possible claim on a tab: a glance the person
  // is editing stopped being a glance (Orca's `markFileDirty` clears
  // `isPreview`, I18nProvider:73000) — and a preview that stayed one would
  // be REPLACED by the next glance, typing and all.
  const glanced = tab.preview;
  if (glanced) tab.preview = false;
  persistStageLayouts({ deferred: true, worktree: tab.worktree });
  paintSaveNote(tab);
  scheduleAutoSave(tab);
  // The strip carries the mark, so it is redrawn when the mark appears or
  // goes — not on every keystroke, which would rebuild it per character.
  if (was !== isDirty(tab) || glanced) renderTabs();
}

function onEditorUpdate(update) {
  const held = editorEntryOf(update.view);
  const owner = held && tabs.find((tab) => tab.id === held.showing);
  if (!owner) return;
  if (update.docChanged) {
    const typed = update.state.doc.toString();
    // The document was just MADE equal to the draft — a re-read landing, or a
    // tab being seated. Nothing was typed, so nothing is marked: recording it
    // would clear the "저장했습니다" note on the save that just wrote it.
    if (typed !== owner.draft) editorTyped(owner, typed);
    // The draft and the mounted document are now the same string, by
    // reference. `paintEditor` compares them to decide whether something
    // OUTSIDE the editor moved the draft underneath it, and that comparison
    // is what this assignment keeps honest.
    owner.cmText = owner.draft;
    refreshEditorMinimapDocument(held);
  }
  if (update.docChanged || update.selectionSet) rememberEditorPlace(update.view, owner);
  // 마크다운 툴바의 등은 캐럿을 따라다닌다 — 켜져 있을 때만 잰다.
  if ((update.docChanged || update.selectionSet) && owner.mode === "markdown") {
    const view = held.editor.dom.closest(".file-view");
    if (view) paintMdToolbarActive(view);
  }
}

/* Where the eye is in this document, recorded as it moves.
 *
 * Not captured at the switch: the leaf's editor is shared between tabs, and by
 * the time a switch repaints it is already showing the next document.
 *
 * A LINE, not a pixel, and the difference is not pedantry — it is what makes
 * the restore land. A pixel is a number in a coordinate space CodeMirror has
 * not finished measuring: seat a document at scroll 140 and the measure pass
 * re-anchors it to 168, so the eye comes back two lines below where it left.
 * A document offset survives the measure, the window being resized and the
 * text getting bigger, because the thing being remembered is the line the
 * person was reading. */
function rememberEditorPlace(editor, tab) {
  const block = editor.lineBlockAtHeight(editor.scrollDOM.scrollTop);
  cacheScroll(scrollKeyOf(tab), {
    at: block.from,
    caret: editor.state.selection.main.head,
  });
}

/* And put it back, one frame after the document is seated.
 *
 * The frame is not a guess, it is the measurement. A newly seated document has
 * ESTIMATED line heights until CodeMirror lays it out: measured on this
 * window, a 200-line file reported a 2,982px scroll height at the moment it
 * was seated and 3,642px one frame later. A scroll target worked out against
 * the first number is worked out against a document that does not exist, which
 * is why every attempt to do this synchronously — assigning `scrollTop`,
 * asking for a measure, dispatching the effect — left the file at its top.
 *
 * The guard is the same one every deferred repaint in this window carries: a
 * frame is long enough for the person to have switched tabs, and scrolling
 * then would move a document they are no longer looking at. */
function restoreEditorPlace(held, tab, at) {
  requestAnimationFrame(() => {
    if (held.showing !== tab.id) return;
    const editor = held.editor;
    editor.dispatch({
      // Clamped, because the file on disk may have got shorter since.
      effects: window.CM6.EditorView.scrollIntoView(Math.min(at, editor.state.doc.length), {
        y: "start",
      }),
    });
  });
}

function paintEditor(view, tab) {
  const host = view.querySelector(".file-edit");
  if (tab.draft === undefined) tab.draft = tab.text;
  let held = editorViews.get(tab.pane);
  if (!held) {
    held = { editor: new window.CM6.EditorView({ state: editorStateFor(tab), parent: host }), showing: tab.id };
    editorViews.set(tab.pane, held);
    tab.cmText = tab.draft;
    // Scrolling on its own moves neither the document nor the selection, so
    // the update listener never hears about it. A scroll event does not
    // bubble either, which is why this is on the scroller itself rather than
    // among the editor's own DOM handlers.
    held.editor.scrollDOM.addEventListener("scroll", () => {
      const owner = tabs.find((one) => one.id === held.showing);
      if (owner) rememberEditorPlace(held.editor, owner);
      refreshEditorMinimapViewport(held);
    });
  } else if (held.showing !== tab.id) {
    // The outgoing document is parked on ITS tab, so coming back is a
    // hand-over rather than a re-read: undo stack, folds and selection
    // included. Orca keeps its Monaco model the same way (`keepCurrentModel:
    // true`); the textarea this replaces kept none of it.
    const leaving = tabs.find((other) => other.id === held.showing);
    if (leaving) leaving.cmState = held.editor.state;
    const parked = tab.cmState;
    held.editor.setState(parked ?? editorStateFor(tab));
    held.showing = tab.id;
    // A fresh state was built FROM the draft; a parked one describes whatever
    // the draft was when it was parked, and `cmText` already says which.
    if (!parked) tab.cmText = tab.draft;
  }
  const editor = held.editor;
  // Something other than the editor moved the draft — a re-read from disk, or
  // a restored session's saved typing. Replaced as a transaction rather than
  // a new state so the undo stack and the selection survive it.
  if (tab.cmText !== tab.draft) {
    editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: tab.draft } });
    tab.cmText = tab.draft;
  }
  // The label lives on the host in ui/index.html, because `applyLocale` walks
  // markup and CodeMirror's editable node is built at runtime. Copied across
  // on every repaint, which is also every locale change.
  editor.contentDOM.setAttribute("aria-label", host.getAttribute("aria-label") ?? "");
  syncMarkdownSpellcheck(held);
  // Where the eye was, put back on mount — Orca restores its two caches at
  // the same moment (`MonacoEditor:937`). The caret rode in with the state;
  // the line is asked for separately, and one frame later, for the reason
  // written over `restoreEditorPlace`.
  const seen = scrollCache.get(scrollKeyOf(tab));
  if (seen?.at) restoreEditorPlace(held, tab, seen.at);
  syncEditorMinimap(held);
}

/* What the header says about this file's edits.
 *
 * Four states, and the order matters: the file being GONE outranks it having
 * been rewritten, which outranks the person's own unsaved mark — each one up
 * changes what saving would do more than the one below it.
 *
 * Written straight in rather than through `say`, and that is correct here —
 * this note carries no translation key, so `applyLocale` never visits it. It
 * follows the language the other way the rule allows: `setLocale` repaints
 * this surface (`repaintDocumentsForLocale` → `updateStage` → `paintDoc`). */
function paintSaveNote(tab) {
  const view = docViewOf(tab);
  const note = view.querySelector(".file-view-note");
  // The diff header has no reload button — its own reload is reopening the
  // diff, which is a different question from re-reading one file.
  const reload = view.querySelector(".file-view-reload") ?? { hidden: true };
  // No reload for a deleted file: there is nothing on disk to read, and the
  // button would be an error message with a delay on it.
  reload.hidden = tab.gone || !tab.stale;
  // A diff whose modified side is not a file on disk — a deletion. Orca's
  // `readOnly: !editable` said out loud, because a two-column editor that
  // silently refuses the keyboard reads as broken rather than as read-only.
  if (tab.kind === "diff" && tab.version == null) {
    note.textContent = t("diff.readOnly", "읽기 전용 — 디스크에 파일이 없습니다");
    note.classList.remove("is-unsaved");
    return;
  }
  if (tab.gone) {
    note.textContent = t("file.deletedOnDisk", "디스크에서 삭제되었습니다 — 이 창의 내용이 마지막 사본입니다. 저장하면 파일을 다시 만듭니다.");
    note.classList.add("is-unsaved");
    return;
  }
  if (tab.stale) {
    note.textContent = t("file.changedOnDisk", "디스크에서 바뀌었습니다. 다시 읽거나, 다시 저장해 덮어씁니다.");
    note.classList.add("is-unsaved");
    return;
  }
  if (isDirty(tab)) {
    note.textContent = t("file.unsaved", "저장 안 됨");
    note.classList.add("is-unsaved");
    return;
  }
  note.textContent = tab.saved ? t("file.saved", "저장했습니다.") : "";
  note.classList.remove("is-unsaved");
}

/* ---- the diff, side by side ----
 *
 * Orca's `DiffViewer` (DiffViewer-MmUuThJc.js:431-446), which is a Monaco
 * DiffEditor with seven options set. Six of them are what CodeMirror's merge
 * view already is or already has — `minimap: {enabled:false}` (the file-only
 * minimap controller never mounts here), `scrollBeyondLastLine: false` (no `scrollPastEnd`
 * extension), `lineNumbers: "on"`, `automaticLayout: true`, `find:
 * monacoFindOptions`. The two that are decisions are honoured here by name:
 * `renderSideBySide` is the person's toggle below, and `originalEditable:
 * false` with `readOnly: !editable` is the pair of rules that decides which
 * of the two documents can be typed in.
 *
 * WORD WRAP IS OFF, and that is the one place the diff deliberately differs
 * from the file editor beside it. Orca's two builders are opposites and both
 * were measured: `buildFileEditorWordWrapOptions` reads `=== false ? "off" :
 * "on"` and `buildDiffEditorWordWrapOptions` reads `=== true ? "on" : "off"`.
 * A wrapped line in a diff moves the line beside it out of step with its
 * partner, which is the one thing two columns exist to keep.
 *
 * WHAT DID NOT MOVE is the diff itself. git still computes it, Rust still
 * parses it, and Rust's ceiling still decides whether anything crosses at all
 * — `file_diff` hands back the two documents ONLY on the road where the
 * ceiling passed and there are changed lines to show. Everything else keeps
 * the rows: the withheld card, the untracked file, the binary file, and the
 * inline shape of this very toggle. Those rows are also where a review note
 * is written, so the annotate-and-send surface is untouched by all of this. */

/* Which shape a diff opens in — Orca's `renderSideBySide`, remembered. Read
 * from the boot report before the first diff can be opened. */
let diffSideBySide = true;

/* Whether this tab has a pair of documents to lay side by side. Asked in one
 * place so the toggle, the painter and the header cannot disagree about what
 * the merge view is able to show. */
function diffMergeable(tab) {
  return Boolean(tab?.texts);
}

/* The merge view's own colours, on top of the editor's.
 *
 * `editorThemeExtension` is passed to both documents unchanged — the font,
 * the gutter, the selection and the caret are the editor's, because a diff
 * that renders text differently from the editor that fixes it is two products.
 * What is added here is the merge view's own vocabulary, and every colour in
 * it is the `--diff-*` pair this window already paints its rows and its
 * source-control tallies with. CodeMirror's own base theme hard-codes
 * `#ee4433`/`#22bb22`; those are out-argued here rather than left to fight
 * the treatment switch. */
let mergeTheme = null;

function mergeThemeExtension() {
  if (mergeTheme) return mergeTheme;
  mergeTheme = window.CM6.EditorView.theme({
    // The two editors sit in one scroll box, so this is the element that has
    // to be the height of the surface rather than the height of the file.
    // Half a pixel under the editor: denser gutters and inline decorations
    // read oversized at full size (Orca computeDiffEditorFontSize,
    // editor-font-zoom.ts:28-32).
    "&": {
      height: "100%",
      fontSize: "var(--diff-font-size, var(--editor-font-size, var(--text-xs)))",
    },
    // Removed on the committed side, added on the working side. The pair is
    // the row render's, so a change is the same colour whichever shape it is
    // being read in.
    "&.cm-merge-a .cm-changedLine, & .cm-deletedChunk": {
      backgroundColor: "var(--diff-del-surface)",
    },
    "&.cm-merge-b .cm-changedLine, & .cm-inlineChangedLine": {
      backgroundColor: "var(--diff-add-surface)",
    },
    // The word-level marks inside a changed line — the thing the row render
    // cannot do at all. An underline rather than a fill: the line already
    // carries the wash, and a second fill on top of it reads as a selection.
    "&.cm-merge-a .cm-changedText, & .cm-deletedChunk .cm-deletedText": {
      background: "linear-gradient(var(--diff-del), var(--diff-del)) bottom/100% 2px no-repeat",
    },
    "&.cm-merge-b .cm-changedText": {
      background: "linear-gradient(var(--diff-add), var(--diff-add)) bottom/100% 2px no-repeat",
    },
    "&.cm-merge-b .cm-deletedText": { backgroundColor: "var(--diff-del-surface)" },
    // The 3px rail down the gutter, which is the row render's inset bar in
    // the one place a two-column view has room for it.
    "&.cm-merge-a .cm-changedLineGutter, & .cm-deletedLineGutter": {
      background: "var(--diff-del)",
    },
    "&.cm-merge-b .cm-changedLineGutter": { background: "var(--diff-add)" },
    // The spacer rows the merge view inserts to keep the two sides in step.
    // Quiet on purpose — they are the absence of a line, not a line.
    ".cm-collapsedLines": {
      padding: "var(--space-1) var(--space-3)",
      background: "var(--surface-well)",
      color: "var(--ink-mist)",
      fontFamily: "var(--font-ui)",
      fontSize: "var(--text-2xs)",
    },
  });
  return mergeTheme;
}

/* What each side of the merge view is built with. The editor's own extensions
 * minus the ones that belong to editing a whole file — no autocomplete, no
 * line wrapping, for the reason written above. Search state IS here: ⌘F on
 * this surface is the doc-find bar riding CodeMirror's query (Orca opens
 * monaco's own find widget on the same surface, DiffViewer.tsx:433), and
 * `setSearchQuery` needs `CM.search()` to land in. No searchKeymap, so
 * CodeMirror's stock panel never stands — the chord stays the window's. */
function diffExtensions(tab, editable) {
  const CM = window.CM6;
  return [
    CM.lineNumbers(),
    CM.highlightSpecialChars(),
    CM.drawSelection(),
    CM.search(),
    CM.highlightSelectionMatches(),
    findWash().field,
    CM.EditorState.allowMultipleSelections.of(true),
    CM.EditorState.readOnly.of(!editable),
    CM.EditorView.editable.of(editable),
    CM.keymap.of([...CM.defaultKeymap, ...CM.historyKeymap]),
    editable ? CM.history() : [],
    diffWrapCompartment.of(editingPrefs?.diff_word_wrap ? CM.EditorView.lineWrapping : []),
    editorThemeExtension(),
    mergeThemeExtension(),
    CM.syntaxHighlighting(editorHighlightStyle()),
    // The same table the editor reads, so a diff of a `.rs` file is coloured
    // by the same grammar the tab that edits it uses.
    window.CM6.languageFor(tab.path) ?? [],
    // Only the side that can be typed in reports. A listener on the committed
    // side would be a listener on a document nothing can change.
    editable ? CM.EditorView.updateListener.of(onDiffUpdate) : [],
  ];
}

/* The live merge views, one per split leaf — the rule every per-leaf surface
 * in this window follows, and the rule the editor beside it follows. */
const diffViews = new Map();

function diffEntryOf(view) {
  for (const held of diffViews.values()) if (held.merge.b === view) return held;
  return null;
}

/* Every change to the modified side, from wherever it came.
 *
 * A transaction rather than a DOM `input` event, which is the difference
 * between hearing about typing and hearing about typing, undo, redo, paste
 * and the re-seat below — all of which move the document and all of which the
 * dirty mark is a statement about. */
function onDiffUpdate(update) {
  if (!update.docChanged) return;
  const held = diffEntryOf(update.view);
  const owner = held && tabs.find((tab) => tab.id === held.showing);
  if (!owner) return;
  const typed = update.state.doc.toString();
  // The document was just MADE equal to the draft — a re-seat, a save landing.
  // Nothing was typed, so nothing is marked: recording it would clear the
  // "저장했습니다" note on the save that just wrote it.
  if (typed !== owner.draft) diffTyped(owner, typed);
  held.text = typed;
}

/* A leaf folded away, or a leaf that has stopped showing a diff. Two editors
 * dropped without `destroy()` keep their DOM listeners and their measure
 * loops, and the merge view holds a third of its own. */
function dropDiffView(group) {
  const held = diffViews.get(group);
  if (!held) return;
  held.merge.destroy();
  diffViews.delete(group);
}

/* Everything one keystroke in the modified side has to move.
 *
 * The same three facts the editor's own typed-handler moves, because they are
 * the same three facts: `draft` is what a save writes, `text` is what the disk
 * last held, and the difference between them IS the dirty mark. Nothing here
 * is a diff-shaped copy of the editor's model — the tab is the model, and this
 * surface writes into it exactly as the editor does. */
function diffTyped(tab, typed) {
  const was = isDirty(tab);
  tab.draft = typed;
  tab.saved = false;
  paintSaveNote(tab);
  scheduleAutoSave(tab);
  if (was !== isDirty(tab)) renderTabs();
}

function paintDiffMerge(view, tab) {
  const host = view.querySelector(".diff-merge");
  const CM = window.CM6;
  // A stamp is what `write_text_file` demands back, so a document without one
  // is a document this window cannot save — and Orca's `readOnly: !editable`
  // is that sentence. The committed side is never editable either way, which
  // is `originalEditable: false` and is also just true: it is not a file.
  const editable = Boolean(tab.version);
  let held = diffViews.get(tab.pane);
  // Rebuilt rather than re-seated when the leaf changes hands. The editor
  // parks an `EditorState` per tab because a file survives being looked away
  // from; a diff is two documents and a chunk table computed from them, and
  // handing one of those to a merge view from outside would leave the other
  // two describing the file that left.
  if (held && (held.showing !== tab.id || held.docs !== tab.texts)) {
    dropDiffView(tab.pane);
    held = null;
  }
  if (!held) {
    host.replaceChildren();
    const merge = new CM.MergeView({
      a: { doc: tab.texts.original, extensions: diffExtensions(tab, false) },
      b: { doc: tab.draft ?? tab.texts.modified, extensions: diffExtensions(tab, editable) },
      parent: host,
      // Committed on the left, working on the right — the direction every
      // review tool in this window already reads in, and the direction the
      // `+`/`-` tallies above are counted in.
      orientation: "a-b",
      // No revert arrows. They write the committed text back over the working
      // file, which is a git operation wearing an editor's clothes — this
      // window's source-control panel is where a change is discarded, and it
      // asks first.
      revertControls: false,
      highlightChanges: true,
      gutter: true,
    });
    held = { merge, showing: tab.id, docs: tab.texts, text: merge.b.state.doc.toString() };
    diffViews.set(tab.pane, held);
  } else if (tab.draft !== undefined && held.text !== tab.draft) {
    // Something outside the merge view moved the draft — a save landing from
    // the strip, or a restored tab. Replaced as a transaction rather than a
    // rebuild so the caret and the undo stack survive it.
    held.merge.b.dispatch({
      changes: { from: 0, to: held.merge.b.state.doc.length, insert: tab.draft },
    });
    held.text = tab.draft;
  }
  // The label lives on the section in ui/index.html, because `applyLocale`
  // walks markup and CodeMirror's editable node is built at runtime.
  held.merge.b.contentDOM.setAttribute("aria-label", view.getAttribute("aria-label") ?? "");
}

/* Is this tab still the one its leaf is showing?
 *
 * The file surface is shared per leaf, and every await on the disk path is a
 * window in which the person can switch tabs. A repaint that arrives after
 * that would draw the tab that asked for it over the tab now showing — the
 * body would disagree with the strip, and somebody's editor would go off
 * screen. The model is still brought up to date either way; only the drawing
 * waits for the tab to come back. */
function stillShowing(tab) {
  // A target with no tab identity is not seated on any leaf — the combined
  // view's sections are saved through the one save but painted by themselves,
  // and there is no document surface here to write a note into.
  if (!tab.id) return false;
  const on = activeTabIn(tab.pane);
  // Nothing claims this leaf — a tab whose worktree is not the active one is
  // hidden, and painting it is neither harmful nor visible. The guard is for
  // the one case that IS harmful: a DIFFERENT tab holding the surface now.
  return on === null || on.id === tab.id;
}

/* Take what is on disk, keeping the person's place in it.
 *
 * `text` and `draft` move together — `isDirty` is the comparison between them,
 * so advancing one alone would show a file nobody typed in as edited. The
 * caret and the scroll are put back because a re-read that jumps to the top
 * loses the line the person was reading, which is most of why they had the
 * file open. */
async function rereadFile(tab) {
  // Where the person is in the file, read BEFORE the await — after it, the
  // leaf's editor may be showing somebody else's tab and this would be their
  // caret. `editorShowing` answers null in exactly that case.
  const field = editorShowing(tab);
  const at = field?.state.selection.main.head ?? 0;
  const scrolled = field?.scrollDOM.scrollTop ?? 0;
  // What the buffer held when the disk was asked. If it is not still that when
  // the answer lands, somebody typed in the meantime.
  const asked = tab.draft;
  let held;
  try {
    held = await invoke("read_text_file", { path: tab.path });
  } catch (error) {
    if (stillShowing(tab)) {
      docHost(tab.pane, "file").querySelector(".file-view-note").textContent = String(error);
    }
    return false;
  }
  // Somebody typed into this tab while the disk was answering — the same race
  // as switching tabs, inside one tab. A clean tab is re-read because there is
  // nothing to lose, and that stopped being true the moment they touched the
  // keyboard. So it is marked instead, exactly as a tab that was already dirty
  // would be, and the choice is theirs.
  if (tab.draft !== asked) {
    tab.stale = true;
    renderTabs();
    if (stillShowing(tab)) paintSaveNote(tab);
    return false;
  }
  tab.text = held.text;
  tab.draft = held.text;
  tab.version = held.version;
  tab.stale = false;
  // A read that succeeded is proof the file exists, whatever mark said
  // otherwise.
  tab.gone = false;
  tab.saved = false;
  renderTabs();
  // The leaf moved on while this was in flight. The tab now holds what is on
  // disk, and `updateStage` paints it from that when it comes back.
  if (!stillShowing(tab)) return true;
  paintFileView(tab);
  // Put back where they were, clamped to a file that may have got shorter.
  // `paintFileView` has already replaced the document by now, and it did so as
  // a transaction — so this is a correction to a caret that mostly survived,
  // not a rescue of one that was thrown away.
  const back = editorShowing(tab);
  if (back) {
    back.dispatch({ selection: { anchor: Math.min(at, held.text.length) } });
    back.scrollDOM.scrollTop = scrolled;
  }
  const said = docHost(tab.pane, "file").querySelector(".file-view-note");
  said.textContent = t("file.reloaded", "디스크에서 다시 읽었습니다.");
  return true;
}

/* Has this file moved while the tab sat there?
 *
 * Asked when a tab is brought forward, and only then: one stat for the file
 * being looked at. A timer over every open tab would be a cost this window
 * pays forever to answer a question nobody asked yet.
 *
 * A clean tab is re-read, because there is nothing to lose and the stale text
 * is the lie. A dirty one is marked instead — throwing away typing to show
 * somebody a file they were not looking at is the worse of the two wrongs. */
async function checkFileMoved(tab) {
  if (tab?.kind !== "file" || tab.version === undefined) return;
  let now;
  try {
    now = await invoke("file_version", { path: tab.path });
  } catch {
    // Gone, or unreadable. The tab keeps what it has rather than blanking:
    // the text in the field is the last thing that file said.
    return;
  }
  if (now === tab.version) return;
  if (isDirty(tab)) {
    tab.stale = true;
    renderTabs();
    if (stillShowing(tab)) paintSaveNote(tab);
    return;
  }
  await rereadFile(tab);
}

/* The file under this tab is gone from the disk.
 *
 * The tab is NOT closed: the text in the field is the last copy of that file
 * anywhere, and a window that closes it on a delete event has just finished
 * the deletion. The mark is Orca's `setExternalMutation("deleted")`
 * (index-ftls8Hg_.js:147372), drawn with this surface's own save-note. */
function markFileGone(tab) {
  if (tab.gone) return;
  tab.gone = true;
  renderTabs();
  if (stillShowing(tab)) paintSaveNote(tab);
}

/* Work that is only in this window.
 *
 * Two kinds, one comparison. The modified side of a side-by-side diff IS the
 * working file — the same bytes the file tab would open, read by the same
 * command, saved by the same one — so a diff that has been typed in is dirty
 * in exactly the sense a file is, and the strip's mark, the close guard and
 * ⌘S all follow from this one sentence rather than from a second copy of it. */
function isDirty(tab) {
  return (
    (tab?.kind === "file" || tab?.kind === "diff") &&
    tab.draft !== undefined &&
    tab.draft !== tab.text
  );
}

/* Which leaf surface a document's header note is written on. The file view
 * and the diff view carry the same `.file-view-note`, because the note says
 * the same thing on both. */
function docViewOf(tab) {
  return docHost(tab.pane, tab.kind === "diff" ? "diff" : "file");
}

/* ---- find and replace in file (⌘F) ----
 *
 * Find AND replace now, which the textarea could not honestly offer: a
 * replace-all across a plain field is one `value` re-assignment that throws
 * away the undo stack and puts the caret at the end. Underneath a document
 * model it is a transaction — undoable with ⌘Z, saved with ⌘S, and shown
 * where it happened. Orca's find is Monaco's, with the same three abilities;
 * this reaches them through `@codemirror/search`.
 *
 * Our own row rather than CodeMirror's panel: this one is already in the
 * grid, already translated by `applyLocale`, and already puts the count where
 * the eye is. What is behind every control is `SearchQuery`.
 *
 * It works on the source. A rendered preview has no offsets to select into,
 * so asking to find in one switches to the source first rather than searching
 * something the person cannot see. */

/* How many matches are counted before the count gives up and says "and more".
 *
 * A bound arrived with the size cap: the ceiling went from 512 KiB to 4 MiB,
 * so `e` in a large source file is now hundreds of thousands of positions,
 * and walking every one of them on every keystroke in the find field is work
 * nobody asked for. Stepping still wraps — within the first `FIND_MAX`, which
 * is more matches than a person walks by hand. */
const FIND_MAX = 5000;

/* Where the matches are, in order, up to the bound. The query does the
 * matching, so case sensitivity is one flag rather than two `toLowerCase`
 * calls that a regex or a whole-word option would immediately outgrow. */
function findMatches(editor, query) {
  const at = [];
  const cursor = query.getCursor(editor.state);
  for (let hit = cursor.next(); !hit.done; hit = cursor.next()) {
    at.push({ from: hit.value.from, to: hit.value.to });
    if (at.length >= FIND_MAX) break;
  }
  return at;
}

/* The wash behind every other hit — held by this window, not borrowed.
 *
 * CodeMirror's own search highlighter paints ONLY while its own panel is
 * open (bundle, `highlight({query, panel})`: no panel, no decorations), and
 * this window's find bars are not that panel — so the wash the find bar
 * promised had quietly never been painted. The decorations wear CodeMirror's
 * own class name, so `editorThemeExtension`'s `.cm-searchMatch` pair colours
 * them and no second theme exists. Lazy because `window.CM6` arrives after
 * this file does. */
let findWashHeld = null;
function findWash() {
  if (findWashHeld) return findWashHeld;
  const CM = window.CM6;
  const set = CM.StateEffect.define();
  const mark = CM.Decoration.mark({ class: "cm-searchMatch" });
  const field = CM.StateField.define({
    create: () => CM.Decoration.none,
    update(value, tr) {
      for (const effect of tr.effects) if (effect.is(set)) return effect.value;
      return tr.docChanged ? value.map(tr.changes) : value;
    },
    provide: (self) => CM.EditorView.decorations.from(self),
  });
  findWashHeld = { set, mark, field };
  return findWashHeld;
}

/* The one transaction a search settles with: the query lands where
 * `replaceNext`/`replaceAll` read it, and the wash lands where the theme
 * paints it. An empty `hits` is how the wash is taken back off. */
function washFind(editor, query, hits) {
  const { set, mark } = findWash();
  editor.dispatch({
    effects: [
      window.CM6.setSearchQuery.of(query),
      set.of(window.CM6.Decoration.set(hits.map((at) => mark.range(at.from, at.to)))),
    ],
  });
}

/* The query the bar currently describes. Built fresh each time rather than
 * kept: the fields ARE the state, and a cached copy would be a second answer
 * to "what is being searched for". */
function findQuery(view, tab) {
  return new window.CM6.SearchQuery({
    search: view.querySelector(".file-find-field").value,
    replace: view.querySelector(".file-find-swap").value,
    caseSensitive: tab.findCase === true,
    // Taken as characters, not as an escape sequence: somebody searching a
    // Windows-line-ending file for `\r` means those two characters.
    literal: true,
  });
}

/* The screen ⌘F should search when a terminal is in front.
 *
 * The ACTIVE pane's, not the tab's first — a split tab is several shells and
 * the one being read is the one to search. `null` for anything that is not a
 * terminal, which is what makes `openFind` able to ask one question. */
function termFindView() {
  const tab = currentTab();
  if (tab?.kind === "term") return termViews.get(activePaneOf(tab)) ?? null;
  if (tab?.kind === "lane") return stageView;
  if (!termFloat.hidden) return floatView;
  return null;
}

/* ⌘G / ⇧⌘G — walk whichever finder is open.
 *
 * One door for both surfaces. A terminal's bar has priority when it is the one
 * showing, and a file's step is the existing `runFind`, so neither engine
 * learns about the other. Pressing it with nothing open does nothing rather
 * than opening a bar: the chord means "the next one", and there is no next
 * one until something has been searched for. */
function stepFind(step) {
  const screen = termFindView();
  if (screen !== null && screen.findOpen()) {
    screen.stepFind(step);
    return;
  }
  const tab = currentTab();
  if (tab?.kind !== "file") return;
  const view = docHost(tab.pane, "file");
  if (view.querySelector(".file-find").hidden) return;
  runFind(tab, step);
}

function openFind() {
  const tab = currentTab();
  // ⌘F in a terminal used to be silence — `openFind` returned on anything that
  // was not a file, so the key everybody presses did nothing and the terminal
  // read as broken rather than as lacking a feature. A 5000-line scrollback
  // with one error in it is exactly the case a person needs this for, and the
  // wheel was the only way to it.
  const screen = termFindView();
  if (screen !== null && (tab?.kind === "term" || tab?.kind === "lane")) {
    screen.openFind();
    return;
  }
  // 프리뷰에서 찾기도 같은 손짓이다(1-g38): ⌘F는 앞에 선 판이 답한다 —
  // 터미널이면 그 화면이, 파일이면 그 편집기가, 프리뷰면 그려진 글이.
  if (tab && DOC_FIND_ROOTS[tab.kind]) {
    // side-by-side diff도 같은 바다 — 다만 그 아래에서는 DOM Range 대신
    // CodeMirror 검색이 걷는다(mergeEditorShowing이 다섯 손 안에서 가른다).
    openDocSearch(tab, docHost(tab.pane, tab.kind));
    return;
  }
  if (tab?.kind !== "file") return;
  if (tab.mode !== "text" && !tab.raw) {
    tab.raw = true;
    paintFileView(tab);
  }
  const view = docHost(tab.pane, "file");
  const field = view.querySelector(".file-find-field");
  const editor = editorShowing(tab);
  view.querySelector(".file-find").hidden = false;
  paintFindCase(view, tab);
  // Whatever is selected is usually what somebody pressing ⌘F is looking
  // for. A selection spanning lines is not a search term, so it is left.
  const picked = editor
    ? editor.state.sliceDoc(editor.state.selection.main.from, editor.state.selection.main.to)
    : "";
  if (picked !== "" && !picked.includes("\n")) field.value = picked;
  field.focus();
  field.select();
  runFind(tab, 0);
}

function closeFind(tab) {
  const view = docHost(tab.pane, "file");
  view.querySelector(".file-find").hidden = true;
  const editor = editorShowing(tab);
  // The wash comes off with the bar; the selected match stays, because that
  // is where the person was looking when they closed it.
  if (editor) washFind(editor, findQuery(view, tab), []);
  // The keyboard goes back to the text with the match still selected.
  editor?.focus();
}

/* The case toggle wears its own state, because a button that changes what a
 * search means and looks identical either way is a button that lies. */
function paintFindCase(view, tab) {
  view.querySelector(".file-find-case").setAttribute("aria-pressed", String(tab.findCase === true));
}

/* `step` of 0 means "from where the caret is", which is what opening the bar
 * and typing into it does; ±1 walks the list and wraps. */
function runFind(tab, step) {
  const view = docHost(tab.pane, "file");
  const count = view.querySelector(".file-find-count");
  const editor = editorShowing(tab);
  const query = findQuery(view, tab);
  if (!editor || !query.valid) {
    count.textContent = "";
    // An emptied field takes its wash with it — matches for a query that no
    // longer exists are a painted lie.
    if (editor) washFind(editor, query, []);
    return;
  }
  // Set even when nothing matches: the query is what `replaceNext` and
  // `replaceAll` read, and the wash is every other hit behind the current.
  const hits = findMatches(editor, query);
  washFind(editor, query, hits);
  if (hits.length === 0) {
    count.textContent = t("file.findNone", "일치 없음");
    return;
  }
  let index;
  if (step === 0) {
    const from = editor.state.selection.main.from;
    index = hits.findIndex((at) => at.from >= from);
    if (index < 0) index = 0;
  } else {
    index = ((tab.findAt ?? 0) + step + hits.length) % hits.length;
  }
  tab.findAt = index;
  const at = hits[index];
  // One transaction carries both: the match is selected, and the editor is
  // asked to bring it into view. The textarea needed the line worked out and
  // the scroll set by hand because a field does not scroll to a selection it
  // was handed while unfocused — a document model has no such gap.
  editor.dispatch({ selection: { anchor: at.from, head: at.to }, scrollIntoView: true });
  count.textContent = t("file.findHits", "{{at}}/{{count}}", {
    at: index + 1,
    // At the bound the number becomes "5000+" rather than a lie. No key of
    // its own: what changes is the value, not the sentence around it.
    count: hits.length >= FIND_MAX ? `${FIND_MAX}+` : hits.length,
  });
}

/* Replace, one match or all of them.
 *
 * `all` is deliberately a separate road rather than a loop over the other:
 * CodeMirror's `replaceAll` is ONE transaction, so ⌘Z takes back the whole
 * sweep. A loop would leave a hundred undo steps and the person would still
 * be pressing ⌘Z when they gave up. */
function runReplace(tab, all) {
  const view = docHost(tab.pane, "file");
  const editor = editorShowing(tab);
  const query = findQuery(view, tab);
  if (!editor || !query.valid) return;
  editor.dispatch({ effects: window.CM6.setSearchQuery.of(query) });
  if (all) window.CM6.replaceAll(editor);
  else window.CM6.replaceNext(editor);
  // The document moved under the search, so the count is recomputed from what
  // is there now. Step 0 rather than ±1: after a replace the person is at the
  // match that follows, not the one they named.
  runFind(tab, 0);
}

/* The bar's controls, wired once per leaf. The second leaf's surface is a
 * clone and a clone carries no listeners, so this runs for both. */
function wireFind(view) {
  if (view.dataset.findWired) return;
  view.dataset.findWired = "yes";
  const field = view.querySelector(".file-find-field");
  const stepBy = (step) => {
    const tab = currentTab();
    if (tab?.kind === "file") runFind(tab, step);
  };
  const swapBy = (all) => {
    const tab = currentTab();
    if (tab?.kind === "file") runReplace(tab, all);
  };
  field.addEventListener("input", () => stepBy(0));
  field.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      const tab = currentTab();
      if (tab?.kind === "file") closeFind(tab);
      return;
    }
    if (event.key !== "Enter") return;
    event.preventDefault();
    stepBy(event.shiftKey ? -1 : 1);
  });
  // Enter in the replace field replaces, which is where a hand that has just
  // typed the replacement already is.
  view.querySelector(".file-find-swap").addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      const tab = currentTab();
      if (tab?.kind === "file") closeFind(tab);
      return;
    }
    if (event.key !== "Enter") return;
    event.preventDefault();
    swapBy(event.shiftKey);
  });
  view.querySelector(".file-find-case").addEventListener("click", () => {
    const tab = currentTab();
    if (tab?.kind !== "file") return;
    tab.findCase = tab.findCase !== true;
    paintFindCase(view, tab);
    // From the caret again rather than from the current index: a search that
    // just changed meaning has a different list, and index 4 of the old one
    // points nowhere in the new.
    runFind(tab, 0);
  });
  for (const button of view.querySelectorAll(".file-find-step")) {
    button.addEventListener("click", () => stepBy(Number(button.dataset.step)));
  }
  view.querySelector(".file-find-swap-one").addEventListener("click", () => swapBy(false));
  view.querySelector(".file-find-swap-all").addEventListener("click", () => swapBy(true));
  view.querySelector(".file-find-close").addEventListener("click", () => {
    const tab = currentTab();
    if (tab?.kind === "file") closeFind(tab);
  });
}

/* What the backend calls a save it refused because the file moved. Matched,
 * not shown: the sentence a person reads is in the catalogs. */
const STALE_SAVE = "zerocode:stale-save";

/* ⌘S. Writes what is in the field and makes that the file's text — the same
 * value the dirty check is against, so saving is what clears the mark.
 *
 * THE ONLY WRITE. The modified side of a side-by-side diff is the working
 * file, so ⌘S there arrives here — not at a second save path with its own
 * idea of optimistic locking, its own `create_if_missing` and its own set of
 * bugs to find twice. What that costs is one word in the guard below; what a
 * second one would cost is a file saved two ways.
 *
 * A diff with no version is Orca's `readOnly: !editable` reaching this far:
 * there is nothing to hand back to the backend's version check, so there was
 * never an editor to type in and there is nothing here to write. */
async function saveFile(tab) {
  if (tab?.kind !== "file" && tab?.kind !== "diff") return false;
  if (tab.artifact?.snapshot) return false;
  if (tab.draft === undefined) return false;
  // Read-only, and only a diff can be. A file tab always came back from
  // `read_text_file` with a stamp; a diff's is absent exactly when there is no
  // working file behind the modified side — a deletion, which this window
  // shows and does not offer to write.
  if (tab.kind === "diff" && tab.version == null) return false;
  cancelAutoSave(tab);
  while (tab.saveInFlight) await tab.saveInFlight;
  let finishSave = () => {};
  const inFlight = new Promise((done) => (finishSave = done));
  tab.saveInFlight = inFlight;
  // The write owns the exact draft present when this save began. Typing can
  // continue while native I/O is in flight; those newer bytes must stay dirty
  // rather than being reported as the bytes this response wrote.
  const draft = tab.draft;
  try {
    // Already told that the file moved, and asked to save anyway: that is the
    // person overwriting on purpose, so this save carries what is on disk NOW
    // rather than what the tab last read. The refusal has to be a dead end for
    // exactly one keystroke, not forever.
    //
    // Not for a file that is GONE: there is nothing to ask for a version of,
    // and asking would fail the save on the one road where the tab's text is
    // the last copy of the file. A tab can wear both marks — rewritten, then
    // deleted — so the gone one has to win here as it does in the note.
    if (tab.stale && !tab.gone) {
      try {
        tab.version = await invoke("file_version", { path: tab.path });
      } catch (error) {
        if (stillShowing(tab)) {
          docViewOf(tab).querySelector(".file-view-note").textContent = String(error);
        }
        return false;
      }
    }
    let written;
    try {
      // The last argument is this window reporting the deletion it was told
      // about, not a licence to create: the backend makes a file again only
      // for a tab that carries the mark, and refuses a save into an empty path
      // otherwise. So it is the tab's own mark, never a written-in `true`.
      written = await invoke("write_text_file", {
        path: tab.path,
        text: draft,
        version: tab.version,
        createIfMissing: tab.gone === true,
      });
    } catch (error) {
      // The one failure with a way forward. The backend names it with a marker
      // rather than a sentence, so the words here are this window's and follow
      // the language.
      if (String(error).includes(STALE_SAVE)) {
        tab.stale = true;
        // This particular refusal is proof that something is standing at that
        // path now — whichever road it came from — so a tab that was carrying
        // the deleted mark stops carrying it. The note it falls back to is the
        // one with a way forward: reload, or save again to overwrite.
        tab.gone = false;
        renderTabs();
        if (stillShowing(tab)) paintSaveNote(tab);
        return false;
      }
      if (stillShowing(tab)) {
        const note = docViewOf(tab).querySelector(".file-view-note");
        note.textContent = String(error);
        note.classList.add("is-unsaved");
      }
      return false;
    }
    tab.text = draft;
    // What the file is now, from the write itself. Statting for it again would
    // race the next writer and could hand back a stamp for somebody else's
    // bytes.
    tab.version = written;
    // The write is proof the file is there — it either survived or was just
    // made again. Leaving the gone mark on would keep a banner up saying the
    // file is missing while the person is looking at the save that put it
    // back.
    tab.stale = false;
    tab.gone = false;
    tab.saved = tab.draft === draft;
    persistStageLayouts({ worktree: tab.worktree });
    // ⌘S on a fresh untitled markdown is the person saying KEEP — even when
    // what they kept is empty — so the untouched-close reclaim must never fire
    // for this path again. The backend forgets its mint for the same reason.
    if (tab.untitledFresh) {
      tab.untitledFresh = false;
      invoke("release_untitled_markdown", { path: tab.path }).catch(() => {});
    }
    renderTabs();
    if (stillShowing(tab)) {
      paintSaveNote(tab);
    }
    // 저장된 글자는 다른 판이 보고 있는 그 파일이기도 하다 — 나란히 선 판이
    // 있으면 방금 쓴 것을 다시 읽어 온다(1-g36·1-g39).
    refreshDerivedViewsOf(tab.path);
    // What changed on disk changed what git has to say about it, and both
    // surfaces that answer that question are on screen.
    reloadBadges();
    if (!el("activity-scm").hidden) refreshScm();
    return true;
  } finally {
    if (tab.saveInFlight === inFlight) tab.saveInFlight = null;
    finishSave();
  }
}

/* ---- markdown ----
 *
 * Orca's `MarkdownPreview` spot. Written here rather than vendored: the
 * window has no bundler and no network at runtime, and a preview is a few
 * block rules — the risky half of a markdown library is the HTML passthrough
 * this one does not have.
 *
 * Nothing on this path touches `innerHTML`. Every node is built and every
 * string arrives as `textContent`, so a document that contains a `<script>`
 * shows the person the characters `<script>` — which is what a file viewer
 * is supposed to do with them. */

/* Inline runs, in the order they win. Code first: a backtick span suppresses
 * everything inside it, which is the one rule people rely on when they write
 * about markdown in markdown.
 *
 * The backtick is escaped rather than typed. The undefined-call gate blanks
 * string bodies before it scans, and a backtick inside a regex opens a
 * template literal it never sees closed \u2014 a literal one here silently
 * swallows every declaration below this line, and the gate then reports the
 * whole rest of the file as undefined. */
const MD_INLINE = [
  "(\\x60+)([\\s\\S]*?)\\1",
  "\\*\\*([\\s\\S]+?)\\*\\*",
  "__([\\s\\S]+?)__",
  "~~([\\s\\S]+?)~~",
  "\\*([^*\\n]+)\\*",
  "_([^_\\n]+)_",
  // 그림과 링크는 한 규칙이다 — 앞의 `!` 하나가 둘을 가른다(1-g36).
  "(!?)\\[([^\\]]*)\\]\\(([^)\\s]+)[^)]*\\)",
].join("|");

/* 지금 그리고 있는 문서의 자리(1-g36): 상대 경로를 어디에서 재는지(`base`),
 * 문서 안 앵커가 어느 판을 뒤지는지(`page`).
 *
 * 인자가 아니라 이 자리에 두는 이유는 두 가지다. 그리는 일은 동기적이라 두
 * 문서가 이 자리를 겹쳐 쓸 수 없고, 무엇보다 `paintMarkdown`·`paintInline`·
 * `mdLink`의 서명은 Rust 문지기가 글자 그대로 붙잡고 있는 것이라 늘릴 수 없다.
 * 나중에 눌릴 손은 만들어질 때 이 자리를 제 것으로 쥔다 — 누를 때는 이미
 * 비어 있다. */
let mdWhere = null;

/* 제목의 이름표: 소문자, 공백은 -, 글자와 숫자가 아닌 것은 버린다. 한글도
 * 글자이므로 남는다(유니코드 letter). */
function mdSlug(said) {
  return said
    .trim()
    .toLowerCase()
    .replace(/\s+/g, "-")
    .replace(/[^\p{L}\p{N}_-]/gu, "");
}

/* 그리고 그 이름표는 `md-`를 달고 판에 붙는다.
 *
 * 접두사가 붙는 이유는 짧지 않다: 이름표는 문서가 짓는 것이고 이 창에도 제
 * 이름표가 있다. 접두사가 없으면 `# sidebar-menu`라고 쓴 문서 하나가 창이
 * `getElementById`로 찾는 원소를 가로챌 수 있다 — 문서는 제 판 안에서만
 * 이름을 가진다. */
function mdHeadingId(said) {
  return `md-${mdSlug(said)}`;
}

/* 문서가 선 자리에서 잰 상대 경로. `..`는 걸어 올라가고, 절대 경로와 자리를
 * 모르는 경우는 그대로 둔다. */
function resolveDocPath(base, href) {
  if (!base || href.startsWith("/")) return href;
  const parts = `${base}/${href}`.split("/");
  const walked = [];
  for (const part of parts) {
    if (part === "." || (part === "" && walked.length > 0)) continue;
    if (part === "..") {
      if (walked.length > 1) walked.pop();
      continue;
    }
    walked.push(part);
  }
  return walked.join("/");
}

/* 문서 안의 #링크가 찾아가는 곳 — 링크가 쥔 것은 사람이 쓴 조각이므로 이름표로
 * 옮겨서 찾는다. */
function scrollToDocHeading(page, id) {
  scrollToDocHeadingId(page, mdHeadingId(id));
}

/* 그리고 이름표를 이미 쥔 쪽(목차, 1-g37)이 부르는 같은 문. 이름표를 하나씩
 * 견주는 것은 `querySelector`가 아니라서다 — 한글 이름표는 선택자 문법이 아니다. */
function scrollToDocHeadingId(page, id) {
  if (!page) return;
  for (const head of page.querySelectorAll(".md-head")) {
    if (head.id !== id) continue;
    head.scrollIntoView({ block: "start" });
    return;
  }
}

/* 구분 줄 한 칸이 말하는 정렬 — 아무 말도 없으면 빈 문자열이고, 그 열은
 * 건드리지 않는다. */
function mdAlign(cell) {
  const left = cell.startsWith(":");
  const right = cell.endsWith(":");
  if (left && right) return "center";
  if (right) return "right";
  if (left) return "left";
  return "";
}

/* 링크는 이 창의 기존 정책을 탄다 — 스킴이 어느 문인지 정한다.
 *
 * 여전히 `<a href>`가 아니다: 웹뷰 안의 앵커는 앱 자신을 다른 곳으로 실어
 * 보내고, 그것은 뷰어가 사람에게 할 수 있는 일이 아니다. 대신 span이 무엇을
 * 하는 손인지 정한다 — http(s)는 브라우저 링크 라우팅을 따라(터미널·편집기
 * 링크와 같은 주인이 설정이 정한 문으로 보낸다), `#`은 이
 * 문서 안의 제목으로, 그 밖의 상대 경로는 문서의 자리에서 재어 편집기로.
 * 알 수 없는 스킴은 문이 아니다 — 주소를 단 글자로만 남는다. */
function mdLink(label, href) {
  const where = mdWhere;
  const node = document.createElement("span");
  node.className = "md-link";
  node.textContent = label || href;
  node.dataset.tip = href;
  if (canonicalHttpLink(href) !== null) {
    node.classList.add("md-link--external");
    actsAsButton(node, (event) => routeHttpLink(href, event));
    return node;
  }
  if (/^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith("//")) {
    node.classList.add("md-link--external");
    return node;
  }
  if (href.startsWith("#")) {
    node.classList.add("md-link--anchor");
    actsAsButton(node, () => scrollToDocHeading(where?.page ?? null, href.slice(1)));
    return node;
  }
  actsAsButton(node, () =>
    openPath(resolveDocPath(where?.base ?? "", href.split("#")[0]), { preview: true }),
  );
  return node;
}

/* 그림. 바깥 그림은 이 창이 대신 받아오지 않는다 — 문서가 가리킨 곳으로 창이
 * 몰래 나가는 일은 없다. 대역(alt)만 자리를 지키고, 주소는 손끝에 남는다.
 * 프로젝트 안의 그림은 이미지 뷰어가 쓰는 그 문으로 읽어 와 앉힌다. */
function mdImage(alt, src) {
  const where = mdWhere;
  const outward = /^[a-z][a-z0-9+.-]*:/i.test(src) || src.startsWith("//");
  if (outward || !where) {
    const stub = document.createElement("span");
    stub.className = "md-image-stub";
    stub.textContent = alt || src;
    stub.dataset.tip = src;
    return stub;
  }
  const node = document.createElement("img");
  node.className = "md-image";
  node.alt = alt;
  node.draggable = false;
  void inlineDocImage(node, resolveDocPath(where.base, src.split("#")[0]));
  return node;
}

/* 그림 하나를 제자리에 앉힌다 — 읽지 못하면 대역만 남는다. */
async function inlineDocImage(node, path) {
  let image;
  try {
    image = await invoke("read_image_file", { path });
  } catch {
    const stub = document.createElement("span");
    stub.className = "md-image-stub";
    stub.textContent = node.alt || path;
    stub.dataset.tip = path;
    node.replaceWith(stub);
    return;
  }
  node.src = `data:${image.mime};base64,${image.data}`;
}

/* One line of markdown, as nodes under `into`. */
function paintInline(into, text) {
  let at = 0;
  // A regex of its own, per call. A `/g` regex carries `lastIndex` on the
  // object, and this function calls itself for the inside of a bold or struck
  // run \u2014 the nested walk left `lastIndex` at 0 and the outer loop then
  // resumed from the start of the line and matched the same run forever.
  const scan = new RegExp(MD_INLINE, "g");
  for (let hit = scan.exec(text); hit !== null; hit = scan.exec(text)) {
    if (hit.index > at) into.appendChild(document.createTextNode(text.slice(at, hit.index)));
    const [, , code, strongStar, strongBar, struck, emStar, emBar, bang, label, href] = hit;
    if (code !== undefined) {
      const node = document.createElement("code");
      node.className = "md-code";
      node.textContent = code.trim();
      into.appendChild(node);
    } else if (href !== undefined) {
      into.appendChild(bang === "!" ? mdImage(label, href) : mdLink(label, href));
    } else {
      const strong = strongStar ?? strongBar;
      const emphasis = emStar ?? emBar;
      const node = document.createElement(strong ? "strong" : struck ? "s" : "em");
      // The inner text is run through again: `**a `b` c**` is bold with code
      // inside it, and stopping at the first level would print the backticks.
      paintInline(node, strong ?? struck ?? emphasis);
      into.appendChild(node);
    }
    at = scan.lastIndex;
  }
  if (at < text.length) into.appendChild(document.createTextNode(text.slice(at)));
}

/* The two fences GFM allows. Held as strings for the reason `fenceOf` gives. */
const MD_FENCES = ["\x60\x60\x60", "~~~"];

/* A row of a GFM table, split on the pipes that are not escaped. */
function mdCells(line) {
  return line
    .replace(/^\s*\|/, "")
    .replace(/\|\s*$/, "")
    .split("|")
    .map((cell) => cell.trim());
}

/* Which fence this line opens or closes, or `null`.
 *
 * Written as string tests rather than as a regex because the fence characters
 * are backticks, and the undefined-call gate blanks string bodies before it
 * scans — a backtick inside a regex literal opens a template literal it never
 * sees closed, and every declaration after it disappears from the scan. */
function fenceOf(line) {
  const trimmed = line.trimStart();
  for (const fence of MD_FENCES) if (trimmed.startsWith(fence)) return fence;
  return null;
}

/* Whether this line starts a block, which is where a paragraph stops. */
function opensABlock(line) {
  if (fenceOf(line) !== null) return true;
  const trimmed = line.trimStart();
  return trimmed.startsWith(">") || /^#{1,6}\s/.test(trimmed);
}

function isTableRule(line) {
  return /^\s*\|?[\s:|-]*-[\s:|-]*\|?\s*$/.test(line) && line.includes("-");
}

/* A rendered block remembers the source lines it came from. Review notes use
 * that stable source coordinate rather than DOM offsets, which change with
 * fonts, wrapping and Mermaid's asynchronous replacement. */
function markMarkdownReviewTarget(node, startLine, endLine = startLine) {
  node.classList.add("md-review-target");
  node.dataset.markdownStartLine = String(startLine);
  node.dataset.markdownLine = String(Math.max(startLine, endLine));
  return node;
}

function paintMarkdown(body, text) {
  const lines = text.split("\n");
  // The open lists, innermost last. Nesting is by indent, the way it is
  // written — a flat renderer turns a nested checklist into one long list,
  // which is exactly the structure the person was expressing.
  let stack = [];
  let at = 0;

  const closeLists = (toDepth) => {
    while (stack.length > toDepth) stack.pop();
  };

  while (at < lines.length) {
    const line = lines[at];
    // 프리뷰의 선택이 원본의 줄을 말할 수 있도록, 블록마다 제 출생 줄을 적어
    // 둔다(1-g43). 스캐너는 이미 몇 번째 줄에 서 있는지 알고 있으니 값은 공짜다.
    const born = String(at + 1);

    const closes = fenceOf(line);
    if (closes !== null) {
      const startLine = at + 1;
      const held = [];
      const language = line.trimStart().slice(closes.length).trim();
      at += 1;
      while (at < lines.length && fenceOf(lines[at]) !== closes) {
        held.push(lines[at]);
        at += 1;
      }
      at += 1;
      closeLists(0);
      const block = document.createElement("pre");
      block.className = "md-block";
      block.dataset.line = born;
      if (language) block.dataset.language = language;
      block.textContent = held.join("\n");
      body.appendChild(markMarkdownReviewTarget(block, startLine, Math.max(startLine, at)));
      continue;
    }

    if (line.trim() === "") {
      closeLists(0);
      at += 1;
      continue;
    }

    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading) {
      const lineNumber = at + 1;
      closeLists(0);
      const node = document.createElement(`h${heading[1].length}`);
      node.className = "md-head";
      node.dataset.line = born;
      const said = heading[2].replace(/\s+#+\s*$/, "");
      // 제목마다 이름표 — 문서 안의 `#`링크가 이걸 보고 찾아온다(1-g36).
      node.id = mdHeadingId(said);
      paintInline(node, said);
      body.appendChild(markMarkdownReviewTarget(node, lineNumber));
      at += 1;
      continue;
    }

    if (/^\s*([-*_])\s*(\1\s*){2,}$/.test(line)) {
      const lineNumber = at + 1;
      closeLists(0);
      const rule = document.createElement("hr");
      rule.dataset.line = born;
      body.appendChild(markMarkdownReviewTarget(rule, lineNumber));
      at += 1;
      continue;
    }

    // A table is a header row and the rule under it. Without the rule it is
    // just a line with pipes in it, which is what GFM says too.
    if (line.trimStart().startsWith("|") && isTableRule(lines[at + 1] ?? "")) {
      const startLine = at + 1;
      closeLists(0);
      const table = document.createElement("table");
      table.className = "md-table";
      table.dataset.line = born;
      // 구분 줄의 콜론이 열의 정렬을 말한다(GFM) — 말하지 않은 열은 그대로 둔다.
      const aligns = mdCells(lines[at + 1]).map(mdAlign);
      const head = document.createElement("tr");
      for (const [column, cell] of mdCells(line).entries()) {
        const th = document.createElement("th");
        if (aligns[column]) th.style.textAlign = aligns[column];
        paintInline(th, cell);
        head.appendChild(th);
      }
      table.appendChild(head);
      at += 2;
      while (at < lines.length && lines[at].trimStart().startsWith("|")) {
        const row = document.createElement("tr");
        for (const [column, cell] of mdCells(lines[at]).entries()) {
          const td = document.createElement("td");
          if (aligns[column]) td.style.textAlign = aligns[column];
          paintInline(td, cell);
          row.appendChild(td);
        }
        table.appendChild(row);
        at += 1;
      }
      body.appendChild(markMarkdownReviewTarget(table, startLine, Math.max(startLine, at)));
      continue;
    }

    const quote = /^\s*>\s?(.*)$/.exec(line);
    if (quote) {
      const startLine = at + 1;
      closeLists(0);
      const held = [];
      while (at < lines.length) {
        const more = /^\s*>\s?(.*)$/.exec(lines[at]);
        if (more === null) break;
        held.push(more[1]);
        at += 1;
      }
      const node = document.createElement("blockquote");
      node.className = "md-quote";
      node.dataset.line = born;
      paintInline(node, held.join(" "));
      body.appendChild(markMarkdownReviewTarget(node, startLine, Math.max(startLine, at)));
      continue;
    }

    const item = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/.exec(line);
    if (item) {
      const lineNumber = at + 1;
      const depth = Math.floor(item[1].replace(/\t/g, "  ").length / 2);
      const ordered = /\d/.test(item[2]);
      closeLists(depth + 1);
      if (stack.length <= depth) {
        const list = document.createElement(ordered ? "ol" : "ul");
        list.className = "md-list";
        list.dataset.line = born;
        (stack.length > 0 ? stack[stack.length - 1].item : body).appendChild(list);
        stack.push({ list, item: null });
      }
      const holder = stack[stack.length - 1];
      const node = document.createElement("li");
      // 줄 하나가 항목 하나다 — 목록 전체가 아니라 이 항목의 줄을 적는다.
      node.dataset.line = born;
      // GFM 할 일 상자: 문서가 그린 대로 보이되 눌리지는 않는다 — 여기는 읽는
      // 자리고, 고치는 자리는 원본이다.
      const task = /^\[([ xX])\]\s+(.*)$/.exec(item[3]);
      if (task) {
        const box = document.createElement("input");
        box.type = "checkbox";
        box.className = "md-task";
        box.checked = task[1] !== " ";
        box.disabled = true;
        node.appendChild(box);
      }
      paintInline(node, task ? task[2] : item[3]);
      holder.list.appendChild(markMarkdownReviewTarget(node, lineNumber));
      holder.item = node;
      at += 1;
      continue;
    }

    // Anything else is a paragraph, running to the next blank line.
    const startLine = at + 1;
    const held = [];
    while (at < lines.length && lines[at].trim() !== "" && !opensABlock(lines[at])) {
      held.push(lines[at].trim());
      at += 1;
    }
    // Nothing was taken, which would mean this line opens a block that no
    // branch above claimed \u2014 and the loop would sit on it forever. A
    // renderer pointed at arbitrary files does not get to hang on one.
    if (held.length === 0) {
      at += 1;
      continue;
    }
    const node = document.createElement("p");
    node.className = "md-para";
    node.dataset.line = born;
    paintInline(node, held.join(" "));
    (stack.length > 0 ? stack[stack.length - 1].item ?? body : body).appendChild(
      markMarkdownReviewTarget(node, startLine, Math.max(startLine, at)),
    );
    stack = [];
  }
}

/* ---- mermaid ----
 *
 * Orca's `MermaidViewer` / `MermaidBlock` spot
 * (MermaidViewer-dUZcjW_i.js, MermaidBlock-O_45bwHQ.js, 1.4.164).
 *
 * The diagram is laid out in Rust — `render_mermaid` answers with placed marks,
 * never markup — and this half turns each mark into an element. Which is why
 * there is no sanitising step here: Orca runs DOMPurify over the SVG string its
 * bundled engine produces, and there is no string on this path to purify. Every
 * label arrives through `textContent`.
 *
 * Orca reaches for the engine twice over: once for a `.mmd` file's rich view,
 * once for a ```mermaid fence inside a markdown file. Both go through
 * `paintDiagram` here. */
const SVG_NS = "http://www.w3.org/2000/svg";

/* Marks are drawn in the order Rust listed them, and each is one element.
 *
 * The `default` is a return rather than a throw: a mark kind added to the Rust
 * side before this one learns it should cost that one shape, not the diagram. */
function markNode(mark) {
  if (mark.mark === "rect") {
    const node = document.createElementNS(SVG_NS, "rect");
    node.setAttribute("x", mark.x);
    node.setAttribute("y", mark.y);
    node.setAttribute("width", mark.width);
    node.setAttribute("height", mark.height);
    if (mark.radius > 0) node.setAttribute("rx", mark.radius);
    node.setAttribute("class", mark.class);
    return node;
  }
  if (mark.mark === "polygon" || mark.mark === "polyline") {
    const node = document.createElementNS(SVG_NS, mark.mark);
    node.setAttribute("points", mark.points.map((point) => point.join(",")).join(" "));
    node.setAttribute("class", mark.class);
    return node;
  }
  if (mark.mark === "text") {
    const node = document.createElementNS(SVG_NS, "text");
    node.setAttribute("x", mark.x);
    node.setAttribute("y", mark.y);
    node.setAttribute("text-anchor", mark.anchor === "start" ? "start" : "middle");
    node.setAttribute("class", mark.class);
    // The one way a label enters the document.
    node.textContent = mark.text;
    return node;
  }
  return null;
}

/* One drawing as an `<svg>`.
 *
 * `viewBox` with no width or height, plus the CSS rule that gives it
 * `max-width: 100%`, is Orca's own sizing contract for these
 * (`.mermaid-block svg { max-width: 100%; height: auto }`) — the diagram
 * shrinks to the column and never forces a horizontal scroll of the page. */
function diagramNode(drawing) {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", `0 0 ${drawing.width} ${drawing.height}`);
  svg.setAttribute("width", drawing.width);
  svg.setAttribute("height", drawing.height);
  svg.setAttribute("role", "img");
  svg.setAttribute("class", "mmd");
  for (const mark of drawing.marks) {
    const node = markNode(mark);
    if (node) svg.appendChild(node);
  }
  return svg;
}

/* What to say when there is no picture.
 *
 * Orca shows `Diagram error:` and the source, because its engine either draws or
 * throws. Here the two reasons are different and get different words: a kind
 * with no layout yet, and a file past the node bound. Both keep the source
 * underneath, which is the part that is still useful. */
function diagramNote(undrawn) {
  if (!undrawn) return null;
  if (undrawn.why === "too_large") {
    return t("mermaid.tooLarge", "노드가 너무 많아 그리지 않습니다 ({{nodes}}개 이상, 한도 {{limit}})", {
      nodes: undrawn.nodes,
      limit: undrawn.limit,
    });
  }
  return t("mermaid.undrawn", "{{kind}} 다이어그램은 아직 그리지 않습니다", {
    kind: undrawn.why === "kind" ? undrawn.name : "",
  });
}

/* Lay out `source` and put the result in `into`, replacing whatever was there.
 *
 * `into` is passed rather than returned because the call is asynchronous and the
 * element it fills is already on screen — a diagram in a long markdown file
 * should appear where it belongs while the rest of the document is readable,
 * not hold the whole preview back.
 *
 * A failed call leaves the source showing. `render_mermaid` reads no files and
 * touches no state, so the only way it fails is the window going away, and
 * blanking the text in that moment would be the worst possible answer. */
async function paintDiagram(into, source) {
  let render;
  try {
    render = await invoke("render_mermaid", { source });
  } catch {
    return;
  }
  // An answer that is not a render leaves the source showing, the same as a
  // failed call. The two are the same situation from here: something went wrong
  // on the way to a layout, and the text is what the person can still use.
  if (!render) return;
  // The element may have been thrown away while we were waiting — another file
  // opened, or the mode toggled back to source.
  if (!into.isConnected) return;
  const note = diagramNote(render.undrawn);
  into.replaceChildren();
  into.classList.toggle("mmd-block--undrawn", note !== null);
  if (note !== null) {
    const why = document.createElement("p");
    why.className = "mmd-why";
    why.textContent = note;
    into.appendChild(why);
    const shown = document.createElement("pre");
    shown.className = "mmd-source";
    shown.textContent = source;
    into.appendChild(shown);
    return;
  }
  if (render.drawing) into.appendChild(diagramNode(render.drawing));
}

/* The `.mmd` file's rich view: Orca's `.mermaid-viewer` > `.mermaid-viewer-canvas`
 * > `.mermaid-block`, which is the nesting that centres one diagram in a
 * scrolling pane. */
function paintMermaid(body, source) {
  const canvas = document.createElement("div");
  canvas.className = "mmd-canvas";
  const block = document.createElement("div");
  block.className = "mmd-block";
  canvas.appendChild(block);
  body.appendChild(canvas);
  paintDiagram(block, source);
}

/* Turn every ```mermaid fence in a painted markdown body into a diagram.
 *
 * A sweep after the fact rather than a branch inside the markdown walker: the
 * walker is synchronous and this is not, and a fence that stays a code block
 * until its diagram arrives is the correct intermediate state — it is what the
 * author typed. */
function paintMarkdownDiagrams(body) {
  for (const fence of body.querySelectorAll('pre.md-block[data-language="mermaid"]')) {
    const source = fence.textContent;
    const block = document.createElement("div");
    block.className = "mmd-block mmd-block--inline";
    fence.replaceWith(block);
    paintDiagram(block, source);
  }
}

/* ---- 렌더된 미리보기 탭 (1-g36) ----
 *
 * Orca의 `MarkdownPreview` 자리. 파일 뷰어에도 markdown 모드가 있지만 그것은 한
 * 탭이 원본과 그림 사이를 오가는 토글이고, 이것은 나란히 서는 판이다 — 왼쪽에서
 * 고치고 오른쪽에서 읽는, Orca가 미리보기로 하는 그 일.
 *
 * 그리는 손은 새로 만들지 않았다. 이 창에는 이미 제 손으로 DOM을 조립하는
 * markdown 워커가 있고(`paintMarkdown`), 두 번째 워커는 두 번째 버그다.
 * 미리보기가 보태는 것은 문서의 자리(`mdWhere` — 상대 경로와 앵커)와 판 위의
 * 장식(언어 딱지와 복사 단추)뿐이다. raw HTML은 여기서도 텍스트로 남는다 —
 * 그게 이 창의 sanitize다. */
function buildMdView() {
  const root = document.createElement("div");
  root.className = "mdview-view";
  root.hidden = true;
  const bar = document.createElement("div");
  bar.className = "mdview-toolbar";
  const name = document.createElement("span");
  name.className = "mdview-name";
  const gap = document.createElement("span");
  gap.className = "mdview-gap";
  const toggle = document.createElement("button");
  toggle.type = "button";
  toggle.className = "mdview-toc-toggle";
  toggle.textContent = t("mdview.toc", "목차");
  const refresh = document.createElement("button");
  refresh.type = "button";
  refresh.className = "mdview-refresh";
  refresh.textContent = t("mdview.refresh", "새로고침");
  const raw = document.createElement("button");
  raw.type = "button";
  raw.className = "mdview-raw";
  raw.textContent = t("mdview.raw", "원본 열기");
  // 메모가 몇인지 말하고, 누르면 첫 메모의 블록으로 데려가는 단추(1-g46) —
  // 메모가 없으면 서지 않는다. 복사와 전송도 툴바의 손이다: 카드가 여백에
  // 흩어져 살므로, 전부를 다루는 손은 한 자리에 모은다.
  const notes = document.createElement("button");
  notes.type = "button";
  notes.className = "mdview-notes-toggle";
  notes.hidden = true;
  const noteSend = document.createElement("button");
  noteSend.type = "button";
  noteSend.className = "mdview-notes-send";
  noteSend.textContent = t("mdview.notesSend", "agent 에 보내기");
  noteSend.hidden = true;
  const noteCopy = document.createElement("button");
  noteCopy.type = "button";
  noteCopy.className = "mdview-notes-copy";
  noteCopy.textContent = t("mdview.notesCopy", "agent 용 메모 복사");
  noteCopy.hidden = true;
  bar.append(name, gap, notes, noteSend, noteCopy, toggle, refresh, raw);
  // 프리뷰에서 시작한 찾기 스트립(1-g38) — 이제 문서 뷰 공용이다(P0-15).
  const findbar = buildDocFindBar(() => root._mdviewTab);
  // 판은 둘로 나뉜다: 왼쪽에 목차, 오른쪽에 글. 목차의 오른쪽 모서리가 손잡이다.
  const deck = document.createElement("div");
  deck.className = "mdview-deck";
  const toc = document.createElement("aside");
  toc.className = "mdview-toc";
  const tocHead = document.createElement("div");
  tocHead.className = "mdview-toc-head";
  const tocName = document.createElement("span");
  tocName.className = "mdview-toc-label";
  tocName.textContent = t("mdview.toc", "목차");
  const levels = document.createElement("span");
  levels.className = "mdview-toc-levels";
  for (let level = 1; level <= MD_TOC_EXPAND_ALL; level += 1) {
    const step = document.createElement("button");
    step.type = "button";
    step.className = "mdview-toc-level";
    step.dataset.level = String(level);
    step.textContent = String(level);
    const said =
      level === MD_TOC_EXPAND_ALL
        ? t("mdview.tocExpandAll", "전부 펼치기")
        : t("mdview.tocLevel", "{{level}}수준까지 접기", { level: String(level) });
    step.dataset.tip = said;
    step.setAttribute("aria-label", said);
    levels.appendChild(step);
  }
  const tocGap = document.createElement("span");
  tocGap.className = "mdview-gap";
  const shut = document.createElement("button");
  shut.type = "button";
  shut.className = "mdview-toc-close";
  shut.textContent = "×";
  shut.dataset.tip = t("mdview.tocClose", "목차 닫기");
  shut.setAttribute("aria-label", t("mdview.tocClose", "목차 닫기"));
  tocHead.append(tocName, levels, tocGap, shut);
  const list = document.createElement("div");
  list.className = "mdview-toc-list";
  const grip = document.createElement("div");
  grip.className = "mdview-toc-grip";
  grip.setAttribute("role", "separator");
  grip.setAttribute("aria-orientation", "vertical");
  toc.append(tocHead, list, grip);
  const body = document.createElement("div");
  body.className = "mdview-body";
  const page = document.createElement("article");
  page.className = "mdview-page";
  body.appendChild(page);
  deck.append(toc, body);
  root.append(bar, findbar, deck);
  return root;
}

/* 그룹≠0의 판은 템플릿의 clone이라 리스너가 없다 — 에뮬레이터 뷰의
 * `wireEmulatorHost` 관용구 그대로, 첫 paint가 배선한다(중복 없음). */
function wireMdViewHost(host) {
  if (host._mdviewWired) return;
  host._mdviewWired = true;
  host.querySelector(".mdview-refresh").addEventListener("click", () => {
    const tab = host._mdviewTab;
    if (tab) void loadMarkdownPreview(tab);
  });
  host.querySelector(".mdview-raw").addEventListener("click", () => {
    const tab = host._mdviewTab;
    if (tab) void openFile(tab.path);
  });
  // 목차의 문 두 짝 — 툴바의 것과 패널 제 안의 것은 같은 일을 한다.
  host.querySelector(".mdview-toc-toggle").addEventListener("click", () => {
    const tab = host._mdviewTab;
    if (!tab) return;
    tab.toc = tab.toc === false;
    paintMdView(tab);
  });
  host.querySelector(".mdview-toc-close").addEventListener("click", () => {
    const tab = host._mdviewTab;
    if (!tab) return;
    tab.toc = false;
    paintMdView(tab);
  });
  // 수준 단추 다섯은 한 손으로 받는다 — 줄마다 리스너를 다는 것은 줄마다
  // 리스너를 지우는 일이 된다.
  host.querySelector(".mdview-toc-levels").addEventListener("click", (event) => {
    const tab = host._mdviewTab;
    const step = event.target.closest(".mdview-toc-level");
    if (!tab || !step) return;
    collapseMdTocTo(tab, Number(step.dataset.level));
    paintMdToc(tab, host);
  });
  // 프리뷰의 글도 읽는 판이다(1-g41) — 다만 여기서 우클릭은 두 줄을 연다:
  // 복사와, 긁은 글자를 AI에게 남길 메모로 만드는 줄(1-g43).
  const page = host.querySelector(".mdview-page");
  armSelectionCopyMenu(page, (text) => {
    const tab = host._mdviewTab;
    if (!tab) return [];
    const where = mdSelectionLines(page);
    return [
      {
        label: t("mdview.note", "AI 메모 추가"),
        run: () => openMdReviewNoteEditor(tab, text, where),
      },
    ];
  });
  host.querySelector(".mdview-notes-toggle").addEventListener("click", () => {
    const tab = host._mdviewTab;
    if (!tab) return;
    // 첫 메모의 블록으로 — Orca의 리뷰 툴바 단추가 하는 그 일.
    const first = sortMdReviewNotes(mdReviewNotesOf(tab))[0];
    if (first) mdJumpToNote(tab, host, first);
  });
  host.querySelector(".mdview-notes-copy").addEventListener("click", () => {
    const tab = host._mdviewTab;
    if (tab) copyMdReviewNotes(tab, host);
  });
  host.querySelector(".mdview-notes-send").addEventListener("click", (event) => {
    const tab = host._mdviewTab;
    if (tab) void sendMdReviewNotes(tab, host, event.currentTarget);
  });
  // 여백의 손들(1-g46)은 판에 위임으로 맨다 — 블록은 문서마다 다시 태어나므로
  // 블록마다 손을 매면 조립마다 다시 매야 한다.
  host.querySelector(".mdview-page").addEventListener("click", (event) => {
    const tab = host._mdviewTab;
    if (!tab) return;
    const add = event.target.closest?.(".mdview-note-add");
    if (add) {
      const seat = add.closest(".mdview-note-block");
      // 같은 블록의 +를 다시 누르면 접는다 — Orca의 activeAnnotationBlockKey 토글.
      if (host._mdComposeAt === seat) {
        closeMdReviewNoteEditor(host);
        return;
      }
      openMdReviewNoteEditor(tab, "", {
        start_line: Number(seat.dataset.noteFrom),
        line_number: Number(seat.dataset.noteTill),
      });
      return;
    }
    // 메모가 있는 블록의 본문을 누르면 그 카드가 눈길을 받는다 — 여백(카드
    // 자신)을 누른 것은 제외: 지우는 손이 곧 부르는 손이 되면 안 된다.
    if (event.target.closest?.(".mdview-note-rail")) return;
    const seat = event.target.closest?.(".mdview-note-block.has-notes");
    if (!seat) return;
    const from = Number(seat.dataset.noteFrom);
    const till = Number(seat.dataset.noteTill);
    const held = sortMdReviewNotes(mdReviewNotesOf(tab)).filter(
      (note) => from <= note.line_number && note.line_number <= till,
    );
    const next = held.find((note) => note !== host._mdActiveNote) ?? held[0];
    if (!next) return;
    host._mdActiveNote = next;
    mdPulseNote(tab, host, next);
  });
  wireMdTocGrip(host);
}

/* 목차의 오른쪽 모서리를 끄는 손 — 변경 트리의 손잡이와 같은 관용구다
 * (`wireChangesTreeResize`): 시작점과 시작 너비를 쥐고, 움직인 만큼 더한다. */
function wireMdTocGrip(host) {
  const grip = host.querySelector(".mdview-toc-grip");
  grip.onpointerdown = (event) => {
    const tab = host._mdviewTab;
    if (!tab || event.button !== 0) return;
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = tab.tocWidth ?? MD_TOC_DEFAULT_WIDTH;
    grip.dataset.dragging = "true";
    grip.setPointerCapture?.(event.pointerId);
    grip.onpointermove = (move) => {
      applyMdTocWidth(tab, host, startWidth + move.clientX - startX);
    };
    const finish = () => {
      delete grip.dataset.dragging;
      grip.onpointermove = null;
      grip.onpointerup = null;
      grip.onpointercancel = null;
    };
    grip.onpointerup = finish;
    grip.onpointercancel = finish;
  };
}

/* 목차가 설 수 있는 너비: 200px보다 좁지 않고, 판의 절반보다 넓지 않다 — 읽을
 * 자리가 목차보다 좁아지는 일은 없다. */
function applyMdTocWidth(tab, host, asked) {
  const deck = host.querySelector(".mdview-deck");
  const room = Math.max(MD_TOC_MIN_WIDTH, deck.clientWidth / 2);
  tab.tocWidth = Math.min(room, Math.max(MD_TOC_MIN_WIDTH, Math.round(asked)));
  host.querySelector(".mdview-toc").style.width = `${tab.tocWidth}px`;
}

function paintMdView(tab) {
  const host = docHost(tab.pane, "mdview");
  wireMdViewHost(host);
  host._mdviewTab = tab;
  host.querySelector(".mdview-name").textContent = basename(tab.path);
  const page = host.querySelector(".mdview-page");
  // 같은 글자를 두 번 그리지 않는다 — 탭을 오갈 때마다 다시 조립하면 읽던
  // 자리(스크롤)를 잃는다. 선택기 목록이 쓰는 그 `_painted` 관용구.
  if (page._painted !== tab.text) {
    page._painted = tab.text;
    renderMarkdownPage(page, tab.text ?? "", dirname(tab.path));
    // 글이 다시 서면 목차도 다시 읽는다 — 판이 곧 목차의 원본이다.
    readMdToc(tab, page);
  }
  // 목차 자체는 매 paint마다 — 접힘도 너비도 보임도 탭이 쥐고 있다.
  paintMdToc(tab, host);
  // 찾기 스트립도 같은 이치로 — 서고 눕는 것은 탭의 상태다(1-g38).
  host.querySelector(".doc-findbar").hidden = tab.docFind !== true;
  paintDocFindCount(tab, host);
  // 메모 줄도 매 paint마다 — 카드도 셈도 탭이 쥐고 있다(1-g43).
  paintMdReviewNotes(tab, host);
}

/* 그려진 판에서 읽어 낸 제목의 나무: 제목 하나는 저보다 작은 수준의 가장 가까운
 * 앞 제목 밑에 선다(스택). 판이 원본이므로 markdown을 두 번 읽지 않는다. */
function mdTocTree(page) {
  const roots = [];
  const stack = [];
  for (const head of page.querySelectorAll(".md-head")) {
    const item = {
      id: head.id,
      level: Number(head.tagName.slice(1)),
      text: head.textContent,
      children: [],
    };
    while (stack.length > 0 && stack[stack.length - 1].level >= item.level) stack.pop();
    (stack.length > 0 ? stack[stack.length - 1].children : roots).push(item);
    stack.push(item);
  }
  return roots;
}

/* 나무를 다시 세우고, 접힘을 그 나무에 견준다 — 사라진 부모의 접힘은 같이
 * 사라진다. 자식 없는 줄은 접힐 수 없으므로 부모만 남는다. */
function readMdToc(tab, page) {
  tab.tocItems = mdTocTree(page);
  const held = tab.tocCollapsed;
  if (!held) return;
  const parents = new Set();
  const walk = (items) => {
    for (const item of items) {
      if (item.children.length > 0) parents.add(item.id);
      walk(item.children);
    }
  };
  walk(tab.tocItems);
  for (const id of [...held]) {
    if (!parents.has(id)) held.delete(id);
  }
}

/* 수준 단추: N을 누르면 N수준 이상의 부모가 전부 접힌다. 5는 접을 수준이 아니라
 * 전부 펼치라는 말이다(Orca의 TOC_EXPAND_ALL_LEVEL). */
function collapseMdTocTo(tab, level) {
  const held = new Set();
  if (level < MD_TOC_EXPAND_ALL) {
    const walk = (items) => {
      for (const item of items) {
        if (item.children.length > 0 && item.level >= level) held.add(item.id);
        walk(item.children);
      }
    };
    walk(tab.tocItems ?? []);
  }
  tab.tocCollapsed = held;
}

/* 목차 판을 그린다. 접힌 줄의 자식은 걷지 않는다 — 숨기는 것이 아니라 애초에
 * 세우지 않는다. */
function paintMdToc(tab, host) {
  // 목차는 기본으로 서 있다 — Orca의 미리보기가 그렇다.
  const panel = host.querySelector(".mdview-toc");
  panel.hidden = tab.toc === false;
  panel.style.width = `${tab.tocWidth ?? MD_TOC_DEFAULT_WIDTH}px`;
  const list = host.querySelector(".mdview-toc-list");
  list.replaceChildren();
  const items = tab.tocItems ?? [];
  if (items.length === 0) {
    const empty = document.createElement("div");
    empty.className = "mdview-toc-empty";
    empty.textContent = t("mdview.tocEmpty", "제목이 없습니다");
    list.appendChild(empty);
    return;
  }
  const page = host.querySelector(".mdview-page");
  const collapsed = tab.tocCollapsed;
  const walk = (held, depth) => {
    for (const item of held) {
      const parent = item.children.length > 0;
      const folded = parent && collapsed?.has(item.id) === true;
      const row = document.createElement("div");
      row.className = "mdview-toc-row";
      // 삼각형은 그 줄의 들여쓰기 안에 산다. 자식이 없는 줄만 그 자리를 비워
      // 두고 한 칸 더 들어간다 — 이름들이 한 선에서 시작한다.
      row.style.paddingLeft = `${
        parent
          ? depth === 0
            ? MD_TOC_INDENT_BASE
            : depth * MD_TOC_INDENT_STEP
          : MD_TOC_INDENT_BASE + depth * MD_TOC_INDENT_STEP
      }px`;
      if (parent) {
        const twist = document.createElement("button");
        twist.type = "button";
        twist.className = "mdview-toc-disclosure";
        // 창이 가진 글리프 하나를 돌려 쓴다 — 트리의 `.twist`와 같은 손짓.
        twist.innerHTML = icon("chevron", !folded);
        const said = folded ? t("mdview.tocOpen", "펼치기") : t("mdview.tocFold", "접기");
        twist.setAttribute("aria-label", said);
        twist.addEventListener("click", (event) => {
          // 삼각형은 줄의 이동이 아니다 — 여기서 멈춘다.
          event.stopPropagation();
          const marks = tab.tocCollapsed ?? new Set();
          tab.tocCollapsed = marks;
          if (marks.has(item.id)) marks.delete(item.id);
          else marks.add(item.id);
          paintMdToc(tab, host);
        });
        row.appendChild(twist);
      }
      const said = document.createElement("span");
      said.className = "mdview-toc-name";
      said.textContent = item.text;
      row.appendChild(said);
      actsAsButton(row, () => scrollToDocHeadingId(page, item.id));
      list.appendChild(row);
      if (!folded) walk(item.children, depth + 1);
    }
  };
  walk(items, 0);
}

/* 한 문서를 판 위에 조립한다: 자리를 세우고, 워커가 그리고, 다이어그램이
 * 들어서고, 마지막으로 코드 울타리가 옷을 입는다. */
function renderMarkdownPage(page, text, base) {
  page.replaceChildren();
  mdWhere = { base, page };
  try {
    paintMarkdown(page, text);
  } finally {
    // 그리는 동안만 서 있는 자리 — 손들은 이미 제 몫을 쥐었다.
    mdWhere = null;
  }
  // markdown 파일 속의 다이어그램도 여기선 그려진다(파일 뷰어와 같은 문).
  paintMarkdownDiagrams(page);
  dressMarkdownFences(page);
  // 마지막에 여백이 선다(1-g46) — 다이어그램과 울타리가 제 옷을 다 입은 뒤라,
  // 무엇으로 변했든 블록 하나가 여백 한 칸을 얻는다.
  railMdBlocks(page, text);
}

/* 코드 울타리에 언어 딱지와 복사 단추를 달아 준다.
 *
 * 워커 안의 가지가 아니라 뒤따르는 비질인 이유는 다이어그램 비질과 같다:
 * 울타리는 파일 뷰어에서도 그려지고, 그 판에는 이 장식이 없다. 그리고 이
 * 비질은 다이어그램이 자기 울타리를 가져간 뒤에 온다 — 남은 것만 옷을 입는다. */
function dressMarkdownFences(page) {
  for (const fence of page.querySelectorAll("pre.md-block")) {
    const source = fence.textContent;
    const figure = document.createElement("figure");
    figure.className = "mdview-fence";
    const head = document.createElement("figcaption");
    head.className = "mdview-fence-head";
    const language = document.createElement("span");
    language.className = "mdview-fence-language";
    language.textContent = fence.dataset.language || t("mdview.code", "코드");
    const copy = document.createElement("button");
    copy.type = "button";
    copy.className = "mdview-copy";
    copy.textContent = t("mdview.copy", "복사");
    copy.addEventListener("click", () => void clipboardText.write(source));
    head.append(language, copy);
    fence.replaceWith(figure);
    // 울타리 안의 글자는 `code`가 안고 있는다 — 워커가 준 `pre`는 그대로 두고
    // 그 안에서만 한 겹 감싼다.
    const held = document.createElement("code");
    held.textContent = source;
    fence.replaceChildren(held);
    figure.append(head, fence);
  }
}

/* ---- 프리뷰에서 찾기 (1-g38) ----
 *
 * Orca의 미리보기 검색은 글자를 <mark>로 감싸지 않는다 — 감싸는 순간 문서의
 * 구조가 검색 때문에 달라지고, 그 위에서 목차도 앵커도 어긋난다. 대신 CSS
 * Custom Highlight API에 Range를 등록한다: DOM은 그대로 두고 칠하기만 바꾼다.
 *
 * 문이 없는 엔진(옛 WebKit)에서도 세는 일과 옮기는 일은 그대로 한다 — 칠하지
 * 못할 뿐 찾지 못하는 것은 아니다(Orca도 `if (api)`로 감싼다). */
const docSearchRanges = new Map();

/* 등록부와 하이라이트를 만드는 손, 또는 없으면 null.
 *
 * 두 이름 모두 `window.` 너머로 잡는다: 이 창이 부르는 이름은 창이 스스로
 * 정의한 것이어야 한다는 문지기가 있고, 플랫폼의 새 전역은 그 명단(main.rs)에
 * 오르기 전까지 맨이름으로 부를 수 없다. 기능 탐지도 어차피 이 모양이다. */
function mdviewHighlightApi() {
  const registry = window.CSS?.highlights;
  const paint = window.Highlight;
  if (!registry || !paint) return null;
  return { registry, make: (ranges) => new paint(...ranges) };
}

/* 판 안의 글자 마디를 훑어 일치하는 Range를 모은다. 공백뿐인 마디는 지나친다 —
 * 조립된 DOM에는 줄바꿈만 든 텍스트 노드가 태그마다 있다. */
function docMatchRanges(page, query) {
  const needle = query.toLowerCase();
  const ranges = [];
  const walk = document.createTreeWalker(page, NodeFilter.SHOW_TEXT, {
    acceptNode: (node) =>
      node.textContent.trim() === "" ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT,
  });
  for (let node = walk.nextNode(); node !== null; node = walk.nextNode()) {
    const said = node.textContent.toLowerCase();
    for (let at = said.indexOf(needle); at !== -1; at = said.indexOf(needle, at + needle.length)) {
      const range = document.createRange();
      range.setStart(node, at);
      range.setEnd(node, at + needle.length);
      ranges.push(range);
    }
  }
  return ranges;
}

/* 질의 하나를 판에 건다. 질의가 바뀌면 걸음은 처음으로 돌아간다 — 새 목록의
 * 네 번째는 옛 목록의 네 번째가 아니다. */
/* ---- 문서 찾기 (P0-15) ----
 *
 * 프리뷰의 찾기(1-g38)가 문서 뷰 공용이 된 것: Orca는 어느 문서 뷰에서든
 * ⌘F에 답한다(diff는 편집기 내장 find, `DiffViewer.tsx:434`). 그려진 DOM
 * 위의 한 검색이 전부를 감당하고, 뷰마다 다른 것은 글이 사는 루트뿐이다.
 * side-by-side diff만 예외다 — CodeMirror의 가상 스크롤은 화면 밖 글을
 * DOM에 두지 않으므로, 그 방은 편집기 검색의 길(다음 조각)로 간다. */
const DOC_FIND_ROOTS = {
  mdview: ".mdview-page",
  tokens: ".tokens-body",
  diff: ".diff-rows",
  changes: ".changes-body",
  csv: ".csv-body",
  ipynb: ".ipynb-page",
};

/* side-by-side로 그려진 diff의 수정측 편집기 — 이 판이 그 탭을 보이고 있을
 * 때만. DOM 길(docMatchRanges)은 CodeMirror 가상 스크롤 아래의 글을 볼 수
 * 없으므로, 이 편집기가 서 있으면 doc-find의 손들이 CM 검색으로 갈아탄다.
 * Orca는 같은 표면을 monaco 자체 find 위젯으로 연다(DiffViewer.tsx:433 —
 * find 전용, replace 없음: monaco-find-options.ts). */
function mergeEditorShowing(tab) {
  if (tab?.kind !== "diff" || !diffSideBySide || !diffMergeable(tab)) return null;
  const held = diffViews.get(tab.pane);
  return held && held.showing === tab.id ? held.merge.b : null;
}

/* 질의를 편집기에 앉히고 일치를 센다 — 매번 새로 잰다: 수정측은 타이핑되는
 * 문서라 캐시한 목록은 한 획 뒤의 거짓말이다(file runFind와 같은 이유).
 * 빈 질의도 dispatch한다 — 그것이 CM의 칠을 걷는 손이다. */
function mergeDocHits(tab, editor) {
  const search = boundedQuery(tab.docFindQuery, DOC_SEARCH_QUERY_MAX_BYTES);
  const query = new window.CM6.SearchQuery({ search: search || "", literal: true });
  const hits = search ? findMatches(editor, query) : [];
  washFind(editor, query, hits);
  return hits;
}

/* 찾기 스트립 하나를 짓고 스스로 배선까지 끝낸다 — 다섯 뷰가 같은 바를
 * 나눠 쓰므로 생성과 배선이 갈라지면 어느 뷰 하나가 반쪽 바를 갖게 된다.
 * `resolveTab`은 이 바가 앉은 판의 탭을 답하는 뷰별 결합이고, host는
 * 바의 부모(뷰 루트)다. */
/* index.html이 마크업을 쥔 두 뷰(diff·changes)의 부착 — 템플릿이 처음
 * 지어질 때 한 번, 머리 아래에 바를 앉힌다. 탭 결합은 두 화가가 이미
 * 남기는 `dataset.tab`이다. */
function wireDocFind(view) {
  if (view.querySelector(".doc-findbar")) return;
  const bar = buildDocFindBar(() => tabs.find((one) => one.id === view.dataset.tab));
  // 판마다 머리의 이름이 다르다 — 토큰 화면의 머리는 자기 이름을 쓴다. 바는
  // 머리 바로 밑에 서야 하고, 못 찾으면 판 맨 앞에 선다.
  const head = view.querySelector(".file-view-head, .tokens-head");
  if (head) head.insertAdjacentElement("afterend", bar);
  else view.prepend(bar);
}

function buildDocFindBar(resolveTab) {
  // 결합은 바가 들고 다닌다 — 게이트(the_window_calls_no_function…)는
  // 선언된 이름의 호출만 세므로, 콜백은 `host._mdviewTab`과 같은 멤버
  // 관용으로 부른다.

  const findbar = document.createElement("div");
  findbar.className = "doc-findbar";
  findbar.hidden = true;
  findbar._resolveTab = resolveTab;
  const hostOf = () => findbar.parentElement;
  const findInput = document.createElement("input");
  findInput.className = "doc-find-input";
  findInput.type = "text";
  findInput.spellcheck = false;
  // 지어지는 판은 로드 시점 낱말을 굽는다(보드 메모 6의 함정 1) — 키를
  // 입혀 두면 applyLocale이 고른 언어로 갈아입힌다.
  findInput.dataset.i18nPlaceholder = "doc.find";
  findInput.placeholder = t("doc.find", "문서에서 찾기");
  const findCount = document.createElement("span");
  findCount.className = "doc-find-count";
  const findPrev = document.createElement("button");
  findPrev.type = "button";
  findPrev.className = "doc-find-prev";
  findPrev.innerHTML = icon("back");
  findPrev.dataset.i18nTitle = "doc.findPrev";
  findPrev.dataset.i18nAria = "doc.findPrev";
  findPrev.dataset.tip = t("doc.findPrev", "이전");
  findPrev.setAttribute("aria-label", t("doc.findPrev", "이전"));
  const findNext = document.createElement("button");
  findNext.type = "button";
  findNext.className = "doc-find-next";
  findNext.innerHTML = icon("forward");
  findNext.dataset.i18nTitle = "doc.findNext";
  findNext.dataset.i18nAria = "doc.findNext";
  findNext.dataset.tip = t("doc.findNext", "다음");
  findNext.setAttribute("aria-label", t("doc.findNext", "다음"));
  const findClose = document.createElement("button");
  findClose.type = "button";
  findClose.className = "doc-find-close";
  findClose.textContent = "\u00d7";
  findClose.dataset.i18nTitle = "doc.findClose";
  findClose.dataset.i18nAria = "doc.findClose";
  findClose.dataset.tip = t("doc.findClose", "찾기 닫기");
  findClose.setAttribute("aria-label", t("doc.findClose", "찾기 닫기"));
  const findSepA = document.createElement("span");
  findSepA.className = "doc-find-sep";
  const findSepB = document.createElement("span");
  findSepB.className = "doc-find-sep";
  findbar.append(findInput, findCount, findSepA, findPrev, findNext, findSepB, findClose);
  // 치는 대로 곧장 찾는다 — 로컬 DOM이라 왕복이 없다(브라우저 판의 200ms
  // 디바운스는 판 너머로 질의를 보내는 값, 1-g34).
  findInput.addEventListener("input", () => {
    const tab = findbar._resolveTab();
    if (tab) {
      tab.docFindQuery = findInput.value;
      applyDocSearch(tab, hostOf());
    }
  });
  findInput.addEventListener("keydown", (event) => {
    // 이 줄이 먼저다: 읽는 판에서의 Escape는 탭을 닫는다(창의 Escape 사슬).
    // 상자 안의 키는 상자의 것이다.
    event.stopPropagation();
    const tab = findbar._resolveTab();
    if (!tab) return;
    if (event.key === "Enter") {
      event.preventDefault();
      moveDocMatch(tab, hostOf(), event.shiftKey ? -1 : 1);
    } else if (event.key === "Escape") {
      event.preventDefault();
      closeDocSearch(tab, hostOf());
    }
  });
  findNext.addEventListener("click", () => {
    const tab = findbar._resolveTab();
    if (tab) moveDocMatch(tab, hostOf(), 1);
  });
  findPrev.addEventListener("click", () => {
    const tab = findbar._resolveTab();
    if (tab) moveDocMatch(tab, hostOf(), -1);
  });
  findClose.addEventListener("click", () => {
    const tab = findbar._resolveTab();
    if (tab) closeDocSearch(tab, hostOf());
  });
  return findbar;
}

function applyDocSearch(tab, host) {
  const merge = mergeEditorShowing(tab);
  if (merge) {
    tab.docFindIndex = -1;
    paintDocFindCount(tab, host, mergeDocHits(tab, merge).length);
    return;
  }
  const query = boundedQuery(tab.docFindQuery, DOC_SEARCH_QUERY_MAX_BYTES);
  const root = host.querySelector(DOC_FIND_ROOTS[tab.kind] ?? ".mdview-page");
  const ranges = query && root ? docMatchRanges(root, query) : [];
  if (ranges.length > 0) docSearchRanges.set(tab.id, ranges);
  else docSearchRanges.delete(tab.id);
  tab.docFindIndex = -1;
  paintDocSearch();
  paintDocFindCount(tab, host);
}

/* 등록부는 창 전체에 하나뿐이므로, 살아 있는 모든 프리뷰의 Range를 한 이름
 * 아래 모아 건다(Orca의 `paintMatchHighlight`와 같은 모양). 죽은 탭의 Range는
 * 어느 판에도 걸리지 않으니 여기서 걷어낸다. */
function paintDocSearch() {
  const api = mdviewHighlightApi();
  if (!api) return;
  const all = [];
  const active = [];
  for (const [id, ranges] of docSearchRanges) {
    const tab = tabs.find((one) => one.id === id);
    if (!tab) {
      docSearchRanges.delete(id);
      continue;
    }
    all.push(...ranges);
    const at = tab.docFindIndex ?? -1;
    if (at >= 0 && at < ranges.length) active.push(ranges[at]);
  }
  if (all.length === 0) api.registry.delete(DOC_SEARCH_HIGHLIGHT);
  else api.registry.set(DOC_SEARCH_HIGHLIGHT, api.make(all));
  if (active.length === 0) api.registry.delete(DOC_SEARCH_ACTIVE_HIGHLIGHT);
  else api.registry.set(DOC_SEARCH_ACTIVE_HIGHLIGHT, api.make(active));
}

/* 개수 칸의 규칙: 질의가 없으면 아무 말도 하지 않고, 없으면 없다고 하고,
 * 있으면 몇 번째인지 말한다 — 아직 걷지 않았어도 Enter가 닿을 첫 자리를.
 * 개수의 출처가 다른 길(merge의 CM 일치)은 세 번째 인자로 자기 수를 들고
 * 온다 — 문장은 한 곳에 산다. */
function paintDocFindCount(tab, host, matches = (docSearchRanges.get(tab.id) ?? []).length) {
  const said = host.querySelector(".doc-find-count");
  const query = boundedQuery(tab.docFindQuery, DOC_SEARCH_QUERY_MAX_BYTES);
  if (!query) said.textContent = "";
  else if (matches === 0) said.textContent = t("doc.findNone", "일치 없음");
  else {
    const at = tab.docFindIndex >= 0 ? tab.docFindIndex : 0;
    said.textContent = `${at + 1} / ${matches}`;
  }
}

/* 한 걸음. 처음 누르는 '다음'은 첫 일치로, 처음 누르는 '이전'은 마지막으로 —
 * Orca가 재는 그 식 그대로. */
function moveDocMatch(tab, host, direction) {
  const merge = mergeEditorShowing(tab);
  if (merge) {
    const hits = mergeDocHits(tab, merge);
    if (hits.length === 0) return;
    const cur = tab.docFindIndex ?? -1;
    tab.docFindIndex =
      ((cur >= 0 ? cur : direction === 1 ? -1 : 0) + direction + hits.length) % hits.length;
    const at = hits[tab.docFindIndex];
    // 한 트랜잭션에 선택과 스크롤 — file runFind가 캐럿을 옮기는 그 식.
    merge.dispatch({ selection: { anchor: at.from, head: at.to }, scrollIntoView: true });
    paintDocFindCount(tab, host, hits.length);
    return;
  }
  const ranges = docSearchRanges.get(tab.id) ?? [];
  if (ranges.length === 0) return;
  const cur = tab.docFindIndex ?? -1;
  tab.docFindIndex =
    ((cur >= 0 ? cur : direction === 1 ? -1 : 0) + direction + ranges.length) % ranges.length;
  paintDocSearch();
  paintDocFindCount(tab, host);
  // Range는 스스로 스크롤하지 못한다 — 그 시작이 걸린 원소가 대신 간다.
  ranges[tab.docFindIndex].startContainer.parentElement?.scrollIntoView({ block: "nearest" });
}

/* 상자를 연다. 이미 서 있으면 다시 찾지 않고 초점만 옮긴다 — 걷던 자리를
 * 잃지 않기 위해서다. */
function openDocSearch(tab, host) {
  const input = host.querySelector(".doc-find-input");
  if (tab.docFind !== true) {
    // 선택된 글이 보통 찾는 글이다 — Orca의 seedSearchStringFromSelection:
    // 'selection'(monaco-find-options.ts), file openFind와 같은 관용. 줄을
    // 넘는 선택은 검색어가 아니라 두고 간다.
    const merge = mergeEditorShowing(tab);
    if (merge) {
      const picked = merge.state.sliceDoc(
        merge.state.selection.main.from,
        merge.state.selection.main.to,
      );
      if (picked !== "" && !picked.includes("\n")) tab.docFindQuery = picked;
    }
    tab.docFind = true;
    // 바는 직접 세운다 — mdview의 paint도 같은 판정을 그리지만(그쪽 21255
    // 부근), diff·CSV·노트북의 화가는 이 바를 모르고 알 필요도 없다.
    host.querySelector(".doc-findbar").hidden = false;
    input.value = tab.docFindQuery ?? "";
    applyDocSearch(tab, host);
  }
  input.focus();
  input.select();
}

/* 닫으면 칠도 걷힌다 — 질의도 걸음도 놓고, 키는 창으로 돌려준다. merge
 * 길에서는 편집기로 돌려준다: 일치가 선택된 채 읽던 자리가 거기다(file
 * closeFind와 같은 이유). */
function closeDocSearch(tab, host) {
  tab.docFind = false;
  tab.docFindQuery = "";
  tab.docFindIndex = -1;
  docSearchRanges.delete(tab.id);
  host.querySelector(".doc-find-input").value = "";
  host.querySelector(".doc-findbar").hidden = true;
  paintDocSearch();
  const merge = mergeEditorShowing(tab);
  if (merge) {
    mergeDocHits(tab, merge);
    merge.focus();
    return;
  }
  keySink.focus();
}

/* ---- AI에게 남길 메모 (1-g43) ----
 *
 * Orca 미리보기의 `Add note for the AI`. 이 창에는 이미 그 기능의 절반이 서
 * 있다 — diff의 노트가 같은 Orca 기능의 다른 문이고, 그래서 여기서는 작곡가도
 * (`noteComposer`) 줄 이름표도(`noteLineLabel`) 다시 만들지 않는다. 프리뷰가
 * 보태는 것은 셋뿐이다: 선택이 가리키는 원본의 줄, 그 자리에서 긁어 온 인용,
 * 그리고 그 둘을 agent가 읽는 말로 옮기는 형식.
 *
 * 기억은 창 안에서 끝난다. 무대 기록(`StageTab`)에는 이 칸이 없고 그 규칙은
 * `mdview` 탭 자체를 되살리지 않으므로(stage_layout.rs의 `keeps`), 경로에 매어
 * 둔 이 지도가 창이 사는 동안의 기억이다 — 탭을 닫았다 다시 열어도 그 파일에
 * 붙여 둔 말은 그대로 있다. */
const mdReviewNotesKept = new Map();

/* 카드가 보여 줄 만큼(60자), 그리고 인용이 접히기 시작하는 길이(8줄). */
const MD_NOTE_QUOTE_MAX = 60;
const MD_NOTE_QUOTE_CUT = 57;
const MD_NOTE_EXCERPT_MAX_LINES = 8;
const MD_NOTE_EXCERPT_EDGE_LINES = 4;
const MD_NOTES_COPIED_MS = 1200;
// 부른 카드가 받는 눈길의 길이 — Orca의 attention 타이머 실측 그대로.
const MD_NOTE_PULSE_MS = 900;
// 카드의 복사 단추가 확인의 얼굴로 서 있는 시간 — Orca 실측 그대로.
const MD_NOTE_CARD_COPIED_MS = 1600;

function mdReviewNotesOf(tab) {
  if (!tab.reviewNotes) tab.reviewNotes = mdReviewNotesKept.get(tab.path) ?? [];
  return tab.reviewNotes;
}

function rememberMdReviewNotes(tab) {
  if (tab.reviewNotes?.length > 0) mdReviewNotesKept.set(tab.path, tab.reviewNotes);
  else mdReviewNotesKept.delete(tab.path);
}

/* 선택의 두 끝이 밟고 선 블록의 출생 줄. 도장이 없는 자리는 1로 답한다 — 줄을
 * 모르겠다는 이유로 사람이 쓴 말을 잃는 것보다 낫다. */
function mdSelectionLines(page) {
  const selection = window.getSelection();
  const lineOf = (node) => {
    const held = selectionElement(node)?.closest?.("[data-line]");
    return held && page.contains(held) ? Number(held.dataset.line) : null;
  };
  const ends = [lineOf(selection?.anchorNode), lineOf(selection?.focusNode)].filter((one) =>
    Number.isFinite(one),
  );
  if (ends.length === 0) return { start_line: 1, line_number: 1 };
  return { start_line: Math.min(...ends), line_number: Math.max(...ends) };
}

/* 카드 한 줄에 들어갈 만큼의 인용: 줄바꿈도 들여쓰기도 한 칸으로 눕히고(NBSP와
 * 한자 공백까지 — JS의 `\s`가 이미 그 둘을 안다), 60자에서 끊는다. */
function mdNoteQuote(text) {
  const packed = (text ?? "").replace(/\s+/g, " ").trim();
  if (packed.length <= MD_NOTE_QUOTE_MAX) return packed;
  return `${packed.slice(0, MD_NOTE_QUOTE_CUT).trimEnd()}...`;
}

/* 메모는 쓰인 차례가 아니라 파일과 줄의 차례로 선다 — 읽는 사람도 agent도
 * 문서를 위에서 아래로 읽기 때문이다. */
function sortMdReviewNotes(notes) {
  return [...notes].sort(
    (a, b) =>
      a.file_path.localeCompare(b.file_path) ||
      (a.start_line ?? a.line_number) - (b.start_line ?? b.line_number) ||
      a.line_number - b.line_number ||
      a.created_at - b.created_at,
  );
}

/* 프롬프트 안의 줄 이름표. diff의 것과 한 글자 다르다(`Line: 12`가 아니라
 * `Line 12`) — 두 형식은 Orca에서도 다른 손이 쓴다. */
function mdNoteWhere(note) {
  return note.start_line != null && note.start_line !== note.line_number
    ? `Lines ${note.start_line}-${note.line_number}`
    : `Line ${note.line_number}`;
}

/* 인용은 긁은 글자가 있으면 그것, 없으면 원본의 그 줄들 — 여덟 줄이 넘으면 앞
 * 넷과 뒤 넷만 남기고 가운데를 접는다. 모든 줄은 `> `를 쓴다. */
function mdNoteExcerpt(note, lines) {
  const held =
    note.selected_text
      ? note.selected_text.split("\n")
      : lines.slice(Math.max(0, (note.start_line ?? note.line_number) - 1), note.line_number);
  const shown =
    held.length <= MD_NOTE_EXCERPT_MAX_LINES
      ? held
      : [
          ...held.slice(0, MD_NOTE_EXCERPT_EDGE_LINES),
          "...",
          ...held.slice(-MD_NOTE_EXCERPT_EDGE_LINES),
        ];
  return shown.map((line) => `> ${line}`).join("\n");
}

/* 몸통은 따옴표 안에 들어가므로 네 글자가 제 모습을 잃는다 — Orca의 순서
 * 그대로 역슬래시가 먼저다.
 *
 * `formatDiffNote`가 같은 네 줄을 제 안에 품고 있는 것은 그 줄들이 프로토콜이라
 * 게이트가 **그 함수의 블록 안에서** 읽기 때문이다(main.rs
 * `a_note_travels_in_the_words_orca_sends`). 옮겨 오면 그 게이트가 깨지므로 여기
 * 한 벌을 따로 둔다 — 둘이 어긋나지 않는지는 하네스가 두 손의 결과를 맞대어
 * 지킨다. */
function escapeNoteBody(body) {
  return body
    .split("\\").join("\\\\")
    .split('"').join('\\"')
    .split("\r").join("\\r")
    .split("\n").join("\\n");
}

/* agent가 받는 말. 파일마다 머리말이 한 번 서고, 그 아래로 메모들이 빈 줄로
 * 갈린다. 이름표(`File:`/`Source:`/`Excerpt:`/`User comment:`)는 번역하지
 * 않는다 — 프로토콜이고, diff의 노트가 같은 이유로 그렇다. */
function formatMdReviewNotes(notes, content) {
  const lines = (content ?? "").split("\n");
  const grouped = new Map();
  for (const note of sortMdReviewNotes(notes)) {
    const held = grouped.get(note.file_path) ?? [];
    held.push(note);
    grouped.set(note.file_path, held);
  }
  return [...grouped]
    .map(([path, held]) => {
      const said = held.map((note) =>
        [
          mdNoteWhere(note),
          "Excerpt:",
          mdNoteExcerpt(note, lines),
          `User comment: "${escapeNoteBody(note.body)}"`,
        ].join("\n"),
      );
      return [`File: ${path}`, "Source: markdown", "", said.join("\n\n")].join("\n");
    })
    .join("\n\n");
}

/* ---- 여백의 메모(1-g46) ----
 *
 * Orca의 프리뷰 노트는 아래 어딘가의 목록이 아니라 **그 블록 옆**에 산다
 * (MarkdownPreview `wrapAnnotatedBlock`): 최상위 블록마다 오른쪽 여백
 * 컬럼이 서고, + 는 스치면 나타나고, 카드는 제 블록의 옆에 쌓인다. 1-g43이
 * 세웠던 하단 스트립은 실측이 아니라 이 창의 창작이었으므로 여기서 걷는다. */

/* 최상위 블록마다 여백 한 칸. 끝 줄은 다음 블록의 출생 줄 바로 앞 — 블록
 * 사이 빈 줄까지 이 블록의 몫이 되어, 어떤 줄의 메모도 자리 잃지 않는다.
 * 코드 울타리는 figure로 옷을 입은 뒤라 출생 줄을 한 겹 안에서 찾는다. */
function railMdBlocks(page, text) {
  const last = text.split("\n").length;
  const blocks = [...page.children];
  for (const [at, node] of blocks.entries()) {
    const bornOf = (one) =>
      Number(one?.dataset?.line ?? one?.querySelector?.("[data-line]")?.dataset.line ?? NaN);
    const from = bornOf(node);
    if (!Number.isFinite(from)) continue;
    const ahead = blocks
      .slice(at + 1)
      .map(bornOf)
      .find((line) => Number.isFinite(line));
    const till = ahead ? Math.max(from, ahead - 1) : last;
    const seat = document.createElement("div");
    seat.className = "mdview-note-block";
    seat.dataset.noteFrom = String(from);
    seat.dataset.noteTill = String(till);
    const rail = document.createElement("div");
    rail.className = "mdview-note-rail";
    const add = document.createElement("button");
    add.type = "button";
    add.className = "mdview-note-add";
    add.innerHTML = icon("plus");
    add.dataset.tip = t("mdview.note", "AI 메모 추가");
    add.setAttribute("aria-label", t("mdview.note", "AI 메모 추가"));
    const stack = document.createElement("div");
    stack.className = "mdview-note-stack";
    rail.append(add, stack);
    node.replaceWith(seat);
    seat.append(node, rail);
  }
}

/* 어느 블록의 여백인가 — 메모의 줄이 낀 블록. */
function mdNoteBlockOf(host, line) {
  return [...host.querySelectorAll(".mdview-note-block")].find(
    (seat) =>
      Number(seat.dataset.noteFrom) <= line && line <= Number(seat.dataset.noteTill),
  );
}

/* 쓰는 자리는 그 블록의 여백이다 — diff의 노트가 쓰는 그 작곡가를 + 아래
 * 앉힌다(Enter 저장, Shift+Enter 줄바꿈, Escape 취소도 거기서 온다). 선택이
 * 연 경우 긁은 글자가 인용으로 실리고, + 가 연 경우 인용은 원본 줄들이
 * 대신한다(mdNoteExcerpt의 폴백 — 블록 전체가 곧 인용이다). */
function openMdReviewNoteEditor(tab, text, where) {
  const host = docHost(tab.pane, "mdview");
  const seat = mdNoteBlockOf(host, where.line_number);
  if (!seat) return;
  closeMdReviewNoteEditor(host);
  host._mdComposeAt = seat;
  const box = document.createElement("div");
  box.className = "mdview-note-railseat";
  box.appendChild(
    noteComposer({
      initial: "",
      onCancel: () => closeMdReviewNoteEditor(host),
      onSave: (body) => {
        addMdReviewNote(tab, {
          ...where,
          file_path: tab.path,
          ...(text ? { selected_text: text } : {}),
          body,
          created_at: Date.now(),
        });
        closeMdReviewNoteEditor(host);
        paintMdReviewNotes(tab, host);
      },
    }),
  );
  seat.querySelector(".mdview-note-add").after(box);
  seat.scrollIntoView({ behavior: "smooth", block: "nearest" });
}

function closeMdReviewNoteEditor(host) {
  host._mdComposeAt = null;
  for (const box of host.querySelectorAll(".mdview-note-railseat")) box.remove();
}

function addMdReviewNote(tab, note) {
  mdReviewNotesOf(tab).push(note);
  rememberMdReviewNotes(tab);
}

function dropMdReviewNote(tab, note) {
  tab.reviewNotes = mdReviewNotesOf(tab).filter((one) => one !== note);
  rememberMdReviewNotes(tab);
}

/* 카드 하나: 줄 이름표와 손들의 알약(1-g47 — Orca DiffCommentCard의
 * actions-pill 실측 순서: 복사→전송→수정→삭제), 긁은 글의 인용, 남긴 말.
 * 손들은 diff 노트의 그 알약 단추(`note-card-act`)를 그대로 입고, 눌림을 제
 * 안에서 끝낸다 — 블록 클릭(카드를 부르는 손)과 섞이지 않도록. 지금 가리키는
 * 카드는 is-active를, 방금 불린 카드는 is-attention(0.9s 펄스)을 입는다. */
function mdNoteCard(tab, host, note) {
  const card = document.createElement("div");
  card.className = "mdview-note-card";
  card.classList.toggle("is-active", host._mdActiveNote === note);
  card.classList.toggle("is-attention", host._mdAttentionNote === note);
  const head = document.createElement("div");
  head.className = "mdview-note-head";
  const line = document.createElement("span");
  line.className = "mdview-note-line";
  line.textContent = noteLineLabel(note);
  const acts = document.createElement("span");
  acts.className = "note-card-acts";
  const act = (name, said, extra, run) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = `note-card-act${extra ? ` ${extra}` : ""}`;
    button.innerHTML = icon(name);
    button.dataset.tip = said;
    button.setAttribute("aria-label", said);
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      run(event);
    });
    acts.appendChild(button);
  };
  // 복사는 잠깐 확인의 얼굴(check)이 됐다가 돌아온다 — Orca 실측 1600ms.
  const copiedNow = host._mdCopiedNote === note;
  act(
    copiedNow ? "check" : "copy",
    copiedNow
      ? t("mdview.notesCopied", "복사되었습니다")
      : t("mdview.notesCopy", "agent 용 메모 복사"),
    "mdview-note-copy",
    () => {
      void clipboardText.write(formatMdReviewNotes([note], tab.text ?? ""));
      clearTimeout(host._mdCopiedTimer);
      host._mdCopiedNote = note;
      paintMdReviewNotes(tab, host);
      host._mdCopiedTimer = setTimeout(() => {
        host._mdCopiedNote = null;
        if (host._mdviewTab === tab) paintMdReviewNotes(tab, host);
      }, MD_NOTE_CARD_COPIED_MS);
    },
  );
  // 이 메모 하나만 보낸다 — 같은 피커, 같은 규칙(전달되면 보낸 것만 떠난다).
  act("send", t("mdview.notesSend", "agent 에 보내기"), "mdview-note-send", (event) => {
    void openSendToAgent(
      event.currentTarget,
      formatMdReviewNotes([note], tab.text ?? ""),
      () => {
        dropMdReviewNote(tab, note);
        if (host._mdviewTab === tab) paintMdReviewNotes(tab, host);
      },
      { submit: false },
    );
  });
  // 수정: 몸이 작곡가로 바뀐다 — diff 카드의 그 관용(같은 말이면 저장하지
  // 않는 것도 작곡가의 계약이다).
  act("pencil", t("review.edit", "노트 수정"), "mdview-note-edit", () => {
    acts.hidden = true;
    body.replaceWith(
      noteComposer({
        initial: note.body,
        onCancel: () => paintMdReviewNotes(tab, host),
        onSave: (text) => {
          note.body = text;
          rememberMdReviewNotes(tab);
          paintMdReviewNotes(tab, host);
        },
      }),
    );
  });
  act("trash", t("review.delete", "노트 삭제"), "mdview-note-del", () => {
    dropMdReviewNote(tab, note);
    paintMdReviewNotes(tab, host);
  });
  head.append(line, acts);
  const quote = document.createElement("div");
  quote.className = "mdview-note-quote";
  quote.textContent = mdNoteQuote(note.selected_text);
  quote.hidden = !note.selected_text;
  const body = document.createElement("div");
  body.className = "mdview-note-body";
  body.textContent = note.body;
  card.append(head, quote, body);
  return card;
}

/* 메모를 저마다의 블록 여백에 분배한다 — 목록이 아니라 지면 위의 자리다.
 * 툴바의 셈 단추는 몇인지 말하고, 누르면 첫 메모의 블록으로 데려간다. */
function paintMdReviewNotes(tab, host) {
  const notes = sortMdReviewNotes(mdReviewNotesOf(tab));
  const toggle = host.querySelector(".mdview-notes-toggle");
  toggle.hidden = notes.length === 0;
  toggle.textContent = t("mdview.notes", "메모 {{n}}", { n: String(notes.length) });
  host.querySelector(".mdview-notes-send").hidden = notes.length === 0;
  host.querySelector(".mdview-notes-copy").hidden = notes.length === 0;
  // 복사했다는 말은 잠깐만 서 있는다 — 브라우저 판의 배너가 쓰는 그 시한 관용구.
  host.querySelector(".mdview-notes-copy").textContent =
    Date.now() < (tab.mdNotesCopiedUntil ?? 0)
      ? t("mdview.notesCopied", "복사되었습니다")
      : t("mdview.notesCopy", "agent 용 메모 복사");
  for (const seat of host.querySelectorAll(".mdview-note-block")) {
    const from = Number(seat.dataset.noteFrom);
    const till = Number(seat.dataset.noteTill);
    const held = notes.filter(
      (note) => from <= note.line_number && note.line_number <= till,
    );
    seat.classList.toggle("has-notes", held.length > 0);
    seat
      .querySelector(".mdview-note-stack")
      .replaceChildren(...held.map((note) => mdNoteCard(tab, host, note)));
  }
}

/* 부른 카드에 0.9초의 눈길 — 다시 부르면 눈길도 처음부터(Orca 실측: rAF로
 * 내려놓았다가 다시 얹는다). */
function mdPulseNote(tab, host, note) {
  clearTimeout(host._mdPulseTimer);
  host._mdAttentionNote = null;
  paintMdReviewNotes(tab, host);
  requestAnimationFrame(() => {
    host._mdAttentionNote = note;
    paintMdReviewNotes(tab, host);
    host._mdPulseTimer = setTimeout(() => {
      host._mdAttentionNote = null;
      if (host._mdviewTab === tab) paintMdReviewNotes(tab, host);
    }, MD_NOTE_PULSE_MS);
  });
}

/* 메모의 블록으로 — 판이 부드럽게 옮겨 가고, 그 카드가 눈길을 받는다. */
function mdJumpToNote(tab, host, note) {
  const seat = mdNoteBlockOf(host, note.line_number);
  if (!seat) return;
  host._mdActiveNote = note;
  seat.scrollIntoView({ behavior: "smooth", block: "center" });
  mdPulseNote(tab, host, note);
}

function copyMdReviewNotes(tab, host) {
  const notes = mdReviewNotesOf(tab);
  if (notes.length === 0) return;
  // 인용을 뜰 원본은 탭이 이미 쥐고 있다 — 파일을 다시 읽지 않는다.
  void clipboardText.write(formatMdReviewNotes(notes, tab.text ?? ""));
  tab.mdNotesCopiedUntil = Date.now() + MD_NOTES_COPIED_MS;
  paintMdReviewNotes(tab, host);
  // 그 말을 거둘 손이 없으면 확인은 영영 남는다.
  setTimeout(() => {
    if (host._mdviewTab === tab) paintMdReviewNotes(tab, host);
  }, MD_NOTES_COPIED_MS);
}

/* 메모를 에이전트의 입력까지 보낸다(1-g44) — Orca의 ReviewNotesSendMenu가
 * 하는 그 일을, 이 창은 grab이 이미 걷는 길(`openSendToAgent`)로 한다:
 * 대상 고르기, 새 에이전트 열기, 그리고 **입력까지만** — Enter는 사람 몫
 * (1-g5의 규칙 그대로). 전달되면 보낸 메모만 떠난다. */
async function sendMdReviewNotes(tab, host, button) {
  // 그 시점의 사본으로 — mdReviewNotesOf는 살아 있는 배열을 주므로, 피커가
  // 뜬 사이에 적힌 메모가 이 목록으로 스며들면 아래 filter가 agent가 보지
  // 못한 말까지 걷어 간다.
  const notes = [...mdReviewNotesOf(tab)];
  if (notes.length === 0) return;
  await openSendToAgent(
    button,
    formatMdReviewNotes(notes, tab.text ?? ""),
    () => {
      // 보낸 말만 떠난다 — 전송 중에 새로 적힌 메모는 agent가 못 본 지시다
      // (diff 노트의 `clear_delivered`와 같은 결).
      tab.reviewNotes = mdReviewNotesOf(tab).filter((held) => !notes.includes(held));
      if (host._mdviewTab === tab) paintMdReviewNotes(tab, host);
    },
    { submit: false },
  );
}

/* 판을 다시 채운다 — 새로고침 단추와, 원본이 저장된 순간이 부른다. */
async function loadMarkdownPreview(tab) {
  let opened;
  try {
    opened = await invoke("read_text_file", { path: tab.path });
  } catch (error) {
    showError(`${tab.path}: ${error}`);
    return;
  }
  tab.text = opened.text;
  if (!stillShowing(tab)) return;
  paintMdView(tab);
  // Range는 DOM에 묶인다 — 새로 조립된 판 위에서는 옛 Range가 아무것도 가리키지
  // 않으므로, 찾고 있던 질의는 새 판에서 다시 건다(1-g38).
  if (tab.docFind === true && tab.docFindQuery) {
    applyDocSearch(tab, docHost(tab.pane, "mdview"));
  }
}

/* 미리보기 문. 이미 선 판이 있으면 그 판으로 갈 뿐 두 번 읽지 않고, 없으면
 * 이미지 뷰어가 하는 그대로 먼저 읽고 나서 탭을 세운다 — 읽지 못하는 파일로
 * 빈 탭을 만들지 않기 위해서다. */
async function openMarkdownPreview(path, opts = {}) {
  const held = tabs.find((tab) => tab.id === `mdview:${path}`);
  if (held) {
    setActiveTab(held.id);
    return;
  }
  let opened;
  try {
    opened = await invoke("read_text_file", { path });
  } catch (error) {
    showError(`${path}: ${error}`);
    return;
  }
  openTab({
    id: `mdview:${path}`,
    kind: "mdview",
    path,
    text: opened.text,
    preview: Boolean(opts.preview),
  });
}

/* 저장은 파생된 판의 사실도 바꾼다 — 같은 파일을 보고 있는 판이 있으면 다시
 * 읽어 온다: 미리보기(1-g36), 표(1-g39), 노트북(1-g40). 저장 문 하나가 이
 * 하나를 부르고, 새 판이 생기면 여기에 한 줄이 는다. */
function refreshDerivedViewsOf(path) {
  const preview = tabs.find((tab) => tab.kind === "mdview" && tab.path === path);
  if (preview) void loadMarkdownPreview(preview);
  const sheet = tabs.find((tab) => tab.kind === "csv" && tab.path === path);
  if (sheet) void loadCsvSheet(sheet);
  const book = tabs.find((tab) => tab.kind === "ipynb" && tab.path === path);
  if (book) void loadIpynbBook(book);
}

/* ---- separated values ----
 *
 * Orca's `CsvViewer` spot. A real split, not `line.split(",")`: a quoted
 * field can hold the separator, a newline and a doubled quote, and a viewer
 * that ignores that shows a table with the columns shifted from the first
 * quoted comma onward — wrong in a way that looks like data.
 *
 * 판은 둘이다(1-g39): 파일 뷰어 안의 작은 표(`paintTable`, 500줄 상한)와, 제 탭으로
 * 서는 시트(`paintCsvView` — 고정 줄 높이로 창을 넓혀 백만 줄도 스크롤한다).
 * 가르는 기계는 하나뿐이다: 두 벌의 파서는 같은 파일을 두 가지로 읽는다. */
const CSV_ROW_CAP = 500;

/* 한 대의 파서. 인용 안의 `""`는 따옴표 한 개이고, 따옴표는 칸의 첫 글자일 때만
 * 인용을 연다 — `a"b`의 것은 글자다. CRLF는 한 번의 끊김이고, 맨 앞의 BOM은
 * 글자가 아니라 표식이라 첫 칸의 이름에 섞이지 않는다. 가장 넓은 줄의 칸 수도
 * 함께 세어 둔다: 짧은 줄은 표에서 빈 칸으로 메워져야 한다. */
function parseCsvText(source, delimiter = ",") {
  const text = source.charCodeAt(0) === CSV_BOM ? source.slice(1) : source;
  const rows = [];
  let row = [];
  let field = "";
  let quoted = false;
  let maxColumns = 0;
  const closeRow = () => {
    row.push(field);
    field = "";
    rows.push(row);
    if (row.length > maxColumns) maxColumns = row.length;
    row = [];
  };
  for (let at = 0; at < text.length; at += 1) {
    const c = text[at];
    if (quoted) {
      if (c !== '"') field += c;
      else if (text[at + 1] === '"') {
        field += '"';
        at += 1;
      } else quoted = false;
      continue;
    }
    if (c === '"' && field === "") quoted = true;
    else if (c === delimiter) {
      row.push(field);
      field = "";
    } else if (c === "\n" || c === "\r") {
      // A CRLF is one ending, not two — counted as two it puts an empty row
      // between every pair of real ones.
      if (c === "\r" && text[at + 1] === "\n") at += 1;
      closeRow();
    } else field += c;
  }
  if (field !== "" || row.length > 0) closeRow();
  return { rows, maxColumns };
}

/* 파일 뷰어의 작은 표가 부르는 문 — 줄만 필요하다. */
function parseSeparated(text, separator) {
  return parseCsvText(text, separator).rows;
}

function paintTable(body, text, tabbed) {
  const rows = parseSeparated(text, tabbed ? "\t" : ",");
  if (rows.length === 0) return;
  const table = document.createElement("table");
  table.className = "md-table";
  for (const [index, cells] of rows.slice(0, CSV_ROW_CAP).entries()) {
    const row = document.createElement("tr");
    for (const cell of cells) {
      // The first row heads the table. Every separated file a person opens in
      // a viewer has one, and a table with no header reads as data with the
      // labels missing rather than as a file that had none.
      const node = document.createElement(index === 0 ? "th" : "td");
      node.textContent = cell;
      row.appendChild(node);
    }
    table.appendChild(row);
  }
  body.appendChild(table);
  if (rows.length > CSV_ROW_CAP) {
    const more = document.createElement("div");
    more.className = "file-line file-line--more";
    more.textContent = t("file.more", "… {{count}}줄 더 (뷰어 상한)", {
      count: rows.length - CSV_ROW_CAP,
    });
    body.appendChild(more);
  }
}

/* 냄새를 맡을 첫 줄: 앞의 64KiB 안에서 처음으로 빈칸이 아닌 줄. 빈 줄로 시작하는
 * 파일이 드물지 않고, 빈 줄에는 셀 구분자가 없다. */
function firstCsvLine(text) {
  const limit = Math.min(text.length, CSV_SNIFF_LIMIT);
  let start = 0;
  let blank = true;
  for (let at = 0; at < limit; at += 1) {
    const code = text.charCodeAt(at);
    if (code === 13 || code === 10) {
      if (!blank) return text.slice(start, at);
      if (code === 13 && text.charCodeAt(at + 1) === 10) at += 1;
      start = at + 1;
      continue;
    }
    if (!CSV_BLANKS.has(code)) blank = false;
  }
  return blank ? "" : text.slice(start, limit);
}

/* 무엇으로 갈린 파일인가. 이름이 `.tsv`라고 말하면 그 말이 이긴다 — 사람이 붙인
 * 이름은 내용보다 확실한 선언이다. 그 밖에는 첫 줄에서 인용 밖의 탭과 쉼표를
 * 세어 더 많은 쪽을 고른다: 인용 안의 탭은 값이지 구분자가 아니다. */
function sniffCsvDelimiter(path, text) {
  if (path.toLowerCase().endsWith(".tsv")) return "\t";
  const held = text.charCodeAt(0) === CSV_BOM ? text.slice(1) : text;
  const line = firstCsvLine(held);
  let tabs = 0;
  let commas = 0;
  let quoted = false;
  for (let at = 0; at < line.length; at += 1) {
    const c = line[at];
    if (c === '"') {
      if (quoted && line[at + 1] === '"') {
        at += 1;
        continue;
      }
      quoted = !quoted;
      continue;
    }
    if (quoted) continue;
    if (c === "\t") tabs += 1;
    else if (c === ",") commas += 1;
  }
  return tabs > commas ? "\t" : ",";
}

/* 칸의 너비: 글자 수에 글자폭을 곱하고 여백을 더해 죈다. 머리줄과 앞의 200줄만
 * 재는 것은 폭을 알자고 파일 전체를 훑을 수는 없어서고, 아무 줄도 닿지 않은
 * 칸은 가장 좁은 폭으로 선다. */
function csvColumnWidths(rows, maxColumns) {
  const widths = new Array(maxColumns).fill(CSV_MIN_COL_PX);
  const sampled = Math.min(rows.length, CSV_WIDTH_SAMPLE_ROWS + 1);
  for (let row = 0; row < sampled; row += 1) {
    const cells = rows[row];
    for (let column = 0; column < cells.length; column += 1) {
      const wide = Math.min(
        CSV_MAX_COL_PX,
        Math.max(CSV_MIN_COL_PX, cells[column].length * CSV_CHAR_PX + CSV_CELL_PADDING_PX),
      );
      if (wide > widths[column]) widths[column] = wide;
    }
  }
  return widths;
}

/* 한 번 읽은 파일은 한 번만 가른다 — 같은 글자가 그대로면 그때 지은 시트를
 * 그대로 쓴다(선택기 목록이 쓰는 그 `_painted` 관용구). */
function csvSheetOf(tab) {
  const text = tab.csvText ?? "";
  if (tab.csvSheet && tab.csvSheet.text === text) return tab.csvSheet;
  const delimiter = sniffCsvDelimiter(tab.path, text);
  const { rows, maxColumns } = parseCsvText(text, delimiter);
  const widths = csvColumnWidths(rows, maxColumns);
  tab.csvSheet = {
    text,
    delimiter,
    rows,
    maxColumns,
    widths,
    // 줄 번호 칸이 맨 앞에 서고, 그 뒤로 잰 너비가 차례로 선다.
    template: [
      `${CSV_ROW_NUMBER_COL_PX}px`,
      ...widths.map((wide) => `${wide}px`),
    ].join(" "),
  };
  return tab.csvSheet;
}

function buildCsvView() {
  const root = document.createElement("div");
  root.className = "csv-view";
  root.hidden = true;
  const bar = document.createElement("div");
  bar.className = "csv-toolbar";
  const name = document.createElement("span");
  name.className = "csv-name";
  const gap = document.createElement("span");
  gap.className = "csv-gap";
  const dims = document.createElement("span");
  dims.className = "csv-dims";
  const raw = document.createElement("button");
  raw.type = "button";
  raw.className = "csv-raw";
  raw.textContent = t("csv.raw", "원본 열기");
  bar.append(name, gap, dims, raw);
  const body = document.createElement("div");
  body.className = "csv-body";
  const sheet = document.createElement("div");
  sheet.className = "csv-sheet";
  sheet.setAttribute("role", "table");
  const head = document.createElement("div");
  head.className = "csv-head";
  head.setAttribute("role", "row");
  const slice = document.createElement("div");
  slice.className = "csv-slice";
  sheet.append(head, slice);
  const empty = document.createElement("div");
  empty.className = "csv-empty";
  empty.hidden = true;
  empty.dataset.i18n = "csv.empty";
  empty.textContent = t("csv.empty", "빈 파일");
  body.append(sheet, empty);
  root.append(bar, buildDocFindBar(() => root._csvTab), body);
  return root;
}

/* 그룹≠0의 판은 템플릿의 clone이라 리스너가 없다 — 미리보기 판과 같은 관용구. */
function wireCsvHost(host) {
  if (host._csvWired) return;
  host._csvWired = true;
  host.querySelector(".csv-raw").addEventListener("click", () => {
    const tab = host._csvTab;
    if (tab) void openFile(tab.path);
  });
  // 스크롤은 창을 옮길 뿐이다 — 머리줄도 너비도 그대로이므로 조각만 다시 그린다.
  host.querySelector(".csv-body").addEventListener(
    "scroll",
    () => {
      const tab = host._csvTab;
      if (tab) paintCsvSlice(tab, host);
    },
    { passive: true },
  );
}

function paintCsvView(tab) {
  const host = docHost(tab.pane, "csv");
  wireCsvHost(host);
  host._csvTab = tab;
  host.querySelector(".csv-name").textContent = basename(tab.path);
  const sheet = csvSheetOf(tab);
  const board = host.querySelector(".csv-sheet");
  const empty = sheet.rows.length === 0;
  host.querySelector(".csv-empty").hidden = !empty;
  board.hidden = empty;
  const dims = host.querySelector(".csv-dims");
  if (empty) {
    dims.textContent = "";
    return;
  }
  // 머리줄은 첫 줄이다 — 사람이 뷰어로 여는 표에는 대개 이름줄이 있고, 이름
  // 없는 표는 이름이 빠진 데이터로 읽힌다.
  dims.textContent = t("csv.dims", "{{rows}} × {{columns}}", {
    rows: Math.max(0, sheet.rows.length - 1).toLocaleString(),
    columns: sheet.maxColumns.toLocaleString(),
  });
  const head = host.querySelector(".csv-head");
  if (head._painted !== sheet) {
    head._painted = sheet;
    head.style.gridTemplateColumns = sheet.template;
    head.replaceChildren();
    const corner = document.createElement("div");
    corner.className = "csv-rownum csv-rownum--head";
    corner.setAttribute("role", "columnheader");
    corner.textContent = "#";
    head.appendChild(corner);
    for (let column = 0; column < sheet.maxColumns; column += 1) {
      const cell = document.createElement("div");
      cell.className = "csv-cell csv-cell--head";
      cell.setAttribute("role", "columnheader");
      cell.textContent = sheet.rows[0][column] ?? "";
      head.appendChild(cell);
    }
  }
  paintCsvSlice(tab, host);
}

/* 보이는 만큼만 그린다.
 *
 * 줄 높이가 고정이라 어디까지 왔는지는 나눗셈 한 번이다: 위아래로 열두 줄을 더
 * 그려 두고, 그리지 않은 줄들의 자리는 위아래 여백이 대신 차지한다 — 스크롤
 * 막대는 파일 전체의 길이를 그대로 말한다. */
function paintCsvSlice(tab, host) {
  const sheet = csvSheetOf(tab);
  if (sheet.rows.length === 0) return;
  const body = host.querySelector(".csv-body");
  const slice = host.querySelector(".csv-slice");
  const count = sheet.rows.length - 1;
  const room = Math.ceil(body.clientHeight / CSV_ROW_HEIGHT) + CSV_OVERSCAN * 2;
  const first = Math.min(
    Math.max(0, Math.floor(body.scrollTop / CSV_ROW_HEIGHT) - CSV_OVERSCAN),
    Math.max(0, count - 1),
  );
  const last = Math.min(count, first + room);
  // 같은 시트의 같은 창이면 다시 그릴 것이 없다 — 스크롤 한 번에 수십 줄을
  // 다시 짓는 일은 스크롤이 하는 일 중 가장 비싼 것이다.
  const frame = `${first}:${last}`;
  if (slice._sheet === sheet && slice._frame === frame) return;
  slice._sheet = sheet;
  slice._frame = frame;
  slice.style.paddingTop = `${first * CSV_ROW_HEIGHT}px`;
  slice.style.paddingBottom = `${(count - last) * CSV_ROW_HEIGHT}px`;
  const held = document.createDocumentFragment();
  for (let at = first; at < last; at += 1) {
    const cells = sheet.rows[at + 1];
    const row = document.createElement("div");
    row.className = "csv-row";
    row.setAttribute("role", "row");
    row.style.gridTemplateColumns = sheet.template;
    const number = document.createElement("div");
    number.className = "csv-rownum";
    number.setAttribute("role", "rowheader");
    number.textContent = String(at + 1);
    row.appendChild(number);
    for (let column = 0; column < sheet.maxColumns; column += 1) {
      const cell = document.createElement("div");
      cell.className = "csv-cell";
      cell.setAttribute("role", "cell");
      // 짧은 줄은 빈 칸으로 메운다 — 칸이 밀리면 표가 거짓말을 한다.
      cell.textContent = cells[column] ?? "";
      row.appendChild(cell);
    }
    held.appendChild(row);
  }
  slice.replaceChildren(held);
}

/* 표의 문. 이미 선 판이 있으면 그 판으로 갈 뿐이고, 없으면 먼저 읽고 나서 탭을
 * 세운다 — 미리보기 탭과 같은 예절이다(1-g36). */
async function openCsv(path, opts = {}) {
  const held = tabs.find((tab) => tab.id === `csv:${path}`);
  if (held) {
    setActiveTab(held.id);
    return;
  }
  let opened;
  try {
    opened = await invoke("read_text_file", { path });
  } catch (error) {
    showError(`${path}: ${error}`);
    return;
  }
  openTab({
    id: `csv:${path}`,
    kind: "csv",
    path,
    csvText: opened.text,
    preview: Boolean(opts.preview),
  });
}

/* 디스크의 글자가 바뀌면 표도 다시 갈린다 — 저장이 부른다. */
async function loadCsvSheet(tab) {
  let opened;
  try {
    opened = await invoke("read_text_file", { path: tab.path });
  } catch (error) {
    showError(`${tab.path}: ${error}`);
    return;
  }
  tab.csvText = opened.text;
  if (stillShowing(tab)) paintCsvView(tab);
}

/* ---- Skills: a shared, keyed document surface ---- */
let skillsReport = null;
let skillsPending = null;
let skillsNeedsRescan = false;
let skillsInstallTerm = null;
let skillsOnboardingSuspended = false;
let skillsInstallWatch = null;
const skillsViews = new WeakMap();
const skillsBadgeWired = new WeakSet();

function openSkillsView() {
  setSettingsOpen(false);
  openTab({ id: "skills", kind: "skills" });
}
function skillText(key, vars = {}) { return t(`skills.${key}`, CATALOG.ko[`skills.${key}`] ?? key, vars); }
function skillsError(error) {
  const key = String(error).replace(/^Error: /, "");
  return key.startsWith("skills.") ? skillText(key.slice("skills.".length)) : key;
}
function skillElement(tag, className, text) {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}
function skillButton(key, click, className = "btn", vars = {}) {
  const button = skillElement("button", className, skillText(key, vars));
  button.type = "button";
  button.addEventListener("click", click);
  return button;
}
function skillOption(label, value, _defaultSelected = false, selected = false) {
  const option = document.createElement("option"); option.textContent = label; option.value = value; option.selected = selected; return option;
}
function buildSkillsView() {
  const view = skillElement("section", "file-view skills-view");
  view.dataset.keyboardOwner = "true";
  view.hidden = true;
  // docHost clones templates, so listeners are wired on the actual host below.
  view.innerHTML = `<header class="skills-heading"><div><p class="skills-eyebrow"></p><h1></h1><p class="skills-about"></p></div><div class="skills-actions"><button type="button" class="btn skills-onboarding-return" hidden></button><button type="button" class="btn skills-rescan"></button><button type="button" class="btn primary skills-bundle-open"></button></div></header>
    <div class="skills-toolbar"><input type="search" class="skills-search"/><select class="skills-root-filter"></select><select class="skills-agent-filter"></select><span class="skills-total" role="status"></span></div>
    <div class="skills-required"></div><p class="skills-evidence-scope skills-usage-scope"></p><p class="skills-error" role="alert" hidden></p><p class="skills-plan-status" role="status" hidden></p><p class="skills-native-status" role="status" hidden></p>
    <div class="skills-columns"><div class="skills-list-pane"><div class="skills-card-list" role="list"></div><p class="skills-empty"></p><details class="skills-roots-panel"><summary></summary><div></div></details></div><aside class="skills-detail"><p class="skills-detail-empty"></p><div class="skills-detail-content" hidden></div></aside></div>
    <section class="skills-bundle" hidden></section>`;
  return view;
}
function wireSkillsView(view) {
  if (skillsViews.has(view)) return skillsViews.get(view);
  const state = { rows: new Map(), selected: null, generation: 0, report: null, query: "", root: "all", agent: "all" };
  skillsViews.set(view, state);
  // A new leaf can be cloned from the populated first leaf. Its cache owns
  // exactly its own rows, never dead copies of another leaf's listeners.
  view.querySelector(".skills-card-list").replaceChildren();
  view.querySelector(".skills-root-filter").replaceChildren();
  view.querySelector(".skills-detail-content").replaceChildren();
  view.querySelector(".skills-detail-content").hidden = true;
  view.querySelector(".skills-detail-empty").hidden = false;
  view.querySelector(".skills-bundle").hidden = true;
  view.classList.remove("has-detail");
  localizeSkillsView(view);
  const query = view.querySelector(".skills-search");
  query.placeholder = skillText("search"); query.setAttribute("aria-label", skillText("search"));
  query.addEventListener("input", () => { state.query = query.value.trim().toLowerCase(); filterSkillsView(view); });
  const root = view.querySelector(".skills-root-filter"); root.setAttribute("aria-label", skillText("root"));
  for (const key of ["all", "home", "repo", "bundled", "plugin"]) root.append(skillOption(skillText(`root_${key}`), key));
  root.addEventListener("change", () => { state.root = root.value; filterSkillsView(view); });
  const agent = view.querySelector(".skills-agent-filter"); agent.setAttribute("aria-label", skillText("provider"));
  agent.addEventListener("change", () => { state.agent = agent.value; filterSkillsView(view); });
  view.querySelector(".skills-onboarding-return").addEventListener("click", resumeSkillsOnboarding);
  view.querySelector(".skills-rescan").addEventListener("click", () => void refreshSkills(true));
  view.querySelector(".skills-bundle-open").addEventListener("click", () => openSkillBundle(view));
  view.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !view.querySelector(".skills-bundle").hidden) {
      event.stopPropagation(); view.querySelector(".skills-bundle").hidden = true;
      view.querySelector(".skills-bundle-open").focus();
    }
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key) || !event.target.closest(".skills-card")) return;
    const buttons = [...state.rows.values()].filter((r) => !r.node.hidden).map((r) => r.button);
    let at = buttons.indexOf(document.activeElement);
    at = event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : at + (event.key === "ArrowDown" ? 1 : -1);
    event.preventDefault(); event.stopPropagation(); buttons[Math.max(0, Math.min(buttons.length - 1, at))]?.focus();
  });
  return state;
}
async function refreshSkills(force = false) {
  if (skillsPending) return skillsPending;
  skillsPending = (async () => {
    try {
      const answer = await invoke(force ? "skills_rescan" : "skills_list", force ? { term: skillsInstallTerm } : {});
      if (!answer) return;
      skillsReport = answer; skillsNeedsRescan = false;
      for (const tab of tabs.filter((tab) => tab.kind === "skills")) paintSkillsView(docHost(tab.pane, "skills"));
      paintSkillsSummary(); paintRequiredSkillBadges();
    } catch (error) {
      for (const view of document.querySelectorAll(".skills-view")) {
        const note = view.querySelector(".skills-error"); note.hidden = false; note.textContent = skillsError(error);
      }
      const summary = el("skills-summary"); if (summary) summary.textContent = skillsError(error);
    } finally { skillsPending = null; }
  })();
  return skillsPending;
}
function paintSkillsSummary() {
  const node = el("skills-summary");
  if (node) node.textContent = skillsReport ? skillText("summary", { count: skillsReport.families.length, roots: skillsReport.sources.length }) : skillText("loading");
}
function paintRequiredSkillBadges() {
  const missing = (skillsReport?.required ?? []).filter((r) => r.missing_agents.length);
  for (const node of document.querySelectorAll("[data-required-skills]")) {
    node.hidden = missing.length === 0;
    node.textContent = skillText("requiredMissing", { count: missing.length });
    if (!skillsBadgeWired.has(node)) { skillsBadgeWired.add(node); node.addEventListener("click", () => {
      if (node.closest("[data-onboarding-overlay]")) { hideModal(el("onb-scrim")); skillsOnboardingSuspended = true; }
      openSkillsView();
    }); }
  }
}
function paintSkillsStage(tab) {
  const view = docHost(tab.pane, "skills");
  paintSkillsView(view);
  void refreshSkills(skillsNeedsRescan);
}
function skillAgents(variant) {
  return skillsReport?.installed_agents_by_skill?.[variant.id] ?? variant.providers;
}
function createSkillCard(family, view) {
  const node = skillElement("article", "skills-card"); node.setAttribute("role", "listitem"); node.dataset.skill = family.id;
  const button = skillElement("button", "skills-card-open"); button.type = "button";
  const name = skillElement("strong", "skills-card-name");
  const desc = skillElement("span", "skills-card-description");
  const marks = skillElement("span", "skills-card-marks");
  const usage = skillElement("span", "skills-card-usage");
  const conflict = skillElement("span", "skills-conflict");
  button.append(name, desc, marks, usage, conflict); node.append(button);
  button.addEventListener("click", () => void selectSkillFamily(view, node.dataset.skill));
  return { node, button, name, desc, marks, usage, conflict, family, search: "" };
}
function localizeSkillsView(view) {
  for (const [selector, key] of [["h1", "title"], [".skills-eyebrow", "library"], [".skills-about", "overview"], [".skills-rescan", "reload"], [".skills-bundle-open", "bundle"], [".skills-detail-empty", "selectDetail"], [".skills-roots-panel summary", "where"], [".skills-usage-scope", "hooksOnly"]]) view.querySelector(selector).textContent = skillText(key);
  const search = view.querySelector(".skills-search"); search.placeholder = skillText("search"); search.setAttribute("aria-label", skillText("search"));
  view.querySelector(".skills-root-filter").setAttribute("aria-label", skillText("root"));
  view.querySelector(".skills-agent-filter").setAttribute("aria-label", skillText("provider"));
  for (const option of view.querySelectorAll(".skills-root-filter option")) option.textContent = skillText(`root_${option.value}`);
}
function paintSkillsView(view) {
  const state = wireSkillsView(view);
  if (state.language !== locale) {
    state.language = locale; state.report = null; localizeSkillsView(view);
    if (state.selected) void selectSkillFamily(view, state.selected);
  }
  const resume = view.querySelector(".skills-onboarding-return"); resume.hidden = !skillsOnboardingSuspended; resume.textContent = skillText("resumeOnboarding");
  const report = skillsReport;
  if (!report) { view.querySelector(".skills-empty").textContent = skillText("loading"); return; }
  if (state.report !== report) {
    state.report = report;
    const list = view.querySelector(".skills-card-list");
    const ids = new Set(report.families.map((f) => f.id));
    for (const [id, row] of state.rows) if (!ids.has(id)) { row.node.remove(); state.rows.delete(id); }
    for (const family of report.families) {
      let row = state.rows.get(family.id);
      if (!row) { row = createSkillCard(family, view); state.rows.set(family.id, row); list.append(row.node); }
      row.family = family;
      const variants = family.variants ?? [];
      row.name.textContent = family.name;
      row.desc.textContent = variants.find((v) => v.description)?.description ?? "";
      row.desc.hidden = !row.desc.textContent;
      row.marks.textContent = [...new Set(variants.flatMap(skillAgents))].join(" · ");
      const usage = family.usage ?? {};
      row.usage.textContent = usage.count_7d ? skillText("usage", { count: usage.count_7d, last: agoWord(usage.last_used_ms, Date.now()), agents: (usage.agents ?? []).join(", ") }) : skillText("unused");
      row.conflict.hidden = !family.conflict;
      row.conflict.textContent = skillText("conflict");
      row.search = [family.name, ...variants.flatMap((v) => [v.description ?? "", v.file, v.source_label, ...v.providers])].join(" ").toLowerCase();
    }
    const pick = view.querySelector(".skills-agent-filter");
    const agents = [...new Set(report.families.flatMap((f) => f.variants.flatMap(skillAgents)))].sort();
    pick.replaceChildren(skillOption(skillText("allProviders"), "all"), ...agents.map((agent) => skillOption(agent, agent)));
    if (!agents.includes(state.agent)) state.agent = "all"; pick.value = state.agent;
    const roots = view.querySelector(".skills-roots-panel div");
    roots.replaceChildren(...report.sources.map((source) => skillElement("p", "skills-root-line", `${source.path} · ${source.found}`)));
    const required = view.querySelector(".skills-required"); required.replaceChildren();
    const missing = (report.required ?? []).filter((r) => r.missing_agents.length);
    if (missing.length) {
      required.append(skillElement("strong", "", skillText("requiredMissing", { count: missing.length })));
      required.append(skillElement("span", "skills-evidence-scope", skillText("nativeNotice")));
      for (const item of missing) {
        const button = skillButton("bundledItem", () => void installRequiredSkill(view, item.name), "btn", { name: item.name });
        button.dataset.tip = skillText("requiredItem", { name: item.name, agents: item.missing_agents.join(", ") });
        required.append(button);
      }
    }
    required.hidden = !missing.length;
    const latestPlan = report.plans?.at(-1);
    const planStatus = view.querySelector(".skills-plan-status"); planStatus.hidden = !latestPlan;
    planStatus.textContent = latestPlan ? (latestPlan.error || skillText(`plan_${latestPlan.status}`)) : "";
    if (latestPlan) planStatus.dataset.tip = latestPlan.command;
    const note = view.querySelector(".skills-error"); note.hidden = !report.capped; note.textContent = report.capped ? skillText("capped") : "";
    if (state.selected && !ids.has(state.selected)) {
      state.selected = null; state.generation++;
      view.querySelector(".skills-detail-content").replaceChildren();
      view.querySelector(".skills-detail-content").hidden = true;
      view.querySelector(".skills-detail-empty").hidden = false;
    }
  }
  filterSkillsView(view);
}
function filterSkillsView(view) {
  const state = skillsViews.get(view); if (!state) return;
  let shown = 0;
  for (const row of state.rows.values()) {
    const matches = (!state.query || row.search.includes(state.query)) && row.family.variants.some((v) =>
      (state.root === "all" || v.source_kind === state.root) && (state.agent === "all" || skillAgents(v).includes(state.agent)));
    row.node.hidden = !matches;
    row.button.setAttribute("aria-pressed", String(state.selected === row.family.id));
    if (matches) shown++;
  }
  view.querySelector(".skills-total").textContent = skillText("filtered", { shown, count: state.rows.size });
  const empty = view.querySelector(".skills-empty"); empty.hidden = shown > 0;
  empty.textContent = skillText(state.rows.size ? "noneMatch" : "none");
}
async function selectSkillFamily(view, id, variantId = null) {
  const state = skillsViews.get(view), row = state.rows.get(id); if (!row) return;
  view.classList.add("has-detail");
  state.selected = id; const generation = ++state.generation;
  filterSkillsView(view);
  const host = view.querySelector(".skills-detail-content");
  view.querySelector(".skills-detail-empty").hidden = true; host.hidden = false;
  host.replaceChildren(skillElement("p", "", skillText("loading")));
  try {
    const variant = row.family.variants.find((v) => v.id === variantId) ?? row.family.variants[0];
    const detail = await invoke("skill_detail", { id: variant.id });
    if (generation !== state.generation || !view.isConnected) return;
    host.replaceChildren();
    host.append(skillElement("p", "skills-eyebrow", skillText("detail")), skillElement("h2", "", row.family.name));
    const select = skillElement("select", "skills-variant"); select.setAttribute("aria-label", skillText("variant"));
    for (const v of row.family.variants) select.append(skillOption(`${v.source_label} · ${v.version ?? skillText("unversioned")}`, v.id, false, v.id === variant.id));
    select.addEventListener("change", () => void selectSkillFamily(view, id, select.value)); host.append(select);
    host.append(skillElement("p", "skills-detail-path", variant.file));
    if (variant.version) host.append(skillElement("p", "skills-meta", skillText("version", { version: variant.version })));
    if (row.family.outdated_ids?.includes(variant.id)) host.append(skillElement("p", "skills-conflict", skillText("outdated", { latest: row.family.latest_version })));
    if (variant.digest) host.append(skillElement("p", "skills-digest", `SHA-256 · ${variant.digest}`));
    const actions = skillElement("div", "skills-actions");
    if (row.family.bundled_name) {
      actions.append(skillButton("bundledSync", () => void installRequiredSkill(view, row.family.bundled_name)));
    } else {
      actions.append(skillButton("install", () => openSkillBundle(view, [row.family.name])),
        skillButton("update", () => void prepareSkillInstall(view, [row.family.name], [], "update")));
    }
    actions.append(skillButton("reveal", () => void invoke("skill_reveal", { path: variant.file }).catch(showError)));
    host.append(actions);
    const evidence = skillElement("div", "skills-detail-evidence");
    evidence.append(skillElement("h3", "", skillText("evidence")));
    if (!detail?.evidence?.length) evidence.append(skillElement("p", "", skillText("unused")));
    for (const event of detail?.evidence ?? []) {
      const line = skillElement("p", "", `${event.agent} · ${event.pane} · ${new Date(event.at_ms).toLocaleString()}`);
      line.dataset.tip = event.source; evidence.append(line);
    }
    host.append(evidence);
    const markdown = skillElement("div", "md-page skills-markdown"); host.append(markdown);
    const previous = mdWhere;
    try { mdWhere = { base: variant.directory, page: markdown }; paintMarkdown(markdown, detail?.markdown ?? ""); } finally { mdWhere = previous; }
    view.querySelector(".skills-detail").scrollIntoView({ block: "nearest" });
  } catch (error) { if (generation === state.generation) host.replaceChildren(skillElement("p", "skills-error", skillsError(error))); }
}
async function openSkillTerminal(command, policy = skillsReport?.policy) {
  if (!policy) { await refreshSkills(); policy = skillsReport?.policy; }
  if (!policy || /[\r\n]/.test(command)) throw new Error(skillText("invalidPlan"));
  setSettingsOpen(false);
  const term = await invoke("open_term_tab", { rows: policy.terminal_rows, cols: policy.terminal_cols, plain: true });
  mountTermTab(term);
  await invoke("term_text", { term, text: command });
  skillsInstallTerm = term; skillsNeedsRescan = true;
  watchSkillInstall(term, skillsReport?.policy);
  return term;
}
async function prepareSkillInstall(view, names, agents, action = "install", repository = null) {
  try {
    const plan = await invoke("skill_install_plan", { names, agents, action, repository });
    if (action !== "list") view.querySelector(".skills-bundle").hidden = true;
    await openSkillTerminal(plan.command, { terminal_rows: plan.rows, terminal_cols: plan.cols });
  } catch (error) {
    const note = view.querySelector(".skills-bundle:not([hidden]) .skills-bundle-note") ?? view.querySelector(".skills-error"); note.hidden = false; note.textContent = skillsError(error);
  }
}
function openSkillBundle(view, selectedNames = null) {
  const box = view.querySelector(".skills-bundle"); box.hidden = false; box.replaceChildren();
  box.append(skillElement("h2", "", skillText("bundle")), skillElement("p", "", skillText("manualRun")));
  const repo = skillElement("input", "skills-repository"); repo.type = "url"; repo.placeholder = skillText("repository"); repo.setAttribute("aria-label", skillText("repository"));
  const output = skillElement("textarea", "skills-bundle-output"); output.placeholder = skillText("pasteOutput"); output.setAttribute("aria-label", skillText("pasteOutput"));
  if (skillsReport?.policy?.max_evidence_bytes) output.maxLength = skillsReport.policy.max_evidence_bytes;
  const choices = skillElement("fieldset", "skills-bundle-choices"); choices.append(skillElement("legend", "", skillText("chooseSkills")));
  const agents = skillElement("fieldset", "skills-bundle-agents"); agents.append(skillElement("legend", "", skillText("chooseAgents")));
  const addChoice = (parent, name, label, checked) => {
    const node = skillElement("label", "skills-choice"); const input = document.createElement("input"); input.type = "checkbox"; input.value = name; input.checked = checked;
    node.append(input, document.createTextNode(label)); parent.append(node);
  };
  const fill = (names) => { for (const node of choices.querySelectorAll("label")) node.remove(); for (const name of names) addChoice(choices, name, name, true); };
  const externalAgents = skillsReport?.external_agents ?? skillsReport?.agents ?? [];
  for (const [id, label] of externalAgents) addChoice(agents, id, label, true);
  const unavailable = skillsReport?.external_unavailable ?? [];
  if (unavailable.length) agents.append(skillElement("p", "skills-evidence-scope", skillText("externalUnavailable", { agents: unavailable.join(", ") })));
  if (selectedNames) fill(selectedNames);
  const note = skillElement("p", "skills-bundle-note"); note.setAttribute("role", "status");
  const parse = async () => {
    try {
      const result = await invoke("skill_bundle_parse", { output: output.value });
      fill(result.names ?? []); note.textContent = result.error || (result.names?.length ? "" : skillText("noBundleSkills"));
    } catch (error) { note.textContent = skillsError(error); }
  };
  const list = skillButton("listBundle", () => void prepareSkillInstall(view, [], [], "list", repo.value));
  const read = skillButton("readTerminal", async () => {
    if (skillsInstallTerm === null) { note.textContent = skillText("noTerminal"); return; }
    try {
      const frame = await invoke("term_snapshot", { term: skillsInstallTerm });
      output.value = (frame?.rows ?? []).map((row) => row.text ?? "").join("\n"); await parse();
    } catch (error) { note.textContent = skillsError(error); }
  });
  const run = skillButton("prepare", () => {
    const names = [...choices.querySelectorAll("input:checked")].map((n) => n.value);
    const targets = [...agents.querySelectorAll("input:checked")].map((n) => n.value);
    if (!names.length || !targets.length) { note.textContent = skillText("selectTargets"); return; }
    void prepareSkillInstall(view, names, targets, "install", repo.value || null);
  });
  run.disabled = externalAgents.length === 0;
  box.append(repo, list, output, read, skillButton("parseOutput", () => void parse()), choices, agents, note, run, skillButton("close", () => { box.hidden = true; view.querySelector(".skills-bundle-open").focus(); }));
  repo.focus();
}
window.addEventListener("focus", () => {
  if (skillsNeedsRescan && tabs.some((tab) => tab.kind === "skills" && stillShowing(tab))) void refreshSkills(true);
});

function skillInstallSummary(outcomes) {
  const labels = (state) => outcomes.filter((row) => row.state === state).map((row) => row.label);
  const written = [...labels("written"), ...labels("unchanged")];
  const kept = labels("kept");
  const failed = outcomes.filter((row) => row.state === "failed");
  const parts = [];
  if (written.length > 0) parts.push(t("orch.installedIn", "{{agents}}에 설치됨", { agents: written.join(" · ") }));
  if (kept.length > 0) parts.push(t("orch.installKept", "{{agents}}: 다른 파일이 있어 그대로 둠", { agents: kept.join(" · ") }));
  for (const row of failed) parts.push(`${row.label}: ${row.detail}`);
  if (parts.length === 0) {
    return t("orch.installNoAgents", "설치할 에이전트가 없습니다. 에이전트를 설치한 뒤 다시 확인하세요.");
  }
  return parts.join(" / ");
}

async function installBundledSkill(name, note, recheck) {
  note.hidden = true;
  try {
    const outcomes = await invoke("install_bundled_skill", { name });
    note.textContent = skillInstallSummary(outcomes ?? []);
  } catch (error) {
    note.textContent = String(error);
  }
  note.hidden = false;
  await recheck();
}


function releaseSkillsView(tab) {
  resumeSkillsOnboarding();
  const view = groups.get(tab.pane)?.skillsView;
  const state = view && skillsViews.get(view);
  if (!state) return;
  state.generation++; state.rows.clear(); state.selected = null; state.report = null;
  view.querySelector(".skills-card-list").replaceChildren();
  view.querySelector(".skills-detail-content").replaceChildren();
  view.querySelector(".skills-bundle").replaceChildren();
  view.querySelector(".skills-bundle").hidden = true;
  view.querySelector(".skills-detail-content").hidden = true;
  view.querySelector(".skills-detail-empty").hidden = false;
  view.classList.remove("has-detail");
}

function resumeSkillsOnboarding() {
  if (!skillsOnboardingSuspended) return;
  skillsOnboardingSuspended = false;
  paintOnboarding(); showModal(el("onb-scrim"));
  for (const view of document.querySelectorAll(".skills-view")) view.querySelector(".skills-onboarding-return").hidden = true;
}

// One bounded observer follows the most recently prepared install. An idle
// shell before Enter is not completion; observe a child first, then its exit.
function watchSkillInstall(term, policy) {
  if (skillsInstallWatch) clearTimeout(skillsInstallWatch.timer);
  skillsInstallWatch = null;
  if (!policy?.install_poll_ms || !policy?.install_watch_ms) return;
  const watch = { term, started: false, until: Date.now() + policy.install_watch_ms, timer: null };
  skillsInstallWatch = watch;
  async function pollSkillInstall() {
    if (skillsInstallWatch !== watch) return;
    if (Date.now() >= watch.until) { skillsInstallWatch = null; return; }
    try {
      const busy = await invoke("term_has_running_process", { term });
      if (skillsInstallWatch !== watch) return;
      if (busy === null) { skillsInstallWatch = null; return; }
      if (busy) watch.started = true;
      else if (watch.started) {
        skillsInstallWatch = null; skillsNeedsRescan = true;
        await refreshSkills(true); return;
      }
    } catch { skillsInstallWatch = null; return; }
    watch.timer = setTimeout(pollSkillInstall, policy.install_poll_ms);
  }
  watch.timer = setTimeout(pollSkillInstall, policy.install_poll_ms);
}

async function installRequiredSkill(view, name) {
  const buttons = [...view.querySelectorAll(".skills-required button")];
  for (const button of buttons) button.disabled = true;
  try {
    await installBundledSkill(name, view.querySelector(".skills-native-status"), () => refreshSkills(true));
  } finally { for (const button of buttons) button.disabled = false; }
}
/* ---- 아티팩트 / what agents made, with where it came from (t-2720) --------
 *
 * Orca 갭 #1. Orca의 `artifacts` 뷰는 세션 하나를 출처로 삼는 파일 그리드다
 * (Collection·DetailDrawer·Actions). 이 판은 같은 세 조각에 셋을 더 얹는다 —
 * 출처가 1급이라 카드·작업·워크트리로 바로 가고(왕복), 워커 보고서가 `/tmp`를
 * 떠나 앱 저장소에 보존 기간과 함께 남으며, 검색은 본문까지 간다(백엔드의 회상
 * 색인). 카탈로그·스캔·보존은 Rust(`artifact_runtime`)의 것이고, 이 파일이 하는
 * 일은 그 답을 **한 번** 받아 걸러 그리는 것뿐이다 — 종류·출처·검색 한 글자는
 * 창 안에서 고르고 백엔드를 다시 부르지 않는다.
 *
 * 그리기의 규칙은 지식 그래프·보드와 같다: 키 DOM. 카드는 풀에서 꺼내 제자리에서
 * 갈아입고(`dressArtifactCard`), 필터 전환은 노드를 만들지 않으며(실측 핀:
 * 생성 0), 보이는 카드만 미리보기를 청한다(가상화). 숫자는 전부 토큰이다. */

/* 하나뿐인 탭 — 보드와 같은 이유로 id가 고정이다: 카탈로그 하나의 그림 하나. */
const ARTIFACTS_TAB = Object.freeze({ id: "artifacts", kind: "artifacts" });
/* 시각 수치의 주소. 값은 tokens.css 한 곳에 있고 판을 지을 때 한 번 읽는다. */
const ARTIFACT_TOKENS = Object.freeze({
  cardMin: "--artifact-card-min",
  cardHeight: "--artifact-card-height",
  faceRatio: "--artifact-face-ratio",
  gap: "--artifact-card-gap",
  overscan: "--artifact-overscan",
  drawerMin: "--artifact-drawer-min",
  tierWide: "--artifact-tier-wide",
  tierCompact: "--artifact-tier-compact",
  thumbCache: "--artifact-thumb-cache",
  searchDebounce: "--artifact-search-debounce",
});
/* 종류의 순서는 백엔드 `ArtifactKind::ALL`의 것이다 — 세그먼트가 그 순서로 선다. */
const ARTIFACT_KINDS = Object.freeze([
  "report", "screenshot", "evidence", "export", "transcript", "page", "document", "web", "other",
]);
const ARTIFACT_KIND_GLYPH = Object.freeze({
  report: "ft-file-text",
  screenshot: "image",
  evidence: "camera",
  export: "download",
  transcript: "list",
  page: "ft-file-code",
  document: "book",
  web: "globe",
  other: "file",
});
/* 낱말은 행이 들고 다니고 `t`가 갈아입힌다 — 지식 그래프의 깃발과 같은 문법. */
const ARTIFACT_KIND_WORDS = Object.freeze({
  all: { key: "artifacts.kind.all", word: "전체" },
  report: { key: "artifacts.kind.report", word: "보고서" },
  screenshot: { key: "artifacts.kind.screenshot", word: "스크린샷" },
  evidence: { key: "artifacts.kind.evidence", word: "증거" },
  export: { key: "artifacts.kind.export", word: "내보내기" },
  transcript: { key: "artifacts.kind.transcript", word: "대화록" },
  page: { key: "artifacts.kind.page", word: "페이지" },
  document: { key: "artifacts.kind.document", word: "문서" },
  web: { key: "artifacts.kind.web", word: "claude.ai" },
  other: { key: "artifacts.kind.other", word: "기타" },
});
/* claude.ai의 세그먼트 셋(전체 / 내 것 / 공유됨)에 해당하는 우리 셋 — 전체 /
 * 이 기계 / claude.ai (t-3233 §4). `remote`는 백엔드 `Filter.remote`의 낱말이고,
 * 창은 이미 든 행을 종류로 가른다(web이 곧 원격). */
const ARTIFACT_SOURCES = Object.freeze([
  { source: "all", key: "artifacts.source.all", word: "전체" },
  { source: "local", key: "artifacts.source.local", word: "이 기계" },
  { source: "remote", key: "artifacts.source.remote", word: "claude.ai" },
]);
/* 카드 아래 줄의 표식 — claude.ai의 🌐(공유됨)/🔒(비공개)를 우리 뜻으로: 🌐는
 * claude.ai에 올라간 것, 🔒는 이 기계에만 있는 것. 낱말이 아니라 글리프라
 * 카탈로그를 타지 않는다. */
const ARTIFACT_WHEN_GLYPH = Object.freeze({ remote: "🌐", local: "🔒" });
/* 렌더 썸네일을 청하는 종류 — 백엔드 `ArtifactKind::renders_thumbnail`과 같은 둘. */
const ARTIFACT_THUMB_KINDS = new Set(["page", "web"]);
/* 출처의 칸과 그 칸이 서랍에서 부르는 낱말. 순서는 서랍의 표 순서다. 낱말은
 * `{ key, word }`로 실려 `t`가 갈아입힌다 — 테마 픽커와 같은 문법. */
const ARTIFACT_ORIGIN_FIELDS = Object.freeze([
  { field: "agent", key: "artifacts.meta.agent", word: "에이전트" },
  { field: "model", key: "artifacts.meta.model", word: "모델" },
  { field: "run", key: "artifacts.meta.run", word: "런" },
  { field: "task", key: "artifacts.meta.task", word: "작업" },
  { field: "worker", key: "artifacts.meta.worker", word: "워커" },
  { field: "pane", key: "artifacts.meta.pane", word: "판" },
  { field: "worktree", key: "artifacts.meta.worktree", word: "워크트리" },
  { field: "automation", key: "artifacts.meta.automation", word: "자동화" },
  { field: "commit", key: "artifacts.meta.commit", word: "커밋" },
]);
/* 서랍의 표 머리 다섯 — 종류·크기·두 시각·경로 — 와 보존. */
const ARTIFACT_META_ROWS = Object.freeze([
  { field: "kind", key: "artifacts.meta.kind", word: "종류" },
  { field: "bytes", key: "artifacts.meta.bytes", word: "크기" },
  { field: "created", key: "artifacts.meta.created", word: "만든 때" },
  { field: "modified", key: "artifacts.meta.modified", word: "고친 때" },
  { field: "path", key: "artifacts.meta.path", word: "경로" },
]);
const ARTIFACT_META_RETENTION = Object.freeze({ field: "retention", key: "artifacts.meta.retention", word: "보존" });
/* 출처 링크 셋과 액션 넷 — 자리(`jump`/`action`)가 곧 데이터 속성이다. */
const ARTIFACT_JUMPS = Object.freeze([
  { jump: "card", key: "artifacts.jump.card", word: "카드", glyph: "kanban" },
  { jump: "task", key: "artifacts.jump.task", word: "작업", glyph: "tasks" },
  { jump: "worktree", key: "artifacts.jump.worktree", word: "워크트리", glyph: "branch" },
]);
const ARTIFACT_ACTIONS = Object.freeze([
  { action: "open", key: "artifacts.open", word: "열기", glyph: "external" },
  { action: "reveal", key: "artifacts.reveal", word: "Finder에서 보기", glyph: "folder-open" },
  { action: "copy", key: "artifacts.copyPath", word: "경로 복사", glyph: "copy" },
  { action: "delete", key: "artifacts.delete", word: "삭제", glyph: "trash" },
]);
// Dynamic data-i18n nodes start with the source words; applyLocale reads
// their keys after construction. Shared actions keep their existing row.
const ARTIFACT_LABELS = Object.freeze({
  sources: { key: "artifacts.sources", word: "출처" },
  fresh: { key: "artifacts.new", word: "새 아티팩트" },
  versions: { key: "artifacts.strip.versions", word: "버전" },
  share: { key: "artifacts.strip.share", word: "공유" },
  reveal: ARTIFACT_ACTIONS.find((row) => row.action === "reveal"),
});

/* 카탈로그 — 백엔드가 마지막으로 답한 행들, id로. 이 맵 하나가 창이 아는 전부다. */
const artifactRows = new Map();
/* 필터·정렬을 지난 뒤의 id, 새것부터. 그리기는 이 배열만 읽는다. */
let artifactOrder = [];
const artifactFilter = { query: "", kind: "all", origin: null, source: "all" };
/* 스튜디오를 접었는가 — 창 안에서 유지되는 사람의 선택(워크스페이스 보드의 보기
 * 선택과 같은 수명). 접힌 스튜디오는 한 줄이고, 갤러리가 그 높이를 받는다. */
let artifactStudioFolded = false;
/* 본문 검색이 골라 준 id — Enter로 한 번 묻고, 다음 글자에 지운다. */
let artifactBodyMatches = null;
let artifactSelectedId = null;
let artifactListing = { total: 0, truncated: false, retentionDays: 0, thumb: null };
let artifactError = null;
let artifactAskedAt = 0;
let artifactAsking = false;
/* 하네스가 세는 수 — 카드 노드가 몇 번 만들어졌는가. 풀이 제 일을 하면 첫 그림
 * 뒤로는 자라지 않는다. */
let artifactCardCreations = 0;
/* 판마다의 풀과 눈금. 판은 `docHost`가 리프마다 복제하므로 상태는 판에 붙는다. */
const artifactCardPools = new WeakMap();
const artifactTunings = new WeakMap();
/* 미리보기 — id → 답. 서랍과 썸네일이 나눠 쓰고, 개수 상한은 토큰이 정한다. */
const artifactPreviews = new Map();
const artifactPreviewAsks = new Map();
/* 렌더 썸네일(t-3233 §3) — id → data URL. 큐는 하나, 동시 1, 보이는 카드만,
 * 실패는 글리프로 물러나고 다시 청하지 않는다. 큐의 상한은 백엔드 표의
 * `thumb_queue_max`가 목록에 실어 보낸다 — 창이 제 숫자를 갖지 않는다. */
const artifactThumbs = new Map();
const artifactThumbFailed = new Set();
const artifactThumbQueue = [];
let artifactThumbBusy = false;
let artifactThumbActiveId = null;

function artifactThumbnailKey(row) {
  return row ? JSON.stringify([row.kind, row.path, row.url, row.modified_ms, row.bytes, row.version]) : null;
}
/* 출처별 수 — 카드·인스펙터·워크트리 행의 칩이 읽는다. `null`은 아직 모른다. */
let artifactCounts = null;
let artifactCountsAsking = false;
let artifactSearchTimer = 0;

function artifactTuning(view) {
  const held = artifactTunings.get(view);
  if (held) return held;
  const style = getComputedStyle(view);
  const read = (name, fallback) => {
    const value = parseFloat(style.getPropertyValue(name));
    return Number.isFinite(value) ? value : fallback;
  };
  const tuning = {
    cardMin: read(ARTIFACT_TOKENS.cardMin, 220),
    cardHeight: read(ARTIFACT_TOKENS.cardHeight, 150),
    faceRatio: read(ARTIFACT_TOKENS.faceRatio, 1),
    gap: read(ARTIFACT_TOKENS.gap, 12),
    overscan: read(ARTIFACT_TOKENS.overscan, 1),
    drawerMin: read(ARTIFACT_TOKENS.drawerMin, 320),
    tierWide: read(ARTIFACT_TOKENS.tierWide, 900),
    tierCompact: read(ARTIFACT_TOKENS.tierCompact, 560),
    thumbCache: read(ARTIFACT_TOKENS.thumbCache, 120),
    searchDebounce: read(ARTIFACT_TOKENS.searchDebounce, 120),
  };
  artifactTunings.set(view, tuning);
  return tuning;
}

/* 보이는 아티팩트 판 — 하네스와 점프가 같은 문으로 찾는다. */
function artifactsView() {
  return [...document.querySelectorAll(".artifacts-view")].find((one) => !one.hidden) ?? null;
}

function artifactsTab() {
  return tabs.find((held) => held.kind === ARTIFACTS_TAB.kind) ?? null;
}

/* 판의 에이전트가 바뀌었다(hook:agent·명부 재적재·워커 도착·판 닫힘): 스튜디오의
 * 「초안을 받을 에이전트」는 그 판들의 목록이라, 서 있는 아티팩트 판만 다시 그린다.
 * 문은 `flushAgentPaint`의 cards 표면 하나다 — 그 표면이 곧 「어느 에이전트가
 * 어느 판에 앉았는가」의 그림이고, 여기에 둘째 리스너를 매지 않는다. */
function noteArtifactSeats() {
  const tab = artifactsTab();
  if (!tab || !stillShowing(tab)) return;
  const view = docHost(tab.pane, ARTIFACTS_TAB.kind);
  if (view?._artifactsWired) paintArtifactStudio(view);
}

/* 문을 여는 두 손. 사이드바(`fresh`)는 「보겠다」는 청이라 다시 읽고, 칩과
 * 링크는 이미 손에 든 행을 다른 각도로 보는 것이라 묻지 않는다. */
function openArtifacts({ origin, select = null, fresh = false } = {}) {
  if (origin !== undefined) artifactFilter.origin = origin;
  if (select) artifactSelectedId = select;
  if (fresh) artifactAskedAt = 0;
  applyArtifactFilter();
  openTab({ ...ARTIFACTS_TAB });
}

/* 무대가 이 탭을 드러낼 때 지나는 문. 카탈로그는 처음 열 때와 사이드바가 다시
 * 청할 때, 그리고 `artifacts:changed`가 울릴 때만 다시 묻는다. */
function paintArtifactsStage(tab) {
  if (artifactAskedAt === 0) void refreshArtifacts();
  return paintArtifactsView(tab);
}

async function refreshArtifacts() {
  if (artifactAsking) return;
  artifactAsking = true;
  try {
    const answer = await invoke("artifacts_list", { filter: {} });
    const incoming = new Map((answer?.rows ?? []).map((row) => [row.id, row]));
    for (const [id, previous] of artifactRows) {
      if (artifactThumbnailKey(previous) === artifactThumbnailKey(incoming.get(id))) continue;
      artifactThumbs.delete(id);
      artifactThumbFailed.delete(id);
      artifactPreviews.delete(id);
    }
    artifactRows.clear();
    for (const [id, row] of incoming) artifactRows.set(id, row);
    artifactListing = {
      total: answer?.total ?? artifactRows.size,
      truncated: answer?.truncated === true,
      retentionDays: answer?.retention_days ?? 0,
      thumb: answer?.thumb ?? null,
    };
    artifactError = null;
  } catch (error) {
    artifactError = String(error);
  } finally {
    artifactAsking = false;
    artifactAskedAt = Date.now();
  }
  artifactBodyMatches = null;
  if (artifactSelectedId && !artifactRows.has(artifactSelectedId)) artifactSelectedId = null;
  applyArtifactFilter();
  void paintArtifactsView();
}

/* 출처별 수 — 원장이 움직일 때와 카탈로그가 움직일 때 다시 읽는다. 폴러가 아니다. */
async function refreshArtifactCounts() {
  if (artifactCountsAsking) return;
  artifactCountsAsking = true;
  let next = null;
  try {
    next = await invoke("artifact_counts");
  } catch {
    next = null;
  } finally {
    artifactCountsAsking = false;
  }
  // Only a change repaints: the ledger beats often, the counts move rarely,
  // and a repaint of every card and row for an unchanged number is the cost
  // this window keeps out of its beats.
  if (JSON.stringify(next) === JSON.stringify(artifactCounts)) return;
  artifactCounts = next;
  scheduleAgentPaint(["cards", "board"]);
  paintArtifactChipsInSidebar();
}

/* 한 출처의 수 — 칩이 묻는 유일한 물음. */
function artifactCountFor(field, value) {
  if (!artifactCounts || !value) return 0;
  const table = {
    worker: artifactCounts.by_worker,
    task: artifactCounts.by_task,
    run: artifactCounts.by_run,
    worktree: artifactCounts.by_worktree,
    automation: artifactCounts.by_automation,
  }[field];
  return table?.[value] ?? 0;
}

/* 「아티팩트 N」 칩 — 카드·인스펙터·워크트리 행이 같은 조립기로 짓는다. 누르면
 * 그 출처로 걸러진 아티팩트 뷰가 열린다; 묻지 않는다. */
function artifactChipNode(className, field, value, label) {
  const count = artifactCountFor(field, value);
  if (count === 0) return null;
  const chip = document.createElement("button");
  chip.type = "button";
  chip.className = `artifact-chip ${className}`;
  chip.innerHTML = icon("archive");
  const words = document.createElement("span");
  words.textContent = t("artifacts.chip", "아티팩트 {{n}}", { n: count });
  chip.appendChild(words);
  chip.dataset.tip = t("artifacts.chipTip", "{{name}}의 아티팩트 {{n}}개 보기", { name: label, n: count });
  chip.setAttribute("aria-label", chip.dataset.tip);
  chip.addEventListener("click", (event) => {
    event.stopPropagation();
    event.preventDefault();
    leavePagesForStage();
    openArtifacts({ origin: { field, value, label } });
  });
  return chip;
}

/* 사이드바의 워크트리 행은 제 그리기 주기가 따로 있다(`makeWorktreeNode`); 수가
 * 도착하면 서 있는 행의 칩만 제자리에서 고친다. */
function paintArtifactChipsInSidebar() {
  for (const row of document.querySelectorAll(".wt-row[data-worktree-path]")) {
    const path = row.dataset.worktreePath;
    const held = row.querySelector(".wt-artifacts");
    const chip = artifactChipNode("wt-artifacts", "worktree", path, basename(path));
    if (held && chip) held.replaceWith(chip);
    else if (held) held.remove();
    else if (chip) row.insertBefore(chip, row.querySelector(".wt-ports"));
  }
}

/* ---- 필터 — 창 안에서, 백엔드 없이 ------------------------------------- */

function artifactMatchesQuery(row, needle) {
  if (needle === "") return true;
  if (artifactBodyMatches?.has(row.id)) return true;
  const hay = [
    row.title,
    row.url ?? "",
    row.preview?.text ?? "",
    row.origin?.agent ?? "",
    row.origin?.model ?? "",
    row.origin?.task ?? "",
    row.origin?.worker ?? "",
    row.origin?.worktree ?? "",
  ].join("\n").toLowerCase();
  return needle.split(/\s+/).filter(Boolean).every((word) => hay.includes(word));
}

function applyArtifactFilter() {
  const needle = artifactFilter.query.trim().toLowerCase();
  const kind = artifactFilter.kind;
  const origin = artifactFilter.origin;
  const source = artifactFilter.source;
  const kept = [];
  for (const row of artifactRows.values()) {
    if (kind !== "all" && row.kind !== kind) continue;
    if (source === "remote" && row.kind !== "web") continue;
    if (source === "local" && row.kind === "web") continue;
    if (origin && String(row.origin?.[origin.field] ?? "") !== origin.value) continue;
    if (!artifactMatchesQuery(row, needle)) continue;
    kept.push(row);
  }
  kept.sort((a, b) => (b.modified_ms - a.modified_ms) || (a.id < b.id ? -1 : 1));
  artifactOrder = kept.map((row) => row.id);
  if (artifactSelectedId && !artifactOrder.includes(artifactSelectedId)) artifactSelectedId = null;
}

/* 본문 검색 — Enter 한 번이 백엔드의 회상 색인을 묻는 유일한 순간이다. 답은 id의
 * 집합이고, 창의 필터가 그 집합을 제 매치에 더한다. */
async function searchArtifactBodies(view) {
  const query = artifactFilter.query.trim();
  if (query === "") return;
  try {
    const answer = await invoke("artifact_search", { query });
    artifactBodyMatches = new Set((answer?.rows ?? []).map((row) => row.id));
    for (const row of answer?.rows ?? []) if (!artifactRows.has(row.id)) artifactRows.set(row.id, row);
  } catch (error) {
    showError(String(error));
    return;
  }
  applyArtifactFilter();
  paintArtifactCards(view);
  paintArtifactHead(view);
}

/* ---- 판 짓기 ---------------------------------------------------------- */

const ARTIFACT_STUDIO_SURFACES = Object.freeze([
  { id: "compare", kind: "report", title: { key: "artifacts.studio.compare", word: "비교·결정" }, hint: { key: "artifacts.studio.compareHint", word: "차이를 선명하게" } },
  { id: "monitor", kind: "evidence", title: { key: "artifacts.studio.monitor", word: "대시보드" }, hint: { key: "artifacts.studio.monitorHint", word: "지금 상태를 한눈에" } },
  { id: "story", kind: "document", title: { key: "artifacts.studio.story", word: "설명·소개" }, hint: { key: "artifacts.studio.storyHint", word: "핵심이 기억되도록" } },
  { id: "operate", kind: "page", title: { key: "artifacts.studio.operate", word: "작은 앱" }, hint: { key: "artifacts.studio.operateHint", word: "직접 써 보는 아이디어" } },
]);
const ARTIFACT_STUDIO_EXPRESSIONS = Object.freeze([
  { id: "expressive", key: "artifacts.studio.expressive", word: "대담하고 풍부하게" },
  { id: "balanced", key: "artifacts.studio.balanced", word: "명확하고 균형 있게" },
  { id: "quiet", key: "artifacts.studio.quiet", word: "간결하고 차분하게" },
]);

function artifactStudioText(tag, className, key, word) {
  const node = document.createElement(tag);
  node.className = className;
  node.dataset.i18n = key;
  node.textContent = word;
  applyLocale(node);
  return node;
}

function buildArtifactStudio() {
  const studio = document.createElement("section");
  studio.className = "artifacts-studio";
  const studioLabel = { key: "artifacts.studio.label", word: "아티팩트 디자인 스튜디오" };
  studio.dataset.i18nAria = studioLabel.key;
  studio.dataset.i18nSourcearialabel = studioLabel.word;
  studio.setAttribute("aria-label", t(studioLabel.key, studioLabel.word));
  const intro = document.createElement("div");
  intro.className = "artifacts-studio-intro";
  const copy = document.createElement("div");
  copy.className = "artifacts-studio-copy";
  copy.append(
    artifactStudioText("span", "artifacts-studio-eyebrow", "artifacts.studio.eyebrow", "디자인 스튜디오"),
    artifactStudioText("h2", "artifacts-studio-title", "artifacts.studio.title", "아이디어를, 완성된 화면으로."),
    artifactStudioText("p", "artifacts-studio-hint", "artifacts.studio.hint", "목적을 고르고, 원하는 표현으로 디자인 초안을 시작하세요."),
  );
  const orbit = document.createElement("div");
  orbit.className = "artifacts-studio-orbit";
  orbit.setAttribute("aria-hidden", "true");
  for (const surface of ARTIFACT_STUDIO_SURFACES) {
    const tile = document.createElement("span");
    tile.className = "artifacts-studio-orbit-tile";
    tile.dataset.surface = surface.id;
    tile.style.setProperty("--artifact-studio-accent", "var(--artifact-kind-" + surface.kind + ")");
    tile.innerHTML = icon(ARTIFACT_KIND_GLYPH[surface.kind]);
    orbit.appendChild(tile);
  }
  // 접힌 줄의 오른쪽 끝 — 접힘을 되돌리는 유일한 손. 접히지 않은 스튜디오에서는 서지 않는다(CSS).
  const expand = artifactStudioText("button", "btn artifacts-studio-expand", "artifacts.studio.expand", "펼치기");
  expand.type = "button";
  intro.append(copy, orbit, expand);
  const choices = document.createElement("div");
  choices.className = "artifacts-studio-choices";
  choices.setAttribute("role", "group");
  const purposeLabel = { key: "artifacts.studio.purpose", word: "사용 목적" };
  choices.dataset.i18nAria = purposeLabel.key;
  choices.dataset.i18nSourcearialabel = purposeLabel.word;
  choices.setAttribute("aria-label", t(purposeLabel.key, purposeLabel.word));
  for (const surface of ARTIFACT_STUDIO_SURFACES) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "artifacts-studio-choice";
    button.dataset.surface = surface.id;
    button.style.setProperty("--artifact-studio-accent", "var(--artifact-kind-" + surface.kind + ")");
    button.setAttribute("aria-pressed", "false");
    const glyph = document.createElement("span");
    glyph.className = "artifacts-studio-glyph";
    glyph.innerHTML = icon(ARTIFACT_KIND_GLYPH[surface.kind]);
    const words = document.createElement("span");
    words.className = "artifacts-studio-choice-copy";
    words.append(
      artifactStudioText("strong", "artifacts-studio-choice-title", surface.title.key, surface.title.word),
      artifactStudioText("span", "artifacts-studio-choice-hint", surface.hint.key, surface.hint.word),
    );
    button.append(glyph, words);
    choices.appendChild(button);
  }

  const form = document.createElement("form");
  form.className = "artifacts-studio-form";
  form.hidden = true;
  const briefLabel = document.createElement("label");
  briefLabel.className = "artifacts-studio-brief-label";
  // Put translation on the text span so applyLocale never replaces its input.
  const briefText = artifactStudioText("span", "artifacts-studio-field-name", "artifacts.studio.brief", "무엇을 만들까요?");
  briefLabel.replaceChildren(briefText);
  const brief = document.createElement("textarea");
  brief.className = "settings-input artifacts-studio-brief";
  brief.required = true;
  brief.rows = 2;
  const briefPlaceholder = { key: "artifacts.studio.briefPlaceholder", word: "내용·독자·필요한 기능을 적어 주세요." };
  brief.dataset.i18nPlaceholder = briefPlaceholder.key;
  brief.dataset.i18nSourceplaceholder = briefPlaceholder.word;
  brief.placeholder = t(briefPlaceholder.key, briefPlaceholder.word);
  briefLabel.appendChild(brief);

  const options = document.createElement("div");
  options.className = "artifacts-studio-options";
  const expressionLabel = document.createElement("label");
  expressionLabel.className = "artifacts-studio-expression-label";
  expressionLabel.appendChild(artifactStudioText("span", "artifacts-studio-field-name", "artifacts.studio.expression", "표현"));
  const expression = document.createElement("select");
  expression.className = "settings-input artifacts-studio-expression";
  for (const row of ARTIFACT_STUDIO_EXPRESSIONS) {
    const option = artifactStudioText("option", "", row.key, row.word);
    option.value = row.id;
    expression.appendChild(option);
  }
  expressionLabel.appendChild(expression);
  const referenceLabel = document.createElement("label");
  referenceLabel.className = "artifacts-studio-reference-label";
  referenceLabel.appendChild(artifactStudioText("span", "artifacts-studio-field-name", "artifacts.studio.reference", "참고 자료·브랜드"));
  const reference = document.createElement("input");
  reference.className = "settings-input artifacts-studio-reference";
  const referencePlaceholder = { key: "artifacts.studio.referencePlaceholder", word: "파일·URL·브랜드 이름 (선택)" };
  reference.dataset.i18nPlaceholder = referencePlaceholder.key;
  reference.dataset.i18nSourceplaceholder = referencePlaceholder.word;
  reference.placeholder = t(referencePlaceholder.key, referencePlaceholder.word);
  referenceLabel.appendChild(reference);
  options.append(expressionLabel, referenceLabel);
  const destinationLabel = document.createElement("label");
  destinationLabel.className = "artifacts-studio-destination-label";
  destinationLabel.appendChild(artifactStudioText("span", "artifacts-studio-field-name", "artifacts.studio.destination", "초안을 받을 에이전트"));
  const destination = document.createElement("select");
  destination.className = "settings-input artifacts-studio-destination";
  destinationLabel.appendChild(destination);
  const actions = document.createElement("div");
  actions.className = "artifacts-studio-actions";
  const status = document.createElement("span");
  status.className = "artifacts-studio-status";
  status.setAttribute("role", "status");
  const close = artifactStudioText("button", "btn artifacts-studio-close", "artifacts.studio.close", "접기");
  close.type = "button";
  const send = artifactStudioText("button", "btn btn--primary artifacts-studio-send", "artifacts.studio.send", "초안 보내기");
  send.type = "submit";
  send.disabled = true;
  actions.append(status, close, send);
  form.append(briefLabel, options, destinationLabel, actions);
  // 닫기는 판의 오른쪽 위 — 「접기」(폼만 닫음)와 다른 손이다: 스튜디오 전체가
  // 한 줄로 접히고(`foldArtifactStudio`), 그 줄의 「펼치기」가 되돌린다.
  const dismiss = document.createElement("button");
  dismiss.type = "button";
  dismiss.className = "foot-icon artifacts-studio-dismiss";
  const dismissLabel = { key: "artifacts.studio.dismiss", word: "스튜디오 닫기" };
  dismiss.dataset.i18nAria = dismissLabel.key;
  dismiss.dataset.i18nSourcearialabel = dismissLabel.word;
  dismiss.setAttribute("aria-label", t(dismissLabel.key, dismissLabel.word));
  dismiss.innerHTML = icon("x");
  studio.append(intro, choices, form, dismiss);
  applyLocale(studio);
  return studio;
}

function openArtifactStudio(view, surfaceId) {
  const surface = ARTIFACT_STUDIO_SURFACES.find((row) => row.id === surfaceId);
  if (!surface) return;
  const form = view.querySelector(".artifacts-studio-form");
  form.dataset.surface = surface.id;
  invalidateArtifactStudio(view);
  form.hidden = false;
  view.querySelector(".artifacts-studio").classList.add("is-composing");
  for (const button of view.querySelectorAll(".artifacts-studio-choice")) {
    button.setAttribute("aria-pressed", button.dataset.surface === surface.id ? "true" : "false");
  }
  view.querySelector(".artifacts-studio-brief").focus();
  paintArtifactCards(view);
  paintArtifactStudio(view);
}

/* 폼을 떠난다 — 초안의 글은 폼에 남고(숨겨질 뿐), 보내던 청은 세대가 바뀌어 낡는다.
 * 초점은 건드리지 않는다: 그림 쪽(`paintArtifactStudio`)에서도 부르기 때문이다. */
function leaveArtifactStudioForm(view) {
  const form = view.querySelector(".artifacts-studio-form");
  form.hidden = true;
  invalidateArtifactStudio(view);
  view.querySelector(".artifacts-studio").classList.remove("is-composing");
  for (const button of view.querySelectorAll(".artifacts-studio-choice")) button.setAttribute("aria-pressed", "false");
}

function closeArtifactStudio(view) {
  const surface = view.querySelector(".artifacts-studio-form").dataset.surface;
  leaveArtifactStudioForm(view);
  view.querySelector(`.artifacts-studio-choice[data-surface="${surface}"]`)?.focus();
  paintArtifactCards(view);
}

/* 닫기: 스튜디오 전체가 한 줄로 접힌다(`is-folded`) — 「접기」가 폼만 닫는 것과
 * 다른 손이다. 갤러리가 그 높이를 받고, 그 줄의 「펼치기」가 되돌린다. 접힘은
 * 창 안에서 유지되는 사람의 선택이라 판을 옮겨도(복제된 판도) 같다. */
function foldArtifactStudio(view) {
  artifactStudioFolded = true;
  paintArtifactStudio(view);
  paintArtifactCards(view);
  view.querySelector(".artifacts-studio-expand").focus();
}

function unfoldArtifactStudio(view) {
  artifactStudioFolded = false;
  paintArtifactStudio(view);
  paintArtifactCards(view);
  view.querySelector(".artifacts-studio-choice").focus();
}

function invalidateArtifactStudio(view) {
  const form = view.querySelector(".artifacts-studio-form");
  form._draftGeneration = (form._draftGeneration ?? 0) + 1;
  delete form.dataset.sent;
  say(view.querySelector(".artifacts-studio-status"), () => "");
}

function paintArtifactStudio(view) {
  const studio = view.querySelector(".artifacts-studio");
  const form = view.querySelector(".artifacts-studio-form");
  // 접힌 스튜디오에 열린 폼은 없다 — 범위 지정 도착이 작성 중에 접었으면 폼을
  // 떠난다(글은 남는다). 초점은 건드리지 않는다: 그림이지 몸짓이 아니다.
  if (artifactStudioFolded && !form.hidden) leaveArtifactStudioForm(view);
  studio.classList.toggle("is-folded", artifactStudioFolded);
  const destination = view.querySelector(".artifacts-studio-destination");
  const seats = artifactDraftSeats();
  const previous = destination.value;
  const rows = seats.map(seat => ({ value: String(seat.term), text: paneTitleOf(seat.tab, seat.term) || tabLabel(seat.tab) }));
  const names = rows.map(row => row.text);
  rows.forEach((row, index) => {
    if (names.filter(name => name === row.text).length > 1) {
      row.text += " · " + t("terminal.numbered", "터미널 {{n}}", { n: seats[index].term });
    }
  });
  if (seats.length !== 1 || destination.dataset.hadSeat === "true") {
    rows.unshift({ value: "", text: t("artifacts.studio.chooseAgent", "에이전트를 선택하세요") });
  }
  if (JSON.stringify(rows) !== destination.dataset.rows) {
    destination.replaceChildren(...rows.map(row => {
      const option = document.createElement("option");
      option.value = row.value; option.textContent = row.text;
      return option;
    }));
    destination.value = rows.some(row => row.value === previous) ? previous : (previous ? "" : rows[0]?.value ?? "");
    destination.dataset.rows = JSON.stringify(rows);
  }
  const seat = seats.find(row => String(row.term) === destination.value) ?? null;
  if (seat) destination.dataset.hadSeat = "true";
  view.querySelector(".artifacts-studio-send").disabled = form.dataset.sending === "true" || seat === null
    || !view.querySelector(".artifacts-studio-brief").value.trim();
  const status = view.querySelector(".artifacts-studio-status");
  say(status, () => seats.length === 0
    ? t("artifacts.newNoPane", "초안을 넣을 에이전트 판이 없습니다")
    : form.dataset.sent === "true" ? t("artifacts.studio.sent", "에이전트 입력줄에 초안을 넣었습니다.") : "");
}

function artifactStudioPrompt(view) {
  const form = view.querySelector(".artifacts-studio-form");
  const surface = ARTIFACT_STUDIO_SURFACES.find((row) => row.id === form.dataset.surface);
  const expression = ARTIFACT_STUDIO_EXPRESSIONS.find((row) => row.id === view.querySelector(".artifacts-studio-expression").value);
  const brief = view.querySelector(".artifacts-studio-brief").value.trim();
  if (!surface || !expression || !brief) return "";
  return t("artifacts.studio.request",
    "artifact-design 스킬을 사용해 완성도 높은 HTML 아티팩트를 만들어 주세요.\n목적: {{surface}}\n표현: {{expression}}\n참고 자료: {{reference}}\n\n{{brief}}\n\n내용에 맞는 구성과 디자인 토큰을 정하고 실제 기능을 구현하세요. 필요한 경우에만 시안을 비교하고, 조절값은 발행 파일에도 반영하세요. 브라우저에서 반응형·테마·상호작용을 확인한 HTML 파일을 zerocode-artifact publish --file-path <절대 경로.html>로 이 창의 아티팩트 갤러리에 발행하고, 답으로 받은 id와 버전을 알려 주세요. 나중에 고칠 때는 같은 원본 경로로 다시 발행해야 같은 아티팩트의 새 버전이 됩니다.",
    {
      surface: t(surface.title.key, surface.title.word),
      expression: t(expression.key, expression.word),
      reference: view.querySelector(".artifacts-studio-reference").value.trim() || t("artifacts.studio.noReference", "별도 지정 없음"),
      brief,
    });
}

async function sendArtifactStudio(view) {
  const form = view.querySelector(".artifacts-studio-form");
  if (form.dataset.sending === "true") return;
  const prompt = artifactStudioPrompt(view);
  const destination = view.querySelector(".artifacts-studio-destination").value;
  const seat = artifactDraftSeats().find(row => String(row.term) === destination);
  if (!prompt || !seat) return;
  const generation = form._draftGeneration;
  const current = () => form.isConnected && !form.hidden && form._draftGeneration === generation
    && view.querySelector(".artifacts-studio-destination").value === destination;
  form.dataset.sending = "true";
  delete form.dataset.sent;
  paintArtifactStudio(view);
  try {
    if (await draftIntoAgentPane(seat, prompt, { shouldFocus: current }) && current()) form.dataset.sent = "true";
  } catch (error) {
    showError(error);
  } finally {
    delete form.dataset.sending;
    paintArtifactStudio(view);
  }
}

function buildArtifactsView() {
  const root = document.createElement("section");
  root.className = "file-view artifacts-view";
  root.id = "artifacts-view";
  root.hidden = true;
  root.dataset.i18nAria = "artifacts.title";
  root.setAttribute("aria-label", t("artifacts.title", "아티팩트"));

  const head = document.createElement("header");
  head.className = "artifacts-head";
  const name = document.createElement("span");
  name.className = "artifacts-name";
  name.dataset.i18n = "artifacts.title";
  name.textContent = t("artifacts.title", "아티팩트");
  const query = document.createElement("input");
  query.className = "settings-input artifacts-query";
  query.type = "search";
  query.dataset.i18nPlaceholder = "artifacts.search";
  query.placeholder = t("artifacts.search", "제목·본문 검색 (Enter: 본문까지)");
  query.dataset.i18nAria = "artifacts.search";
  query.setAttribute("aria-label", t("artifacts.search", "제목·본문 검색 (Enter: 본문까지)"));
  const kinds = document.createElement("div");
  kinds.className = "artifacts-kinds";
  kinds.setAttribute("role", "group");
  kinds.dataset.i18nAria = "artifacts.kinds";
  kinds.setAttribute("aria-label", t("artifacts.kinds", "종류"));
  for (const kind of ["all", ...ARTIFACT_KINDS]) {
    const one = document.createElement("button");
    one.type = "button";
    one.className = "btn artifacts-kind";
    one.dataset.artifactKind = kind;
    one.setAttribute("aria-pressed", kind === "all" ? "true" : "false");
    if (kind !== "all") one.innerHTML = icon(ARTIFACT_KIND_GLYPH[kind]);
    const word = document.createElement("span");
    word.dataset.i18n = ARTIFACT_KIND_WORDS[kind].key;
    word.textContent = t(ARTIFACT_KIND_WORDS[kind].key, ARTIFACT_KIND_WORDS[kind].word);
    one.appendChild(word);
    kinds.appendChild(one);
  }
  const sources = document.createElement("div");
  sources.className = "artifacts-sources";
  sources.setAttribute("role", "group");
  sources.dataset.i18nAria = ARTIFACT_LABELS.sources.key;
  sources.setAttribute("aria-label", ARTIFACT_LABELS.sources.word);
  for (const row of ARTIFACT_SOURCES) {
    const one = document.createElement("button");
    one.type = "button";
    one.className = "btn artifacts-source";
    one.dataset.artifactSource = row.source;
    one.setAttribute("aria-pressed", row.source === "all" ? "true" : "false");
    one.dataset.i18n = row.key;
    one.textContent = row.word;
    sources.appendChild(one);
  }
  const origins = document.createElement("div");
  origins.className = "artifacts-origins";
  const originChip = document.createElement("span");
  originChip.className = "artifacts-origin-chip";
  originChip.hidden = true;
  const originClear = document.createElement("button");
  originClear.type = "button";
  originClear.className = "btn artifacts-origin-clear";
  originClear.innerHTML = icon("x");
  originClear.dataset.i18nAria = "artifacts.clearOrigin";
  originClear.setAttribute("aria-label", t("artifacts.clearOrigin", "출처 필터 지우기"));
  originClear.hidden = true;
  origins.append(originChip, originClear);
  const gap = document.createElement("span");
  gap.className = "artifacts-gap";
  const stat = document.createElement("span");
  stat.className = "artifacts-stat";
  const refresh = document.createElement("button");
  refresh.type = "button";
  refresh.className = "btn artifacts-refresh";
  refresh.innerHTML = icon("refresh");
  refresh.dataset.i18nAria = "artifacts.refresh";
  refresh.setAttribute("aria-label", t("artifacts.refresh", "다시 읽기"));
  // 「새 아티팩트」(t-3233 §4): 사람이 고른 에이전트의 입력줄에 초안을 넣는다
  // (t-3952) — 창이 직접 만들 길은 없고, 받을 곳을 창이 짐작하지도 않는다.
  const fresh = document.createElement("button");
  fresh.type = "button";
  fresh.className = "btn btn--primary artifacts-new";
  fresh.innerHTML = icon("plus");
  const freshWord = document.createElement("span");
  freshWord.dataset.i18n = ARTIFACT_LABELS.fresh.key;
  freshWord.textContent = ARTIFACT_LABELS.fresh.word;
  fresh.appendChild(freshWord);
  head.append(name, query, sources, kinds, origins, gap, stat, refresh, fresh);

  const trouble = document.createElement("p");
  trouble.className = "artifacts-error";
  trouble.hidden = true;

  const body = document.createElement("div");
  body.className = "artifacts-body";
  const grid = document.createElement("div");
  grid.className = "artifacts-grid";
  grid.tabIndex = 0;
  grid.setAttribute("role", "listbox");
  grid.dataset.i18nAria = "artifacts.grid";
  grid.setAttribute("aria-label", t("artifacts.grid", "아티팩트 카드"));
  const spacer = document.createElement("div");
  spacer.className = "artifacts-spacer";
  const cards = document.createElement("div");
  cards.className = "artifacts-cards";
  const empty = document.createElement("div");
  empty.className = "artifacts-empty";
  empty.hidden = true;
  const emptyWord = document.createElement("p");
  emptyWord.className = "artifacts-empty-word";
  emptyWord.dataset.i18n = "artifacts.empty";
  emptyWord.textContent = t("artifacts.empty", "아직 아티팩트가 없습니다");
  const emptyHint = document.createElement("p");
  emptyHint.className = "artifacts-empty-hint";
  emptyHint.dataset.i18n = "artifacts.emptyHint";
  emptyHint.textContent = t(
    "artifacts.emptyHint",
    "에이전트가 만든 페이지, claude.ai 아티팩트, 보고서와 스크린샷이 출처와 함께 모입니다.",
  );
  empty.append(emptyWord, emptyHint);
  grid.append(spacer, cards, empty);
  body.append(grid, buildArtifactDrawer());
  root.append(head, buildArtifactStudio(), trouble, body);
  return root;
}

function artifactMetaRow(list, row) {
  const term = document.createElement("dt");
  term.dataset.i18n = row.key;
  term.textContent = t(row.key, row.word);
  const value = document.createElement("dd");
  value.dataset.meta = row.field;
  list.append(term, value);
}

function buildArtifactDrawer() {
  const drawer = document.createElement("aside");
  drawer.className = "artifacts-drawer";
  drawer.dataset.i18nAria = "artifacts.drawer";
  drawer.setAttribute("aria-label", t("artifacts.drawer", "아티팩트 상세"));
  const empty = document.createElement("p");
  empty.className = "artifacts-drawer-empty";
  empty.dataset.i18n = "artifacts.drawerEmpty";
  empty.textContent = t("artifacts.drawerEmpty", "카드를 고르면 미리보기와 출처가 여기에 섭니다.");
  const detail = document.createElement("div");
  detail.className = "artifacts-detail";
  detail.hidden = true;

  const head = document.createElement("header");
  head.className = "artifact-detail-head";
  const kind = document.createElement("span");
  kind.className = "artifact-detail-kind";
  const title = document.createElement("h2");
  title.className = "artifact-detail-title";
  head.append(kind, title);

  const preview = document.createElement("div");
  preview.className = "artifact-preview";
  const md = document.createElement("div");
  md.className = "md-body artifact-preview-md";
  md.hidden = true;
  const img = document.createElement("img");
  img.className = "artifact-preview-image";
  img.alt = "";
  img.hidden = true;
  const text = document.createElement("pre");
  text.className = "artifact-preview-text";
  text.hidden = true;
  const none = document.createElement("p");
  none.className = "artifact-preview-none";
  none.hidden = true;
  const truncated = document.createElement("p");
  truncated.className = "artifact-preview-truncated";
  truncated.dataset.i18n = "artifacts.previewTruncated";
  truncated.textContent = t("artifacts.previewTruncated", "표 상한까지만 보입니다 — 전체는 「열기」로.");
  truncated.hidden = true;
  preview.append(md, img, text, none, truncated);

  const meta = document.createElement("dl");
  meta.className = "artifact-meta";
  for (const row of ARTIFACT_META_ROWS) artifactMetaRow(meta, row);
  for (const row of ARTIFACT_ORIGIN_FIELDS) artifactMetaRow(meta, row);
  artifactMetaRow(meta, ARTIFACT_META_RETENTION);

  const links = document.createElement("div");
  links.className = "artifact-origin-links";
  for (const row of ARTIFACT_JUMPS) {
    const one = document.createElement("button");
    one.type = "button";
    one.className = "btn artifact-jump";
    one.dataset.artifactJump = row.jump;
    one.innerHTML = icon(row.glyph);
    const label = document.createElement("span");
    label.dataset.i18n = row.key;
    label.textContent = t(row.key, row.word);
    one.appendChild(label);
    links.appendChild(one);
  }

  const actions = document.createElement("div");
  actions.className = "artifact-actions";
  for (const row of ARTIFACT_ACTIONS) {
    const one = document.createElement("button");
    one.type = "button";
    one.className = `btn artifact-action${row.action === "delete" ? " btn--halt" : ""}`;
    one.dataset.artifactAction = row.action;
    one.innerHTML = icon(row.glyph);
    const label = document.createElement("span");
    label.dataset.i18n = row.key;
    label.textContent = t(row.key, row.word);
    one.appendChild(label);
    actions.appendChild(one);
  }
  detail.append(head, preview, meta, links, actions);
  drawer.append(empty, detail);
  return drawer;
}

/* ---- 손 매기 (판마다 한 번) --------------------------------------------- */

function wireArtifactsView(view) {
  if (view._artifactsWired) return;
  view._artifactsWired = true;
  view.querySelector(".artifacts-studio-choices").addEventListener("click", (event) => {
    const button = event.target.closest(".artifacts-studio-choice");
    if (!button) return;
    // 눌린 타일을 다시 누르면 닫힌다 — 같은 손이 여닫는다.
    if (button.getAttribute("aria-pressed") === "true") closeArtifactStudio(view);
    else openArtifactStudio(view, button.dataset.surface);
  });
  view.querySelector(".artifacts-studio-dismiss").addEventListener("click", () => foldArtifactStudio(view));
  view.querySelector(".artifacts-studio-expand").addEventListener("click", () => unfoldArtifactStudio(view));
  view.querySelector(".artifacts-studio-form").addEventListener("submit", (event) => {
    event.preventDefault();
    void sendArtifactStudio(view);
  });
  view.querySelector(".artifacts-studio-form").addEventListener("input", () => {
    invalidateArtifactStudio(view);
    paintArtifactStudio(view);
  });
  view.querySelector(".artifacts-studio-form").addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closeArtifactStudio(view);
    }
  });
  view.querySelector(".artifacts-studio-close").addEventListener("click", () => closeArtifactStudio(view));
  const query = view.querySelector(".artifacts-query");
  query.addEventListener("input", () => {
    artifactFilter.query = query.value;
    artifactBodyMatches = null;
    clearTimeout(artifactSearchTimer);
    artifactSearchTimer = setTimeout(() => {
      applyArtifactFilter();
      paintArtifactCards(view);
      paintArtifactHead(view);
      paintArtifactDrawer(view);
    }, artifactTuning(view).searchDebounce);
  });
  query.addEventListener("keydown", (event) => {
    if (event.key !== "Enter") return;
    event.preventDefault();
    void searchArtifactBodies(view);
  });
  view.querySelector(".artifacts-kinds").addEventListener("click", (event) => {
    const button = event.target.closest("[data-artifact-kind]");
    if (!button) return;
    artifactFilter.kind = button.dataset.artifactKind;
    applyArtifactFilter();
    view.querySelector(".artifacts-grid").scrollTop = 0;
    paintArtifactCards(view);
    paintArtifactHead(view);
    paintArtifactDrawer(view);
  });
  view.querySelector(".artifacts-sources").addEventListener("click", (event) => {
    const button = event.target.closest("[data-artifact-source]");
    if (!button) return;
    artifactFilter.source = button.dataset.artifactSource;
    applyArtifactFilter();
    view.querySelector(".artifacts-grid").scrollTop = 0;
    paintArtifactCards(view);
    paintArtifactHead(view);
    paintArtifactDrawer(view);
  });
  view.querySelector(".artifacts-new").addEventListener("click", (event) => {
    void draftNewArtifact(event.currentTarget);
  });
  view.querySelector(".artifacts-origin-clear").addEventListener("click", () => {
    artifactFilter.origin = null;
    applyArtifactFilter();
    view.querySelector(".artifacts-grid").scrollTop = 0;
    paintArtifactCards(view);
    paintArtifactHead(view);
    paintArtifactDrawer(view);
  });
  view.querySelector(".artifacts-refresh").addEventListener("click", async () => {
    artifactAskedAt = 0;
    // 다시 읽기는 전사도 다시 읽는다(t-3233 §2): 부팅이 도는 같은 유계·증분
    // 길을 사람의 청으로 한 번 더. 답이 오면 카탈로그가 `artifacts:changed`로
    // 말하니 여기서는 목록만 청한다.
    await invoke("artifact_import_transcripts").catch(() => {});
    void refreshArtifacts();
  });
  const grid = view.querySelector(".artifacts-grid");
  grid.addEventListener("scroll", () => paintArtifactCards(view), { passive: true });
  grid.addEventListener("click", (event) => {
    const card = event.target.closest(".artifact-card");
    if (!card) return;
    selectArtifact(view, card.dataset.id);
  });
  grid.addEventListener("dblclick", (event) => {
    const card = event.target.closest(".artifact-card");
    if (card) void artifactAction(view, "open", card.dataset.id);
  });
  // 썸네일 큐는 스크롤에 맞춰 다시 청한다 — 보이는 카드만이라 `paintArtifactCards`가
  // 옷을 입힐 때 청하고, 그 사이 사라진 카드는 펌프가 건너뛴다.
  grid.addEventListener("keydown", (event) => {
    const columns = artifactColumns(view);
    const at = artifactOrder.indexOf(artifactSelectedId);
    const step = { ArrowDown: columns, ArrowUp: -columns, ArrowRight: 1, ArrowLeft: -1 }[event.key];
    if (step !== undefined) {
      event.preventDefault();
      const next = Math.min(artifactOrder.length - 1, Math.max(0, (at < 0 ? -1 : at) + step));
      if (artifactOrder[next]) selectArtifact(view, artifactOrder[next], { reveal: true });
      return;
    }
    if (event.key === "Enter" && artifactSelectedId) {
      event.preventDefault();
      void artifactAction(view, "open", artifactSelectedId);
    } else if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "c" && artifactSelectedId) {
      event.preventDefault();
      void artifactAction(view, "copy", artifactSelectedId);
    } else if ((event.key === "Delete" || event.key === "Backspace") && artifactSelectedId) {
      event.preventDefault();
      void artifactAction(view, "delete", artifactSelectedId);
    }
  });
  view.querySelector(".artifact-origin-links").addEventListener("click", (event) => {
    const button = event.target.closest("[data-artifact-jump]");
    if (button && artifactSelectedId) void jumpFromArtifact(button.dataset.artifactJump, artifactSelectedId);
  });
  view.querySelector(".artifact-actions").addEventListener("click", (event) => {
    const button = event.target.closest("[data-artifact-action]");
    if (button && artifactSelectedId) void artifactAction(view, button.dataset.artifactAction, artifactSelectedId);
  });
  // 판의 폭이 열의 수다: 관찰자 하나가 다시 앉힌다.
  if (typeof ResizeObserver === "function") {
    new ResizeObserver(() => {
      if (!view.hidden) paintArtifactCards(view);
      paintArtifactTier(view);
    }).observe(grid);
  }
}

/* ---- 그리기 ----------------------------------------------------------- */

function paintArtifactsView(tab = artifactsTab()) {
  if (!tab) return;
  const view = docHost(tab.pane, ARTIFACTS_TAB.kind);
  wireArtifactsView(view);
  paintWorkbenchNavigation(view, "artifacts");
  applyArtifactFilter();
  const trouble = view.querySelector(".artifacts-error");
  trouble.hidden = artifactError === null;
  trouble.textContent = artifactError ?? "";
  paintArtifactTier(view);
  paintArtifactHead(view);
  paintArtifactCards(view);
  paintArtifactDrawer(view);
}

/* 티어는 판의 폭이지 창의 폭이 아니다 — 지식 그래프와 같은 이유, 같은 기구.
 * 값은 컨테이너의 커스텀 속성으로도 적어, 서랍의 자세는 CSS의 `@container
 * style()` 질의가 고른다(숫자는 여전히 토큰 한 곳). */
function paintArtifactTier(view) {
  const tuning = artifactTuning(view);
  const body = view.querySelector(".artifacts-body");
  const width = body.clientWidth || view.clientWidth;
  const tier = width >= tuning.tierWide ? "wide" : width >= tuning.tierCompact ? "middle" : "compact";
  for (const name of ["wide", "middle", "compact"]) view.classList.toggle(`is-tier-${name}`, tier === name);
  body.style.setProperty("--artifact-tier", tier);
}

function paintArtifactHead(view) {
  paintArtifactStudio(view);
  for (const button of view.querySelectorAll("[data-artifact-kind]")) {
    button.setAttribute("aria-pressed", button.dataset.artifactKind === artifactFilter.kind ? "true" : "false");
  }
  for (const button of view.querySelectorAll("[data-artifact-source]")) {
    button.setAttribute(
      "aria-pressed",
      button.dataset.artifactSource === artifactFilter.source ? "true" : "false",
    );
  }
  // 받을 에이전트는 사람이 고른다(t-3952) — 「보낼 곳」 고르개가 도는 판과 새
  // 에이전트를 함께 내놓으니, 판이 없어도 이 손은 선다.
  const fresh = view.querySelector(".artifacts-new");
  fresh.disabled = false;
  fresh.dataset.tip = t("artifacts.newTip", "초안을 받을 에이전트를 고릅니다");
  fresh.setAttribute("aria-label", fresh.dataset.tip);
  const chip = view.querySelector(".artifacts-origin-chip");
  const clear = view.querySelector(".artifacts-origin-clear");
  const origin = artifactFilter.origin;
  chip.hidden = !origin;
  clear.hidden = !origin;
  if (origin) {
    const field = ARTIFACT_ORIGIN_FIELDS.find((row) => row.field === origin.field);
    const fieldWord = field ? t(field.key, field.word) : origin.field;
    chip.textContent = t("artifacts.filterOrigin", "{{field}}: {{name}}", {
      field: fieldWord,
      name: origin.label ?? origin.value,
    });
  }
  const stat = view.querySelector(".artifacts-stat");
  const shown = artifactOrder.length;
  const total = artifactListing.total || artifactRows.size;
  stat.textContent = shown === total
    ? t("artifacts.count", "{{n}}개", { n: total })
    : t("artifacts.countOf", "{{shown}} / {{n}}개", { shown, n: total });
  if (artifactListing.truncated) {
    stat.textContent += ` · ${t("artifacts.truncated", "표 상한까지만")}`;
  }
}

function artifactColumns(view) {
  const tuning = artifactTuning(view);
  const grid = view.querySelector(".artifacts-grid");
  const width = Math.max(0, grid.clientWidth - tuning.gap);
  return Math.max(1, Math.floor(width / (tuning.cardMin + tuning.gap)));
}

/* 카드 한 장의 옷 — 제자리에서. 노드는 풀의 것이고 id만 갈아입는다. */
function dressArtifactCard(node, row, selected) {
  if (node._artifactRow !== row) {
    node._artifactRow = row;
    node.dataset.id = row.id;
    node.dataset.kind = row.kind;
    node.querySelector(".artifact-card-title").textContent = row.title;
    const origin = row.origin ?? {};
    const parts = [
      origin.agent ?? "",
      origin.task ?? origin.automation ?? "",
      origin.worktree ? basename(origin.worktree) : "",
    ].filter(Boolean);
    node.querySelector(".artifact-card-origin").textContent = parts.length
      ? parts.join(" · ")
      : t("artifacts.noOrigin", "출처 없음");
    node.querySelector(".artifact-card-description").textContent = row.description ?? "";
    node.querySelector(".artifact-card-description").dataset.tip = row.description ?? "";
    node.querySelector(".artifact-card-kind").textContent = row.version
      ? t("artifacts.publishedVersion", "발행 · 버전 {{n}}", { n: row.version })
      : t(
      ARTIFACT_KIND_WORDS[row.kind]?.key ?? "artifacts.kind.other",
      ARTIFACT_KIND_WORDS[row.kind]?.word ?? "기타",
    );
    node.querySelector(".artifact-card-glyph use")
      .setAttribute("href", `#i-${ARTIFACT_KIND_GLYPH[row.kind] ?? "file"}`);
    // claude.ai 아티팩트는 제 파비콘(이모지)을 글리프 자리에 입는다.
    const emoji = node.querySelector(".artifact-card-emoji");
    emoji.textContent = row.favicon ?? "";
    emoji.hidden = !row.favicon;
    node.querySelector(".artifact-card-glyph").hidden = Boolean(row.favicon);
    // 문서(md)는 기존 미리보기 활자의 첫 블록이 얼굴이다(t-3233 §3).
    const doc = node.querySelector(".artifact-card-doc");
    const block = row.kind === "document" ? artifactFirstBlock(row.preview?.text ?? "") : "";
    doc.textContent = block;
    doc.hidden = block === "";
    const thumb = node.querySelector(".artifact-card-thumb");
    const rendered = ARTIFACT_THUMB_KINDS.has(row.kind) ? artifactThumbs.get(row.id) ?? null : null;
    const held = row.kind === "screenshot" ? artifactPreviews.get(row.id) : null;
    const src = rendered ?? held?.data_url ?? "";
    thumb.hidden = src === "";
    thumb.src = src;
    if (row.kind === "screenshot" && !held) void askArtifactThumb(node, row.id);
  }
  if (ARTIFACT_THUMB_KINDS.has(row.kind)) askArtifactThumbnail(row);
  node.querySelector(".artifact-card-when").textContent = artifactWhenWords(row);
  node.classList.toggle("is-selected", selected);
  node.setAttribute("aria-selected", selected ? "true" : "false");
}

/* 카드 아래 줄: 「🌐 9월 1일에 편집됨」/「🔒 …」. 오늘 것은 상대 낱말(`spaceAgo`,
 * `Intl.RelativeTimeFormat`), 그보다 오래된 것은 로케일의 달·날(`Intl.DateTimeFormat`)
 * — claude.ai의 줄과 같은 문법, 낱말은 카탈로그. */
function artifactWhenWords(row) {
  const glyph = ARTIFACT_WHEN_GLYPH[row.kind === "web" ? "remote" : "local"];
  const ms = row.modified_ms;
  const words = artifactEditedWords(ms);
  return [glyph, words].join(" ");
}

function artifactEditedWords(ms) {
  if (!ms) return spaceAgo(ms);
  const then = new Date(ms);
  const now = new Date();
  const sameDay = then.getFullYear() === now.getFullYear()
    && then.getMonth() === now.getMonth()
    && then.getDate() === now.getDate();
  if (sameDay) return t("artifacts.editedAgo", "{{ago}} 편집됨", { ago: spaceAgo(ms) });
  const code = locale === "system" ? systemLocale : locale;
  const when = new Intl.DateTimeFormat(code, { month: "long", day: "numeric" }).format(then);
  return t("artifacts.editedOn", "{{when}}에 편집됨", { when });
}

/* 마크다운 미리보기의 첫 활자 — 제목과 그 아래 첫 문단, 표식을 벗긴 글. 카드
 * 얼굴에 서는 활자라 몇 줄이면 되고, 나머지는 CSS가 자른다. */
const ARTIFACT_FACE_BLOCKS = 2;
function artifactFirstBlock(text) {
  const blocks = text.split(/\n\s*\n/).map((one) => one.trim()).filter((one) => one !== "");
  return blocks
    .slice(0, ARTIFACT_FACE_BLOCKS)
    .map((block) => block
      .split("\n")
      .map((line) => line.replace(/^[#>*\-\s]+/, "").replace(/[*_\x60]/g, ""))
      .join(" ")
      .trim())
    .join(" — ");
}

function makeArtifactCardNode() {
  artifactCardCreations += 1;
  const node = document.createElement("div");
  node.className = "artifact-card";
  node.setAttribute("role", "option");
  node.tabIndex = -1;
  const face = document.createElement("div");
  face.className = "artifact-card-face";
  const glyph = document.createElement("span");
  glyph.className = "artifact-card-glyph";
  glyph.innerHTML = icon("file");
  const emoji = document.createElement("span");
  emoji.className = "artifact-card-emoji";
  emoji.hidden = true;
  const doc = document.createElement("span");
  doc.className = "artifact-card-doc";
  doc.hidden = true;
  const thumb = document.createElement("img");
  thumb.className = "artifact-card-thumb";
  thumb.alt = "";
  thumb.hidden = true;
  // 출처 한 줄은 얼굴 위에 눕고 가리킬 때만 보인다(t-3233 §4: 출처 칩은
  // hover/드로어). 글은 늘 노드에 있어 하네스와 보조 기술이 읽는다.
  const origin = document.createElement("span");
  origin.className = "artifact-card-origin";
  face.append(glyph, emoji, doc, thumb, origin);
  const title = document.createElement("span");
  title.className = "artifact-card-title";
  const foot = document.createElement("span");
  foot.className = "artifact-card-foot";
  const kind = document.createElement("span");
  kind.className = "artifact-card-kind";
  const when = document.createElement("span");
  when.className = "artifact-card-when";
  foot.append(kind, when);
  const description = document.createElement("span");
  description.className = "artifact-card-origin artifact-card-description";
  node.append(face, title, description, foot);
  return node;
}

/* 가상화된 키 DOM. 보이는 행(+overscan)의 카드만 서고, 풀의 노드는 id를 갈아입되
 * 만들어지지 않는다 — 필터 전환·검색·스크롤 모두 생성 0. 남는 노드는 풀에 남되
 * 판에서 떼어 둔다(hidden 노드는 여전히 셈에 든다). */
function artifactGridLayout(view) {
  const tuning = artifactTuning(view);
  const grid = view.querySelector(".artifacts-grid");
  const columns = artifactColumns(view);
  const width = Math.max(0, grid.clientWidth - tuning.gap * (columns + 1));
  const cardWidth = Math.floor(width / columns);
  const cardHeight = tuning.cardHeight + (cardWidth - tuning.cardMin) * tuning.faceRatio;
  return { tuning, grid, columns, cardWidth, cardHeight, rowHeight: cardHeight + tuning.gap };
}

function paintArtifactCards(view) {
  const { tuning, grid, columns, cardWidth, cardHeight, rowHeight } = artifactGridLayout(view);
  const cards = view.querySelector(".artifacts-cards");
  const spacer = view.querySelector(".artifacts-spacer");
  const empty = view.querySelector(".artifacts-empty");
  let pool = artifactCardPools.get(view);
  if (!pool) {
    pool = { nodes: [], byId: new Map() };
    artifactCardPools.set(view, pool);
  }
  const rows = Math.ceil(artifactOrder.length / columns);
  spacer.style.height = `${rows * rowHeight + tuning.gap}px`;
  empty.hidden = artifactOrder.length > 0 || artifactError !== null;
  const firstRow = Math.max(0, Math.floor(grid.scrollTop / rowHeight) - tuning.overscan);
  const lastRow = Math.min(rows, Math.ceil((grid.scrollTop + grid.clientHeight) / rowHeight) + tuning.overscan);
  const first = firstRow * columns;
  const last = Math.min(artifactOrder.length, lastRow * columns);
  const wanted = artifactOrder.slice(first, last);
  const wantedSet = new Set(wanted);
  // Nodes that still show a wanted id keep it; the rest are free.
  const free = [];
  for (const node of pool.nodes) {
    if (!wantedSet.has(node.dataset.id)) {
      free.push(node);
      pool.byId.delete(node.dataset.id);
    }
  }
  for (let at = 0; at < wanted.length; at += 1) {
    const id = wanted[at];
    let node = pool.byId.get(id);
    if (!node) {
      node = free.pop();
      if (!node) {
        node = makeArtifactCardNode();
        pool.nodes.push(node);
      }
      pool.byId.set(id, node);
    }
    const row = artifactRows.get(id);
    if (!row) continue;
    dressArtifactCard(node, row, id === artifactSelectedId);
    const index = first + at;
    const x = tuning.gap + (index % columns) * (cardWidth + tuning.gap);
    const y = tuning.gap + Math.floor(index / columns) * rowHeight;
    node.style.transform = `translate(${x}px, ${y}px)`;
    node.style.width = `${cardWidth}px`;
    node.style.height = `${cardHeight}px`;
    if (node.parentElement !== cards) cards.appendChild(node);
  }
  for (const node of free) {
    if (!wantedSet.has(node.dataset.id) && node.parentElement) node.remove();
  }
}

/* 썸네일 — 보이는 스크린샷 카드만 청한다. 답은 서랍과 같은 캐시에 들고, 카드가
 * 그 사이 다른 id를 입었으면 옷을 건드리지 않는다. */
async function askArtifactThumb(node, id) {
  const payload = await askArtifactPreview(id);
  if (!payload?.data_url || node.dataset.id !== id) return;
  const thumb = node.querySelector(".artifact-card-thumb");
  thumb.src = payload.data_url;
  thumb.hidden = false;
}

async function askArtifactPreview(id) {
  const held = artifactPreviews.get(id);
  if (held) return held;
  const pending = artifactPreviewAsks.get(id);
  if (pending) return pending;
  const ask = (async () => {
    try {
      const payload = await invoke("artifact_preview", { id });
      const cap = artifactTuning(artifactsView() ?? document.documentElement).thumbCache;
      while (artifactPreviews.size >= cap) {
        const oldest = artifactPreviews.keys().next().value;
        if (oldest === undefined) break;
        artifactPreviews.delete(oldest);
      }
      artifactPreviews.set(id, payload);
      return payload;
    } catch (error) {
      return { kind: "none", error: String(error) };
    } finally {
      artifactPreviewAsks.delete(id);
    }
  })();
  artifactPreviewAsks.set(id, ask);
  return ask;
}

/* ---- 렌더 썸네일 큐 (t-3233 §3) ---------------------------------------- */

/* 보이는 카드 하나의 썸네일을 청한다. 이미 있거나 실패했으면 아무것도 하지
 * 않고, 큐가 표의 상한에 찼으면 이번엔 건너뛴다 — 카드가 다음 그림에서도
 * 보이면 다시 청한다. */
function askArtifactThumbnail(row) {
  const id = row.id;
  if (artifactThumbs.has(id) || artifactThumbFailed.has(id)) return;
  if (artifactThumbActiveId === id || artifactThumbQueue.includes(id)) return;
  const cap = artifactListing.thumb?.queue_max ?? 1;
  if (artifactThumbQueue.length >= cap) return;
  artifactThumbQueue.push(id);
  // 한 박자 뒤에: 옷을 입히는 그림이 아직 도는 중이고, 첫 카드의 노드는 옷을
  // 다 입은 뒤에야 판에 붙는다 — 지금 펌프를 돌리면 그 카드를 「안 보인다」고
  // 건너뛴다.
  queueMicrotask(() => void pumpArtifactThumbs());
}

/* 큐의 펌프 — 동시 1. 청할 차례가 온 카드가 그 사이 스크롤로 사라졌으면
 * 청하지 않는다(보이는 카드만). 답이 그림이면 캐시에 들고 아직 그 id를 입은
 * 노드에 입히고, 아니면 실패로 적어 다시 청하지 않는다(글리프로 물러남). */
async function pumpArtifactThumbs() {
  if (artifactThumbBusy) return;
  artifactThumbBusy = true;
  try {
    for (;;) {
      if (artifactThumbQueue.length === 0) refillArtifactThumbQueue();
      if (artifactThumbQueue.length === 0) break;
      const id = artifactThumbQueue.shift();
      if (artifactThumbs.has(id) || artifactThumbFailed.has(id)) continue;
      if (artifactCardNodeFor(id) === null) continue;
      artifactThumbActiveId = id;
      const key = artifactThumbnailKey(artifactRows.get(id));
      let answer = null;
      try {
        answer = await invoke("artifact_thumbnail", { id });
      } catch {
        answer = null;
      }
      artifactThumbActiveId = null;
      if (key !== artifactThumbnailKey(artifactRows.get(id))) {
        refillArtifactThumbQueue();
        continue;
      }
      if (typeof answer?.data_url === "string" && answer.data_url !== "") {
        rememberArtifactThumb(id, answer.data_url);
        const node = artifactCardNodeFor(id);
        if (node) {
          const thumb = node.querySelector(".artifact-card-thumb");
          thumb.src = answer.data_url;
          thumb.hidden = false;
        }
      } else {
        artifactThumbFailed.add(id);
      }
      refillArtifactThumbQueue();
    }
  } finally {
    artifactThumbBusy = false;
    artifactThumbActiveId = null;
  }
}

/* A full queue defers visible cards; every settled picture gives them a seat. */
function refillArtifactThumbQueue() {
  const view = artifactsView();
  if (!view) return;
  for (const node of view.querySelectorAll(".artifact-card")) {
    const row = artifactRows.get(node.dataset.id);
    if (row && ARTIFACT_THUMB_KINDS.has(row.kind)) askArtifactThumbnail(row);
  }
}

/* 지금 판에 서서 그 id를 입고 있는 카드 노드, 없으면 null. */
function artifactCardNodeFor(id) {
  const view = artifactsView();
  const node = view?.querySelector(`.artifact-card[data-id="${CSS.escape(id)}"]`) ?? null;
  return node?.isConnected ? node : null;
}

function rememberArtifactThumb(id, dataUrl) {
  const cap = artifactTuning(artifactsView() ?? document.documentElement).thumbCache;
  while (artifactThumbs.size >= cap) {
    const oldest = artifactThumbs.keys().next().value;
    if (oldest === undefined) break;
    artifactThumbs.delete(oldest);
  }
  artifactThumbs.set(id, dataUrl);
}

function selectArtifact(view, id, { reveal = false } = {}) {
  artifactSelectedId = id;
  if (reveal) {
    // Virtual cards do not exist until their row is in view. Use the renderer's
    // geometry before asking the DOM to reveal the selected card.
    const index = artifactOrder.indexOf(id);
    if (index >= 0) {
      const { tuning, grid, columns, rowHeight, cardHeight } = artifactGridLayout(view);
      const top = tuning.gap + Math.floor(index / columns) * rowHeight;
      if (top < grid.scrollTop || top + cardHeight > grid.scrollTop + grid.clientHeight) {
        grid.scrollTop = top;
      }
    }
  }
  paintArtifactCards(view);
  paintArtifactDrawer(view);
  if (reveal) {
    const node = view.querySelector(`.artifact-card[data-id="${CSS.escape(id)}"]`);
    node?.scrollIntoView({ block: "nearest" });
  }
}

function artifactBytesWord(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/* 서랍 — 판 자체가 아니라 글자와 목록의 줄을 고친다(`replaceChildren`은 마크다운
 * 미리보기의 본문에만, 그것도 다른 아티팩트로 옮길 때만). */
function paintArtifactDrawer(view) {
  const drawer = view.querySelector(".artifacts-drawer");
  const empty = drawer.querySelector(".artifacts-drawer-empty");
  const detail = drawer.querySelector(".artifacts-detail");
  const row = artifactSelectedId ? artifactRows.get(artifactSelectedId) : null;
  empty.hidden = Boolean(row);
  detail.hidden = !row;
  if (!row) {
    detail.dataset.artifactId = "";
    return;
  }
  drawer.querySelector(".artifact-detail-kind").textContent = t(
    ARTIFACT_KIND_WORDS[row.kind]?.key ?? "artifacts.kind.other",
    ARTIFACT_KIND_WORDS[row.kind]?.word ?? "기타",
  );
  drawer.querySelector(".artifact-detail-title").textContent = row.title;
  const put = (key, value) => {
    const cell = drawer.querySelector(`.artifact-meta [data-meta="${key}"]`);
    cell.textContent = value ?? "";
    cell.previousElementSibling.hidden = !value;
    cell.hidden = !value;
  };
  put("kind", t(ARTIFACT_KIND_WORDS[row.kind]?.key ?? "artifacts.kind.other", ARTIFACT_KIND_WORDS[row.kind]?.word ?? "기타"));
  put("bytes", artifactBytesWord(row.bytes ?? 0));
  put("created", row.created_ms ? new Date(row.created_ms).toLocaleString() : "");
  put("modified", row.modified_ms ? `${new Date(row.modified_ms).toLocaleString()} (${spaceAgo(row.modified_ms)})` : "");
  put("path", row.url ?? row.path);
  for (const { field } of ARTIFACT_ORIGIN_FIELDS) put(field, row.origin?.[field] ?? "");
  // claude.ai 아티팩트에는 파일이 없다: Finder와 경로 복사는 설 자리가 없다.
  const fileless = !row.path;
  drawer.querySelector('[data-artifact-action="reveal"]').disabled = fileless;
  drawer.querySelector('[data-artifact-action="copy"]').disabled = fileless;
  put(
    "retention",
    artifactListing.retentionDays > 0
      ? t("artifacts.retentionDays", "{{n}}일", { n: artifactListing.retentionDays })
      : t("artifacts.retentionLedger", "원장과 같음"),
  );
  // 점프는 갈 곳이 있을 때만 선다.
  const seat = artifactSeatFor(row);
  drawer.querySelector('[data-artifact-jump="card"]').disabled = seat === null;
  drawer.querySelector('[data-artifact-jump="task"]').disabled = !row.origin?.task;
  drawer.querySelector('[data-artifact-jump="worktree"]').disabled = !row.origin?.worktree;
  if (detail.dataset.artifactId !== row.id) {
    detail.dataset.artifactId = row.id;
    void paintArtifactPreview(view, row);
  }
}

async function paintArtifactPreview(view, row) {
  const drawer = view.querySelector(".artifacts-drawer");
  const md = drawer.querySelector(".artifact-preview-md");
  const img = drawer.querySelector(".artifact-preview-image");
  const text = drawer.querySelector(".artifact-preview-text");
  const none = drawer.querySelector(".artifact-preview-none");
  const truncated = drawer.querySelector(".artifact-preview-truncated");
  const payload = await askArtifactPreview(row.id);
  // 기다리는 사이 다른 카드로 옮겼으면 그 카드의 그림이 이긴다.
  if (drawer.querySelector(".artifacts-detail").dataset.artifactId !== row.id) return;
  md.hidden = payload.kind !== "markdown";
  img.hidden = payload.kind !== "image" || !payload.data_url;
  text.hidden = payload.kind !== "text";
  none.hidden = !(payload.kind === "none" || (payload.kind === "image" && !payload.data_url));
  truncated.hidden = payload.truncated !== true;
  if (payload.kind === "markdown") {
    if (md._painted !== payload.text) {
      md._painted = payload.text;
      md.replaceChildren();
      paintMarkdown(md, payload.text ?? "");
    }
  } else if (payload.kind === "image" && payload.data_url) {
    img.src = payload.data_url;
  } else if (payload.kind === "text") {
    text.textContent = payload.text ?? "";
  }
  if (!none.hidden) {
    none.textContent = payload.error
      ? payload.error
      : payload.kind === "image"
        ? t("artifacts.previewTooBig", "표 상한보다 큰 이미지입니다 — 「열기」로 보세요.")
        : t("artifacts.previewNone", "이 종류는 미리보기가 없습니다.");
  }
}

/* ---- 출처 링크 — 아티팩트 → 카드/작업/워크트리 --------------------------- */

/* 이 아티팩트의 워커가 지금 앉아 있는 판. 원장 명부(`paneLedger`)의 역이고,
 * 없으면 `null` — 끝난 워커의 카드는 없을 수 있다. */
function artifactSeatFor(row) {
  const worker = row.origin?.worker;
  const task = row.origin?.task;
  for (const [term, facts] of paneLedger) {
    if ((worker && facts.worker === worker) || (!worker && task && facts.taskId === task)) return term;
  }
  return null;
}

/* 보드는 열리면서 제 기본 선택을 고르므로(`openBoard`), 고르는 손은 그 첫 그림
 * **뒤에** 온다 — 보드의 자기 선택 문(`selectAgentGraphEntity`)으로, 판이 그
 * 카드를 실제로 들고 있을 때만. */
async function jumpFromArtifact(jump, id) {
  const row = artifactRows.get(id);
  if (!row) return;
  if (jump === "worktree") {
    if (row.origin?.worktree) await activateWorktree(row.origin.worktree);
    return;
  }
  const term = jump === "task"
    ? [...paneLedger].find(([, facts]) => facts.taskId === row.origin?.task)?.[0] ?? artifactSeatFor(row)
    : artifactSeatFor(row);
  if (term === null || term === undefined) {
    showError(t("artifacts.noSeat", "이 아티팩트의 워커는 지금 열린 판이 없습니다."));
    return;
  }
  // 지식 그래프의 좌석 문과 같은 길(`revealTaskBoardPane`).
  await revealTaskBoardPane(term);
}

/* ---- 액션 넷 ---------------------------------------------------------- */

async function artifactAction(view, action, id) {
  const row = artifactRows.get(id);
  if (!row) return;
  try {
    if (action === "open") {
      // 페이지·문서·claude.ai 아티팩트는 창 안에서 연다(t-3233 §5); 나머지는
      // 기존대로 시스템 기본 앱이다.
      if (!(await openArtifactPage(row))) await invoke("artifact_open", { id });
    } else if (action === "reveal") {
      await invoke("artifact_reveal", { id });
    } else if (action === "copy") {
      const path = await invoke("artifact_copy_path", { id });
      await clipboardText.write(String(path ?? row.path));
    } else if (action === "delete") {
      const answer = await askConfirm({
        title: t("artifacts.deleteTitle", "아티팩트를 삭제할까요?"),
        body: t("artifacts.deleteBody", "«{{name}}» — 카탈로그에서 지우고, 이 앱이 복사한 파일이면 파일도 지웁니다.", {
          name: row.title,
        }),
        confirm: t("artifacts.deleteConfirm", "삭제"),
        deny: t("artifacts.deleteDeny", "취소"),
        danger: true,
        cancel: false,
      });
      if (answer !== true) return;
      const gone = await invoke("artifact_delete", { id, confirmed: true });
      if (gone) {
        artifactRows.delete(id);
        artifactPreviews.delete(id);
        artifactListing.total = Math.max(0, artifactListing.total - 1);
        if (artifactSelectedId === id) artifactSelectedId = null;
        applyArtifactFilter();
        paintArtifactCards(view);
        paintArtifactHead(view);
        paintArtifactDrawer(view);
        void refreshArtifactCounts();
      }
    }
  } catch (error) {
    showError(String(error));
  }
}

/* ---- 아티팩트 페이지 (t-3233 §5) ---------------------------------------
 *
 * claude.ai 아티팩트는 창 안 브라우저 판에 그 url — 전사·터미널의 링크 클릭이
 * 이미 타는 길(`openBrowserTab`)이고 둘째 길은 없다. 이 기계의 페이지(html/svg)
 * 도 같은 문으로 `file://`을 열되 창의 머리띠 한 줄을 얹고, 문서(md)는 기존
 * 마크다운 뷰어에 같은 머리띠를 얹는다. 열었으면 true, 이 길이 아닌 종류면
 * false — 그러면 부르는 쪽이 시스템 기본 앱으로 간다. */
async function openArtifactPage(row) {
  if (row.kind === "web") {
    if (!row.url) return false;
    await openBrowserTab(row.url);
    return true;
  }
  if (row.kind === "page") {
    const facts = artifactStripFacts(row);
    let path = row.path;
    if (facts.current != null) {
      const origin = activeTabId;
      const checkout = activeWorktreePath;
      const versions = await invoke("artifact_versions", { id: row.id });
      if (activeTabId !== origin || activeWorktreePath !== checkout) return true;
      facts.versions = Array.isArray(versions) ? versions : [];
      facts.versionsGeneration = artifactVersionsGeneration;
      path = artifactShownPath(facts);
      if (!path) throw new Error(t("artifacts.versionUnavailable", "선택한 발행 버전을 찾을 수 없습니다. 갤러리를 다시 읽어 주세요."));
    }
    const label = await openBrowserTab(pathAsFileUrl(path));
    const tab = tabs.find((one) => one.kind === "browser" && one.label === label);
    if (tab) {
      tab.artifact = facts;
      if (stillShowing(tab)) paintBrowserView(tab);
    }
    return true;
  }
  if (row.kind === "document" && extensionOf(row.path) === "md") {
    await openFile(row.path, { preview: true });
    const tab = tabs.find((one) => one.id === `file:${row.path}`);
    if (tab) {
      tab.artifact = artifactStripFacts(row);
      if (stillShowing(tab)) paintFileView(tab);
    }
    return true;
  }
  return false;
}

// Catalog changes invalidate version lists, including an in-flight answer.
let artifactVersionsGeneration = 0;

/* 머리띠의 원본 사실과 선택한 스냅샷은 탭이 들고 다닌다. 발행물(t-3952)은
 * 제 번호(`current`)와 고칠 원본(`sourcePath`)을 따로 든다 — `path`는 스토어의
 * 사본이라 고칠 곳이 아니다. */
function artifactStripFacts(row) {
  return {
    id: row.id,
    title: row.title,
    agent: row.origin?.agent ?? "",
    project: basename(row.origin?.project ?? row.origin?.worktree ?? "") || "",
    path: row.path,
    current: row.version ?? null,
    sourcePath: row.source_path ?? null,
    versions: null,
    version: row.version ?? null,
  };
}

/* 지금 고른 판의 파일: 번호를 골랐으면 그 불변 스냅샷, 아니면 현재 파일. */
function artifactShownPath(facts) {
  const version = facts?.version ?? facts?.current ?? null;
  if (version == null) return facts?.path ?? "";
  return facts?.versions?.find((one) => one.n === version)?.path ?? "";
}

/* 두 file:// 주소가 같은 파일인가 — 브라우저가 돌려준 주소는 인코딩이 다를 수
 * 있어 풀어서 견준다. 조각·질의는 파일이 아니다. */
function sameFileUrl(a, b) {
  const plain = (url) => {
    const bare = String(url ?? "").split(/[?#]/)[0];
    try {
      return decodeURI(bare);
    } catch {
      return bare;
    }
  };
  return plain(a) !== "" && plain(a) === plain(b);
}

/* 브라우저 주석이 아티팩트의 어느 판을 두고 한 말인지(t-3952). 탭이 지금 그
 * 아티팩트의 현재 파일이나 번호 붙은 스냅샷을 볼 때만 말한다 — 링크를 따라 다른
 * 곳으로 갔으면 머리띠가 서 있어도 그 페이지는 아티팩트가 아니다. 본 판은 번호·
 * 불변 스냅샷·SHA-256으로, 고칠 곳은 원본으로 따로 적는다: 에이전트에게
 * 스토어의 스냅샷을 고치라고 하지 않는다. */
function artifactFeedbackContext(tab) {
  const facts = tab?.artifact;
  if (!facts || !tab.url) return "";
  const versions = facts.versions ?? [];
  const picked = facts.version != null ? versions.find((one) => one.n === facts.version) ?? null : null;
  const onCurrent = sameFileUrl(tab.url, pathAsFileUrl(facts.path));
  const onPicked = picked !== null && sameFileUrl(tab.url, pathAsFileUrl(picked.path));
  const onVersion = picked === null ? versions.find((one) => sameFileUrl(tab.url, pathAsFileUrl(one.path))) ?? null : null;
  if (!onCurrent && !onPicked && onVersion === null) return "";
  // 발행물의 「현재 파일」은 가장 새 번호의 사본이다 — 그 번호로 말한다.
  const shown = onPicked
    ? picked
    : onVersion;
  const lines = [t("artifacts.feedback.subject", "아티팩트 «{{title}}» ({{id}})", { title: facts.title, id: facts.id })];
  if (shown) {
    lines.push(t("artifacts.feedback.version", "본 판: 버전 {{n}} — 바꿀 수 없는 스냅샷 {{path}} · SHA-256 {{sha}}", {
      n: shown.n,
      path: shown.path,
      sha: shown.sha256 || t("artifacts.feedback.noSha", "알 수 없음"),
    }));
  } else {
    lines.push(t("artifacts.feedback.current", "본 판: 현재 파일 {{path}}", { path: facts.path }));
  }
  if (facts.current != null) {
    lines.push(facts.sourcePath
      ? t("artifacts.feedback.republish", "고칠 원본: {{source}} — 스냅샷이 아니라 이 원본을 고친 뒤 zerocode-artifact publish --file-path {{source}}로 다시 발행하면 같은 아티팩트의 새 버전이 됩니다.", { source: facts.sourcePath })
      : t("artifacts.feedback.unknownSource", "고칠 원본이 기록되지 않았습니다 — 스냅샷을 고치지 말고 zerocode-artifact read --id {{id}}의 source_path를 확인하세요.", { id: facts.id }));
  } else {
    lines.push(t("artifacts.feedback.editFile", "고칠 원본: {{source}} — 스냅샷이 아니라 이 파일을 고치세요.", { source: facts.path }));
  }
  return lines.join("\n");
}

/* 머리띠 노드: 「제목 · <agent>가 만듦 · <프로젝트> · 버전 N ⌄ · 공유 · Finder에서
 * 보기」. 브라우저 판과 파일 뷰가 같은 조립기로 짓고 `paintArtifactStrip`이 입힌다. */
function artifactStripNode() {
  const strip = document.createElement("div");
  strip.className = "artifact-strip";
  strip.hidden = true;
  const title = document.createElement("span");
  title.className = "artifact-strip-title";
  const by = document.createElement("span");
  by.className = "artifact-strip-by";
  const project = document.createElement("span");
  project.className = "artifact-strip-project";
  const versions = document.createElement("select");
  versions.className = "settings-select artifact-strip-version";
  versions.dataset.i18nAria = ARTIFACT_LABELS.versions.key;
  versions.setAttribute("aria-label", ARTIFACT_LABELS.versions.word);
  const gap = document.createElement("span");
  gap.className = "artifact-strip-gap";
  const share = document.createElement("button");
  share.type = "button";
  share.className = "btn artifact-strip-share";
  share.innerHTML = icon("share");
  const shareWord = document.createElement("span");
  shareWord.dataset.i18n = ARTIFACT_LABELS.share.key;
  shareWord.textContent = ARTIFACT_LABELS.share.word;
  share.appendChild(shareWord);
  const reveal = document.createElement("button");
  reveal.type = "button";
  reveal.className = "btn artifact-strip-reveal";
  reveal.innerHTML = icon("folder-open");
  const revealWord = document.createElement("span");
  revealWord.dataset.i18n = ARTIFACT_LABELS.reveal.key;
  revealWord.textContent = ARTIFACT_LABELS.reveal.word;
  reveal.appendChild(revealWord);
  strip.append(title, by, project, versions, gap, share, reveal);
  return strip;
}

// Paint also wires cloned split-leaf surfaces. Assigned handlers replace the
// previous paint's handlers rather than stacking them.
function paintArtifactStripActions(strip) {
  const available = Boolean(artifactShownPath(strip._facts));
  strip.querySelector(".artifact-strip-share").disabled = !available;
  strip.querySelector(".artifact-strip-reveal").disabled = !available;
}

function wireArtifactStrip(strip) {
  const versions = strip.querySelector(".artifact-strip-version");
  versions.onchange = () => {
    const facts = strip._facts;
    const picked = versions.value === "current"
      ? { n: null, path: facts?.path }
      : facts?.versions?.find((one) => String(one.n) === versions.value);
    if (!facts || !picked) return;
    facts.version = picked.n;
    paintArtifactStripActions(strip);
    strip._onVersion?.(picked);
  };
  strip.querySelector(".artifact-strip-share").onclick = () => {
    if (strip._facts) void shareArtifact(strip._facts, strip.querySelector(".artifact-strip-share"));
  };
  // 고른 판이 번호면 그 불변 파일을 Finder에 세운다(t-3952) — 현재 파일이 아니라.
  strip.querySelector(".artifact-strip-reveal").onclick = () => {
    const facts = strip._facts;
    if (facts) {
      invoke("artifact_reveal", { id: facts.id, version: facts.version ?? facts.current ?? null })
        .catch((error) => showError(String(error)));
    }
  };
}

/* 번호는 항상 그 스냅샷을 연다. 현재 파일은 번호와 구별된 선택지다. */
function paintArtifactStrip(strip, tab, onVersion) {
  if (!strip) return;
  const facts = tab?.artifact ?? null;
  strip.hidden = facts === null;
  strip._facts = facts;
  strip._tab = tab;
  strip._onVersion = onVersion;
  if (facts === null) return;
  wireArtifactStrip(strip);
  applyLocale(strip);
  strip.querySelector(".artifact-strip-title").textContent = facts.title;
  strip.querySelector(".artifact-strip-by").textContent = facts.agent
    ? t("artifacts.strip.by", "{{agent}}가 만듦", { agent: facts.agent })
    : "";
  strip.querySelector(".artifact-strip-project").textContent = facts.project;
  const select = strip.querySelector(".artifact-strip-version");
  const paintVersions = () => {
    const held = facts.versions ?? [];
    select.replaceChildren();
    if (facts.current == null) {
      const current = document.createElement("option");
      current.value = "current";
      current.textContent = t("artifacts.strip.current", "현재 파일");
      select.appendChild(current);
    }
    for (const one of held) {
      const option = document.createElement("option");
      option.value = String(one.n);
      option.textContent = t("artifacts.strip.version", "버전 {{n}}", { n: one.n });
      select.appendChild(option);
    }
    // Retention may remove the selected snapshot while its bytes remain on
    // screen. Keep its label, disabled, until the person picks another view.
    if (facts.version != null && !held.some(one => one.n === facts.version)) {
      const selected = document.createElement("option");
      selected.value = String(facts.version);
      selected.textContent = t("artifacts.strip.version", "버전 {{n}}", { n: facts.version });
      selected.disabled = true;
      select.appendChild(selected);
    }
    select.value = String(facts.version ?? "current");
    select.disabled = select.options.length < 2;
    paintArtifactStripActions(strip);
  };
  paintVersions();
  if (facts.versionsGeneration !== artifactVersionsGeneration && facts.versionsRequest !== artifactVersionsGeneration) {
    const generation = artifactVersionsGeneration;
    facts.versionsRequest = generation;
    invoke("artifact_versions", { id: facts.id })
      .then((rows) => {
        if (generation !== artifactVersionsGeneration) return;
        facts.versions = Array.isArray(rows) ? rows : [];
        facts.versionsGeneration = generation;
        if (strip._facts === facts) paintVersions();
      })
      .catch(() => {})
      .finally(() => {
        if (facts.versionsRequest === generation) facts.versionsRequest = null;
      });
  }
}

/* Claude 공유 요청은 받을 판을 명시적으로 고른 뒤 초안으로 남긴다.
 * 이미 선택한 불변 버전을 사용하며, 새 판에서도 자동 전송하지 않는다. */
async function shareArtifact(facts, button) {
  const path = artifactShownPath(facts);
  if (!path) return;
  await openSendToAgent(
    button,
    t("artifacts.shareDraft", "이 파일을 claude.ai 아티팩트로 올려 줘: {{path}}", { path }),
    null,
    { submit: false, agent: "claude" },
  );
}

/* 「새 아티팩트」: 받을 에이전트를 사람이 「보낼 곳」 고르개에서 고른다(t-3952) —
 * 도는 판이든 새 에이전트든. 초안은 입력줄에 머물고 Enter는 사람의 몫이며, 초안은
 * 어느 에이전트든 이 창의 갤러리에 닿는 문(`zerocode-artifact publish`)을 적는다. */
async function draftNewArtifact(button, prompt = null) {
  await openSendToAgent(
    button,
    prompt ?? t("artifacts.newDraft", "HTML 한 페이지 아티팩트를 만들어 zerocode-artifact publish --file-path <절대 경로.html>로 이 창의 갤러리에 발행하고 id와 버전을 알려 줘. 만들 것: "),
    null,
    { submit: false },
  );
}

/* 초점 있는 에이전트 판 — 앞에 선 터미널 탭의 활성 판, 없으면 무대의 다른
 * 리프에 선 터미널 탭, 없으면 이 워크트리의 터미널 탭 — 중 에이전트가 앉은
 * 후보를 모은다. `wanted`를 주면 그 에이전트의 판만 돌려준다. */
function artifactDraftSeats(wanted = null) {
  const candidates = [];
  const front = currentTab();
  if (front?.kind === "term") candidates.push(activePaneOf(front));
  for (const index of groups.keys()) {
    const on = activeTabIn(index);
    if (on?.kind === "term") candidates.push(activePaneOf(on));
  }
  for (const tab of tabs) {
    if (tab.kind === "term" && tab.worktree === activeWorktreePath) candidates.push(activePaneOf(tab), ...paneLeaves(tab.layout));
  }
  const seats = [];
  for (const term of new Set(candidates)) {
    if (term === null || term === undefined) continue;
    const agent = paneSessions.get(term)?.agent ?? paneAgents.get(term) ?? null;
    if (!agent) continue;
    if (wanted !== null && agent !== wanted) continue;
    const tab = tabOfTerm(term);
    if (tab) seats.push({ term, agent, tab });
  }
  return seats;
}

/* 초안을 판의 입력줄에 넣고 그 판을 앞으로 — 보내지는 않는다: 사람이 읽고 고치고
 * Enter를 친다. 붙여넣기는 컴포저와 같은 문(`term_paste`)이다. */
async function draftIntoAgentPane(seat, text, options = {}) {
  const origin = currentTab();
  if (!artifactDraftSeats().some(current => current.term === seat.term && current.agent === seat.agent && current.tab === seat.tab)) return false;
  try {
    await invoke("term_paste", { term: seat.term, text });
  } catch (error) {
    showError(String(error));
    return false;
  }
  const tab = tabOfTerm(seat.term);
  if (tab && tab === seat.tab && currentTab() === origin && (options.shouldFocus?.() ?? true)) {
    setActiveTab(tab.id);
    setActivePane(tab, seat.term);
  }
  return true;
}

/* 카탈로그가 움직였다는 백엔드의 말: 판이 서 있으면 다시 읽고, 칩의 수는 늘 다시
 * 센다. 폴러가 아니다 — 이 이벤트가 곧 폴이다. */
listen("artifacts:changed", () => {
  artifactVersionsGeneration += 1;
  for (const strip of document.querySelectorAll(".artifact-strip")) {
    if (strip._facts && !strip.hidden) paintArtifactStrip(strip, strip._tab, strip._onVersion);
  }
  artifactAskedAt = 0;
  const tab = artifactsTab();
  if (tab && stillShowing(tab)) void refreshArtifacts();
  void refreshArtifactCounts();
});
