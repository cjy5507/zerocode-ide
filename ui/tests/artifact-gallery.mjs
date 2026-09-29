import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { createRequire } from "node:module";
import { axeViolations } from "./settings-quality.mjs";
import { installHarnessWaits } from "./harness-waits.mjs";
import { openWindowTestPage } from "./window-boot.mjs";

export async function testArtifactChrome(page, ok) {
await installHarnessWaits(page);
/* ---- 갤러리 (t-3233): claude.ai처럼 채우고 보여 준다 --------------------------
 *
 * 소스 둘(전사의 claude.ai 아티팩트·에이전트가 쓴 페이지)은 Rust가 등록하고
 * (`artifact_transcripts`·`artifact_runtime`의 픽스처들), 여기서 재는 것은 창이
 * 그 행으로 무엇을 그리는가다 — 세그먼트 셋이 종류로 가르고 백엔드를 다시 묻지
 * 않는 것, 카드 아래 줄의 글리프와 날짜 낱말 셋, 문서의 얼굴이 첫 블록의 활자인
 * 것, 파비콘이 글리프 자리에 서는 것, 렌더 썸네일은 보이는 page·web 카드만 큐
 * 하나·동시 1로 청하고 실패는 글리프로 물러나 다시 청하지 않는 것, 「새 아티팩트」가
 * 초점 있는 에이전트 판에 초안을 넣고 판이 없으면 비활성인 것, 열기 길 둘(web →
 * 브라우저 url, page → 브라우저 + 머리띠, document → 마크다운 뷰어 + 머리띠),
 * 버전 픽커, 「공유」의 Claude 판 초안, 카탈로그 넷. */
const gallery = await page.evaluate(async () => {
  const seen = {};
  dropTab("artifacts");
  artifactFilter.tab = "pages";
  artifactTabPicked = false;
  window.__ARTIFACTS__ = { count: 6, gallery: 9 };
  window.__THUMB_ASKS__ = [];
  window.__THUMB_INFLIGHT_MAX__ = 0;
  window.__ARTIFACT_ASKS__ = 0;
  el("nav-artifacts").click();
  await window.__PAINTED__();
  await new Promise((done) => setTimeout(done, 200));
  const view = artifactsView();
  const cards = () => [...view.querySelectorAll(".artifact-card")];
  const tab = (name) => view.querySelector(`[data-artifact-tab="${name}"]`);
  const kindsShown = () => [...new Set(artifactOrder.map((id) => artifactRows.get(id).kind))].sort().join(",");
  // 카드 노드는 보이는 만큼만(가상화); 행의 수는 순서 배열이 말한다.
  seen.rows = artifactOrder.length;
  seen.cards = cards().length;
  seen.defaultTab = view.querySelector('[role="tab"][aria-selected="true"]')?.dataset.artifactTab;
  seen.tabs = [...view.querySelectorAll("[data-artifact-tab]")].filter((one) => !one.hidden)
    .map((one) => `${one.dataset.artifactTab}:${one.querySelector(".artifacts-tab-count").textContent}`).join(",");
  seen.pagesKinds = kindsShown();
  // 탭 전환은 백엔드를 다시 묻지 않는다: 보고서 → 증거 → 기타 → 페이지·문서.
  const asksBefore = window.__ARTIFACT_ASKS__;
  tab("reports").click();
  await new Promise((done) => setTimeout(done, 60));
  seen.reportsKinds = kindsShown();
  seen.reportsSelected = tab("reports").getAttribute("aria-selected");
  tab("evidence").click();
  await new Promise((done) => setTimeout(done, 60));
  seen.evidenceKinds = kindsShown();
  tab("other").click();
  await new Promise((done) => setTimeout(done, 60));
  seen.otherKinds = kindsShown();
  tab("pages").click();
  await new Promise((done) => setTimeout(done, 120));
  seen.pagesBack = artifactOrder.length === 9;
  seen.asksOnTabs = window.__ARTIFACT_ASKS__ - asksBefore;
  // 편집 시각 내림차순: 오늘 것들이 맨 앞이고, 날의 무리 셋이 그 순서로 머리를 단다.
  seen.newestFirst = artifactOrder.slice(0, 3).every((id) => (artifactRows.get(id).modified_ms) > Date.now() - 60_000);
  seen.groups = artifactGroups.map((group) => `${group.period}:${group.count}`).join(",");
  seen.groupHeads = [...view.querySelectorAll(".artifacts-group:not([hidden])")].map((one) => one.textContent).join(",");
  // 카드 아래 줄: 🌐/🔒 + 오늘은 상대 낱말, 어제·지난달은 달·날.
  const whenOf = (id) => view.querySelector(`.artifact-card[data-id="${id}"] .artifact-card-when`)?.textContent ?? "";
  seen.whenLocalToday = whenOf("gal-0");
  seen.whenYesterday = whenOf("gal-1");
  const yesterday = new Date(Date.now() - 24 * 60 * 60 * 1000);
  seen.yesterdayWords = new Intl.DateTimeFormat("ko", { month: "long", day: "numeric" }).format(yesterday);
  // 문서의 얼굴은 첫 블록의 활자.
  seen.docFace = view.querySelector('.artifact-card[data-id="gal-1"] .artifact-card-doc')?.textContent ?? "";
  seen.docFaceHidden = view.querySelector('.artifact-card[data-id="gal-1"] .artifact-card-doc')?.hidden;
  // 렌더 썸네일: page·web 카드만(문서·보고서는 아니다), 한 카드에 한 번, 동시 1,
  // 실패는 글리프.
  await window.__UNTIL__(() => !artifactThumbBusy && artifactThumbQueue.length === 0, "first thumbnails");
  seen.thumbShown = view.querySelector('.artifact-card[data-id="gal-0"] .artifact-card-thumb')?.hidden === false;
  seen.thumbFailedGlyph = view.querySelector('.artifact-card[data-id="gal-3"] .artifact-card-thumb')?.hidden === true
    && artifactThumbFailed.has("gal-3");
  // claude.ai 카드(가장 오래된 무리)는 제 파비콘을 글리프 자리에 입고 🌐를 단다.
  selectArtifact(view, "gal-2", { reveal: true });
  await window.__PAINTED__();
  await window.__UNTIL__(() => !artifactThumbBusy && artifactThumbQueue.length === 0, "revealed thumbnails");
  const web = artifactCardNodeFor("gal-2");
  seen.whenRemote = web?.querySelector(".artifact-card-when").textContent ?? "";
  seen.emoji = web?.querySelector(".artifact-card-emoji").textContent ?? "";
  seen.emojiGlyphHidden = web?.querySelector(".artifact-card-glyph").hidden;
  seen.thumbAsks = window.__THUMB_ASKS__.slice().sort();
  seen.thumbAskedOnce = new Set(seen.thumbAsks).size === seen.thumbAsks.length;
  seen.thumbOnlyRendered = seen.thumbAsks.every((id) => ["page", "web"].includes(artifactRows.get(id)?.kind));
  seen.thumbInflightMax = window.__THUMB_INFLIGHT_MAX__;
  // 탭을 한 바퀴 돌아와도 실패한 것은 다시 청하지 않고, 있는 것은 캐시에서.
  artifactSelectedId = null;
  tab("reports").click();
  await new Promise((done) => setTimeout(done, 40));
  tab("pages").click();
  await new Promise((done) => setTimeout(done, 200));
  seen.thumbAsksAfterRoundTrip = window.__THUMB_ASKS__.length;
  seen.thumbAsksBeforeRoundTrip = seen.thumbAsks.length;
  // 「새로 만들기」: 받을 곳은 고르개가 묻는다(t-3952) — 앞선 케이스들이 남긴
  // 판이 있든 없든 단추는 선다(새 에이전트도 고를 수 있다).
  seen.newFollowsSeat = view.querySelector(".artifacts-new").disabled === false;
  return seen;
});
ok(
  "the gallery opens on 「페이지·문서」 with each tab's count, the tabs split kinds without asking, cards group by day and wear 🌐/🔒 with relative or month-day words, a document's face is its first block, a claude.ai card wears its favicon, and thumbnails are asked for visible page·web cards only, one at a time, failing to a glyph once",
  gallery.rows === 9 && gallery.cards > 0 && gallery.cards <= 9 &&
    gallery.defaultTab === "pages" &&
    gallery.tabs === "pages:9,reports:1,evidence:2,other:3" &&
    gallery.pagesKinds === "document,page,web" &&
    gallery.reportsKinds === "report" && gallery.reportsSelected === "true" &&
    gallery.evidenceKinds === "evidence,screenshot" &&
    gallery.otherKinds === "export,other,transcript" &&
    gallery.pagesBack && gallery.asksOnTabs === 0 &&
    gallery.newestFirst &&
    gallery.groups === "today:3,week:3,older:3" &&
    gallery.groupHeads === "오늘,이번 주,이전" &&
    gallery.whenRemote.startsWith("🌐") &&
    gallery.whenLocalToday.startsWith("🔒") &&
    gallery.whenLocalToday.includes("편집됨") &&
    gallery.whenYesterday.includes(gallery.yesterdayWords) &&
    gallery.docFace.startsWith("문서 1") && gallery.docFace.includes("활자") && !gallery.docFace.includes("둘째") &&
    gallery.docFaceHidden === false &&
    gallery.emoji === "🧱" && gallery.emojiGlyphHidden === true &&
    gallery.thumbAsks.includes("gal-0") && gallery.thumbAsks.includes("gal-2") &&
    gallery.thumbAsks.includes("gal-3") && gallery.thumbAsks.includes("gal-6") &&
    gallery.thumbAskedOnce && gallery.thumbOnlyRendered &&
    gallery.thumbInflightMax === 1 &&
    gallery.thumbShown && gallery.thumbFailedGlyph &&
    gallery.thumbAsksAfterRoundTrip === gallery.thumbAsksBeforeRoundTrip &&
    gallery.newFollowsSeat,
  JSON.stringify(gallery),
);

/* 썸네일 큐는 표의 상한(`thumb_queue_max`)까지만 — 넘치는 카드는 다음 그림에서
 * 다시 청한다 — 그리고 천 장에서도 보이는 카드만 청한다. */
const galleryQueue = await page.evaluate(async () => {
  await window.__UNTIL__(() => !artifactAsking && !artifactThumbBusy
    && artifactThumbQueue.length === 0, "previous gallery thumbnails");
  dropTab("artifacts");
  window.__THUMB_QUEUE_MAX__ = 2;
  window.__ARTIFACTS__ = { count: 0, gallery: 900 };
  // Load the 900-row canonical list while closed. Reopening with fresh=true
  // would first render the previous 15-row catalog and count its thumbnails.
  await refreshArtifacts();
  window.__THUMB_ASKS__ = [];
  artifactThumbs.clear();
  artifactThumbFailed.clear();
  openArtifacts();
  await window.__PAINTED__();
  await window.__UNTIL__(() => !artifactAsking && artifactRows.size === 900
    && artifactsView()?.querySelector(".artifact-card"), "900-row gallery");
  const view = artifactsView();
  const visible = view.querySelectorAll(".artifact-card").length;
  const wantThumb = [...view.querySelectorAll(".artifact-card")]
    .filter((one) => one.dataset.kind === "page" || one.dataset.kind === "web").length;
  const queuedAtOnce = artifactThumbQueue.length;
  await window.__UNTIL__(() => !artifactThumbBusy && artifactThumbQueue.length === 0, "visible thumbnail responses");
  const seen = {
    visible,
    wantThumb,
    queuedAtOnce,
    asked: window.__THUMB_ASKS__.length,
    // 스크롤로 끝까지 가면 그 자리의 카드만 새로 청한다.
    askedBeforeScroll: window.__THUMB_ASKS__.length,
  };
  const grid = view.querySelector(".artifacts-grid");
  grid.scrollTop = grid.scrollHeight;
  grid.dispatchEvent(new Event("scroll"));
  await window.__PAINTED__();
  await window.__UNTIL__(() => !artifactThumbBusy && artifactThumbQueue.length === 0, "visible thumbnail responses");
  seen.askedAfterScroll = window.__THUMB_ASKS__.length;
  seen.rows = artifactRows.size;
  delete window.__THUMB_QUEUE_MAX__;
  return seen;
});
ok(
  "the thumbnail queue holds the table's cap and only visible cards are ever asked, at 900 rows",
  galleryQueue.rows === 900 &&
    galleryQueue.visible < 120 &&
    galleryQueue.queuedAtOnce <= 2 &&
    galleryQueue.asked === galleryQueue.wantThumb &&
    galleryQueue.asked > 0 &&
    galleryQueue.askedAfterScroll > galleryQueue.askedBeforeScroll &&
    galleryQueue.askedAfterScroll < 900,
  JSON.stringify(galleryQueue),
);


const refreshed = await page.evaluate(async () => {
  dropTab("artifacts");
  artifactFilter.tab = "pages";
  // Keep both thumbnail subjects in view regardless of the studio height.
  const rows = window.__buildArtifacts__({ count: 0, gallery: 4 }).filter(row => ["gal-0", "gal-3"].includes(row.id));
  const savedList = window.__ANSWER__.artifacts_list;
  window.__ANSWER__.artifacts_list = () => ({ rows, total: rows.length, thumb: { queue_max: 2 } });
  window.__THUMB_ASKS__ = [];
  el("nav-artifacts").click();
  await window.__PAINTED__();
  await new Promise(done => setTimeout(done, 160));
  const node = artifactCardNodeFor("gal-0");
  const before = window.__THUMB_ASKS__.length;
  rows[0] = { ...rows[0], title: "수정한 페이지", description: "발행된 결정 근거", favicon: "📘", version: 2, modified_ms: rows[0].modified_ms + 1 };
  rows[1] = { ...rows[1], modified_ms: rows[1].modified_ms + 1 };
  await refreshArtifacts();
  await new Promise(done => setTimeout(done, 160));
  const seen = {
    title: artifactCardNodeFor("gal-0")?.querySelector(".artifact-card-title").textContent,
    description: artifactCardNodeFor("gal-0")?.querySelector(".artifact-card-title").dataset.tip,
    favicon: artifactCardNodeFor("gal-0")?.querySelector(".artifact-card-emoji").textContent,
    version: artifactCardNodeFor("gal-0")?.querySelector(".artifact-card-kind").textContent,
    reused: !!node && node === artifactCardNodeFor("gal-0"),
    changedAsks: window.__THUMB_ASKS__.slice(before).sort(),
  };
  window.__ANSWER__.artifacts_list = savedList;
  return seen;
});
ok("refresh updates a keyed card and retries only changed thumbnail keys including failures",
  refreshed.title === "수정한 페이지" && refreshed.reused &&
  refreshed.description === "발행된 결정 근거" && refreshed.favicon === "📘" && refreshed.version.includes("2") &&
  refreshed.changedAsks.join(",") === "gal-0,gal-3", JSON.stringify(refreshed));

const proportions = await page.evaluate(() => [...artifactsView().querySelectorAll(".artifact-card-face")].map(face => {
  const rect = face.getBoundingClientRect();
  return rect.width / rect.height;
}));
ok("rendered card faces keep the 4:3 aspect ratio", proportions.length > 0 &&
  proportions.every(ratio => Math.abs(ratio - 4 / 3) < 0.01), JSON.stringify(proportions));

const newDraft = await page.evaluate(async () => {
  const agents = new Map(paneAgents);
  const sessions = new Map(paneSessions);
  const paste = window.__ANSWER__.term_paste;
  const terms = [3234, 3239];
  paneAgents.clear(); paneSessions.clear();
  let pasted = null, sent = null;
  window.__ANSWER__.term_paste = args => { pasted = args; };
  window.__ANSWER__.send_prompt = args => ((sent = { ...args }), null);
  // Two agent panes: the window must not guess which one receives the draft.
  for (const term of terms) {
    openTab({ id: `term:${term}`, kind: "term", worktree: activeWorktreePath,
      layout: { type: "leaf", term }, activePane: term }, { focus: false });
    paneAgents.set(term, "codex");
  }
  // Running terminals answer in this order; the catalog knows `codex`.
  window.__ANSWER__.agent_terms = () => terms.map(term => [term, "codex"]);
  openArtifacts();
  await window.__PAINTED__();
  artifactsView().querySelector(".artifacts-new").click();
  await new Promise(done => setTimeout(done, 60));
  artifactsView().querySelector(".artifacts-new-blank").click();
  await new Promise(done => setTimeout(done, 120));
  const rows = [...document.querySelectorAll("#note-pop .note-pop-row")];
  const pastedBeforeChoice = pasted !== null || sent !== null;
  rows[1]?.click();
  await new Promise(done => setTimeout(done, 120));
  const focused = activeTabId === `term:${terms[1]}`;
  for (const term of terms) dropTab(`term:${term}`);
  paneAgents.clear(); paneSessions.clear();
  for (const [id, agent] of agents) paneAgents.set(id, agent);
  for (const [id, session] of sessions) paneSessions.set(id, session);
  window.__ANSWER__.term_paste = paste;
  delete window.__ANSWER__.send_prompt;
  delete window.__ANSWER__.agent_terms;
  openArtifacts();
  return { rows: rows.length, pastedBeforeChoice, sent, pasted, focused };
});
ok("new artifact asks for its recipient, leaves the draft unsent in the chosen agent's input, and names the gallery's publication door",
  newDraft.rows >= 2 && newDraft.pastedBeforeChoice === false && newDraft.pasted === null &&
  newDraft.sent?.term === 3239 && newDraft.sent?.submit === false && newDraft.focused &&
  newDraft.sent?.text.includes("zerocode-artifact publish --file-path"), JSON.stringify(newDraft));

}

export async function testArtifactPages(page, ok) {
/* 열기 길 둘과 머리띠, 버전 픽커, 「새 아티팩트」와 「공유」의 초안. web 카드는
 * 창 안 브라우저 탭에 그 url(둘째 길 없음), page 카드는 브라우저 탭 + 머리띠,
 * document 카드는 마크다운 뷰어 + 머리띠. 초안은 `term_paste`로 판의 입력줄에
 * 들어가고 보내지지 않는다. */
const galleryDoors = await page.evaluate(async () => {
  const seen = {};
  dropTab("artifacts");
  window.__ARTIFACTS__ = { count: 0, gallery: 3 };
  window.__PASTES__ = [];
  window.__ANSWER__.term_paste = (args) => (window.__PASTES__.push({ ...args }), null);
  window.__ANSWER__.term_key = () => null;
  window.__ANSWER__.read_text_file = (args) => ({ text: `# 문서\n\n${args.path}`, version: 1 });
  let born = 0;
  window.__ANSWER__.open_browser_pane = (args) => {
    born += 1;
    window.__BROWSER_OPENS__ = (window.__BROWSER_OPENS__ ?? []).concat([args.url]);
    return `browser-gal-${born}`;
  };
  window.__BROWSER_OPENS__ = [];
  el("nav-artifacts").click();
  await window.__PAINTED__();
  await new Promise((done) => setTimeout(done, 120));
  const view = artifactsView();
  const rowOf = (id) => artifactRows.get(id);
  // web → 브라우저 탭에 그 url.
  await openArtifactPage(rowOf("gal-2"));
  await new Promise((done) => setTimeout(done, 40));
  seen.webOpened = window.__BROWSER_OPENS__[0] ?? "";
  const webTab = tabs.find((one) => one.kind === "browser" && one.label === "browser-gal-1");
  seen.webTabHasNoStrip = webTab ? !webTab.artifact : false;
  seen.webStripHidden = docHost(webTab.pane, "browser").querySelector(".artifact-strip").hidden;
  // page → 브라우저 탭 + 머리띠(제목 · 만든 이 · 프로젝트 · 버전 · 공유 · Finder).
  window.__VERSION_ASKS__ = 0;
  await openArtifactPage(rowOf("gal-0"));
  await new Promise((done) => setTimeout(done, 80));
  seen.pageOpened = window.__BROWSER_OPENS__[1] ?? "";
  const pageTab = tabs.find((one) => one.kind === "browser" && one.label === "browser-gal-2");
  const strip = docHost(pageTab.pane, "browser").querySelector(".artifact-strip");
  seen.stripShown = strip.hidden === false;
  seen.stripTitle = strip.querySelector(".artifact-strip-title").textContent;
  seen.stripBy = strip.querySelector(".artifact-strip-by").textContent;
  seen.stripProject = strip.querySelector(".artifact-strip-project").textContent;
  seen.versionAsks = window.__VERSION_ASKS__;
  const select = strip.querySelector(".artifact-strip-version");
  seen.versionOptions = [...select.options].map((one) => one.textContent);
  seen.versionCurrent = select.value;
  // 픽커가 지난 판을 같은 판에 연다.
  window.__ANSWER__.browser_navigate = (args) => (window.__NAVIGATED__ = args, null);
  select.value = "1";
  select.dispatchEvent(new Event("change", { bubbles: true }));
  await new Promise((done) => setTimeout(done, 40));
  seen.navigatedTo = window.__NAVIGATED__?.url ?? "";
  seen.navigatedLabel = window.__NAVIGATED__?.label ?? "";
  // 고른 판에 대한 주석은 그 불변 스냅샷(번호·SHA)과 고칠 원본을 따로 말하고,
  // Finder도 그 스냅샷을 세운다(t-3952). 링크를 따라 나간 페이지는 아티팩트가 아니다.
  pageTab.url = pathAsFileUrl("/data/artifacts/versions/gal-0/1/page.html");
  seen.pickedFeedback = artifactFeedbackContext(pageTab);
  pageTab.url = "https://example.com/elsewhere";
  seen.elsewhereFeedback = artifactFeedbackContext(pageTab);
  pageTab.url = pathAsFileUrl("/data/artifacts/versions/gal-0/1/page.html");
  window.__ARTIFACT_REVEALED__ = null;
  strip.querySelector(".artifact-strip-reveal").click();
  await new Promise((done) => setTimeout(done, 40));
  seen.revealed = window.__ARTIFACT_REVEALED__;
  // 「공유」: Claude 판이 없으면 초안이 없다. 앞선 케이스들이 남긴 판의 에이전트
  // 표를 잠시 비워 「없음」을 만든다.
  const savedAgents = new Map(paneAgents);
  const savedSessions = new Map(paneSessions);
  paneAgents.clear();
  paneSessions.clear();
  seen.seatWithoutClaude = artifactDraftSeats("claude")[0] ?? null;
  paintArtifactHead(view);
  // 받을 곳은 고르개가 묻는다 — 판이 없어도 새 에이전트를 고를 수 있다(t-3952).
  seen.newEnabledWithoutSeat = view.querySelector(".artifacts-new").disabled === false;
  strip.querySelector(".artifact-strip-share").click();
  await new Promise((done) => setTimeout(done, 40));
  seen.shareWithoutClaude = window.__PASTES__.length;
  closeNoteSend();
  // 에이전트 판 하나를 앉히면 「새 아티팩트」와 「공유」가 그 판에 초안을 넣는다.
  const term = 3233;
  openTab({ id: `term:${term}`, kind: "term", worktree: activeWorktreePath, layout: { type: "leaf", term }, activePane: term }, { focus: false });
  paneAgents.set(term, "claude");
  setActiveTab(pageTab.id);
  seen.seatWithClaude = artifactDraftSeats("claude")[0]?.term ?? null;
  let shared = null;
  window.__ANSWER__.agent_terms = () => [[term, "claude"]];
  window.__ANSWER__.send_prompt = (args) => { shared = args; return null; };
  strip.querySelector(".artifact-strip-share").click();
  await new Promise((done) => setTimeout(done, 40));
  seen.shareBeforeChoice = shared;
  document.querySelector("#note-pop .note-pop-row")?.click();
  await new Promise((done) => setTimeout(done, 80));
  seen.sharePaste = shared;
  seen.shareBroughtForward = activeTabId === `term:${term}`;
  // document → 마크다운 뷰어 + 같은 머리띠.
  await openArtifactPage(rowOf("gal-1"));
  await new Promise((done) => setTimeout(done, 80));
  const fileTab = tabs.find((one) => one.id === `file:${rowOf("gal-1").path}`);
  seen.docTab = fileTab ? fileTab.mode : "";
  const fileView = docHost(fileTab.pane, "file");
  const fileStrip = fileView.querySelector(".file-view-head .artifact-strip");
  seen.docStrip = fileStrip ? fileStrip.hidden === false : false;
  seen.docStripTitle = fileStrip?.querySelector(".artifact-strip-title")?.textContent ?? "";
  seen.docRendered = fileView.querySelector(".file-body h1")?.textContent ?? "";
  // 지난 문서 판은 미리보기다. 현재 파일의 초안·저장 stamp를 건드리지 않는다.
  const originalText = fileTab.text;
  const originalDraft = fileTab.draft;
  const docSelect = fileStrip.querySelector(".artifact-strip-version");
  docSelect.value = "1";
  docSelect.dispatchEvent(new Event("change", { bubbles: true }));
  await new Promise(done => setTimeout(done, 40));
  seen.docSnapshotShown = fileView.querySelector(".file-body").textContent.includes("/versions/gal-1/1/");
  seen.docOriginalPreserved = fileTab.text === originalText && fileTab.draft === originalDraft;
  seen.snapshotCannotSave = await saveFile(fileTab) === false;
  docSelect.value = "current";
  docSelect.dispatchEvent(new Event("change", { bubbles: true }));
  await new Promise(done => setTimeout(done, 40));
  seen.docLatestRestored = fileView.querySelector(".file-body").textContent.includes("/project/document-1.md");
  // 「새 아티팩트」: 누르면 「보낼 곳」 고르개가 서고, 사람이 고른 판의 입력줄에
  // 초안이 머문다(보내지 않음). 창이 판을 짐작해 붙여 넣지 않는다(t-3952).
  openArtifacts();
  await new Promise((done) => setTimeout(done, 40));
  const gallery = artifactsView();
  seen.newEnabled = gallery.querySelector(".artifacts-new").disabled === false;
  const pastesBeforeNew = window.__PASTES__.length;
  let sentNew = null;
  window.__ANSWER__.agent_terms = () => [[term, "claude"]];
  window.__ANSWER__.send_prompt = (args) => ((sentNew = { ...args }), null);
  gallery.querySelector(".artifacts-new").click();
  await new Promise((done) => setTimeout(done, 60));
  gallery.querySelector(".artifacts-new-blank").click();
  await new Promise((done) => setTimeout(done, 120));
  seen.newPickerRows = document.querySelectorAll("#note-pop .note-pop-row").length;
  seen.newPastedWithoutChoice = window.__PASTES__.length !== pastesBeforeNew;
  document.querySelector("#note-pop .note-pop-row")?.click();
  await new Promise((done) => setTimeout(done, 120));
  seen.newSent = sentNew;
  delete window.__ANSWER__.agent_terms;
  delete window.__ANSWER__.send_prompt;
  // 서랍의 「열기」도 같은 문이고, claude.ai 행에는 Finder·경로 복사가 서지 않는다.
  openArtifacts();
  await new Promise((done) => setTimeout(done, 40));
  selectArtifact(artifactsView(), "gal-2");
  await new Promise((done) => setTimeout(done, 40));
  seen.revealDisabled = artifactsView().querySelector('[data-artifact-action="reveal"]').disabled;
  seen.copyDisabled = artifactsView().querySelector('[data-artifact-action="copy"]').disabled;
  seen.drawerPath = artifactsView().querySelector('.artifact-meta [data-meta="path"]').textContent;
  window.__ARTIFACT_OPENED__ = null;
  await artifactAction(artifactsView(), "open", "gal-2");
  await new Promise((done) => setTimeout(done, 40));
  seen.drawerOpenUsedBrowser = window.__BROWSER_OPENS__.length === 3 && window.__ARTIFACT_OPENED__ === null;
  // 다시 읽기는 전사 가져오기도 한 번 청한다. (열기가 브라우저 탭을 앞에
  // 세웠으니 갤러리를 다시 앞으로.)
  openArtifacts();
  await new Promise((done) => setTimeout(done, 40));
  window.__IMPORT_ASKS__ = 0;
  artifactsView().querySelector(".artifacts-refresh").click();
  await new Promise((done) => setTimeout(done, 40));
  seen.importAsked = window.__IMPORT_ASKS__;
  seen.catalogs = ["en", "ja", "zh", "es"].map((code) => [
    CATALOG[code]["artifacts.tab.pages"], CATALOG[code]["artifacts.new"], CATALOG[code]["artifacts.strip.share"],
    CATALOG[code]["artifacts.editedOn"], CATALOG[code]["artifacts.kind.page"],
  ].every((word) => typeof word === "string" && word !== ""));
  // 정리.
  for (const tab of tabs.filter((one) => one.kind === "browser" && String(one.label).startsWith("browser-gal-"))) dropTab(tab.id);
  dropTab(`file:${rowOf("gal-1").path}`);
  dropTab(`term:${term}`);
  paneAgents.clear();
  for (const [key, value] of savedAgents) paneAgents.set(key, value);
  for (const [key, value] of savedSessions) paneSessions.set(key, value);
  delete window.__ANSWER__.open_browser_pane;
  delete window.__ANSWER__.browser_navigate;
  delete window.__ANSWER__.read_text_file;
  delete window.__ANSWER__.term_paste;
  delete window.__ANSWER__.term_key;
  return seen;
});
ok(
  "a claude.ai card opens its url in the window's browser, a page opens in the browser with the header strip and a version picker that navigates the same pane, feedback, Finder and share act on the picked immutable version, a document opens in the markdown viewer with the same strip, new-artifact asks for its recipient, and the drawer's open walks the same doors",
  galleryDoors.webOpened.startsWith("https://claude.ai/code/artifact/") &&
    galleryDoors.webTabHasNoStrip && galleryDoors.webStripHidden === true &&
    galleryDoors.pageOpened.startsWith("file:///tmp/zerocode-window-test/project/page-0.html") &&
    galleryDoors.stripShown &&
    galleryDoors.stripTitle === "page-0.html" &&
    galleryDoors.stripBy.includes("claude") &&
    galleryDoors.stripProject === "project" &&
    galleryDoors.versionAsks === 1 &&
    galleryDoors.versionOptions.join(",") === "현재 파일,버전 1,버전 2" &&
    galleryDoors.versionCurrent === "current" &&
    galleryDoors.navigatedTo.includes("/versions/gal-0/1/") &&
    galleryDoors.navigatedLabel === "browser-gal-2" &&
    galleryDoors.pickedFeedback.includes("gal-0") &&
    galleryDoors.pickedFeedback.includes("/data/artifacts/versions/gal-0/1/page.html") &&
    galleryDoors.pickedFeedback.includes("1".repeat(64)) &&
    galleryDoors.pickedFeedback.includes("/tmp/zerocode-window-test/project/page-0.html") &&
    galleryDoors.elsewhereFeedback === "" &&
    galleryDoors.revealed?.id === "gal-0" && galleryDoors.revealed?.version === 1 &&
    galleryDoors.seatWithoutClaude === null && galleryDoors.shareWithoutClaude === 0 &&
    galleryDoors.newEnabledWithoutSeat &&
    galleryDoors.seatWithClaude === 3233 &&
    galleryDoors.shareBeforeChoice === null && galleryDoors.sharePaste?.submit === false &&
    galleryDoors.sharePaste?.term === 3233 &&
    galleryDoors.sharePaste?.text.includes("claude.ai") &&
    galleryDoors.sharePaste?.text.endsWith("/data/artifacts/versions/gal-0/1/page.html") &&
    galleryDoors.shareBroughtForward &&
    galleryDoors.docTab === "markdown" &&
    galleryDoors.docStrip && galleryDoors.docStripTitle === "document-1.md" &&
    galleryDoors.docRendered === "문서" &&
    galleryDoors.docSnapshotShown && galleryDoors.docOriginalPreserved &&
    galleryDoors.snapshotCannotSave && galleryDoors.docLatestRestored &&
    galleryDoors.newEnabled &&
    galleryDoors.newPickerRows > 0 && galleryDoors.newPastedWithoutChoice === false &&
    galleryDoors.newSent?.term === 3233 && galleryDoors.newSent?.submit === false &&
    galleryDoors.newSent?.text.includes("아티팩트") &&
    galleryDoors.newSent?.text.includes("zerocode-artifact publish --file-path") &&
    galleryDoors.revealDisabled && galleryDoors.copyDisabled &&
    galleryDoors.drawerPath.startsWith("https://claude.ai/") &&
    galleryDoors.drawerOpenUsedBrowser &&
    galleryDoors.importAsked === 1 &&
    galleryDoors.catalogs.every(Boolean),
  JSON.stringify(galleryDoors),
);

const review = await page.evaluate(async () => {
  const seen = {};
  const pause = () => new Promise(done => setTimeout(done, 40));
  setLocale("en");
  const strip = artifactStripNode();
  document.body.appendChild(strip);
  const tab = { artifact: artifactStripFacts({ id: "review", title: "Review", path: "/repo/page.md" }) };
  let versionRows = [1, 2].map(n => ({ n, path: `/versions/review/${n}/page.md`, bytes: n, modified_ms: n }));
  const answer = window.__ANSWER__.artifact_versions;
  window.__ANSWER__.artifact_versions = () => versionRows;
  let picked = null;
  paintArtifactStrip(strip, tab, value => { picked = value; });
  await pause();
  seen.englishShare = strip.querySelector(".artifact-strip-share").textContent;
  setLocale("ko");
  seen.koreanShare = strip.querySelector(".artifact-strip-share").textContent;
  seen.koreanReveal = strip.querySelector(".artifact-strip-reveal").textContent;
  const select = strip.querySelector("select");
  // Disk V3 exists before its change notification reaches this open tab.
  versionRows = [...versionRows, { n: 3, path: "/versions/review/3/page.md", bytes: 3, modified_ms: 3 }];
  select.value = "2";
  select.dispatchEvent(new Event("change"));
  seen.staleV2Path = picked?.path;
  for (const handler of window.__LISTENERS__["artifacts:changed"] ?? []) handler({ payload: null });
  await pause();
  seen.versionsAfterSave = [...select.options].map(option => option.value);
  seen.selectionAfterSave = select.value;
  let finishOld;
  window.__ANSWER__.artifact_versions = () => new Promise(done => { finishOld = done; });
  for (const handler of window.__LISTENERS__["artifacts:changed"] ?? []) handler({ payload: null });
  await pause();
  const retained = [{ n: 3, path: "/versions/review/3/page.md" }, { n: 4, path: "/versions/review/4/page.md" }];
  window.__ANSWER__.artifact_versions = () => retained;
  for (const handler of window.__LISTENERS__["artifacts:changed"] ?? []) handler({ payload: null });
  await pause();
  finishOld(versionRows);
  await pause();
  seen.oldReplyIgnored = [...select.options].some(option => option.value === "4");
  seen.retiredSelection = select.value === "2" && select.selectedOptions[0].disabled;
  // docHost clones browser templates for split leaves, which drops listeners.
  const clone = strip.cloneNode(true);
  document.body.appendChild(clone);
  let clonePicks = 0;
  const onVersion = () => { clonePicks += 1; };
  paintArtifactStrip(clone, tab, onVersion);
  paintArtifactStrip(clone, tab, onVersion);
  const clonedSelect = clone.querySelector("select");
  clonedSelect.value = "3";
  clonedSelect.dispatchEvent(new Event("change"));
  seen.clonePicks = clonePicks;
  const revealAnswer = window.__ANSWER__.artifact_reveal;
  let reveals = 0;
  window.__ANSWER__.artifact_reveal = () => { reveals += 1; };
  clone.querySelector(".artifact-strip-reveal").click();
  await pause();
  seen.cloneReveals = reveals;
  window.__ANSWER__.artifact_reveal = revealAnswer;
  window.__ANSWER__.artifact_versions = answer;
  clone.remove();
  strip.remove();
  // 발행물의 지난 판에 단 주석(t-3952): 본 판은 번호·불변 스냅샷·SHA로, 고칠 곳은
  // 원본으로 따로 말하고, 원본 경로로 다시 발행하라고 한다 — 스냅샷을 고치라고
  // 하지 않는다. 「현재 파일」은 가장 새 번호의 판이다.
  const published = {
    url: pathAsFileUrl("/data/artifacts/pages/p-deck/v1/index.html"),
    artifact: {
      ...artifactStripFacts({
        id: "p-deck", title: "Deck", path: "/data/artifacts/pages/p-deck/index.html",
        version: 2, source_path: "/src/deck.html",
      }),
      versions: [
        { n: 1, path: "/data/artifacts/pages/p-deck/v1/index.html", sha256: "a".repeat(64) },
        { n: 2, path: "/data/artifacts/pages/p-deck/v2/index.html", sha256: "b".repeat(64) },
      ],
      version: 1,
    },
  };
  seen.oldFeedback = artifactFeedbackContext(published);
  published.artifact.version = null;
  published.url = pathAsFileUrl("/data/artifacts/pages/p-deck/index.html");
  seen.currentFeedback = artifactFeedbackContext(published);
  browserAnnotations.set("review-label", [{ comment: "tighten", intent: "change", tag: "h1" }]);
  seen.annotated = formatAnnotationsText("review-label", published.url, seen.currentFeedback).split("\n");
  browserAnnotations.delete("review-label");
  return seen;
});
ok("feedback on an older publication names its immutable version, digest and editable source, never the snapshot as the thing to edit",
  review.oldFeedback.includes("p-deck") &&
    review.oldFeedback.includes("/data/artifacts/pages/p-deck/v1/index.html") &&
    review.oldFeedback.includes("a".repeat(64)) && !review.oldFeedback.includes("b".repeat(64)) &&
    review.oldFeedback.includes("zerocode-artifact publish --file-path /src/deck.html") &&
    !review.oldFeedback.includes("--file-path /data/artifacts") &&
    !review.currentFeedback.includes("b".repeat(64)) &&
    review.currentFeedback.includes("/data/artifacts/pages/p-deck/index.html") &&
    review.annotated[1] === review.currentFeedback.split("\n")[0],
  JSON.stringify(review));
ok("a strip born in English returns its Share and Finder labels to Korean", review.englishShare === "Share" && review.koreanShare === "공유" && review.koreanReveal === "Finder에서 보기", JSON.stringify(review));
ok("an open strip refreshes saved versions and every numbered choice opens its snapshot", review.staleV2Path === "/versions/review/2/page.md" && review.versionsAfterSave.includes("3") && review.selectionAfterSave === "2", JSON.stringify(review));
ok("retention preserves the visible snapshot label and an older response cannot replace newer versions", review.oldReplyIgnored && review.retiredSelection, JSON.stringify(review));
ok("a cloned split-leaf strip wires its actions once when painted", review.clonePicks === 1 && review.cloneReveals === 1, JSON.stringify(review));

const publication = await page.evaluate(async () => {
  const oldOpen = window.__ANSWER__.open_browser_pane;
  const oldVersions = window.__ANSWER__.artifact_versions;
  let opened;
  window.__ANSWER__.artifact_versions = () => [{ n: 2, path: "/pub/v2/index.html", sha256: "b".repeat(64) }];
  window.__ANSWER__.open_browser_pane = args => { opened = args.url; return "browser-publication-proof"; };
  try {
    await openArtifactPage({ id: "p-proof", kind: "page", title: "Published", path: "/pub/index.html", version: 2, source_path: "/src/proof.html" });
    const tab = tabs.find(one => one.label === "browser-publication-proof");
    const strip = docHost(tab.pane, "browser").querySelector(".artifact-strip");
    return { opened, selected: tab.artifact.version,
      mutableChoice: Boolean(strip.querySelector('option[value="current"]')),
      shown: artifactShownPath(tab.artifact) };
  } finally {
    const tab = tabs.find(one => one.label === "browser-publication-proof");
    if (tab) dropTab(tab.id);
    if (oldOpen === undefined) delete window.__ANSWER__.open_browser_pane; else window.__ANSWER__.open_browser_pane = oldOpen;
    if (oldVersions === undefined) delete window.__ANSWER__.artifact_versions; else window.__ANSWER__.artifact_versions = oldVersions;
  }
});
ok("opening a publication pins preview and actions to its immutable version",
  publication.opened.endsWith("/pub/v2/index.html") && publication.selected === 2 &&
  !publication.mutableChoice && publication.shown === "/pub/v2/index.html", JSON.stringify(publication));

const freshDraft = await page.evaluate(async () => {
  openArtifacts();
  await window.__PAINTED__();
  const names = ["agent_terms", "launch_agent_tab", "send_prompt"];
  const saved = new Map(names.map(name => [name, window.__ANSWER__[name]]));
  const launched = [], sent = [];
  const term = Math.max(0, ...tabs.filter(tab => tab.kind === "term").map(tab => tab.term ?? 0)) + 1;
  window.__ANSWER__.agent_terms = () => [];
  window.__ANSWER__.launch_agent_tab = args => { launched.push(args); return term; };
  window.__ANSWER__.send_prompt = args => { sent.push(args); return null; };
  try {
    await openSendToAgent(artifactsView().querySelector(".artifacts-new"), "owned unsent artifact draft", null, { submit: false });
    const choice = document.querySelector("#note-pop .note-pop-row");
    choice.click(); choice.click();
    await new Promise(done => setTimeout(done, 100));
    return { launched, sent };
  } finally {
    dropTab(termTabId(term)); paneAgents.delete(term); closeNoteSend();
    for (const [name, value] of saved) {
      if (value === undefined) delete window.__ANSWER__[name]; else window.__ANSWER__[name] = value;
    }
  }
});
ok("a fresh-agent artifact draft is staged once without submitting a launch prompt",
  freshDraft.launched.length === 1 && freshDraft.launched[0].prompt === "" &&
  freshDraft.sent.length === 1 && freshDraft.sent[0].submit === false &&
  freshDraft.sent[0].text === "owned unsent artifact draft", JSON.stringify(freshDraft));


}

export async function testArtifactStudio(page, ok) {
  const seen = await page.evaluate(async () => {
    const savedAgents = new Map(paneAgents), savedSessions = new Map(paneSessions);
    paneAgents.clear(); paneSessions.clear();
    dropTab("artifacts");
    openArtifacts();
    await window.__PAINTED__();
    const view = artifactsView();
    const form = view.querySelector(".artifacts-studio-form");
    const brief = view.querySelector(".artifacts-studio-brief");
    const status = view.querySelector(".artifacts-studio-status");
    const send = view.querySelector(".artifacts-studio-send");
    const choice = view.querySelector('[data-surface="monitor"].artifacts-studio-choice');
    const paste = window.__ANSWER__.term_paste, key = window.__ANSWER__.term_key;
    const term = 3235, out = { keys: 0, pastes: [] };
    paneAgents.clear(); paneSessions.clear();
    choice.click();
    brief.value = "우리 팀의 진행 상황과 실패 원인을 탐색하는 화면";
    brief.dispatchEvent(new Event("input", { bubbles: true }));
    out.noSeat = send.disabled && status.textContent.length > 0;
    mountTermTab(term, { agent: "zo", worktree: activeWorktreePath }, { focus: false });
    paneAgents.set(term, "zo");
    openArtifacts();
    paintArtifactStudio(view);
    const destination = view.querySelector(".artifacts-studio-destination");
    destination.value = String(term); destination.dispatchEvent(new Event("input", { bubbles: true }));
    out.ready = !send.disabled && status.textContent === "" && !form.hidden;
    out.selected = form.dataset.surface === "monitor"
      && view.querySelector(".artifacts-studio-title").textContent === "대시보드"
      && !view.querySelector(".artifacts-studio").hidden;
    brief.value = "  "; brief.dispatchEvent(new Event("input", { bubbles: true }));
    out.blankDisabled = send.disabled;
    brief.value = "진행 상황과 실패 원인을 탐색하는 화면";
    view.querySelector(".artifacts-studio-expression").value = "quiet";
    view.querySelector(".artifacts-studio-reference").value = "우리 브랜드 가이드.md";
    brief.dispatchEvent(new Event("input", { bubbles: true }));
    let finish;
    window.__ANSWER__.term_key = () => { out.keys++; };
    window.__ANSWER__.term_paste = args => {
      out.pastes.push({ ...args });
      return new Promise(resolve => { finish = resolve; });
    };
    const pending = sendArtifactStudio(view);
    await Promise.resolve();
    await sendArtifactStudio(view);
    out.coalesced = out.pastes.length === 1 && send.disabled;
    // A user can keep editing during IPC; the old draft must not mark the new one sent.
    brief.value = "새로 수정한 요청";
    brief.dispatchEvent(new Event("input", { bubbles: true }));
    if (typeof finish !== "function") throw new Error(JSON.stringify({ ...out, destination: view.querySelector(".artifacts-studio-destination").value, seats: artifactDraftSeats().map(seat => seat.term), error: el("error-toast")?.textContent }));
    finish(null); await pending;
    out.noStaleSuccess = status.textContent === "" && brief.value === "새로 수정한 요청";
    openArtifacts();
    window.__ANSWER__.term_paste = args => { out.pastes.push({ ...args }); return null; };
    await sendArtifactStudio(view);
    out.success = status.textContent === t("artifacts.studio.sent", "에이전트 입력줄에 초안을 넣었습니다.")
      && activeTabId === tabOfTerm(term)?.id;
    openArtifacts();
    window.__ANSWER__.term_paste = () => { throw new Error("fixture rejected draft"); };
    await sendArtifactStudio(view);
    out.rejectedNotSent = status.textContent === "" && !send.disabled && activeTabId === artifactsTab()?.id;
    brief.focus();
    brief.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    out.collapsed = form.hidden && view.querySelector(".artifacts-studio").hidden
      && document.activeElement === view.querySelector(".artifacts-new");
    choice.click();
    out.retained = brief.value === "새로 수정한 요청" && document.activeElement === brief;
    // 새 에이전트가 판에 앉으면(hook:agent) 서 있는 스튜디오의 목록이 따라온다 —
    // 다시 열거나 필터를 만질 필요 없이. 판이 닫히면 그 자리도 빠진다.
    const arrived = term + 1;
    mountTermTab(arrived, { agent: "claude", worktree: activeWorktreePath }, { focus: false });
    const seatsBefore = [...destination.options].map(option => option.value);
    for (const handler of window.__LISTENERS__["hook:agent"] ?? []) {
      handler({ payload: { term: arrived, state: "working", agent: "claude", model: "claude-fable-5-1" } });
    }
    await window.__PAINTED__();
    await new Promise(resolve => setTimeout(resolve, 200));
    const seatsAfter = [...destination.options].map(option => option.value);
    out.seatFollowsArrival = !seatsBefore.includes(String(arrived)) && seatsAfter.includes(String(arrived))
      && seatsAfter.includes(String(term)) && destination.value === String(term);
    dropTab(tabOfTerm(arrived)?.id);
    await window.__PAINTED__();
    await new Promise(resolve => setTimeout(resolve, 200));
    out.seatLeavesWithPane = ![...destination.options].some(option => option.value === String(arrived));
    dropTab(tabOfTerm(term)?.id);
    paneAgents.clear(); paneSessions.clear();
    for (const [id, value] of savedAgents) paneAgents.set(id, value);
    for (const [id, value] of savedSessions) paneSessions.set(id, value);
    window.__ANSWER__.term_paste = paste; window.__ANSWER__.term_key = key;
    closeArtifactStudio(view);
    return out;
  });
  ok("studio requires a live agent and a nonblank brief, and clears the missing-seat explanation on recovery",
    seen.noSeat && seen.ready && seen.blankDisabled && seen.selected, JSON.stringify(seen));
  ok("studio drafts preserve purpose, expression, reference and user content without submitting Enter",
    seen.pastes[0]?.term === 3235 && ["대시보드", "간결하고 차분하게", "우리 브랜드 가이드.md", "진행 상황과 실패 원인을 탐색하는 화면", "artifact-design"].every(word => seen.pastes[0]?.text.includes(word)) && seen.keys === 0,
    JSON.stringify(seen.pastes));
  ok("studio coalesces pending sends, preserves edits, and never calls a rejected or outdated draft successful",
    seen.coalesced && seen.noStaleSuccess && seen.success && seen.rejectedNotSent, JSON.stringify(seen));
  ok("studio Escape collapses with focus restored and reopening retains the brief", seen.collapsed && seen.retained, JSON.stringify(seen));
  ok("the destination list follows an agent arriving in a new pane and a pane leaving, without reopening the view",
    seen.seatFollowsArrival && seen.seatLeavesWithPane, JSON.stringify({ seatFollowsArrival: seen.seatFollowsArrival, seatLeavesWithPane: seen.seatLeavesWithPane }));
}

/* 「새로 만들기 ▾」(2026-09-28): 스튜디오 띠가 첫 화면의 229px를 쓰던 것을 메뉴
 * 하나가 대신한다. 첫 화면에 스튜디오는 없고, 메뉴는 목적 넷과 빈 초안 하나에
 * 모두 닿으며 키보드로 돈다. 목적을 고르면 그 목적의 초안 폼만 갤러리 위에 서고,
 * 닫으면 갤러리가 그 높이를 되받으며 초안의 글은 남는다. 그리고 1280×860에서
 * 첫 화면은 카드 두 줄을 온전히 보인다. */
export async function testArtifactNewMenu(page, ok) {
  await page.setViewportSize({ width: 1280, height: 860 });
  const seen = await page.evaluate(async () => {
    dropTab("artifacts");
    artifactFilter.tab = "pages";
    artifactFilter.query = "";
    window.__ARTIFACTS__ = { count: 0, gallery: 18 };
    artifactAskedAt = 0;
    openArtifacts();
    await window.__PAINTED__();
    await window.__UNTIL__(() => !artifactAsking && artifactRows.size === 18, "18-row gallery");
    await window.__PAINTED__();
    const view = artifactsView();
    const studio = view.querySelector(".artifacts-studio");
    const menu = view.querySelector(".artifacts-new-menu");
    const button = view.querySelector(".artifacts-new");
    const grid = view.querySelector(".artifacts-grid");
    const out = {};
    out.noBanner = studio.hidden && studio.getBoundingClientRect().height === 0;
    const frame = grid.getBoundingClientRect();
    const whole = [...view.querySelectorAll(".artifact-card")].filter((card) => {
      const box = card.getBoundingClientRect();
      return box.top >= frame.top - 0.5 && box.bottom <= frame.bottom + 0.5;
    });
    out.wholeRows = new Set(whole.map((card) => Math.round(card.getBoundingClientRect().top))).size;
    out.wholeCards = whole.length;
    out.closed = menu.hidden && button.getAttribute("aria-haspopup") === "menu" && button.getAttribute("aria-expanded") === "false";
    const gridBefore = frame.height;
    button.click();
    await window.__PAINTED__();
    const items = [...menu.querySelectorAll('[role="menuitem"]')];
    out.opened = !menu.hidden && button.getAttribute("aria-expanded") === "true";
    out.items = items.map((item) => item.dataset.surface ?? "blank").join(",");
    out.firstFocused = document.activeElement === items[0];
    const key = (name) => menu.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true }));
    key("ArrowDown");
    out.downMoves = document.activeElement === items[1];
    key("End");
    out.endIsBlank = document.activeElement === items[items.length - 1];
    key("ArrowDown");
    out.wraps = document.activeElement === items[0];
    const menuBox = menu.getBoundingClientRect(), buttonBox = button.getBoundingClientRect();
    out.underButton = menuBox.top >= buttonBox.bottom - 1 && menuBox.right <= window.innerWidth;
    document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await new Promise((done) => setTimeout(done, 250));
    out.escapeCloses = [menu.hidden, document.activeElement === button, button.getAttribute("aria-expanded")].join(",");
    // 목적 넷 모두 제 폼에 닿는다 — 메뉴를 열고 고르는 사람의 길 그대로.
    out.reached = [];
    for (const surface of ARTIFACT_STUDIO_SURFACES) {
      button.click();
      await window.__PAINTED__();
      menu.querySelector(`[data-surface="${surface.id}"]`).click();
      await window.__PAINTED__();
      const title = studio.querySelector(".artifacts-studio-title").textContent;
      if (!studio.hidden && !view.querySelector(".artifacts-studio-form").hidden && title === surface.title.word
        && document.activeElement === view.querySelector(".artifacts-studio-brief")) out.reached.push(surface.id);
    }
    await new Promise((done) => setTimeout(done, 250));
    out.menuClosedAfterPick = menu.hidden;
    const brief = view.querySelector(".artifacts-studio-brief");
    brief.value = "닫아도 남는 초안";
    brief.dispatchEvent(new Event("input", { bubbles: true }));
    out.gridShrank = grid.getBoundingClientRect().height < gridBefore;
    brief.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await window.__PAINTED__();
    out.closedBack = studio.hidden && Math.abs(grid.getBoundingClientRect().height - gridBefore) < 1
      && document.activeElement === button;
    button.click();
    await window.__PAINTED__();
    menu.querySelector('[data-surface="operate"]').click();
    await window.__PAINTED__();
    out.briefKept = brief.value === "닫아도 남는 초안";
    closeArtifactStudio(view);
    // 빈 초안은 받을 에이전트를 묻는다(t-3952) — 초안 폼을 세우지 않는다.
    button.click();
    await window.__PAINTED__();
    menu.querySelector(".artifacts-new-blank").click();
    await new Promise((done) => setTimeout(done, 120));
    out.blankAsksRecipient = document.querySelectorAll("#note-pop .note-pop-row").length > 0 && studio.hidden;
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await new Promise((done) => setTimeout(done, 200));
    return out;
  });
  ok("the first screen has no studio band and shows two whole rows of cards at 1280×860",
    seen.noBanner && seen.wholeRows >= 2 && seen.wholeCards >= 4, JSON.stringify(seen));
  ok("「새로 만들기 ▾」 is a keyboard menu under its button that reaches every studio template and a blank draft",
    seen.closed && seen.opened && seen.items === "compare,monitor,story,operate,blank" && seen.firstFocused
      && seen.downMoves && seen.endIsBlank && seen.wraps && seen.underButton && seen.escapeCloses === "true,true,false"
      && seen.reached.join(",") === "compare,monitor,story,operate" && seen.menuClosedAfterPick
      && seen.blankAsksRecipient, JSON.stringify(seen));
  ok("a picked purpose stands its brief above the gallery, and closing hands the height back and keeps the brief",
    seen.gridShrank && seen.closedBack && seen.briefKept, JSON.stringify(seen));
}

export async function testArtifactStudioLayout(page, ok) {
  const axeModule = createRequire(import.meta.url)("@axe-core/playwright");
  const AxeBuilder = axeModule.default ?? axeModule;
  const viewport = page.viewportSize();
  const settings = await page.evaluate(() => ({ locale, theme: document.documentElement.dataset.theme }));
  await page.emulateMedia({ reducedMotion: "reduce" });
  for (const width of [720, 1280]) {
    await page.setViewportSize({ width, height: 860 });
    for (const theme of ["dark", "light"]) {
      const seen = await page.evaluate(async ({ theme, width }) => {
        locale = theme === "light" ? "en" : "ko";
        document.documentElement.dataset.theme = theme;
        openArtifacts();
        const view = artifactsView();
        applyLocale(view);
        openArtifactStudio(view, "compare");
        await window.__PAINTED__();
        const studio = view.querySelector(".artifacts-studio");
        const choices = [...view.querySelectorAll(".artifacts-new-menu .artifacts-studio-choice")];
        const input = studio.querySelector("textarea");
        const send = studio.querySelector(".artifacts-studio-send");
        send.scrollIntoView({ block: "nearest" });
        const box = studio.getBoundingClientRect(), action = send.getBoundingClientRect();
        const result = {
          width, theme, studioWidth: box.width,
          noOverflow: studio.scrollWidth <= studio.clientWidth + 1
            && choices.every(node => node.scrollWidth <= node.clientWidth + 1),
          actionsReachable: action.top >= box.top && action.bottom <= box.bottom + 1,
          reducedMotion: choices.every(node => getComputedStyle(node).transitionDuration === "0s"),
          namedInputs: [...studio.querySelectorAll("input,textarea,select")].every(node => node.labels?.length > 0),
          localized: studio.querySelector(".artifacts-studio-title").textContent.includes(theme === "light" ? "Compare" : "비교")
            && choices[0].textContent.includes(theme === "light" ? "Compare" : "비교"),
          fieldStyled: getComputedStyle(input).backgroundColor === "rgba(0, 0, 0, 0)",
          purposeMaterials: new Set(choices.map(node => getComputedStyle(node.querySelector(".artifacts-studio-glyph")).backgroundColor)).size,
        };
        return result;
      }, { theme, width });
      ok(`artifact studio reflows with usable controls in ${theme} at ${width}px`,
        seen.studioWidth > 0 && seen.noOverflow && seen.actionsReachable && seen.reducedMotion && seen.namedInputs && seen.localized && seen.fieldStyled && seen.purposeMaterials > 1,
        JSON.stringify(seen));
      const violations = await axeViolations(page, AxeBuilder, ".artifacts-studio");
      ok(`artifact studio accessibility in ${theme} at ${width}px`, violations.length === 0, JSON.stringify(violations));
      await page.evaluate(() => closeArtifactStudio(artifactsView()));
    }
  }
  await page.setViewportSize(viewport);
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await page.evaluate(saved => {
    locale = saved.locale;
    if (saved.theme === undefined) delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = saved.theme;
    applyLocale();
  }, settings);
}

export async function testArtifactCatalog(page, ok, outputDir) {
await mkdir(outputDir, { recursive: true });
await installHarnessWaits(page);
/* ---- 아티팩트 뷰 (t-2720, Orca 갭 #1) --------------------------------------
 *
 * 에이전트가 만든 것은 출처와 함께 남는다. 스토어·스캔·보존은 Rust가 시험하고
 * (`artifact_runtime`의 픽스처들), 여기서 재는 것은 창이 그 답으로 무엇을
 * 그리는가다 — 사이드바 문과 탭, 카드의 수와 옷, 필터가 백엔드를 다시 부르지
 * 않고 노드를 다시 만들지도 않는다는 것, 서랍의 미리보기 세 종류, 출처 링크의
 * 왕복, 두 테마의 대비, 그리고 천 개의 행에서 첫 그림이 서는 시간. */
const artifactsOpen = await page.evaluate(async () => {
  artifactFilter.tab = "pages";
  artifactTabPicked = false;
  window.__ARTIFACTS__ = { count: 12 };
  window.__ARTIFACT_ASKS__ = 0;
  const entryHidden = el("nav-artifacts").hidden;
  el("nav-artifacts").click();
  await window.__PAINTED__();
  await new Promise((done) => setTimeout(done, 120));
  const view = artifactsView();
  const cards = [...view.querySelectorAll(".artifact-card")];
  const first = cards[0];
  return {
    entryHidden,
    tab: [...document.querySelectorAll(".tab")].map((one) => one.textContent).join("|"),
    asks: window.__ARTIFACT_ASKS__,
    askedFilter: window.__ARTIFACT_ASKED__?.filter ?? null,
    cards: cards.length,
    rows: artifactOrder.length,
    landed: artifactFilter.tab,
    tabs: [...view.querySelectorAll("[data-artifact-tab]")].filter((one) => !one.hidden)
      .map((one) => `${one.dataset.artifactTab}:${one.querySelector(".artifacts-tab-count").textContent}`).join(","),
    held: artifactRows.size,
    firstTitle: first?.querySelector(".artifact-card-title")?.textContent ?? "",
    firstMaker: first?.querySelector(".artifact-card-maker-mark")?.dataset.tip ?? "",
    firstBranch: first?.querySelector(".artifact-card-maker-branch")?.textContent ?? "",
    claude: agentName("claude"),
    firstWhen: first?.querySelector(".artifact-card-when")?.textContent ?? "",
    firstKind: first?.dataset.kind ?? "",
    statHidden: view.querySelector(".artifacts-stat").hidden,
    empty: view.querySelector(".artifacts-empty").hidden,
    catalogs: ["en", "ja", "zh", "es"].map((code) => CATALOG[code]["artifacts.title"]),
    fallback: t("artifacts.title", "아티팩트"),
    gridScrollsX: view.querySelector(".artifacts-grid").scrollWidth
      > view.querySelector(".artifacts-grid").clientWidth,
    // 썸네일은 보이는 스크린샷 카드만 청한다 — 열두 장 중 둘.
    thumbAsks: (window.__PREVIEW_ASKS__ ?? []).slice().sort(),
    visibleScreenshots: cards.filter(card => card.dataset.kind === "screenshot").map(card => card.dataset.id).sort(),
  };
});
ok(
  "the artifacts entry opens a doc tab that draws the catalog as keyed cards with kind, maker and time, landing on the first tab with rows when there are no pages",
  artifactsOpen.entryHidden === false &&
    artifactsOpen.tab.includes("아티팩트") &&
    artifactsOpen.asks === 1 &&
    artifactsOpen.askedFilter !== null &&
    artifactsOpen.held === 12 &&
    artifactsOpen.landed === "reports" &&
    artifactsOpen.tabs === "pages:0,reports:2,evidence:4,other:6" &&
    artifactsOpen.rows === 2 && artifactsOpen.cards === 2 &&
    artifactsOpen.firstTitle !== "" &&
    artifactsOpen.firstMaker.includes(artifactsOpen.claude) && artifactsOpen.firstMaker.includes("fable") &&
    artifactsOpen.firstBranch !== "" &&
    artifactsOpen.firstWhen !== "" &&
    artifactsOpen.firstKind === "report" &&
    artifactsOpen.statHidden === true &&
    artifactsOpen.empty === true &&
    new Set(artifactsOpen.catalogs).size === 4 &&
    artifactsOpen.catalogs.every((word) => word && word !== artifactsOpen.fallback) &&
    !artifactsOpen.gridScrollsX &&
    artifactsOpen.thumbAsks.join(",") === artifactsOpen.visibleScreenshots.join(","),
  JSON.stringify(artifactsOpen),
);

/* 서랍의 미리보기 셋 — 마크다운은 그려진 제목으로, 이미지는 `data:` URL로,
 * 텍스트는 `<pre>`로 — 그리고 한 아티팩트의 미리보기는 한 번만 묻는다. */
const artifactDrawer = await page.evaluate(async () => {
  window.__PREVIEW_ASKS__ = [];
  const view = artifactsView();
  // 한 행의 카드는 제 탭에 선다 — 고르는 문(`openArtifacts({ select })`)이 탭을 옮긴다.
  const card = (id) => {
    openArtifacts({ select: id });
    selectArtifact(view, id, { reveal: true });
    return view.querySelector(`.artifact-card[data-id="${id}"]`);
  };
  const seen = {};
  card("art-0").click();
  await new Promise((done) => setTimeout(done, 60));
  seen.markdown = view.querySelector(".artifact-preview-md h1")?.textContent ?? "";
  seen.markdownBold = view.querySelector(".artifact-preview-md strong")?.textContent ?? "";
  seen.selected = view.querySelector(".artifact-card.is-selected")?.dataset.id ?? "";
  seen.meta = [...view.querySelectorAll(".artifact-meta [data-meta]")]
    .map((row) => `${row.dataset.meta}=${row.textContent.trim() !== ""}`).join(",");
  seen.jumps = [...view.querySelectorAll("[data-artifact-jump]")].map((one) => one.dataset.artifactJump);
  seen.actions = [...view.querySelectorAll("[data-artifact-action]")].map((one) => one.dataset.artifactAction);
  card("art-1").click();
  await new Promise((done) => setTimeout(done, 60));
  const img = view.querySelector("img.artifact-preview-image");
  seen.image = Boolean(img) && !img.hidden && img.src.startsWith("data:image/gif");
  card("art-4").click();
  await new Promise((done) => setTimeout(done, 60));
  seen.text = view.querySelector("pre.artifact-preview-text")?.textContent ?? "";
  card("art-3").click();
  await new Promise((done) => setTimeout(done, 60));
  seen.none = view.querySelector(".artifact-preview-none").hidden === false;
  card("art-0").click();
  await new Promise((done) => setTimeout(done, 60));
  seen.asks = window.__PREVIEW_ASKS__.slice();
  return seen;
});
ok(
  "the drawer previews markdown, image and text, lists the meta, and asks each preview once (a thumbnail already asked is not asked again)",
  artifactDrawer.markdown.includes("보고서 0") &&
    artifactDrawer.markdownBold === "굵게" &&
    artifactDrawer.selected === "art-0" &&
    artifactDrawer.meta.includes("kind=true") &&
    artifactDrawer.meta.includes("path=true") &&
    artifactDrawer.meta.includes("agent=true") &&
    artifactDrawer.jumps.join(",") === "card,task,worktree" &&
    artifactDrawer.actions.join(",") === "open,reveal,copy,delete" &&
    artifactDrawer.image === true &&
    artifactDrawer.text.includes('{"n":4}') &&
    artifactDrawer.none === true &&
    ["art-0", "art-4", "art-3"].every(id => artifactDrawer.asks.includes(id)) &&
    new Set(artifactDrawer.asks).size === artifactDrawer.asks.length,
  JSON.stringify(artifactDrawer),
);

/* 액션 넷과 키보드: 열기·Finder·경로 복사는 백엔드의 문이고, 삭제는 확인 대화가
 * 먼저 서며 거절하면 아무것도 묻지 않는다. ↓는 다음 카드, Enter는 열기, ⌘C는 경로. */
const artifactActions = await page.evaluate(async () => {
  const view = artifactsView();
  const seen = {};
  window.__ARTIFACT_OPENED__ = null;
  window.__ARTIFACT_DELETED__ = null;
  window.__ARTIFACT_COPIED__ = null;
  view.querySelector('[data-artifact-action="open"]').click();
  await new Promise((done) => setTimeout(done, 30));
  seen.opened = window.__ARTIFACT_OPENED__?.id ?? null;
  view.querySelector('[data-artifact-action="reveal"]').click();
  await new Promise((done) => setTimeout(done, 30));
  seen.revealed = window.__ARTIFACT_REVEALED__?.id ?? null;
  view.querySelector('[data-artifact-action="copy"]').click();
  await new Promise((done) => setTimeout(done, 30));
  seen.copied = window.__ARTIFACT_COPIED__?.id ?? null;
  view.querySelector('[data-artifact-action="delete"]').click();
  await new Promise((done) => setTimeout(done, 30));
  seen.askedFirst = !el("ask-scrim").hidden;
  seen.askTitle = el("ask-title").textContent;
  el("ask-no").click();
  await new Promise((done) => setTimeout(done, 30));
  seen.deniedNoop = window.__ARTIFACT_DELETED__ === null;
  view.querySelector('[data-artifact-action="delete"]').click();
  await new Promise((done) => setTimeout(done, 30));
  el("ask-yes").click();
  await new Promise((done) => setTimeout(done, 60));
  seen.deleted = window.__ARTIFACT_DELETED__;
  seen.goneFromGrid = view.querySelector('.artifact-card[data-id="art-0"]') === null;
  const grid = view.querySelector(".artifacts-grid");
  grid.focus();
  grid.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
  await new Promise((done) => setTimeout(done, 30));
  seen.afterDown = view.querySelector(".artifact-card.is-selected")?.dataset.id ?? "";
  window.__ARTIFACT_OPENED__ = null;
  grid.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  await new Promise((done) => setTimeout(done, 30));
  seen.enterOpened = window.__ARTIFACT_OPENED__?.id ?? null;
  window.__ARTIFACT_COPIED__ = null;
  grid.dispatchEvent(new KeyboardEvent("keydown", { key: "c", metaKey: true, ctrlKey: true, bubbles: true }));
  await new Promise((done) => setTimeout(done, 30));
  seen.chordCopied = window.__ARTIFACT_COPIED__?.id ?? null;
  return seen;
});
ok(
  "the four actions reach the backend, delete asks first and a refusal is a no-op, and the keyboard selects, opens and copies",
  artifactActions.opened === "art-0" &&
    artifactActions.revealed === "art-0" &&
    artifactActions.copied === "art-0" &&
    artifactActions.askedFirst &&
    artifactActions.askTitle !== "" &&
    artifactActions.deniedNoop &&
    artifactActions.deleted?.id === "art-0" &&
    artifactActions.deleted?.confirmed === true &&
    artifactActions.goneFromGrid &&
    artifactActions.afterDown !== "" &&
    artifactActions.enterOpened === artifactActions.afterDown &&
    artifactActions.chordCopied === artifactActions.afterDown,
  JSON.stringify(artifactActions),
);

/* 출처 링크의 왕복. 서랍의 「카드」는 보드를 열고 그 워커의 카드를 고르고,
 * 보드 카드의 「아티팩트 N」 칩은 그 워커로 걸러진 아티팩트 뷰를 연다;
 * 워크트리 행의 칩도 같은 문이다. 어느 쪽도 백엔드를 다시 묻지 않는다. */
const artifactRoundTrip = await page.evaluate(async () => {
  const seen = {};
  window.__LEDGER__ = [
    { run: "run-1", worker: "w-2", agent: "codex", state: "working", ledger: "working", hearing: "hook",
      hearing_at: 0, checkout: "/tmp/zerocode-window-test/wt-b", task: "artifacts view", task_id: "t-2",
      reported: false, review: null, term: 77, at: Date.now() },
  ];
  window.__PANES__ = [{ term: 77, agent: "codex", state: "working", at: Date.now(), resumable: false }];
  // The board's columns are the backend's grouping, stubbed from `__COLUMNS__`:
  // one working card for the seat the ledger names.
  window.__COLUMNS_BEFORE_ARTIFACTS__ = window.__COLUMNS__;
  window.__COLUMNS__ = [{ bucket: "working", cards: [
    { pane: "term:77", agent: "codex", state: "working", heading: "Codex", worktree: "wt-b",
      project: "", task: "artifacts view", unseen: false, changed_at: 1000, at: 1000 },
  ] }];
  await refreshPaneLedger();
  await refreshArtifactCounts();
  const view = artifactsView();
  openArtifacts({ select: "art-2" });
  await window.__PAINTED__();
  selectArtifact(view, "art-2", { reveal: true });
  view.querySelector('.artifact-card[data-id="art-2"]').click();
  await new Promise((done) => setTimeout(done, 40));
  const asksBefore = window.__ARTIFACT_ASKS__;
  view.querySelector('[data-artifact-jump="card"]').click();
  await new Promise((done) => setTimeout(done, 80));
  seen.boardActive = activeTabId === "board";
  seen.selectedKey = agentGraphSelectedKey;
  const boardView = docHost(boardTab().pane, "board");
  seen.entities = [...(agentGraphModels.get(boardView)?.entities.keys() ?? [])]
    .filter((key) => key.startsWith("agent:"));
  // The ledger row's 「보고서 보기」: the inspector names the worker's newest
  // report, and opening it lands on that card, filtered to the worker.
  const reportButton = boardView.querySelector(".agent-inspector-report:not([hidden])");
  seen.reportButton = reportButton?.textContent ?? "";
  // The task inspector exposes the newest report directly; its current layout
  // does not embed the former graph inspector card.
  reportButton?.click();
  await new Promise((done) => setTimeout(done, 80));
  seen.reportSelected = artifactSelectedId;
  seen.reportShown = artifactsView()?.querySelector(".artifacts-detail")?.dataset.artifactId ?? "";
  // Back to the board for the chip half.
  leavePagesForStage();
  openBoard();
  await paintBoardView(boardTab(), { force: true });
  // Back through the card's chip: a card drawn for the seat the ledger names.
  const card = boardCardNode(
    { pane: "term:77", agent: "codex", state: "working", ledger: "working", heading: "wt-b", worktree: "wt-b",
      project: "", task: "artifacts view", you: "", said: "", ask: "", ask_prompt: null, approval: null,
      parent: "", lineage: { depth: 0, root: "77", parent: null }, unseen: false, changed_at: 0,
      at: Date.now(), autonomy: null },
    Date.now(), "working", null, null,
  );
  const chip = card.querySelector(".board-card-artifacts");
  seen.chip = chip?.textContent ?? "";
  artifactSelectedId = null;
  chip?.click();
  await new Promise((done) => setTimeout(done, 80));
  seen.viewActive = activeTabId === "artifacts";
  seen.originChip = artifactsView().querySelector(".artifacts-origin-chip")?.textContent ?? "";
  const tabSum = () => [...artifactTabCounts.values()].reduce((sum, n) => sum + n, 0);
  seen.filtered = artifactOrder.length;
  seen.filteredAll = tabSum();
  seen.filteredTab = artifactFilter.tab;
  seen.filteredWorkers = new Set([...artifactsView().querySelectorAll(".artifact-card")]
    .map((one) => artifactRows.get(one.dataset.id)?.origin.worker)).size;
  seen.asksAfter = window.__ARTIFACT_ASKS__ - asksBefore;
  // The worktree door: the sidebar row's chip.
  const wt = makeWorktreeNode({ path: "/tmp/zerocode-window-test/wt-b", branch: "wt-b", is_main: false }, new Map());
  const wtChip = wt.querySelector(".wt-artifacts");
  seen.wtChip = wtChip?.textContent ?? "";
  wtChip?.click();
  await new Promise((done) => setTimeout(done, 80));
  seen.wtFiltered = artifactOrder.length;
  seen.wtFilteredAll = tabSum();
  seen.wtTab = artifactFilter.tab;
  seen.wtOriginChip = artifactsView().querySelector(".artifacts-origin-chip")?.textContent ?? "";
  // And clearing the origin brings everything back, still without asking.
  artifactsView().querySelector(".artifacts-origin-clear").click();
  await new Promise((done) => setTimeout(done, 40));
  seen.cleared = tabSum();
  seen.asksEnd = window.__ARTIFACT_ASKS__ - asksBefore;
  return seen;
});
ok(
  "origin links round-trip: drawer → board card, board card chip → filtered view, worktree chip → filtered view, without asking the backend again",
  artifactRoundTrip.boardActive &&
    artifactRoundTrip.selectedKey === "agent:term:77" &&
    artifactRoundTrip.reportButton.includes("보고서") &&
    artifactRoundTrip.reportSelected === "art-6" &&
    artifactRoundTrip.reportShown === "art-6" &&
    /3/.test(artifactRoundTrip.chip) &&
    artifactRoundTrip.viewActive &&
    // The chip wears the card's own name for the seat, not the raw worker id.
    artifactRoundTrip.originChip.includes("artifacts view") &&
    // A scoped arrival lands on the first tab holding its rows; the tabs count the rest.
    artifactRoundTrip.filteredAll === 3 &&
    artifactRoundTrip.filteredTab === "reports" && artifactRoundTrip.filtered === 1 &&
    artifactRoundTrip.filteredWorkers === 1 &&
    artifactRoundTrip.asksAfter === 0 &&
    /6/.test(artifactRoundTrip.wtChip) &&
    artifactRoundTrip.wtFilteredAll === 6 &&
    artifactRoundTrip.wtTab === "evidence" && artifactRoundTrip.wtFiltered === 2 &&
    artifactRoundTrip.wtOriginChip.includes("wt-b") &&
    artifactRoundTrip.cleared === 11 &&
    artifactRoundTrip.asksEnd === 0,
  JSON.stringify(artifactRoundTrip),
);

/* 두 테마의 대비와 두 크기의 그림. 카드의 제목 잉크와 카드의 바탕이 3:1을 넘는가를
 * 계산된 색으로 재고 — 카드는 제 바탕을 불투명하게 칠하므로 조상 합성이 끼어들
 * 자리가 없다 — 보고서에 실을 그림을 두 테마 × 두 크기로 남긴다. */
const artifactContrast = {};
for (const theme of ["dark", "light"]) {
  await page.evaluate((next) => {
    document.documentElement.setAttribute("data-theme", next);
  }, theme);
  await new Promise((done) => setTimeout(done, 700));
  artifactContrast[theme] = await page.evaluate(() => {
    const view = artifactsView();
    const card = view.querySelector(".artifact-card");
    const title = card.querySelector(".artifact-card-title");
    const origin = card.querySelector(".artifact-card-maker");
    // `rgb(...)` from a hex token, `color(srgb r g b / a)` from a color-mix —
    // the second speaks in 0..1 and is scaled to the first's 0..255.
    const channel = (text) => {
      const parts = text.match(/[\d.]+/g).map(Number);
      const unit = text.startsWith("color(srgb");
      const [r, g, b, a = 1] = unit ? parts.map((v, at) => (at < 3 ? v * 255 : v)) : parts;
      return { r, g, b, a };
    };
    const luminance = ({ r, g, b }) => {
      const lin = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; };
      return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
    };
    const contrast = (front, back) => {
      const [hi, lo] = [luminance(front), luminance(back)].sort((a, b) => b - a);
      return (hi + 0.05) / (lo + 0.05);
    };
    const back = channel(getComputedStyle(card).backgroundColor);
    return {
      opaque: back.a === 1,
      title: Math.round(contrast(channel(getComputedStyle(title).color), back) * 100) / 100,
      origin: Math.round(contrast(channel(getComputedStyle(origin).color), back) * 100) / 100,
    };
  });
  for (const [name, size] of [["wide", { width: 1998, height: 1069 }], ["narrow", { width: 1280, height: 800 }]]) {
    await page.setViewportSize(size);
    await new Promise((done) => setTimeout(done, 250));
    await page.screenshot({ path: join(outputDir, `artifacts-${theme}-${name}.png`) });
  }
}
await page.setViewportSize({ width: 1280, height: 860 });
await page.evaluate(() => document.documentElement.setAttribute("data-theme", "dark"));
ok(
  "the card's title and maker line keep 3:1 against an opaque card in both themes",
  artifactContrast.dark.opaque && artifactContrast.light.opaque &&
    artifactContrast.dark.title >= 3 && artifactContrast.light.title >= 3 &&
    artifactContrast.dark.origin >= 3 && artifactContrast.light.origin >= 3,
  JSON.stringify(artifactContrast),
);

/* 성능 계약: 천 개의 행에서 첫 그림 <150ms(다섯 라운드 중 최소, t-2412 규칙 1b),
 * 화면의 카드 노드는 보이는 만큼만(가상화), 종류 필터 전환은 노드 생성 0이고
 * 백엔드를 묻지 않으며, 검색 한 글자도 그렇다. */
const artifactScale = await page.evaluate(async () => {
  window.__ARTIFACTS__ = { count: 1000 };
  const round = async () => {
    const asksBefore = window.__ARTIFACT_ASKS__ ?? 0;
    dropTab("artifacts");
    await window.__PAINTED__();
    window.__ARTIFACTS_ANSWERED_AT__ = 0;
    el("nav-artifacts").click();
    let firstPaint = 0;
    for (let at = 0; at < 300 && firstPaint === 0; at += 1) {
      await new Promise((done) => requestAnimationFrame(done));
      if (window.__ARTIFACTS_ANSWERED_AT__ && artifactsView()?.querySelector(".artifact-card")) {
        firstPaint = performance.now() - window.__ARTIFACTS_ANSWERED_AT__;
      }
    }
    return { firstPaint: Math.round(firstPaint), asked: (window.__ARTIFACT_ASKS__ ?? 0) - asksBefore };
  };
  const rounds = [];
  for (let n = 0; n < 5; n += 1) rounds.push(await round());
  const view = artifactsView();
  const seen = {
    firstPaint: Math.min(...rounds.map((one) => one.firstPaint)),
    rounds: rounds.map((one) => one.firstPaint),
    askedPerOpen: rounds.every((one) => one.asked === 1),
    rows: artifactRows.size,
    nodes: view.querySelectorAll(".artifact-card").length,
    statHidden: view.querySelector(".artifacts-stat").hidden,
  };
  const creationsBefore = artifactCardCreations;
  const asksBefore = window.__ARTIFACT_ASKS__;
  view.querySelector('[data-artifact-tab="evidence"]').click();
  await new Promise((done) => setTimeout(done, 60));
  seen.screenshotNodes = view.querySelectorAll(".artifact-card").length;
  seen.screenshotOnly = [...view.querySelectorAll(".artifact-card")].every((one) => ["screenshot", "evidence"].includes(one.dataset.kind));
  view.querySelector('[data-artifact-tab="reports"]').click();
  await new Promise((done) => setTimeout(done, 60));
  const query = view.querySelector(".artifacts-query");
  query.value = "보고서 7";
  query.dispatchEvent(new Event("input", { bubbles: true }));
  await new Promise((done) => setTimeout(done, 260));
  seen.searchNodes = view.querySelectorAll(".artifact-card").length;
  seen.searchRows = artifactOrder.length;
  seen.searchAll = [...artifactTabCounts.values()].reduce((sum, n) => sum + n, 0);
  query.value = "";
  query.dispatchEvent(new Event("input", { bubbles: true }));
  await new Promise((done) => setTimeout(done, 260));
  seen.creations = artifactCardCreations - creationsBefore;
  seen.asked = window.__ARTIFACT_ASKS__ - asksBefore;
  // Scrolling to the end paints the last card and still only a pool's worth.
  const grid = view.querySelector(".artifacts-grid");
  grid.scrollTop = grid.scrollHeight;
  grid.dispatchEvent(new Event("scroll"));
  await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  seen.tailNodes = view.querySelectorAll(".artifact-card").length;
  // Newest first: the oldest row is the last card.
  seen.tailHasLast = view.querySelector('.artifact-card[data-id="art-0"]') !== null;
  seen.gridScrollsX = grid.scrollWidth > grid.clientWidth;
  const firstId = artifactOrder[0];
  selectArtifact(view, firstId, { reveal: true });
  await window.__PAINTED__();
  const selected = artifactCardNodeFor(firstId);
  const bounds = selected?.getBoundingClientRect(), frame = grid.getBoundingClientRect();
  seen.offscreenRevealed = !!selected && selected.classList.contains("is-selected")
    && bounds.top >= frame.top && bounds.top < frame.bottom;
  return seen;
});
ok(
  "1,000 artifacts: first paint under 150 ms (min of five), a pool of visible cards only, zero node creations across a tab switch and a search, and no second ask",
  artifactScale.firstPaint < 150 &&
    artifactScale.askedPerOpen &&
    artifactScale.rows === 1000 &&
    artifactScale.nodes < 120 &&
    artifactScale.statHidden &&
    artifactScale.screenshotOnly &&
    artifactScale.screenshotNodes > 0 &&
    artifactScale.searchNodes >= 1 &&
    artifactScale.searchRows === 33 && artifactScale.searchAll === 33 &&
    artifactScale.creations === 0 &&
    artifactScale.asked === 0 &&
    artifactScale.tailNodes < 120 &&
    artifactScale.tailHasLast && artifactScale.offscreenRevealed &&
    !artifactScale.gridScrollsX,
  JSON.stringify(artifactScale),
);
}

export async function testArtifactStudioOwnership(page, ok) {
  const seen = await page.evaluate(async () => {
    const agents = new Map(paneAgents), sessions = new Map(paneSessions), originalLocale = locale;
    const paste = window.__ANSWER__.term_paste;
    const terms = [3240, 3241, 3242];
    paneAgents.clear(); paneSessions.clear();
    for (const term of terms.slice(0, 2)) {
      mountTermTab(term, { agent: "zo", worktree: activeWorktreePath }, { focus: false });
      paneAgents.set(term, "zo");
    }
    locale = "en";
    dropTab("artifacts"); openArtifacts(); await window.__PAINTED__();
    const view = artifactsView(); openArtifactStudio(view, "operate");
    const form = view.querySelector("form"), destination = view.querySelector(".artifacts-studio-destination");
    const brief = view.querySelector(".artifacts-studio-brief"), status = view.querySelector(".artifacts-studio-status");
    brief.value = "팀을 위한 작은 앱"; brief.dispatchEvent(new Event("input", { bubbles: true }));
    const out = { needsChoice: destination.value === "" && view.querySelector(".artifacts-studio-send").disabled };
    destination.value = String(terms[1]); destination.dispatchEvent(new Event("input", { bubbles: true }));
    let finish, delivered;
    window.__ANSWER__.term_paste = args => { delivered = args; return new Promise(resolve => { finish = resolve; }); };
    const pending = sendArtifactStudio(view); await Promise.resolve();
    const away = tabOfTerm(terms[0]).id;
    setActiveTab(away);
    finish(null); await pending;
    out.exactDestination = delivered.term === terms[1];
    out.focusKept = activeTabId === away;
    openArtifacts();
    locale = "ko"; applyLocale(); paintArtifactStudio(view);
    out.korean = view.querySelector(".artifacts-studio-title").textContent === "작은 앱"
      && status.textContent === "에이전트 입력줄에 초안을 넣었습니다.";
    locale = "en"; applyLocale(); paintArtifactStudio(view);
    out.englishStatus = status.textContent === CATALOG.en["artifacts.studio.sent"];
    const firstTab = tabOfTerm(terms[0]);
    firstTab.layout = addPane(firstTab.layout, terms[0], terms[2], "horizontal");
    paneAgents.set(terms[2], "zo");
    paintArtifactStudio(view);
    out.stable = destination.value === String(terms[1]);
    out.inactiveSplitListed = [...destination.options].some(option => option.value === String(terms[2]));
    dropTab(tabOfTerm(terms[1]).id); paneAgents.delete(terms[1]);
    paintArtifactStudio(view); paintArtifactStudio(view);
    out.noReplacement = destination.value === "" && view.querySelector(".artifacts-studio-send").disabled;
    for (const term of terms) { const tab = tabOfTerm(term); if (tab) dropTab(tab.id); }
    paneAgents.clear(); paneSessions.clear();
    for (const [id, agent] of agents) paneAgents.set(id, agent);
    for (const [id, session] of sessions) paneSessions.set(id, session);
    window.__ANSWER__.term_paste = paste;
    locale = originalLocale; applyLocale();
    closeArtifactStudio(view);
    return out;
  });
  ok("studio requires an explicit destination among agents and never silently replaces a lost destination", seen.needsChoice && seen.exactDestination && seen.stable && seen.noReplacement && seen.inactiveSplitListed, JSON.stringify(seen));
  ok("late draft placement keeps newer navigation and dynamic studio text survives an English/Korean round trip", seen.focusKept && seen.korean && seen.englishStatus, JSON.stringify(seen));
}

/* 첫 화면(2026-09-28 항목 4): 탭 셋이 제 수를 달고 페이지·문서가 기본이며, 행이
 * 없는 종류는 서지 않는다. 파일이 사라진 행은 런타임이 목록에서 말하고(`missing`),
 * 창은 그 행을 숨긴 채 「사라진 파일 N행 숨김 · 보기」 한 줄로 셈한다. 카드는
 * 출처가 말하는 만큼만 만든 이를 달고(에이전트 표식 · 판 · 브랜치), 출처가 비면
 * 그 줄이 없다 — 「출처 없음」은 서랍의 말이다. 피드백 수가 실린 행은 카드에 단다.
 * 페이지의 서랍은 「미리보기가 없습니다」가 아니라 그 페이지의 썸네일을 보인다. */
export async function testArtifactFirstScreen(page, ok) {
  await installHarnessWaits(page);
  const seen = await page.evaluate(async () => {
    // 한 행의 모양 — 백엔드 `Artifact`의 필드 그대로. 제목·경로는 합성이다.
    const row = (id, kind, at, extra = {}) => ({
      id, kind, title: `${id} 제목`, path: kind === "web" ? "" : `/tmp/zerocode-window-test/files/${id}`,
      bytes: 10, created_ms: at, modified_ms: at, origin: {}, tags: [], preview: { kind: "none" }, source: "manual",
      ...extra,
    });
    const now = Date.now();
    const rows = [
      row("pub-1", "page", now, { version: 3, feedback_count: 2, url: "file:///tmp/zerocode-window-test/files/pub-1" }),
      row("made-1", "page", now - 1000, { origin: { agent: "claude", model: "opus", pane: "term-4",
        worktree: "/tmp/zerocode-window-test", project: "/tmp/zerocode-window-test" } }),
      row("doc-1", "document", now - 2000, { origin: { agent: "codex", project: "/elsewhere/other-repo" },
        preview: { kind: "markdown", text: "# 문서\n\n첫 블록" } }),
      row("rep-1", "report", now - 3000, { origin: { agent: "codex", worker: "w-1", pane: "team-1/%1",
        worktree: "/tmp/zerocode-window-test/wt-b" }, preview: { kind: "markdown", text: "# 보고서" } }),
      row("shot-live", "screenshot", now - 4000, { origin: { automation: "computer-use" } }),
      row("shot-dead", "screenshot", now - 5000, { origin: { automation: "computer-use" } }),
      row("steps-dead", "evidence", now - 6000, { origin: { automation: "computer-use" } }),
      row("page-dead", "page", now - 7000),
    ];
    const saved = { list: window.__ANSWER__.artifacts_list, preview: window.__ANSWER__.artifact_preview,
      thumb: window.__ANSWER__.artifact_thumbnail };
    let missing = ["shot-dead", "steps-dead", "page-dead"];
    const asked = [];
    window.__ANSWER__.artifacts_list = (args) => {
      asked.push(args.filter);
      return { rows, total: rows.length, truncated: false, missing, missing_total: missing.length, thumb: { queue_max: 4 } };
    };
    const PICTURE = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    const previews = [];
    let pageAnswer = { kind: "none", bytes: 10, truncated: false };
    window.__ANSWER__.artifact_preview = (args) => {
      previews.push(args.id);
      return rows.find((one) => one.id === args.id)?.kind === "page" ? pageAnswer : { kind: "none", bytes: 1, truncated: false };
    };
    // 썸네일: pub-1만 그림이 있고, made-1은 아직(숨은 판이 못 찍었다).
    window.__ANSWER__.artifact_thumbnail = (args) => ({ data_url: args.id === "pub-1" ? PICTURE : null, cached: true });
    dropTab("artifacts");
    artifactThumbs.clear(); artifactThumbFailed.clear(); artifactPreviews.clear();
    Object.assign(artifactFilter, { tab: "pages", origin: null, project: null, agent: null, period: "all", showMissing: false, query: "" });
    artifactTabPicked = false;
    artifactListing.filtered = false;
    artifactSelectedId = null;
    openArtifacts({ fresh: true });
    await window.__PAINTED__();
    await window.__UNTIL__(() => !artifactAsking && artifactRows.size === rows.length, "first-screen rows");
    await window.__UNTIL__(() => !artifactThumbBusy && artifactThumbQueue.length === 0, "first-screen thumbnails");
    await window.__PAINTED__();
    const view = artifactsView();
    const out = {};
    const tabs = () => [...view.querySelectorAll("[data-artifact-tab]")].filter((one) => !one.hidden)
      .map((one) => `${one.dataset.artifactTab}:${one.querySelector(".artifacts-tab-count").textContent}`).join(",");
    const line = () => view.querySelector(".artifacts-missing");
    const card = (id) => artifactCardNodeFor(id);
    out.tab = artifactFilter.tab;
    out.tabs = tabs();
    out.order = artifactOrder.join(",");
    out.hiddenLine = !line().hidden && line().textContent;
    out.askedFilter = JSON.stringify(asked[0] ?? null);
    // 만든 이의 줄 — 있는 행과 없는 행.
    const maker = (id) => card(id)?.querySelector(".artifact-card-maker");
    out.madeMaker = { hidden: maker("made-1").hidden, tip: maker("made-1").querySelector(".artifact-card-maker-mark").dataset.tip,
      pane: maker("made-1").querySelector(".artifact-card-maker-pane").textContent,
      branch: maker("made-1").querySelector(".artifact-card-maker-branch").textContent,
      letter: maker("made-1").querySelector(".agent-ico") !== null };
    out.docBranch = maker("doc-1").querySelector(".artifact-card-maker-branch").textContent;
    out.docPaneHidden = maker("doc-1").querySelector(".artifact-card-maker-pane").hidden;
    out.pubMakerHidden = maker("pub-1").hidden;
    out.noOriginOnCards = ![...view.querySelectorAll(".artifact-card")].some((one) => one.textContent.includes("출처 없음"));
    // 피드백 수.
    out.pubFeedback = !card("pub-1").querySelector(".artifact-card-feedback").hidden
      && card("pub-1").querySelector(".artifact-card-feedback").textContent;
    out.madeFeedbackHidden = card("made-1").querySelector(".artifact-card-feedback").hidden;
    // 사라진 행의 토글.
    line().querySelector("button").click();
    await window.__PAINTED__();
    out.shownLine = line().textContent;
    out.shownOrder = artifactOrder.join(",");
    out.deadCard = card("page-dead")?.classList.contains("is-missing") ?? false;
    out.tabsShown = tabs();
    selectArtifact(view, "page-dead");
    await new Promise((done) => setTimeout(done, 40));
    const drawer = view.querySelector(".artifacts-drawer");
    out.deadDrawer = drawer.querySelector(".artifact-preview-none").textContent;
    out.deadReveal = drawer.querySelector('[data-artifact-action="reveal"]').disabled;
    out.deadAsked = previews.includes("page-dead");
    line().querySelector("button").click();
    await window.__PAINTED__();
    out.hiddenAgain = !artifactOrder.includes("page-dead") && line().textContent.includes("숨김");
    view.querySelector('[data-artifact-tab="evidence"]').click();
    await window.__PAINTED__();
    out.evidenceLine = line().textContent;
    out.evidenceOrder = artifactOrder.join(",");
    view.querySelector('[data-artifact-tab="pages"]').click();
    await window.__PAINTED__();
    // 페이지의 서랍: 카드가 그린 썸네일이 미리보기다(묻지 않는다).
    previews.length = 0;
    selectArtifact(view, "pub-1");
    await new Promise((done) => setTimeout(done, 60));
    const img = drawer.querySelector(".artifact-preview-image");
    out.pubPreview = !img.hidden && img.src === PICTURE && drawer.querySelector(".artifact-preview-none").hidden;
    out.pubAsked = previews.includes("pub-1");
    out.pubNoOrigin = !drawer.querySelector(".artifact-no-origin").hidden && drawer.querySelector(".artifact-no-origin").textContent;
    // 썸네일이 아직 없는 페이지는 런타임에 묻고, 「없음」은 기억하지 않는다 —
    // 그림이 생긴 뒤의 물음은 그 그림을 받는다.
    selectArtifact(view, "made-1");
    await new Promise((done) => setTimeout(done, 60));
    out.madeFirst = drawer.querySelector(".artifact-preview-none").hidden === false;
    out.madeNoOrigin = drawer.querySelector(".artifact-no-origin").hidden;
    selectArtifact(view, "pub-1");
    await new Promise((done) => setTimeout(done, 40));
    pageAnswer = { kind: "image", data_url: PICTURE, bytes: 10, truncated: false };
    selectArtifact(view, "made-1");
    await new Promise((done) => setTimeout(done, 60));
    out.madeLater = !img.hidden && img.src === PICTURE;
    out.madeAsks = previews.filter((id) => id === "made-1").length;
    // 목록이 더는 사라졌다고 말하지 않으면 그 행은 다시 선다 — 창은 디스크에 묻지 않는다.
    missing = ["shot-dead", "steps-dead"];
    await refreshArtifacts();
    await window.__PAINTED__();
    out.revived = artifactOrder.includes("page-dead") && line().hidden;
    // 행이 하나도 없는 종류의 탭은 서지 않는다 — 그 종류의 행이 오면 선다.
    out.otherHidden = view.querySelector('[data-artifact-tab="other"]').hidden;
    rows.push(row("export-1", "export", now - 8000));
    await refreshArtifacts();
    await window.__PAINTED__();
    out.otherAppears = !view.querySelector('[data-artifact-tab="other"]').hidden;
    window.__ANSWER__.artifacts_list = saved.list;
    window.__ANSWER__.artifact_preview = saved.preview;
    window.__ANSWER__.artifact_thumbnail = saved.thumb;
    artifactSelectedId = null;
    return out;
  });
  ok("the first screen opens on 「페이지·문서」 with each tab's count, and kinds with no rows stand no tab",
    seen.tab === "pages" && seen.tabs === "pages:3,reports:1,evidence:1" && seen.order === "pub-1,made-1,doc-1"
      && seen.askedFilter === "{}" && seen.otherHidden === true && seen.otherAppears === true, JSON.stringify(seen));
  ok("rows whose file is gone hide behind 「사라진 파일 N행 숨김 · 보기」, come back dimmed, and follow the runtime's `missing` rather than the disk",
    seen.hiddenLine === "사라진 파일 1행 숨김·보기" && seen.shownLine === "사라진 파일 1행 보는 중·숨기기"
      && seen.shownOrder === "pub-1,made-1,doc-1,page-dead" && seen.deadCard && seen.tabsShown === "pages:4,reports:1,evidence:3"
      && seen.deadDrawer.includes("파일이 사라졌습니다") && seen.deadReveal && !seen.deadAsked && seen.hiddenAgain
      && seen.evidenceLine === "사라진 파일 2행 숨김·보기" && seen.evidenceOrder === "shot-live" && seen.revived,
    JSON.stringify(seen));
  ok("a card names its maker — agent mark, pane, branch — only as far as the origin says, and 「출처 없음」 lives in the drawer alone",
    seen.madeMaker.hidden === false && seen.madeMaker.tip === "Claude · opus" && seen.madeMaker.pane === "term:4"
      && seen.madeMaker.branch === "main" && seen.madeMaker.letter && seen.docBranch === "" && seen.docPaneHidden
      && seen.pubMakerHidden === true && seen.noOriginOnCards && seen.pubNoOrigin === "출처 없음" && seen.madeNoOrigin === true,
    JSON.stringify(seen));
  ok("a card carries its feedback count when the row has one",
    seen.pubFeedback === "피드백 2" && seen.madeFeedbackHidden === true, JSON.stringify(seen));
  ok("a page's drawer previews its thumbnail — the card's own picture unasked, else the runtime's answer once it has one",
    seen.pubPreview && seen.pubAsked === false && seen.madeFirst && seen.madeLater && seen.madeAsks === 2, JSON.stringify(seen));
}

/* 다시 찾기(2026-09-28 항목 6): 프로젝트·에이전트·기간(오늘 · 이번 주 · 전체)으로
 * 좁히고, 카드는 날로 무리를 짓는다(오늘 · 이번 주 · 이전). 창이 카탈로그를 다
 * 들고 있으면 거르기는 창 안에서 하고 묻지 않는다. 표 상한이 답을 잘랐으면
 * 거르개를 런타임에 실어 묻는다 — 상한 너머의 행이 거르개에 가려지지 않게. */
export async function testArtifactRecall(page, ok) {
  await installHarnessWaits(page);
  const seen = await page.evaluate(async () => {
    // 한 행의 모양 — 백엔드 `Artifact`의 필드 그대로. 제목·경로는 합성이다.
    const row = (id, kind, at, extra = {}) => ({
      id, kind, title: `${id} 제목`, path: kind === "web" ? "" : `/tmp/zerocode-window-test/files/${id}`,
      bytes: 10, created_ms: at, modified_ms: at, origin: {}, tags: [], preview: { kind: "none" }, source: "manual",
      ...extra,
    });
    const now = Date.now();
    const day = 24 * 60 * 60 * 1000;
    const home = "/tmp/zerocode-window-test";
    const rows = [
      row("today-claude", "page", now, { origin: { agent: "claude", project: home } }),
      row("today-codex", "document", now - 60_000, { origin: { agent: "codex", worktree: `${home}/wt-b` } }),
      row("week-claude", "page", now - 3 * day, { origin: { agent: "claude", project: "/elsewhere/other-repo" } }),
      row("old-codex", "page", now - 30 * day, { origin: { agent: "codex", project: home } }),
      row("old-bare", "web", now - 40 * day),
      row("rep-today", "report", now - 120_000, { origin: { agent: "codex", worktree: home } }),
    ];
    const saved = window.__ANSWER__.artifacts_list;
    const asked = [];
    let capped = false;
    window.__ANSWER__.artifacts_list = (args) => {
      asked.push(args.filter);
      if (!capped) return { rows, total: rows.length, truncated: false, missing: [], missing_total: 0, thumb: { queue_max: 2 } };
      // 상한 둘: 런타임이 거른 뒤 새것부터 둘만 싣고, 탭의 수는 거른 전부로 센다.
      const filter = args.filter ?? {};
      const kinds = filter.kinds ?? [];
      const admitted = rows.filter((one) => (!filter.agent || one.origin.agent === filter.agent)
        && (!filter.worktree || one.origin.worktree === filter.worktree)
        && (!filter.since_ms || one.modified_ms >= filter.since_ms)
        && (!filter.roots || [one.origin.project, one.origin.worktree].some((path) => path && filter.roots.some((root) => path === root || path.startsWith(`${root}/`)))));
      const byKind = {};
      for (const one of admitted) byKind[one.kind] = (byKind[one.kind] ?? 0) + 1;
      const tabbed = admitted.filter((one) => kinds.length === 0 || kinds.includes(one.kind));
      return { rows: tabbed.slice(0, 2), total: tabbed.length, truncated: tabbed.length > 2, missing: [], missing_total: 0,
        by_kind: byKind, thumb: { queue_max: 2 } };
    };
    dropTab("artifacts");
    Object.assign(artifactFilter, { tab: "pages", origin: null, project: null, agent: null, period: "all", showMissing: false, query: "" });
    artifactTabPicked = false;
    artifactListing.filtered = false;
    artifactProjects = [];
    openArtifacts({ fresh: true });
    await window.__PAINTED__();
    await window.__UNTIL__(() => !artifactAsking && artifactRows.size === rows.length, "recall rows");
    await window.__PAINTED__();
    const view = artifactsView();
    const pick = (name, value) => {
      const select = view.querySelector(`[data-artifact-pick="${name}"]`);
      select.value = value;
      select.dispatchEvent(new Event("change", { bubbles: true }));
    };
    const options = (name) => [...view.querySelector(`[data-artifact-pick="${name}"]`).options].map((one) => `${one.value}=${one.textContent}`).join("|");
    const heads = () => [...view.querySelectorAll(".artifacts-group:not([hidden])")].map((one) => one.textContent).join(",");
    const reportCount = () => view.querySelector('[data-artifact-tab="reports"] .artifacts-tab-count').textContent;
    const out = {};
    const asksBefore = asked.length;
    out.groups = artifactGroups.map((group) => `${group.period}:${group.count}`).join(",");
    out.heads = heads();
    out.order = artifactOrder.join(",");
    // 무리 머리는 제 무리의 첫 카드 바로 위에 선다.
    const top = (node) => node.getBoundingClientRect().top;
    const weekHead = view.querySelector('.artifacts-group[data-period="week"]');
    out.headAboveItsCards = top(weekHead) < top(artifactCardNodeFor("week-claude"))
      && top(weekHead) > top(artifactCardNodeFor("today-claude"));
    out.projectOptions = options("project");
    out.agentOptions = options("agent");
    out.periodOptions = options("period");
    pick("project", home);
    await window.__PAINTED__();
    out.projectOrder = artifactOrder.join(",");
    out.projectReports = reportCount();
    pick("project", "/elsewhere/other-repo");
    await window.__PAINTED__();
    out.strayOrder = artifactOrder.join(",");
    pick("project", "");
    pick("agent", "codex");
    await window.__PAINTED__();
    out.agentOrder = artifactOrder.join(",");
    pick("agent", "");
    pick("period", "today");
    await window.__PAINTED__();
    out.todayOrder = artifactOrder.join(",");
    out.todayHeads = heads();
    pick("period", "week");
    await window.__PAINTED__();
    out.weekOrder = artifactOrder.join(",");
    pick("agent", "claude");
    await window.__PAINTED__();
    out.combined = artifactOrder.join(",");
    out.combinedReports = reportCount();
    pick("agent", "");
    pick("period", "");
    await window.__PAINTED__();
    out.back = artifactOrder.join(",");
    out.asksUncapped = asked.length - asksBefore;
    // 표 상한에 잘린 창: 거르개가 런타임으로 간다.
    capped = true;
    asked.length = 0;
    await refreshArtifacts();
    await window.__UNTIL__(() => !artifactAsking, "capped listing");
    await window.__PAINTED__();
    out.cappedFirst = JSON.stringify(asked);
    out.cappedOrder = artifactOrder.join(",");
    out.cappedStat = view.querySelector(".artifacts-stat").hidden ? "" : view.querySelector(".artifacts-stat").textContent;
    out.cappedTabs = [...view.querySelectorAll("[data-artifact-tab]")].filter((one) => !one.hidden)
      .map((one) => `${one.dataset.artifactTab}:${one.querySelector(".artifacts-tab-count").textContent}`).join(",");
    asked.length = 0;
    pick("agent", "codex");
    await window.__UNTIL__(() => !artifactAsking && asked.length > 0, "capped agent ask");
    await window.__PAINTED__();
    out.cappedAgent = JSON.stringify(asked.at(-1));
    out.cappedAgentOrder = artifactOrder.join(",");
    pick("agent", "");
    pick("period", "today");
    await window.__UNTIL__(() => !artifactAsking && asked.at(-1)?.since_ms !== undefined, "capped period ask");
    await window.__PAINTED__();
    const since = asked.at(-1).since_ms;
    const midnight = new Date(); midnight.setHours(0, 0, 0, 0);
    out.cappedSince = since === midnight.getTime();
    pick("period", "");
    pick("project", home);
    await window.__UNTIL__(() => !artifactAsking && asked.at(-1)?.roots !== undefined, "capped project ask");
    out.cappedRoots = JSON.stringify(asked.at(-1).roots);
    pick("project", "");
    await window.__UNTIL__(() => !artifactAsking, "capped reset");
    // 범위를 들고 온 도착(보드의 「보고서 보기」처럼): 든 행은 페이지 탭의 것뿐이지만,
    // 종류를 가리지 않고 한 번 물어 고른 행의 탭에 서고, 그 탭으로 한 번 더 묻는다.
    asked.length = 0;
    openArtifacts({ origin: { field: "worktree", value: home, label: "home" }, select: "rep-today" });
    await window.__UNTIL__(() => !artifactAsking && asked.length >= 2 && !artifactAskAgain, "capped landing");
    await window.__PAINTED__();
    out.landingAsks = JSON.stringify(asked);
    out.landingTab = artifactFilter.tab;
    out.landingSelected = artifactSelectedId;
    out.landingShown = view.querySelector(".artifacts-detail").dataset.artifactId;
    artifactFilter.origin = null;
    window.__ANSWER__.artifacts_list = saved;
    artifactListing.filtered = false;
    Object.assign(artifactFilter, { tab: "pages", origin: null, project: null, agent: null, period: "all" });
    artifactSelectedId = null;
    await refreshArtifacts();
    return out;
  });
  ok("cards group by day — 오늘 · 이번 주 · 이전 — each head standing over its own cards",
    seen.groups === "today:2,week:1,older:2" && seen.heads === "오늘,이번 주,이전"
      && seen.order === "today-claude,today-codex,week-claude,old-codex,old-bare" && seen.headAboveItsCards, JSON.stringify(seen));
  ok("the project, agent and period pickers narrow the tab and its counts in the window without asking again",
    seen.projectOptions === "=전체|/tmp/zerocode-window-test=zerocode|/elsewhere/other-repo=other-repo"
      && seen.agentOptions === "=전체|codex=Codex|claude=Claude" && seen.periodOptions === "=전체|today=오늘|week=이번 주"
      && seen.projectOrder === "today-claude,today-codex,old-codex" && seen.projectReports === "1"
      && seen.strayOrder === "week-claude" && seen.agentOrder === "today-codex,old-codex"
      && seen.todayOrder === "today-claude,today-codex" && seen.todayHeads === "오늘"
      && seen.weekOrder === "today-claude,today-codex,week-claude" && seen.combined === "today-claude,week-claude"
      && seen.combinedReports === "0" && seen.back === "today-claude,today-codex,week-claude,old-codex,old-bare"
      && seen.asksUncapped === 0, JSON.stringify(seen));
  ok("a listing cut by the table's cap hands the tab, project roots, agent and period to the runtime, and counts tabs from its answer",
    seen.cappedFirst === JSON.stringify([{}, { kinds: ["page", "document", "web"], present: true }])
      && seen.cappedOrder === "today-claude,today-codex" && seen.cappedStat.startsWith("2 / 5")
      && seen.cappedTabs === "pages:5,reports:1"
      && seen.cappedAgent === JSON.stringify({ kinds: ["page", "document", "web"], present: true, agent: "codex" })
      && seen.cappedAgentOrder === "today-codex,old-codex" && seen.cappedSince
      && seen.cappedRoots === JSON.stringify(["/tmp/zerocode-window-test"]),
    JSON.stringify(seen));
  ok("a scoped arrival at a capped listing asks across kinds once, lands on the picked row's tab, then asks that tab",
    seen.landingAsks === JSON.stringify([
      { kinds: [], present: true, worktree: "/tmp/zerocode-window-test" },
      { kinds: ["report"], present: true, worktree: "/tmp/zerocode-window-test" },
    ]) && seen.landingTab === "reports" && seen.landingSelected === "rep-today" && seen.landingShown === "rep-today",
    JSON.stringify(seen));
}

/* 발행물의 출처 (아티팩트 재연결 R1). 판에서 발행한 페이지는 그 판의 열쇠
 * (`term-<n>`)와 워크트리를 싣고, 서랍의 카드·워크트리 단추가 그 출처로 선다 —
 * 카드는 보드에서 그 판의 카드를 고르고, 워크트리는 그 워크트리로 옮긴다. 판
 * 번호는 창을 띄울 때마다 새로 나오므로 발행보다 늦게 태어난 판은 같은 번호여도
 * 그 판이 아니다(카드 단추가 서지 않는다). 워커의 판에서 발행한 페이지는 원장
 * 좌석의 과업도 실어서 「과업」 단추가 그 워커의 카드로 간다. 출처가 없는
 * 페이지는 세 단추 모두 서지 않는다. */
export async function testArtifactProvenance(page, ok) {
  const seen = await page.evaluate(async () => {
    const out = {};
    const now = Date.now();
    const pageRow = (id, origin, modified) => ({
      id, kind: "page", title: `${id}.html`, bytes: 10, created_ms: modified, modified_ms: modified, version: 1,
      path: `/tmp/zerocode-window-test/artifacts/pages/${id}/index.html`,
      url: `file:///tmp/zerocode-window-test/artifacts/pages/${id}/index.html`,
      source_path: `/tmp/zerocode-window-test/${id}.html`, origin, tags: [],
      preview: { kind: "text", text: "" }, source: "manual",
    });
    const rows = [
      pageRow("p-live", { pane: "term-91", agent: "claude", model: "claude-opus-5",
        worktree: "/tmp/zerocode-window-test/wt-b", project: "/tmp/zerocode-window-test" }, now - 1_000),
      pageRow("p-stale", { pane: "term-92", agent: "codex" }, now - 60_000),
      pageRow("p-bare", {}, now - 2_000),
      pageRow("p-worker", { pane: "term-93", agent: "codex", run: "run-9", worker: "w-9", task: "t-9",
        worktree: "/tmp/zerocode-window-test/wt-c" }, now - 3_000),
    ];
    const held = {
      list: window.__ANSWER__.artifacts_list, panes: window.__PANES__, columns: window.__COLUMNS__,
      activate: window.activateWorktree, ledger: window.__LEDGER__,
    };
    await window.__UNTIL__(() => !artifactAsking, "previous gallery listing");
    window.__ANSWER__.artifacts_list = () =>
      ({ rows, total: rows.length, truncated: false, thumb: { width: 320, height: 240, queue_max: 24 } });
    // 91 은 발행보다 먼저 태어났고, 92 는 제 발행보다 늦게 — 다른 기동의 같은 번호다.
    paneBorn.set(91, now - 30_000); paneAgents.set(91, "claude");
    paneBorn.set(92, now - 5_000); paneAgents.set(92, "codex");
    dropTab("artifacts");
    // 탭은 이미 물은 목록을 다시 묻지 않는다: 이 표를 먼저 읽힌다.
    await refreshArtifacts();
    openArtifacts();
    await window.__PAINTED__();
    await new Promise((done) => setTimeout(done, 120));
    let view = artifactsView();
    const jumps = (id) => {
      selectArtifact(view, id, { reveal: true });
      paintArtifactDrawer(view);
      const disabled = (jump) => view.querySelector(`[data-artifact-jump="${jump}"]`).disabled;
      return { card: disabled("card"), task: disabled("task"), worktree: disabled("worktree") };
    };
    out.live = jumps("p-live");
    out.stale = jumps("p-stale");
    out.bare = jumps("p-bare");
    const activated = [];
    window.activateWorktree = async (path) => { activated.push(path); };
    jumps("p-live");
    view.querySelector('[data-artifact-jump="worktree"]').click();
    await new Promise((done) => setTimeout(done, 40));
    out.worktreeGoesThere = activated.join() === "/tmp/zerocode-window-test/wt-b";
    window.__PANES__ = [{ term: 91, agent: "claude", state: "working", at: now, resumable: false }];
    window.__COLUMNS__ = [{ bucket: "working", cards: [
      { pane: "term:91", agent: "claude", state: "working", heading: "Claude", worktree: "wt-b",
        project: "", task: "", unseen: false, changed_at: 1000, at: 1000 },
    ] }];
    jumps("p-live");
    view.querySelector('[data-artifact-jump="card"]').click();
    await new Promise((done) => setTimeout(done, 120));
    out.cardLandsOnThePane = activeTabId === "board" && agentGraphSelectedKey === "agent:term:91";
    // 워커의 발행물: 원장이 그 좌석(93)에 과업 t-9 를 둔다.
    window.__LEDGER__ = [
      { run: "run-9", worker: "w-9", agent: "codex", state: "working", ledger: "working", hearing: "hook",
        hearing_at: 0, checkout: "/tmp/zerocode-window-test/wt-c", task: "deck", task_id: "t-9",
        reported: false, review: null, term: 93, at: now },
    ];
    await refreshPaneLedger();
    out.worker = jumps("p-worker");
    window.__PANES__ = [{ term: 93, agent: "codex", state: "working", at: now, resumable: false }];
    window.__COLUMNS__ = [{ bucket: "working", cards: [
      { pane: "term:93", agent: "codex", state: "working", heading: "Codex", worktree: "wt-c",
        project: "", task: "deck", unseen: false, changed_at: 1000, at: 1000 },
    ] }];
    leavePagesForStage();
    openArtifacts();
    await window.__PAINTED__();
    view = artifactsView();
    jumps("p-worker");
    view.querySelector('[data-artifact-jump="task"]').click();
    await new Promise((done) => setTimeout(done, 120));
    out.taskLandsOnTheWorker = activeTabId === "board" && agentGraphSelectedKey === "agent:term:93";
    window.__LEDGER__ = held.ledger;
    await refreshPaneLedger();
    window.activateWorktree = held.activate;
    window.__PANES__ = held.panes;
    window.__COLUMNS__ = held.columns;
    if (held.list === undefined) delete window.__ANSWER__.artifacts_list;
    else window.__ANSWER__.artifacts_list = held.list;
    for (const term of [91, 92]) { paneBorn.delete(term); paneAgents.delete(term); }
    artifactSelectedId = null;
    dropTab("artifacts");
    leavePagesForStage();
    await refreshArtifacts();
    return out;
  });
  ok(
    "a publication's drawer offers its maker's card and worktree when its origin names them, and never a same-numbered pane born after it",
    seen.live.card === false && seen.live.worktree === false && seen.live.task === true &&
      seen.stale.card === true && seen.stale.worktree === true &&
      seen.bare.card === true && seen.bare.task === true && seen.bare.worktree === true &&
      seen.worker.card === false && seen.worker.task === false && seen.worker.worktree === false,
    JSON.stringify(seen),
  );
  ok(
    "the drawer's worktree, card and task jumps land on the publication's worktree, its maker pane's board card and its worker's card",
    seen.worktreeGoesThere && seen.cardLandsOnThePane && seen.taskLandsOnTheWorker,
    JSON.stringify(seen),
  );
}

/* 새 발행이 사람에게 닿는 길 (t-11958): 「새 N」, 상태 줄 한 줄, 만든 판 옆.
 *
 * 문이 발행을 마치면 백엔드가 그 행을 `artifacts:published`로 보낸다. 이 스위트는
 * 그 이벤트를 가짜 백엔드에서 쏘고 창이 하는 일을 잰다. 규칙은 디자인 검토의
 * 것이다(output/design/artifacts-20260928): 사람이 보고 있는 판의 첫 발행은 그
 * 옆에 열고, 다시 발행은 선 자리에서 바뀌며(새 나눔 없음), 좁은 판은 같은 그룹의
 * 탭, 워커의 발행은 「새 N」만, 설정을 끄면 아무것도 열지 않는다. 만든 판이 화면에
 * 없을 때만 상태 줄이 말하고, 주석의 초안은 만든 판이 받을 곳의 기본이며 쓰던
 * 입력 뒤에 붙을 뿐 보내지지 않는다. 본 것은 다시 읽어도 남는다. */
export async function testArtifactBeside(browser, origin, ok, outputDir) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  const axeModule = createRequire(import.meta.url)("@axe-core/playwright");
  const AxeBuilder = axeModule.default ?? axeModule;
  await mkdir(outputDir, { recursive: true });
  // 두 테마로 그 자리를 찍고, 새로 선 것들의 접근성을 axe로 묻는다.
  const inBothThemes = async (name, include) => {
    for (const theme of ["dark", "light"]) {
      await page.evaluate((next) => document.documentElement.setAttribute("data-theme", next), theme);
      await new Promise((done) => setTimeout(done, 300));
      await page.screenshot({ path: join(outputDir, `artifacts-r2-${theme}-${name}.png`) });
      // 그 자리가 서지 않았으면 axe가 던진다 — 그것도 실패로 센다.
      const violations = await axeViolations(page, AxeBuilder, include).catch((error) => [String(error)]);
      ok(`the ${name} marks pass axe in ${theme}`, violations.length === 0, JSON.stringify(violations));
    }
    await page.evaluate(() => document.documentElement.setAttribute("data-theme", "dark"));
  };
  try {
    const install = () => page.evaluate(() => {
      window.__BESIDE__ = { opens: [], navigations: [], settings: [], pastes: [], prompts: [] };
      let born = 0;
      window.__ANSWER__.open_browser_pane = (args) => {
        born += 1;
        window.__BESIDE__.opens.push(args.url);
        return `browser-beside-${born}`;
      };
      window.__ANSWER__.browser_zoom = () => null;
      window.__ANSWER__.browser_navigate = (args) => (window.__BESIDE__.navigations.push({ ...args }), null);
      window.__ANSWER__.artifact_versions = (args) => {
        const top = window.__BESIDE_VERSIONS__?.[args.id] ?? 1;
        return Array.from({ length: top }, (_, at) => ({
          n: at + 1,
          path: `/tmp/zerocode-window-test/artifacts/versions/${args.id}/${at + 1}/index.html`,
          sha256: `sha-${at + 1}`,
        }));
      };
      window.__ANSWER__.set_artifacts_auto_open_beside = (args) => (window.__BESIDE__.settings.push(args.on), null);
      window.__ANSWER__.term_paste = (args) => (window.__BESIDE__.pastes.push({ ...args }), null);
      window.__ANSWER__.send_prompt = (args) => (window.__BESIDE__.prompts.push({ ...args }), null);
    });
    const settle = () => page.evaluate(() => new Promise((done) => setTimeout(done, 160)));
    const clear = () => page.evaluate(() => {
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const term of [...termViews.keys()]) dropTermView(term);
      for (const group of stageGroups()) collapseStageGroup(group);
      globalThis.hideArtifactNotice?.();
      renderTabs();
      updateStage();
    });
    // 한 번의 발행: 문이 기록한 출처를 입은 행, 지금 시각에 — 판이 그보다 먼저 태어났다.
    const publish = (id, version, origin) => page.evaluate(({ id: key, version: n, origin: made }) => {
      window.__BESIDE_VERSIONS__ = { ...(window.__BESIDE_VERSIONS__ ?? {}), [key]: n };
      const at = Date.now();
      const row = {
        id: key, kind: "page", title: `${key} 시안`, bytes: 10, created_ms: at, modified_ms: at, version: n,
        path: `/tmp/zerocode-window-test/artifacts/pages/${key}/index.html`,
        url: `file:///tmp/zerocode-window-test/artifacts/pages/${key}/index.html`,
        source_path: `/tmp/zerocode-window-test/${key}.html`, origin: made, tags: [],
        preview: { kind: "text", text: "" }, source: "manual",
      };
      for (const handler of window.__LISTENERS__["artifacts:published"] ?? []) handler({ payload: row });
    }, { id, version, origin });
    // 새 판은 `group`에 탭으로 선다 — 안 주면 지금 초점 그룹에.
    const seat = async (group = null) => {
      const term = await page.evaluate(async (at) => {
        if (at !== null) focusedPane = at;
        const made = await openTermTab({ placement: "tab" });
        paneAgents.set(made, "claude");
        renderTabs();
        return made;
      }, group);
      await settle();
      return term;
    };
    const look = (maker) => page.evaluate((term) => {
      const tab = tabOfTerm(term);
      const strip = tab ? document.querySelector(`[data-tab="${CSS.escape(tab.id)}"]`) : null;
      const browsers = tabs.filter((one) => one.kind === "browser");
      const notice = document.getElementById("sb-artifact-notice");
      return {
        opens: window.__BESIDE__.opens.length,
        navigations: window.__BESIDE__.navigations.map((one) => one.url),
        groups: stageGroups().length,
        makerGroup: tab?.pane ?? null,
        makerWidth: Math.round(termViews.get(term)?.host?.getBoundingClientRect().width ?? 0),
        makerActive: activeTabId === tab?.id,
        browsers: browsers.map((one) => ({ id: one.artifact?.id ?? null, pane: one.pane, current: one.artifact?.current ?? null })),
        tabPill: strip?.querySelector(".tab-new")?.textContent ?? "",
        navPill: document.querySelector("#nav-artifacts .nav-new")?.textContent ?? "",
        unseen: typeof artifactUnseen === "undefined" ? null : [...artifactUnseen.keys()],
        notice: notice && !notice.hidden ? notice.textContent.trim() : "",
        noticeOpen: notice?.querySelector(".sb-artifact-notice-open")?.textContent ?? "",
      };
    }, maker);
    const hintOf = (id) => page.evaluate((key) => {
      const tab = tabs.find((one) => one.kind === "browser" && one.artifact?.id === key);
      const hint = tab ? docHost(tab.pane, "browser").querySelector(".artifact-hint") : null;
      return {
        shown: hint ? hint.hidden === false : false,
        words: hint?.querySelector(".artifact-hint-words")?.textContent ?? "",
        act: hint?.querySelector(".artifact-hint-act")?.textContent ?? "",
      };
    }, id);
    await install();
    // 첫 장면들은 넓은 판에서 — 사이드바와 오른쪽 열을 빼고도 720px이 남게.
    await page.setViewportSize({ width: 1440, height: 860 });

    /* ---- 1. 보고 있는 판의 첫 발행은 그 옆에 선다 --------------------------- */
    await clear();
    const maker = await seat();
    const before = await look(maker);
    await publish("p-first", 1, { pane: `term-${maker}`, agent: "claude" });
    await settle();
    const first = await look(maker);
    const firstHint = await hintOf("p-first");
    ok(
      "a first publish from the pane the person is looking at opens beside that pane, leaves the focus there and says so under the header band",
      before.makerWidth >= 720 && first.opens === 1 && first.groups === before.groups + 1
        && first.browsers.length === 1 && first.browsers[0].id === "p-first"
        && first.browsers[0].pane !== first.makerGroup && first.makerActive
        && first.unseen?.length === 0 && first.tabPill === "" && first.navPill === "" && first.notice === ""
        && firstHint.shown && firstHint.words.includes("처음 발행이라 만든 판 옆에 열었습니다")
        && firstHint.act === "자동으로 열지 않기",
      JSON.stringify({ before, first, firstHint }),
    );
    await inBothThemes("beside", ".artifact-hint:not([hidden])");

    /* ---- 2. 다시 발행은 선 자리에서 바뀐다 ---------------------------------- */
    const tree = await page.evaluate(() => JSON.stringify(stageTree()));
    await publish("p-first", 2, { pane: `term-${maker}`, agent: "claude" });
    await settle();
    const again = await look(maker);
    const treeAfter = await page.evaluate(() => JSON.stringify(stageTree()));
    // 새 판도 그 번호의 불변 스냅샷으로 선다 — 바뀌는 최신 사본이 아니다(t-11959).
    ok(
      "a republish updates the open page in place: no new pane, no new split, the new version's immutable snapshot on the same tab",
      again.opens === 1 && treeAfter === tree && again.browsers.length === 1
        && again.browsers[0].current === 2
        && again.navigations.at(-1) === "file:///tmp/zerocode-window-test/artifacts/versions/p-first/2/index.html"
        && again.unseen?.length === 0,
      JSON.stringify({ again, tree, treeAfter }),
    );

    /* ---- 3. 워커의 발행은 열지 않고 「새 N」만 ------------------------------ */
    await publish("p-worker", 1, { pane: `term-${maker}`, agent: "claude", run: "run-1", worker: "w-1", task: "t-1" });
    await settle();
    const worker = await look(maker);
    ok(
      "a background worker's publication never opens: the maker pane's tab and the sidebar wear 「새 1」 and the status bar stays quiet",
      worker.opens === 1 && worker.browsers.length === 1 && worker.tabPill === "새 1" && worker.navPill === "새 1"
        && worker.notice === "" && worker.unseen?.join() === "p-worker",
      JSON.stringify(worker),
    );

    /* ---- 4. 안내 줄의 「자동으로 열지 않기」 = 설정 끔 → 아무것도 열지 않는다 ---- */
    await page.evaluate(() => {
      const tab = tabs.find((one) => one.kind === "browser" && one.artifact?.id === "p-first");
      if (tab) docHost(tab.pane, "browser").querySelector(".artifact-hint-act")?.click();
    });
    await settle();
    const offHint = await hintOf("p-first");
    await publish("p-off", 1, { pane: `term-${maker}`, agent: "claude" });
    await settle();
    const off = await look(maker);
    const offSetting = await page.evaluate(() => ({
      saved: [...window.__BESIDE__.settings],
      field: document.getElementById("artifacts-auto-open-beside")?.checked ?? null,
    }));
    ok(
      "with automatic opening off — from the hint line or the settings field — a first publish opens nothing and only raises 「새 N」",
      offSetting.saved.join() === "false" && offSetting.field === false
        && offHint.words.includes("자동 열기를 껐습니다") && offHint.act === "되돌리기"
        && off.opens === 1 && off.tabPill === "새 2" && off.navPill === "새 2" && off.notice === "",
      JSON.stringify({ offSetting, offHint, off }),
    );
    await page.evaluate(() => {
      const tab = tabs.find((one) => one.kind === "browser" && one.artifact?.id === "p-first");
      if (tab) docHost(tab.pane, "browser").querySelector(".artifact-hint-act")?.click();
    });
    await settle();
    const undone = await page.evaluate(() => ({
      saved: [...window.__BESIDE__.settings],
      on: typeof artifactsAutoOpenBeside === "undefined" ? null : artifactsAutoOpenBeside,
    }));
    ok("the hint's 「되돌리기」 turns automatic opening back on through the same setting",
      undone.saved.join() === "false,true" && undone.on === true, JSON.stringify(undone));

    /* ---- 5. 만든 판이 화면에 없으면 상태 줄 한 줄, 「옆에 열기」로 그 옆에 ----- */
    // 사람은 만든 판과 같은 그룹의 다른 판으로 넘어갔다 — 만든 판은 가려졌다.
    const other = await seat(first.makerGroup);
    await publish("p-away", 1, { pane: `term-${maker}`, agent: "claude" });
    await settle();
    const away = await look(maker);
    ok(
      "a publish whose maker pane is off screen opens nothing and says one line on the status bar",
      away.opens === 1 && away.notice.includes("p-away 시안") && away.noticeOpen === "옆에 열기"
        && away.tabPill === "새 3" && away.navPill === "새 3" && !away.makerActive && other !== maker,
      JSON.stringify(away),
    );
    await inBothThemes("notice", "#sb-artifact-notice, .tab-new, .nav-new");
    await page.evaluate(() => document.querySelector("#sb-artifact-notice .sb-artifact-notice-open")?.click());
    await settle();
    const opened = await look(maker);
    ok(
      "the status line's 「옆에 열기」 brings the maker pane forward, opens the page beside it and takes the page off 「새 N」",
      opened.opens === 2 && opened.makerActive && opened.notice === ""
        && opened.browsers.some((one) => one.id === "p-away" && one.pane !== opened.makerGroup)
        && opened.tabPill === "새 2" && opened.unseen !== null && !opened.unseen.includes("p-away"),
      JSON.stringify(opened),
    );

    /* ---- 6. 다시 읽어도 본 것과 안 본 것은 남는다 ---------------------------- */
    await page.reload();
    await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
    await install();
    await settle();
    const reloaded = await page.evaluate(() => ({
      nav: document.querySelector("#nav-artifacts .nav-new")?.textContent ?? "",
      unseen: typeof artifactUnseen === "undefined" ? null : [...artifactUnseen.keys()],
    }));
    await page.evaluate(async () => {
      await openArtifactPage({
        id: "p-worker", kind: "page", title: "p-worker 시안", version: 1, modified_ms: Date.now(),
        path: "/tmp/zerocode-window-test/artifacts/pages/p-worker/index.html", origin: {},
      });
    });
    await settle();
    const seenAfter = await page.evaluate(() => ({
      nav: document.querySelector("#nav-artifacts .nav-new")?.textContent ?? "",
      stored: JSON.parse(localStorage.getItem("zerocode.artifacts.unseen.v1") ?? "[]").map((one) => one.id),
    }));
    ok(
      "unseen publications survive a reload, and opening one takes it off the sidebar's 「새 N」 and out of storage",
      reloaded.nav === "새 2" && reloaded.unseen?.join() === "p-worker,p-off"
        && seenAfter.nav === "새 1" && seenAfter.stored.join() === "p-off",
      JSON.stringify({ reloaded, seenAfter }),
    );

    /* ---- 7. 좁은 판은 나누지 않고 같은 그룹의 탭 ----------------------------- */
    await clear();
    await page.setViewportSize({ width: 860, height: 860 });
    const narrowMaker = await seat();
    const narrowBefore = await look(narrowMaker);
    await publish("p-narrow", 1, { pane: `term-${narrowMaker}`, agent: "claude" });
    await settle();
    const narrow = await look(narrowMaker);
    ok(
      "a maker pane narrower than 720px gets the page as a tab in its own group instead of a split",
      narrowBefore.makerWidth > 0 && narrowBefore.makerWidth < 720 && narrow.opens === narrowBefore.opens + 1
        && narrow.groups === narrowBefore.groups
        && narrow.browsers.some((one) => one.id === "p-narrow" && one.pane === narrow.makerGroup),
      JSON.stringify({ narrowBefore, narrow }),
    );
    await page.setViewportSize({ width: 1280, height: 860 });

    /* ---- 8. 주석의 받을 곳은 만든 판, 초안은 쓰던 입력 뒤에 붙고 보내지 않는다 -- */
    const draft = await page.evaluate(async (term) => {
      const elsewhere = await openTermTab({ placement: "tab" });
      paneAgents.set(elsewhere, "codex");
      window.__ANSWER__.agent_terms = () => [[elsewhere, "codex"], [term, "claude"]];
      // 앞 장면이 연 탭 — 없으면(좁은 판 장면이 열지 못했으면) 갤러리의 길로 연다.
      if (!tabs.some((one) => one.kind === "browser" && one.artifact?.id === "p-narrow")) {
        await openArtifactPage({
          id: "p-narrow", kind: "page", title: "p-narrow 시안", version: 1, modified_ms: Date.now(),
          path: "/tmp/zerocode-window-test/artifacts/pages/p-narrow/index.html", origin: { pane: `term-${term}`, agent: "claude" },
        });
      }
      const tab = tabs.find((one) => one.kind === "browser" && one.artifact?.id === "p-narrow");
      browserAnnotations.set(tab.label, [
        { intent: "change", comment: "핀 번호가 카드 글자를 가립니다", selector: ".pin", tag: "div" },
      ]);
      setActiveTab(tab.id);
      await deliverAnnotations(docHost(tab.pane, "browser"), tab);
      await new Promise((done) => setTimeout(done, 80));
      const rows = [...document.querySelectorAll("#note-pop .note-pop-row")];
      const first = rows[0] ?? null;
      const picked = {
        firstIsMaker: first?.classList.contains("is-maker") ?? false,
        firstWords: first?.textContent ?? "",
        focused: document.activeElement === first,
        makers: rows.filter((row) => row.classList.contains("is-maker")).length,
      };
      first?.click();
      await new Promise((done) => setTimeout(done, 120));
      delete window.__ANSWER__.agent_terms;
      return {
        ...picked,
        elsewhere,
        pastes: window.__BESIDE__.pastes.map((one) => ({ term: one.term, ends: one.text.slice(-12), has: one.text.includes("핀 번호가 카드 글자를 가립니다") })),
        prompts: window.__BESIDE__.prompts.length,
      };
    }, narrowMaker);
    ok(
      "an annotation on an artifact defaults to its maker pane, and the draft is pasted after what is typed there — no clearing keys, never sent",
      draft.firstIsMaker && draft.makers === 1 && draft.focused && draft.firstWords.includes("만든 에이전트")
        && draft.pastes.length === 1 && draft.pastes[0].term === narrowMaker && draft.pastes[0].has
        && !draft.pastes[0].ends.includes("\r") && draft.prompts === 0,
      JSON.stringify(draft),
    );
    ok("the window raised no errors", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}

/* 머리띠(t-11959): 만든 이 한 줄, 첫 행동 「주석」, 「피드백 N · vN」, 「나란히」,
 * 「내보내기」, 그리고 480px 아래에서 단추를 한 메뉴로 접는 것. 주석을 사람이 고른
 * 판에 초안으로 넣은 순간 그 기록이 백엔드의 문(`artifact_feedback_record`)으로
 * 간다 — 보낸 것은 없다(Enter도 `send_prompt`도 없음). */
export async function testArtifactBand(browser, origin, ok, outputDir) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  const axeModule = createRequire(import.meta.url)("@axe-core/playwright");
  const AxeBuilder = axeModule.default ?? axeModule;
  await mkdir(outputDir, { recursive: true });
  const shoot = async (name, include) => {
    for (const theme of ["dark", "light"]) {
      await page.evaluate((next) => document.documentElement.setAttribute("data-theme", next), theme);
      await new Promise((done) => setTimeout(done, 300));
      await page.screenshot({ path: join(outputDir, `artifacts-r3-${theme}-${name}.png`) });
      const violations = await axeViolations(page, AxeBuilder, include).catch((error) => [String(error)]);
      ok(`the band (${name}) passes axe in ${theme}`, violations.length === 0, JSON.stringify(violations));
    }
    await page.evaluate(() => document.documentElement.setAttribute("data-theme", "dark"));
  };
  try {
    await page.setViewportSize({ width: 1440, height: 860 });
    await page.evaluate(() => {
      window.__BAND__ = { opens: [], navigations: [], pastes: [], prompts: [], records: [], exports: [], folders: 0 };
      let born = 0;
      window.__ANSWER__.open_browser_pane = (args) => {
        born += 1;
        window.__BAND__.opens.push(args.url);
        return `browser-band-${born}`;
      };
      window.__ANSWER__.browser_zoom = () => null;
      window.__ANSWER__.browser_navigate = (args) => (window.__BAND__.navigations.push({ ...args }), null);
      window.__ANSWER__.artifact_versions = (args) => Array.from({ length: args.id === "p-lone" ? 1 : 3 }, (_, at) => ({
        n: at + 1,
        path: `/tmp/zerocode-window-test/artifacts/pages/${args.id}/v${at + 1}/index.html`,
        sha256: `sha-${at + 1}`,
      }));
      window.__ANSWER__.term_paste = (args) => (window.__BAND__.pastes.push({ ...args }), null);
      window.__ANSWER__.send_prompt = (args) => (window.__BAND__.prompts.push({ ...args }), null);
      window.__ANSWER__.artifact_feedback_record = (args) => {
        window.__BAND__.records.push(JSON.parse(JSON.stringify(args)));
        return { count: 2 + window.__BAND__.records.length, version: args.feedback.version };
      };
      window.__ANSWER__.choose_project = () => ((window.__BAND__.folders += 1), "/tmp/zerocode-window-test/shared");
      window.__ANSWER__.artifact_export = (args) => {
        window.__BAND__.exports.push({ ...args });
        return { id: args.id, version: args.version, path: `${args.folder}/card-v${args.version}.html`, bytes: 10 };
      };
      for (const tab of [...tabs]) dropTab(tab.id);
      renderTabs();
      updateStage();
    });
    const settle = () => page.evaluate(() => new Promise((done) => setTimeout(done, 160)));
    // 만든 판 하나 — 발행보다 먼저 태어난 claude 판.
    const maker = await page.evaluate(async () => {
      const made = await openTermTab({ placement: "tab" });
      paneAgents.set(made, "claude");
      renderTabs();
      return made;
    });
    await settle();
    const row = (id, version) => ({
      id, kind: "page", title: `${id} 카드 시안`, bytes: 10, created_ms: Date.now(), modified_ms: Date.now(), version,
      path: `/tmp/zerocode-window-test/artifacts/pages/${id}/index.html`,
      url: `file:///tmp/zerocode-window-test/artifacts/pages/${id}/index.html`,
      source_path: `/tmp/zerocode-window-test/${id}.html`,
      origin: { pane: `term-${maker}`, agent: "claude", worktree: "/tmp/zerocode-window-test/orbit-card" },
      feedback_count: 2, feedback_version: 2, tags: [], preview: { kind: "text", text: "" }, source: "manual",
    });
    await page.evaluate(async (made) => { await openArtifactPage(made); }, row("p-band", 3));
    await settle();
    const band = (id) => page.evaluate((key) => {
      const shownAt = (node) => Boolean(node) && !node.hidden && getComputedStyle(node).display !== "none"
        && node.getBoundingClientRect().width > 0;
      return tabs.filter((one) => one.kind === "browser" && one.artifact?.id === key).map((tab) => {
        const strip = docHost(tab.pane, "browser").querySelector(".artifact-strip");
        const q = (selector) => strip.querySelector(selector);
        const actions = [...strip.querySelectorAll(".artifact-strip-actions > button")];
        return {
          tab: tab.id,
          pane: tab.pane,
          url: tab.url,
          width: Math.round(strip.getBoundingClientRect().width),
          overflows: strip.scrollWidth > strip.clientWidth + 1,
          maker: shownAt(q(".artifact-strip-maker")) ? q(".artifact-strip-maker").textContent.trim() : "",
          makerMark: shownAt(q(".artifact-strip-maker .artifact-card-maker-mark svg, .artifact-strip-maker .artifact-card-maker-mark img, .artifact-strip-maker .artifact-card-maker-mark .agent-ico")),
          feedback: shownAt(q(".artifact-strip-feedback")) ? q(".artifact-strip-feedback").textContent.trim() : "",
          annotate: shownAt(q(".artifact-strip-annotate")) ? q(".artifact-strip-annotate").textContent.trim() : "",
          annotatePrimary: q(".artifact-strip-annotate")?.classList.contains("is-primary") ?? false,
          annotatePressed: q(".artifact-strip-annotate")?.getAttribute("aria-pressed") ?? "",
          firstAction: actions.find(shownAt)?.className ?? "",
          compare: shownAt(q(".artifact-strip-compare")) ? q(".artifact-strip-compare").textContent.trim() : "",
          compareDisabled: q(".artifact-strip-compare")?.disabled ?? null,
          exportWord: shownAt(q(".artifact-strip-export")) ? q(".artifact-strip-export").textContent.trim() : "",
          share: shownAt(q(".artifact-strip-share")),
          reveal: shownAt(q(".artifact-strip-reveal")),
          more: shownAt(q(".artifact-strip-more")),
          version: q(".artifact-strip-version")?.value ?? "",
        };
      });
    }, id);

    /* ---- 1. 넓은 머리띠: 만든 이, 「주석」이 첫 행동, 피드백 수, 나란히, 내보내기 ---- */
    const [wide] = await band("p-band");
    ok(
      "the header band names its maker (agent tile, pane, branch), leads with 「주석」, says 「피드백 2 · v2」 and offers 「나란히」 and 「내보내기」",
      wide !== undefined && wide.maker.includes("claude") && wide.maker.includes(`term:${maker}`)
        && wide.maker.includes("orbit-card") && wide.makerMark
        && wide.annotate === "주석" && wide.annotatePrimary && wide.firstAction.includes("artifact-strip-annotate")
        && wide.feedback === "피드백 2 · v2" && wide.compare === "나란히" && wide.compareDisabled === false
        && wide.exportWord === "내보내기" && wide.share && wide.reveal && !wide.more && !wide.overflows
        && wide.url.endsWith("/pages/p-band/v3/index.html"),
      JSON.stringify(wide),
    );
    await shoot("band-wide", ".artifact-strip:not([hidden])");

    /* ---- 2. 「주석」은 주석 모드를 켜고 끈다 -------------------------------------- */
    const armed = await page.evaluate(async () => {
      const tab = tabs.find((one) => one.kind === "browser" && one.artifact?.id === "p-band");
      const strip = docHost(tab.pane, "browser").querySelector(".artifact-strip");
      strip.querySelector(".artifact-strip-annotate").click();
      await new Promise((done) => setTimeout(done, 60));
      const on = { mode: browserGrab?.mode ?? null, pressed: strip.querySelector(".artifact-strip-annotate").getAttribute("aria-pressed") };
      strip.querySelector(".artifact-strip-annotate").click();
      await new Promise((done) => setTimeout(done, 60));
      return { on, off: { mode: browserGrab?.mode ?? null, pressed: strip.querySelector(".artifact-strip-annotate").getAttribute("aria-pressed") } };
    });
    ok("the band's 「주석」 arms the annotation crosshair on this page and disarms it again",
      armed.on.mode === "annotate" && armed.on.pressed === "true" && armed.off.mode === null && armed.off.pressed === "false",
      JSON.stringify(armed));

    /* ---- 3. 초안으로 넣은 주석은 기록되고, 머리띠의 수가 오른다 --------------------- */
    const delivered = await page.evaluate(async (term) => {
      window.__ANSWER__.agent_terms = () => [[term, "claude"]];
      const tab = tabs.find((one) => one.kind === "browser" && one.artifact?.id === "p-band");
      const host = docHost(tab.pane, "browser");
      const deliver = async (notes) => {
        browserAnnotations.set(tab.label, notes);
        setActiveTab(tab.id);
        await deliverAnnotations(host, tab);
        await new Promise((done) => setTimeout(done, 80));
        document.querySelector("#note-pop .note-pop-row")?.click();
        await new Promise((done) => setTimeout(done, 160));
      };
      await deliver([
        { intent: "change", comment: "핀 번호가 카드 글자를 가립니다", selector: ".pin", tag: "div" },
        { intent: "question", comment: "답하기가 너무 작지 않나요?", selector: ".e-reply", tag: "button" },
      ]);
      const after = {
        records: window.__BAND__.records.slice(),
        pastes: window.__BAND__.pastes.length,
        prompts: window.__BAND__.prompts.length,
        pasteEndsInEnter: /[\r\n]$/.test(window.__BAND__.pastes.at(-1)?.text ?? ""),
      };
      // 초안을 받은 판이 앞으로 왔다 — 페이지로 돌아오면 머리띠가 새 수를 말한다.
      setActiveTab(tab.id);
      await window.__PAINTED__();
      after.feedback = host.querySelector(".artifact-strip-feedback")?.textContent.trim() ?? "";
      // 링크를 따라 아티팩트 밖으로 나간 페이지의 주석도 떠나온 판에 그 주소를 붙여
      // 기록한다(t-14586) — 스토어의 발행물이 아닌 주소라 스토어에 묻지 않는다.
      const was = tab.url;
      tab.url = "https://example.com/elsewhere";
      await deliver([{ intent: "change", comment: "바깥 페이지", selector: "h1", tag: "h1" }]);
      tab.url = was;
      delete window.__ANSWER__.agent_terms;
      return {
        ...after,
        recordsAfterElsewhere: window.__BAND__.records.length,
        elsewhere: window.__BAND__.records.at(-1)?.feedback ?? null,
        pastesAfterElsewhere: window.__BAND__.pastes.length,
      };
    }, maker);
    const record = delivered.records[0]?.feedback ?? null;
    ok(
      "annotations pasted into the chosen pane are recorded once — id, the version they were made on, their selectors and comments, the receiving pane and its agent — and the band says 「피드백 3 · v3」; nothing was sent",
      delivered.records.length === 1 && record?.id === "p-band" && record?.version === 3
        && JSON.stringify(record?.items) === JSON.stringify([
          { selector: ".pin", comment: "핀 번호가 카드 글자를 가립니다" },
          { selector: ".e-reply", comment: "답하기가 너무 작지 않나요?" },
        ])
        && record?.recipient?.pane === `term-${maker}` && record?.recipient?.agent === "claude"
        && Object.keys(record ?? {}).sort().join() === "id,items,recipient,version"
        && delivered.pastes === 1 && delivered.prompts === 0 && !delivered.pasteEndsInEnter
        && delivered.feedback === "피드백 3 · v3",
      JSON.stringify(delivered),
    );
    ok("annotations on a page the tab followed away from the artifact are pasted and recorded on the version it left, with that page's address",
      delivered.pastesAfterElsewhere === 2 && delivered.recordsAfterElsewhere === 2
        && delivered.elsewhere?.id === "p-band" && delivered.elsewhere?.version === 3
        && delivered.elsewhere?.page_url === "https://example.com/elsewhere",
      JSON.stringify(delivered));

    /* ---- 4. 「내보내기」: 폴더를 묻고 본 판을 쓰며, 공개라고 말하지 않는다 ---------- */
    const exported = await page.evaluate(async () => {
      const tab = tabs.find((one) => one.kind === "browser" && one.artifact?.id === "p-band");
      setActiveTab(tab.id);
      document.querySelectorAll(".toast").forEach((one) => one.remove());
      docHost(tab.pane, "browser").querySelector(".artifact-strip-export").click();
      await new Promise((done) => setTimeout(done, 160));
      return {
        folders: window.__BAND__.folders,
        exports: window.__BAND__.exports.slice(),
        toast: [...document.querySelectorAll(".toast")].map((one) => one.textContent).join(" | "),
      };
    });
    ok(
      "「내보내기」 asks for a folder and exports the immutable version on screen there, and its notice names the local file without calling it public",
      exported.folders === 1 && exported.exports.length === 1 && exported.exports[0].id === "p-band"
        && exported.exports[0].version === 3 && exported.exports[0].folder === "/tmp/zerocode-window-test/shared"
        && exported.toast.includes("/tmp/zerocode-window-test/shared/card-v3.html")
        && exported.toast.includes("내보냈습니다") && !/공개|public|공유 링크/i.test(exported.toast),
      JSON.stringify(exported),
    );

    /* ---- 5. 「나란히」: 두 불변 판이 나란히 선다 ----------------------------------- */
    const groupsBefore = await page.evaluate(() => stageGroups().length);
    await page.evaluate(() => {
      const tab = tabs.find((one) => one.kind === "browser" && one.artifact?.id === "p-band");
      setActiveTab(tab.id);
      docHost(tab.pane, "browser").querySelector(".artifact-strip-compare").click();
    });
    await page.evaluate(() => new Promise((done) => setTimeout(done, 400)));
    const pair = await band("p-band");
    const groupsAfter = await page.evaluate(() => stageGroups().length);
    const pairUrls = pair.map((one) => one.url).sort();
    ok(
      "「나란히」 stands the version before the one on screen beside it: two tabs of the same artifact in two groups, each on its own immutable snapshot, never the mutable latest copy",
      pair.length === 2 && pair[0].pane !== pair[1].pane && groupsAfter === groupsBefore + 1
        && pairUrls[0].endsWith("/pages/p-band/v2/index.html") && pairUrls[1].endsWith("/pages/p-band/v3/index.html")
        && pair.map((one) => one.version).sort().join() === "2,3"
        && !pairUrls.some((url) => url.endsWith("/pages/p-band/index.html")),
      JSON.stringify({ pair, groupsBefore, groupsAfter }),
    );
    await shoot("side-by-side", ".artifact-strip:not([hidden])");
    const again = await page.evaluate(async () => {
      const tab = tabs.find((one) => one.kind === "browser" && one.artifact?.id === "p-band" && one.artifact?.version === 3);
      const opens = window.__BAND__.opens.length;
      docHost(tab.pane, "browser").querySelector(".artifact-strip-compare").click();
      await new Promise((done) => setTimeout(done, 200));
      return { opens: window.__BAND__.opens.length - opens, tabs: tabs.filter((one) => one.artifact?.id === "p-band").length };
    });
    ok("a second 「나란히」 with the pair already standing opens nothing new", again.opens === 0 && again.tabs === 2, JSON.stringify(again));

    /* ---- 6. 판이 하나뿐이면 나란히 볼 것이 없다 ---------------------------------- */
    await page.evaluate(async (made) => { await openArtifactPage(made); }, row("p-lone", 1));
    await settle();
    const [lone] = await band("p-lone");
    ok("a publication with one kept version offers 「나란히」 disabled", lone?.compareDisabled === true, JSON.stringify(lone));

    /* ---- 7. 480px 아래: 단추는 한 메뉴로 접히고 모든 행동이 닿는다 ------------------- */
    await page.evaluate(() => {
      for (const tab of tabs.filter((one) => one.kind === "browser" && one.artifact?.id !== "p-band")) dropTab(tab.id);
      const held = tabs.filter((one) => one.kind === "browser" && one.artifact?.id === "p-band");
      for (const tab of held.slice(1)) dropTab(tab.id);
      for (const group of stageGroups()) collapseStageGroup(group);
      setActiveTab(held[0].id);
      renderTabs();
      updateStage();
    });
    await page.setViewportSize({ width: 720, height: 860 });
    await settle();
    const [narrow] = await band("p-band");
    ok(
      "under 480px the band folds 「나란히」, 「내보내기」, 「공유」 and Finder into one menu button, keeps 「주석」 and 「피드백 N · vN」, and nothing runs off its edge",
      narrow !== undefined && narrow.width < 480 && narrow.width > 0 && narrow.more && narrow.annotate === "주석"
        && narrow.compare === "" && narrow.exportWord === "" && !narrow.share && !narrow.reveal
        && narrow.feedback.startsWith("피드백") && !narrow.overflows,
      JSON.stringify(narrow),
    );
    const menu = await page.evaluate(async () => {
      const tab = tabs.find((one) => one.kind === "browser" && one.artifact?.id === "p-band");
      const strip = docHost(tab.pane, "browser").querySelector(".artifact-strip");
      strip.querySelector(".artifact-strip-more").click();
      await new Promise((done) => setTimeout(done, 80));
      const rows = [...document.querySelectorAll("#sidebar-menu .sidebar-menu-item")];
      const words = rows.map((one) => one.textContent.trim());
      const expanded = strip.querySelector(".artifact-strip-more").getAttribute("aria-expanded");
      const exports = window.__BAND__.exports.length;
      rows.find((one) => one.textContent.includes("내보내기"))?.click();
      await new Promise((done) => setTimeout(done, 160));
      return { words, expanded, exported: window.__BAND__.exports.length - exports };
    });
    ok(
      "the folded menu holds every action the band hid — 「나란히」, 「내보내기」, 「공유」, 「Finder에서 보기」 — and its 「내보내기」 acts",
      ["나란히", "내보내기", "공유", "Finder에서 보기"].every((word) => menu.words.some((one) => one.includes(word)))
        && menu.expanded === "true" && menu.exported === 1,
      JSON.stringify(menu),
    );
    await shoot("band-narrow", ".artifact-strip:not([hidden])");
    ok("the window raised no errors", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}

/* 따라간 페이지의 주석(t-14586): 발행물 탭이 링크를 따라 다른 곳으로 가도 전달한
 * 주석은 기록된다. 따라간 페이지가 스토어의 발행물이면 그 발행물의 그 판에, 아니면
 * 떠나온 발행물의 떠날 때 보이던 판에 그 주소(`page_url`)를 붙여서. 초안은 어느
 * 페이지에 단 주석인지 말한다. 아티팩트를 보인 적 없는 탭은 아무것도 적지 않는다.
 * 스토어는 이 창 시험의 가짜다: `feedback.jsonl`은 발행물마다 JSON 줄의 배열로
 * 서고, 기록 문은 청의 필드를 Rust의 `deny_unknown_fields`처럼 가린다. */
export async function testArtifactFollowed(browser, origin, ok, outputDir) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  await mkdir(outputDir, { recursive: true });
  const store = "/tmp/zerocode-window-test/artifacts/pages";
  const plainUrl = "file:///tmp/zerocode-window-test/notes/plain.html";
  const directUrl = "file:///tmp/zerocode-window-test/notes/direct.html";
  try {
    await page.setViewportSize({ width: 1440, height: 860 });
    const maker = await page.evaluate(async (pages) => {
      const now = Date.now();
      const row = (id, version, count, last) => ({
        id, kind: "page", title: `${id} 시안`, bytes: 10, created_ms: now, modified_ms: now, version,
        path: `${pages}/${id}/index.html`, url: `file://${pages}/${id}/index.html`,
        source_path: `/tmp/zerocode-window-test/${id}.html`,
        origin: { pane: "term-1", agent: "claude", worktree: "/tmp/zerocode-window-test/orbit-card" },
        feedback_count: count, feedback_version: last, tags: [], preview: { kind: "text", text: "" }, source: "manual",
      });
      const oldLine = JSON.stringify({ id: "p-origin", version: 2, sha256: null, source_path: null,
        recipient: { pane: "term-2", agent: "claude" }, items: [{ selector: ".pin", comment: "옛 줄" }], at_ms: 1 });
      window.__FOLLOW__ = {
        rows: { "p-origin": row("p-origin", 3, 1, 2), "p-other": row("p-other", 2, 0, null) },
        files: { "p-origin": [oldLine], "p-other": [] },
        asks: [], refused: [], pastes: [], prompts: [], pageAt: [],
      };
      const held = window.__FOLLOW__;
      let born = 0;
      window.__ANSWER__.open_browser_pane = () => `browser-follow-${(born += 1)}`;
      window.__ANSWER__.browser_zoom = () => null;
      window.__ANSWER__.browser_navigate = () => null;
      window.__ANSWER__.artifact_versions = (args) => Array.from({ length: held.rows[args.id]?.version ?? 0 }, (_, at) => ({
        n: at + 1, path: `${pages}/${args.id}/v${at + 1}/index.html`, sha256: `sha-${args.id}-${at + 1}`,
      }));
      window.__ANSWER__.artifact_page_at = (args) => {
        held.pageAt.push(args.path);
        const found = new RegExp(`^${pages}/([^/]+)/(?:v(\\d+)/)?index\\.html$`).exec(args.path);
        if (found === null || !held.rows[found[1]]) return null;
        return { artifact: { ...held.rows[found[1]] }, version: found[2] ? Number(found[2]) : null };
      };
      window.__ANSWER__.artifact_feedback_record = (args) => {
        const ask = JSON.parse(JSON.stringify(args.feedback));
        const unknown = Object.keys(ask).filter((key) => !["id", "version", "items", "recipient", "page_url"].includes(key));
        if (unknown.length > 0 || !held.rows[ask.id] || ask.version > held.rows[ask.id].version) {
          held.refused.push(ask);
          throw new Error(`refused ${unknown.join()}`);
        }
        held.asks.push(ask);
        const line = { id: ask.id, version: ask.version, sha256: `sha-${ask.id}-${ask.version}`, source_path: held.rows[ask.id].source_path,
          recipient: ask.recipient, items: ask.items, at_ms: Date.now() };
        if (ask.page_url !== undefined) line.page_url = ask.page_url;
        held.files[ask.id].push(JSON.stringify(line));
        return { count: held.files[ask.id].length, version: ask.version };
      };
      window.__ANSWER__.term_paste = (args) => (held.pastes.push({ ...args }), null);
      window.__ANSWER__.send_prompt = (args) => (held.prompts.push({ ...args }), null);
      for (const tab of [...tabs]) dropTab(tab.id);
      renderTabs();
      updateStage();
      const made = await openTermTab({ placement: "tab" });
      paneAgents.set(made, "claude");
      renderTabs();
      return made;
    }, store);
    const settle = () => page.evaluate(() => new Promise((done) => setTimeout(done, 160)));
    await settle();
    // 둘째 발행물은 제 탭에도 서 있다 — 그 탭의 머리띠가 새 수를 받는지 본다.
    await page.evaluate(async () => { await openArtifactPage(window.__FOLLOW__.rows["p-other"]); });
    await settle();
    await page.evaluate(async () => { await openArtifactPage(window.__FOLLOW__.rows["p-origin"]); });
    await settle();

    const follow = (url) => page.evaluate(async (next) => {
      const tab = tabs.find((one) => one.kind === "browser" && one.artifact?.id === "p-origin");
      for (const speak of window.__LISTENERS__["browser:nav"] ?? []) {
        speak({ payload: { label: tab.label, url: next, state: "finished" } });
      }
      await new Promise((done) => setTimeout(done, 60));
      return tab.url;
    }, url);
    const deliver = (find, comment) => page.evaluate(async ({ term, find, comment }) => {
      window.__ANSWER__.agent_terms = () => [[term, "claude"]];
      const tab = find === "origin"
        ? tabs.find((one) => one.kind === "browser" && one.artifact?.id === "p-origin")
        : tabs.find((one) => one.kind === "browser" && one.url === find);
      const host = docHost(tab.pane, "browser");
      const held = window.__FOLLOW__;
      const before = { asks: held.asks.length, pastes: held.pastes.length };
      browserAnnotations.set(tab.label, [{ intent: "change", comment, selector: "main h1", tag: "h1" }]);
      setActiveTab(tab.id);
      await deliverAnnotations(host, tab);
      await new Promise((done) => setTimeout(done, 80));
      document.querySelector("#note-pop .note-pop-row")?.click();
      await new Promise((done) => setTimeout(done, 160));
      delete window.__ANSWER__.agent_terms;
      setActiveTab(tab.id);
      await window.__PAINTED__();
      const bandOf = (one) => {
        const node = docHost(one.pane, "browser").querySelector(".artifact-strip-feedback");
        return node && !node.hidden ? node.textContent.trim() : "";
      };
      const band = bandOf(tab);
      // 둘째 발행물의 탭은 앞으로 불러야 제 머리띠를 칠한다 — 읽고 나서 돌아온다.
      const other = tabs.find((one) => one.kind === "browser" && one.artifact?.id === "p-other"
        && one.url.endsWith("/p-other/v2/index.html"));
      let otherBand = null;
      if (other) {
        setActiveTab(other.id);
        await window.__PAINTED__();
        otherBand = bandOf(other);
        setActiveTab(tab.id);
        await window.__PAINTED__();
      }
      const paste = held.pastes.at(-1)?.text ?? "";
      return {
        url: tab.url,
        asks: held.asks.slice(before.asks),
        refused: held.refused.length,
        pasted: held.pastes.length - before.pastes,
        paste,
        pasteEndsInEnter: /[\r\n]$/.test(paste),
        prompts: held.prompts.length,
        originFile: held.files["p-origin"].slice(),
        otherFile: held.files["p-other"].slice(),
        band,
        otherBand,
      };
    }, { term: maker, find, comment });

    /* ---- 1. 다른 발행물로 따라갔다: 그 발행물의 그 판에 기록한다 ---------------- */
    const otherUrl = `file://${store}/p-other/v1/index.html`;
    await follow(otherUrl);
    const onOther = await deliver("origin", "둘째 시안의 제목이 잘립니다");
    const otherLine = onOther.otherFile.at(-1) ? JSON.parse(onOther.otherFile.at(-1)) : null;
    ok(
      "annotations on another publication the tab followed to are recorded on that publication and that version, and its own tab's band says 「피드백 1 · v1」; the origin's count stays",
      onOther.url === otherUrl && onOther.asks.length === 1 && onOther.otherFile.length === 1
        && otherLine?.id === "p-other" && otherLine?.version === 1 && !("page_url" in (otherLine ?? {}))
        && otherLine?.items?.[0]?.comment === "둘째 시안의 제목이 잘립니다"
        && onOther.originFile.length === 1 && onOther.band === "피드백 1 · v2" && onOther.otherBand === "피드백 1 · v1",
      JSON.stringify(onOther),
    );
    ok(
      "the draft for another publication names that publication and its immutable version, not the origin",
      onOther.paste.includes("p-other 시안") && onOther.paste.includes("(p-other)") && onOther.paste.includes("버전 1")
        && onOther.paste.includes("sha-p-other-1") && !onOther.paste.includes("(p-origin)"),
      onOther.paste,
    );

    /* ---- 2. 발행물이 아닌 로컬 파일로 따라갔다: 떠나온 판에 주소를 붙여 적는다 ---- */
    await follow(plainUrl);
    const onPlain = await deliver("origin", "따라간 노트의 표가 넘칩니다");
    const plainLine = onPlain.originFile.at(-1) ? JSON.parse(onPlain.originFile.at(-1)) : null;
    ok(
      "annotations on a plain page the tab followed away to are recorded on the artifact it left, on the version it showed, with the page's address — and the band says 「피드백 2 · v3」",
      onPlain.url === plainUrl && onPlain.asks.length === 1 && onPlain.originFile.length === 2
        && plainLine?.id === "p-origin" && plainLine?.version === 3 && plainLine?.page_url === plainUrl
        && plainLine?.items?.[0]?.comment === "따라간 노트의 표가 넘칩니다"
        && JSON.parse(onPlain.originFile[0]).page_url === undefined
        && onPlain.band === "피드백 2 · v3" && onPlain.refused === 0,
      JSON.stringify(onPlain),
    );
    ok(
      "the draft says which page the notes were made on and that it is not the artifact's file",
      onPlain.paste.includes(`주석을 단 페이지: ${plainUrl}`) && onPlain.paste.includes("(p-origin)의 버전 3에서 링크를 따라간 페이지")
        && !onPlain.paste.includes("고칠 원본"),
      onPlain.paste,
    );
    ok(
      "every followed-page delivery is pasted without Enter and nothing is sent",
      onOther.pasted === 1 && onPlain.pasted === 1 && !onOther.pasteEndsInEnter && !onPlain.pasteEndsInEnter && onPlain.prompts === 0,
      JSON.stringify({ onOther: onOther.pasted, onPlain: onPlain.pasted, prompts: onPlain.prompts }),
    );
    for (const theme of ["dark", "light"]) {
      await page.evaluate((next) => document.documentElement.setAttribute("data-theme", next), theme);
      await new Promise((done) => setTimeout(done, 300));
      await page.screenshot({ path: join(outputDir, `artifacts-followed-${theme}-band.png`) });
    }
    await page.evaluate(() => document.documentElement.setAttribute("data-theme", "dark"));

    /* ---- 3. 아티팩트를 보인 적 없는 탭: 판에는 넣되 아무것도 적지 않는다 --------- */
    await page.evaluate(async (url) => { await openBrowserTab(url); }, directUrl);
    await settle();
    const direct = await deliver(directUrl, "직접 연 파일");
    ok(
      "a tab that never showed an artifact (a local HTML file opened directly) pastes its notes and records nothing",
      direct.pasted === 1 && direct.asks.length === 0 && direct.originFile.length === 2 && direct.otherFile.length === 1
        && !direct.paste.includes("아티팩트"),
      JSON.stringify(direct),
    );
    ok("the window raised no errors", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}

/* 다른 프로젝트의 문서 (t-16006): 갤러리는 모든 프로젝트의 아티팩트를 늘어놓는데, 열린
 * 프로젝트의 파일 문(`read_text_file`)은 제 뿌리 밖의 경로를 「path escapes the project」로
 * 막는다 — 막아야 하는 문이다. 그래서 밖의 문서는 저장소가 id로 읽어 주는 읽기 전용 탭으로
 * 열고(`artifact_document`: 창은 id와 버전 번호만 보낸다), 안의 문서는 지금처럼 고칠 수 있는
 * 파일 탭으로 연다. 더블클릭도 Enter도 한 번에 한 번만 열고, 실패해도 알림은 하나다. 머리띠의
 * 버전 선택기가 보관 스냅샷(앱 데이터 폴더)을 읽는 길도 같은 문을 지난다. 목의 `read_text_file`은
 * 백엔드와 같은 말로 뿌리 밖을 막으므로, 옛 길로 열면 이 시험은 그 알림으로 빨개진다. 미리보기
 * 탭은 서로를 대신하므로(한 번에 하나) 문서마다 열자마자 잰다. */
export async function testArtifactOtherProject(browser, origin, ok, outputDir) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  await mkdir(outputDir, { recursive: true });
  try {
    await page.setViewportSize({ width: 1440, height: 860 });
    await page.evaluate(async () => {
      const root = "/tmp/zerocode-window-test";
      const other = "/tmp/zerocode-other-project";
      const versions = "/data/artifacts/versions";
      const now = Date.now();
      const line = (name) => `# ${name}\n\n본문 **굵게** 한 줄\n\n[다음 문서](./next.md) ![그림](./pic.png) [맨 위로](#top)\n`;
      const rows = [
        { id: "doc-out", path: `${other}/output/ledger/zo-parallel-flakes.md`, text: line("병렬 부하에서만 빨개지는 시험") },
        { id: "doc-key", path: `${other}/output/ledger/keyboard.md`, text: line("키보드로 연 문서") },
        { id: "doc-far", path: "/tmp/zerocode-elsewhere/notes/plan.md", text: line("창이 모르는 폴더의 문서") },
        { id: "doc-hist", path: `${other}/output/history.md`, text: line("고쳐 온 문서") },
        { id: "doc-gone", path: `${other}/output/gone.md`, text: "" },
        { id: "doc-in", path: `${root}/project/inside.md`, text: line("열린 프로젝트 안의 문서") },
      ];
      const held = new Map(rows.map((row) => [row.id, row]));
      const inside = (path) => path === root || path.startsWith(`${root}/`);
      const seen = (window.__OTHER__ = { reads: [], images: [], docs: [], writes: [] });
      window.__ANSWER__.artifacts_list = () => ({
        rows: rows.map((row, at) => ({
          id: row.id, kind: "document", title: row.path.split("/").at(-1), path: row.path, bytes: row.text.length,
          created_ms: now - at, modified_ms: now - at, tags: [], source: "agent_page",
          origin: { agent: "claude", session: `s-${row.id}`, project: inside(row.path) ? root : other },
          preview: { kind: "markdown", text: row.text.slice(0, 60) },
        })),
        total: rows.length, truncated: false, missing: [], missing_total: 0,
        thumb: { width: 320, height: 240, queue_max: 24 },
      });
      // 프로젝트의 파일 문: 백엔드의 `resolve_in_project`와 같은 말로 뿌리 밖을 막는다.
      window.__ANSWER__.read_text_file = (args) => {
        seen.reads.push(args.path);
        if (!inside(args.path)) throw "path escapes the project";
        return { text: rows.find((row) => row.path === args.path)?.text ?? "# 파일\n", version: "v1" };
      };
      window.__ANSWER__.read_image_file = (args) => {
        seen.images.push(args.path);
        throw "path escapes the project";
      };
      window.__ANSWER__.write_text_file = (args) => {
        seen.writes.push({ path: args.path, text: args.text });
        return "v-saved";
      };
      // 도장이 그대로인 파일: 하네스의 `file_version`은 null이라 「파일이 움직였다」로 읽혀 다시 읽는다.
      window.__ANSWER__.file_version = () => "v1";
      // 저장소의 문: 창은 id와 버전 번호만 보낸다. 사라진 파일은 한 문장으로 거절한다.
      window.__ANSWER__.artifact_document = (args) => {
        seen.docs.push({ id: args.id, version: args.version ?? null, keys: Object.keys(args).sort().join(",") });
        const row = held.get(args.id);
        if (!row || row.id === "doc-gone") throw "아티팩트 파일이 사라졌습니다";
        const text = args.version == null ? row.text : `# 옛 판\n\n${row.id}의 버전 ${args.version} 본문\n`;
        return { text, bytes: text.length, truncated: false, in_project: inside(row.path) };
      };
      window.__ANSWER__.artifact_versions = (args) => (args.id === "doc-hist" || args.id === "doc-in"
        ? [1, 2].map((n) => ({ n, path: `${versions}/${args.id}/${n}/history.md`, bytes: 10, modified_ms: n, sha256: String(n).repeat(64) }))
        : []);
      // 창이 아는 프로젝트 하나: 밖의 문서가 사는 폴더의 이름을 머리띠가 말한다.
      projects = [
        { name: "harbor", path: other, worktrees: [{ path: other, branch: "main", base: "", is_main: true, active: false, is_folder: false }] },
        ...projects,
      ];
      dropTab("artifacts");
      artifactFilter.tab = "pages";
      artifactTabPicked = false;
      el("nav-artifacts").click();
      await window.__PAINTED__();
      await new Promise((done) => setTimeout(done, 200));
    });
    const settle = (ms = 400) => page.evaluate((wait) => new Promise((done) => setTimeout(done, wait)), ms);
    const clear = () => page.evaluate(() => document.querySelectorAll(".toast").forEach((one) => one.remove()));
    // 갤러리를 앞으로 — 앞선 열림이 문서 탭을 앞에 세웠다.
    const gallery = async () => {
      await page.evaluate(() => openArtifacts());
      await settle(150);
    };
    const look = (id) => page.evaluate((artifactId) => {
      const seen = window.__OTHER__;
      const tab = tabs.find((one) => one.kind === "file" && one.artifact?.id === artifactId);
      const view = tab ? docHost(tab.pane, "file") : null;
      const strip = view?.querySelector(".file-view-head .artifact-strip");
      return {
        toasts: [...document.querySelectorAll(".toast")].map((one) => one.textContent.trim()),
        tabs: tabs.filter((one) => one.artifact?.id === artifactId).length,
        readOnly: tab?.readOnly === true,
        editable: typeof tab?.version === "string",
        active: tab ? activeTabId === tab.id : false,
        body: view?.querySelector(".file-body")?.textContent ?? "",
        editorHidden: view?.querySelector(".file-edit")?.hidden ?? null,
        note: view?.querySelector(".file-view-note")?.textContent ?? "",
        strip: strip && !strip.hidden
          ? { title: strip.querySelector(".artifact-strip-title").textContent, project: strip.querySelector(".artifact-strip-project").textContent }
          : null,
        opened: seen.docs.filter((one) => one.id === artifactId && one.version === null).length,
        reads: seen.reads.length,
        images: seen.images.length,
        path: tab?.path ?? null,
      };
    }, id);
    // 머리띠의 버전 선택기에서 하나를 고른다 — 보관 스냅샷은 저장소의 문으로 읽어야 한다.
    const pick = (id, value) => page.evaluate(async ({ artifactId, next }) => {
      const tab = tabs.find((one) => one.kind === "file" && one.artifact?.id === artifactId);
      if (!tab) return { missing: true, options: [], body: "", readsAfter: -1, versionDocs: [], toasts: [] };
      setActiveTab(tab.id);
      await window.__PAINTED__();
      const view = docHost(tab.pane, "file");
      const select = view.querySelector(".file-view-head .artifact-strip-version");
      const reads = window.__OTHER__.reads.length;
      select.value = next;
      select.dispatchEvent(new Event("change", { bubbles: true }));
      await new Promise((done) => setTimeout(done, 250));
      return {
        options: [...select.options].map((one) => one.textContent),
        body: view.querySelector(".file-body").textContent,
        readsAfter: window.__OTHER__.reads.length - reads,
        versionDocs: window.__OTHER__.docs.filter((one) => one.id === artifactId && one.version !== null).map((one) => one.version),
        toasts: [...document.querySelectorAll(".toast")].map((one) => one.textContent.trim()),
      };
    }, { artifactId: id, next: value });

    /* ---- 1. 더블클릭: 밖의 문서가 읽기 전용으로 한 번 열리고 알림이 없다 --------- */
    await page.locator('.artifact-card[data-id="doc-out"]').dblclick();
    await settle();
    const dbl = await look("doc-out");
    ok(
      "a double-click on a document outside the open project opens it once, read-only, from the artifact store by id — the project's file door is never asked and nothing raises an error",
      dbl.tabs === 1 && dbl.readOnly && dbl.active && dbl.body.includes("병렬 부하에서만 빨개지는 시험")
        && dbl.opened === 1 && dbl.reads === 0 && dbl.toasts.length === 0,
      JSON.stringify(dbl),
    );
    ok(
      "the read-only tab has no editor, says it is read-only, and its strip names the project the window knows that folder as",
      dbl.editorHidden === true && dbl.note.includes("읽기 전용")
        && dbl.strip?.title === "zo-parallel-flakes.md" && dbl.strip?.project.includes("harbor"),
      JSON.stringify(dbl),
    );
    const sent = await page.evaluate(() => window.__OTHER__.docs.filter((one) => one.id === "doc-out"));
    ok(
      "the window sends the store only the row's id — no path, no folder",
      sent.length > 0 && sent.every((one) => one.keys === "id,version" || one.keys === "id"),
      JSON.stringify(sent),
    );
    await page.screenshot({ path: join(outputDir, "artifacts-other-project-dark.png") });
    await page.evaluate(() => document.documentElement.setAttribute("data-theme", "light"));
    await settle(300);
    await page.screenshot({ path: join(outputDir, "artifacts-other-project-light.png") });
    await page.evaluate(() => document.documentElement.setAttribute("data-theme", "dark"));

    /* ---- 2. 아무도 쓸 수 없다: 저장 없음, 재시작 기록에도 없음 ------------------- */
    const guarded = await page.evaluate(async () => {
      const tab = tabs.find((one) => one.kind === "file" && one.artifact?.id === "doc-out");
      if (!tab) return { missing: true, saved: null, writes: -1, record: "" };
      const before = window.__OTHER__.writes.length;
      const draft = tab.draft;
      tab.draft = "고쳐 쓴 것";
      const saved = await saveFile(tab);
      tab.draft = draft;
      persistStageLayouts();
      const record = JSON.stringify(pendingStageLayouts.get(tab.worktree) ?? null);
      return { saved, writes: window.__OTHER__.writes.length - before, record };
    });
    ok(
      "nothing can write to the file from the read-only tab — a save is refused and writes nothing — and the restart record does not hold the tab, so the next start does not go looking for it through the project's door",
      guarded.saved === false && guarded.writes === 0 && !guarded.record.includes("zo-parallel-flakes"),
      JSON.stringify(guarded),
    );

    /* ---- 3. 문서 안의 상대 링크와 그림은 이 한계 밖을 읽지 않는다 ---------------- */
    await clear();
    const inner = await page.evaluate(async () => {
      const tab = tabs.find((one) => one.kind === "file" && one.artifact?.id === "doc-out");
      if (!tab) return { missing: true, links: 0, reads: -1, images: -1, toasts: [], rawShown: false, editable: null };
      const view = docHost(tab.pane, "file");
      const links = [...view.querySelectorAll(".file-body .md-link")].filter((one) => one.dataset.tip === "./next.md");
      for (const link of links) link.click();
      await new Promise((done) => setTimeout(done, 200));
      view.querySelector(".file-view-mode").click();
      await new Promise((done) => setTimeout(done, 100));
      const shown = {
        rawShown: view.querySelector(".file-body").textContent.includes("# 병렬 부하에서만 빨개지는 시험"),
        editable: view.querySelector('.file-edit:not([hidden]) [contenteditable="true"]') !== null,
      };
      view.querySelector(".file-view-mode").click();
      await new Promise((done) => setTimeout(done, 100));
      return {
        links: links.length,
        reads: window.__OTHER__.reads.length,
        images: window.__OTHER__.images.length,
        toasts: [...document.querySelectorAll(".toast")].map((one) => one.textContent.trim()),
        ...shown,
      };
    });
    ok(
      "a relative link and a relative picture in the read-only document reach nothing outside the store — no file is read through the project's door, no picture is asked for, and no error appears — and its 원본 view is text nobody can type into",
      inner.links >= 1 && inner.reads === 0 && inner.images === 0 && inner.toasts.length === 0
        && inner.rawShown && inner.editable === false,
      JSON.stringify(inner),
    );

    /* ---- 4. 닫았다가 ⌘⇧T: 읽기 전용 탭은 재열기 줄에 서지 않는다---------------- */
    await clear();
    const reopened = await page.evaluate(async () => {
      const tab = tabs.find((one) => one.kind === "file" && one.artifact?.id === "doc-out");
      if (!tab) return { missing: true, stillOpen: null, readsAfter: -1, toasts: [] };
      closeTab(tab.id);
      await new Promise((done) => setTimeout(done, 200));
      const reads = window.__OTHER__.reads.length;
      reopenClosedTab();
      await new Promise((done) => setTimeout(done, 300));
      return {
        stillOpen: tabs.some((one) => one.artifact?.id === "doc-out"),
        readsAfter: window.__OTHER__.reads.length - reads,
        toasts: [...document.querySelectorAll(".toast")].map((one) => one.textContent.trim()),
      };
    });
    ok(
      "closing the read-only tab and pressing ⌘⇧T raises no error and asks the project's door for nothing",
      reopened.stillOpen === false && reopened.readsAfter === 0 && reopened.toasts.length === 0,
      JSON.stringify(reopened),
    );

    /* ---- 5. Enter (눌러 둔 채 반복해도 한 번) --------------------------------- */
    await clear();
    await gallery();
    await page.locator('.artifact-card[data-id="doc-key"]').click();
    await page.keyboard.down("Enter");
    await page.keyboard.down("Enter");
    await page.keyboard.up("Enter");
    await settle();
    const enter = await look("doc-key");
    ok(
      "Enter on a selected outside document opens it once — read-only, no error — even when the key is held and repeats",
      enter.tabs === 1 && enter.readOnly && enter.body.includes("키보드로 연 문서")
        && enter.opened === 1 && enter.toasts.length === 0,
      JSON.stringify(enter),
    );

    /* ---- 6. 창이 모르는 폴더: 머리띠는 폴더 자체를 말한다 ------------------------ */
    await clear();
    await gallery();
    await page.locator('.artifact-card[data-id="doc-far"]').dblclick();
    await settle();
    const far = await look("doc-far");
    ok(
      "a document in a folder the window does not know as a project says the folder itself in its strip",
      far.readOnly && far.strip?.project === "/tmp/zerocode-elsewhere/notes" && far.toasts.length === 0,
      JSON.stringify(far),
    );

    /* ---- 7. 버전 선택기: 보관 스냅샷도 저장소의 문으로 읽는다 --------------------- */
    await clear();
    await gallery();
    await page.locator('.artifact-card[data-id="doc-hist"]').dblclick();
    await settle();
    const snapshot = await pick("doc-hist", "1");
    const back = await pick("doc-hist", "current");
    ok(
      "the version picker of a read-only document reads the kept snapshot from the store by number — never through the project's door — and 「현재 파일」 brings the current text back",
      snapshot.options.join(",") === "현재 파일,버전 1,버전 2" && snapshot.body.includes("doc-hist의 버전 1 본문")
        && snapshot.readsAfter === 0 && snapshot.versionDocs.join(",") === "1" && snapshot.toasts.length === 0
        && back.body.includes("고쳐 온 문서") && back.readsAfter === 0 && back.toasts.length === 0,
      JSON.stringify({ snapshot, back }),
    );

    /* ---- 8. 안의 문서는 지금처럼 고칠 수 있는 파일 탭 ----------------------------- */
    await clear();
    await gallery();
    const readsBefore = await page.evaluate(() => window.__OTHER__.reads.length);
    await page.locator('.artifact-card[data-id="doc-in"]').dblclick();
    await settle();
    const inside = await look("doc-in");
    const insideReads = await page.evaluate((from) => window.__OTHER__.reads.slice(from), readsBefore);
    const insidePick = await pick("doc-in", "2");
    // 고른 옛 판을 보는 동안은 저장이 막힌다 — 현재 파일로 돌아와서 고친다.
    await pick("doc-in", "current");
    const edited = await page.evaluate(async () => {
      const tab = tabs.find((one) => one.kind === "file" && one.artifact?.id === "doc-in");
      if (!tab) return { saved: null, writes: [] };
      const before = window.__OTHER__.writes.length;
      tab.draft = `${tab.text}\n고쳐 쓴 줄\n`;
      const saved = await saveFile(tab);
      return { saved, writes: window.__OTHER__.writes.slice(before) };
    });
    ok(
      "a document inside the open project still opens as an editable file tab with the artifact strip — the project's door reads it, and a save writes it",
      inside.tabs === 1 && !inside.readOnly && inside.editable
        && insideReads.length >= 1 && insideReads.every((path) => path === inside.path)
        && inside.toasts.length === 0 && inside.strip?.title === "inside.md"
        && edited.saved === true && edited.writes.length === 1 && edited.writes[0].path === inside.path,
      JSON.stringify({ inside, insideReads, edited }),
    );
    ok(
      "the version picker of an editable document reads its snapshot from the store too, not through the project's door — the snapshot lives in the app's own folder, outside every project",
      insidePick.body.includes("doc-in의 버전 2 본문") && insidePick.readsAfter === 0
        && insidePick.versionDocs.join(",") === "2" && insidePick.toasts.length === 0,
      JSON.stringify(insidePick),
    );

    /* ---- 9. 실패해도 알림은 하나 ------------------------------------------------ */
    await clear();
    await gallery();
    await page.locator('.artifact-card[data-id="doc-gone"]').dblclick();
    await settle();
    const gone = await look("doc-gone");
    ok(
      "when the store refuses a document, one double-click raises exactly one error and opens no tab",
      gone.toasts.length === 1 && gone.tabs === 0 && !gone.toasts[0].includes("path escapes"),
      JSON.stringify(gone),
    );

    /* ---- 10. 네 카탈로그와 창의 오류 ------------------------------------------- */
    const words = await page.evaluate(() => ["en", "ja", "zh", "es"].flatMap((code) => [
      "artifacts.readOnlyNote", "artifacts.readOnlyCut",
    ].filter((key) => typeof CATALOG[code][key] !== "string" || CATALOG[code][key] === "").map((key) => `${code}:${key}`)));
    ok("the new words exist in the en, ja, zh and es catalogs", words.length === 0, words.join(", "));
    ok("the window raised no errors", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}
