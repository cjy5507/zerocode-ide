import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { createRequire } from "node:module";
import { axeViolations } from "./settings-quality.mjs";
import { installHarnessWaits } from "./harness-waits.mjs";

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
  window.__ARTIFACTS__ = { count: 6, gallery: 9 };
  window.__THUMB_ASKS__ = [];
  window.__THUMB_INFLIGHT_MAX__ = 0;
  window.__ARTIFACT_ASKS__ = 0;
  el("nav-artifacts").click();
  await window.__PAINTED__();
  await new Promise((done) => setTimeout(done, 200));
  const view = artifactsView();
  const cards = () => [...view.querySelectorAll(".artifact-card")];
  // 카드 노드는 보이는 만큼만(가상화); 행의 수는 순서 배열이 말한다.
  seen.rows = artifactOrder.length;
  seen.cards = cards().length;
  seen.sources = [...view.querySelectorAll("[data-artifact-source]")].map((one) => one.dataset.artifactSource);
  seen.sourceWords = [...view.querySelectorAll("[data-artifact-source]")].map((one) => one.textContent);
  // 세그먼트 셋: 전체 → claude.ai → 이 기계, 백엔드를 다시 묻지 않고.
  const asksBefore = window.__ARTIFACT_ASKS__;
  view.querySelector('[data-artifact-source="remote"]').click();
  await new Promise((done) => setTimeout(done, 120));
  seen.remoteOnly = cards().every((one) => one.dataset.kind === "web") && artifactOrder.length === 3;
  seen.whenRemote = cards()[0].querySelector(".artifact-card-when").textContent;
  seen.emoji = cards()[0].querySelector(".artifact-card-emoji").textContent;
  seen.emojiGlyphHidden = cards()[0].querySelector(".artifact-card-glyph").hidden;
  seen.remotePressed = view.querySelector('[data-artifact-source="remote"]').getAttribute("aria-pressed");
  view.querySelector('[data-artifact-source="local"]').click();
  await new Promise((done) => setTimeout(done, 120));
  seen.localOnly = cards().every((one) => one.dataset.kind !== "web") && artifactOrder.length === 12;
  view.querySelector('[data-artifact-source="all"]').click();
  await new Promise((done) => setTimeout(done, 120));
  seen.allBack = artifactOrder.length === 15;
  seen.asksOnSegments = window.__ARTIFACT_ASKS__ - asksBefore;
  // 편집 시각 내림차순: 오늘 것들이 맨 앞.
  seen.newestFirst = artifactOrder.slice(0, 3).every((id) => (artifactRows.get(id).modified_ms) > Date.now() - 60_000);
  // 카드 아래 줄: 🌐/🔒 + 오늘은 상대 낱말, 어제·지난달은 달·날.
  const whenOf = (id) => view.querySelector(`.artifact-card[data-id="${id}"] .artifact-card-when`)?.textContent ?? "";
  seen.whenLocalToday = whenOf("gal-0");
  // 렌더 썸네일: page·web 카드만(문서·보고서는 아니다), 한 카드에 한 번, 동시 1,
  // 실패는 글리프. 세그먼트를 오가며 보인 page·web 카드 여섯 중 어느 것도 두 번
  // 청하지 않고, 문서·보고서·증거는 한 번도 청하지 않는다.
  await new Promise((done) => setTimeout(done, 200));
  seen.thumbAsks = window.__THUMB_ASKS__.slice().sort();
  seen.thumbAskedOnce = new Set(seen.thumbAsks).size === seen.thumbAsks.length;
  seen.thumbOnlyRendered = seen.thumbAsks.every((id) => ["page", "web"].includes(artifactRows.get(id)?.kind));
  seen.thumbInflightMax = window.__THUMB_INFLIGHT_MAX__;
  seen.thumbShown = view.querySelector('.artifact-card[data-id="gal-0"] .artifact-card-thumb')?.hidden === false;
  seen.thumbFailedGlyph = view.querySelector('.artifact-card[data-id="gal-3"] .artifact-card-thumb')?.hidden === true
    && artifactThumbFailed.has("gal-3");
  // 종류 전환을 두 번 돌아와도 실패한 것은 다시 청하지 않고, 있는 것은 캐시에서.
  view.querySelector('[data-artifact-kind="document"]').click();
  await new Promise((done) => setTimeout(done, 40));
  seen.whenYesterday = whenOf("gal-1");
  const yesterday = new Date(Date.now() - 24 * 60 * 60 * 1000);
  seen.yesterdayWords = new Intl.DateTimeFormat("ko", { month: "long", day: "numeric" }).format(yesterday);
  // 문서의 얼굴은 첫 블록의 활자, claude.ai 카드의 글리프 자리에는 파비콘.
  seen.docFace = view.querySelector('.artifact-card[data-id="gal-1"] .artifact-card-doc')?.textContent ?? "";
  seen.docFaceHidden = view.querySelector('.artifact-card[data-id="gal-1"] .artifact-card-doc')?.hidden;
  view.querySelector('[data-artifact-kind="all"]').click();
  await new Promise((done) => setTimeout(done, 200));
  seen.thumbAsksAfterRoundTrip = window.__THUMB_ASKS__.length;
  seen.thumbAsksBeforeRoundTrip = seen.thumbAsks.length;
  // 「새 아티팩트」: 받을 곳은 고르개가 묻는다(t-3952) — 앞선 케이스들이 남긴
  // 판이 있든 없든 단추는 선다(새 에이전트도 고를 수 있다).
  seen.newFollowsSeat = view.querySelector(".artifacts-new").disabled === false;
  return seen;
});
ok(
  "the gallery's three segments filter without asking, cards wear 🌐/🔒 with relative or month-day words, a document's face is its first block, a claude.ai card wears its favicon, and thumbnails are asked for visible page·web cards only, one at a time, failing to a glyph once",
  gallery.rows === 15 && gallery.cards > 0 && gallery.cards <= 15 &&
    gallery.sources.join(",") === "all,local,remote" &&
    gallery.sourceWords.join(",") === "전체,이 기계,claude.ai" &&
    gallery.remoteOnly && gallery.remotePressed === "true" &&
    gallery.localOnly && gallery.allBack &&
    gallery.asksOnSegments === 0 &&
    gallery.newestFirst &&
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
  artifactFilter.source = "all";
  artifactFilter.kind = "all";
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
    description: artifactCardNodeFor("gal-0")?.querySelector(".artifact-card-description").textContent,
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
    CATALOG[code]["artifacts.source.local"], CATALOG[code]["artifacts.new"], CATALOG[code]["artifacts.strip.share"],
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
    out.selected = choice.getAttribute("aria-pressed") === "true";
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
    out.collapsed = form.hidden && document.activeElement === choice && choice.getAttribute("aria-pressed") === "false";
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

/* 닫기(2026-09-13, 사용자 지적 「비교·대시보드 등 열었을 때 닫기도 없음」): 스튜디오는
 * 「접기」(폼만)와 다른 손으로 한 줄로 접히고 갤러리가 그 높이를 받는다. 눌린 타일을
 * 다시 누르면 폼이 닫힌다. 접힘은 사람의 선택이라 범위를 들고 온 도착이 바꾸지
 * 않는다. 초안의 글은 접었다 펼쳐도 남는다. */
export async function testArtifactStudioFold(page, ok) {
  const seen = await page.evaluate(async () => {
    dropTab("artifacts");
    artifactStudioFolded = false;
    openArtifacts();
    await window.__PAINTED__();
    const view = artifactsView();
    const studio = view.querySelector(".artifacts-studio");
    const grid = view.querySelector(".artifacts-grid");
    const choice = view.querySelector('[data-surface="monitor"].artifacts-studio-choice');
    const brief = view.querySelector(".artifacts-studio-brief");
    const out = {};
    const heroHeight = studio.getBoundingClientRect().height;
    const gridBefore = grid.getBoundingClientRect().height;
    choice.click();
    brief.value = "접어도 남는 초안";
    brief.dispatchEvent(new Event("input", { bubbles: true }));
    out.composing = studio.classList.contains("is-composing") && choice.getAttribute("aria-pressed") === "true";
    choice.click();
    out.tileToggles = !studio.classList.contains("is-composing") && view.querySelector(".artifacts-studio-form").hidden
      && choice.getAttribute("aria-pressed") === "false" && document.activeElement === choice;
    choice.click();
    view.querySelector(".artifacts-studio-dismiss").click();
    await window.__PAINTED__();
    out.folded = studio.classList.contains("is-folded") && !studio.classList.contains("is-composing");
    out.heights = [heroHeight, studio.getBoundingClientRect().height, gridBefore, grid.getBoundingClientRect().height];
    out.slim = out.heights[1] < heroHeight / 2;
    out.gridGrew = out.heights[3] > gridBefore;
    out.dismissGone = view.querySelector(".artifacts-studio-dismiss").offsetParent === null;
    out.tilesGone = choice.offsetParent === null;
    out.expandFocused = document.activeElement === view.querySelector(".artifacts-studio-expand");
    view.querySelector(".artifacts-studio-expand").click();
    await window.__PAINTED__();
    out.unfolded = !studio.classList.contains("is-folded") && choice.offsetParent !== null
      && document.activeElement?.classList.contains("artifacts-studio-choice") === true;
    out.briefKept = brief.value === "접어도 남는 초안";
    view.querySelector(".artifacts-studio-dismiss").click();
    openArtifacts({ origin: { field: "worktree", value: "/repos/scoped", label: "scoped" } });
    await window.__PAINTED__();
    out.scopedKeepsFold = artifactsView().querySelector(".artifacts-studio").classList.contains("is-folded");
    artifactFilter.origin = null;
    applyArtifactFilter();
    view.querySelector(".artifacts-studio-expand").click();
    openArtifacts({ origin: { field: "worktree", value: "/repos/scoped", label: "scoped" } });
    await window.__PAINTED__();
    out.scopedKeepsUnfold = !artifactsView().querySelector(".artifacts-studio").classList.contains("is-folded");
    artifactFilter.origin = null;
    artifactStudioFolded = false;
    applyArtifactFilter();
    openArtifacts();
    await window.__PAINTED__();
    out.restored = !artifactsView().querySelector(".artifacts-studio").classList.contains("is-folded");
    return out;
  });
  ok("studio closes to one slim row that hands the gallery its height, its tiles toggle, and reopening keeps the brief",
    seen.composing && seen.tileToggles && seen.folded && seen.slim && seen.gridGrew && seen.dismissGone
      && seen.tilesGone && seen.expandFocused && seen.unfolded && seen.briefKept, JSON.stringify(seen));
  ok("the fold is the person's choice: a scoped arrival changes neither a folded nor an open studio",
    seen.scopedKeepsFold && seen.scopedKeepsUnfold && seen.restored, JSON.stringify(seen));
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
        const choices = [...studio.querySelectorAll(".artifacts-studio-choice")];
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
          localized: choices[0].textContent.includes(theme === "light" ? "Compare" : "비교"),
          fieldStyled: getComputedStyle(input).backgroundColor === "rgba(0, 0, 0, 0)",
          purposeMaterials: new Set([...studio.querySelectorAll(".artifacts-studio-glyph")].map(node => getComputedStyle(node).backgroundColor)).size,
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
    kinds: view.querySelectorAll(".artifacts-kinds [data-artifact-kind]").length,
    firstTitle: first?.querySelector(".artifact-card-title")?.textContent ?? "",
    firstOrigin: first?.querySelector(".artifact-card-origin")?.textContent ?? "",
    firstWhen: first?.querySelector(".artifact-card-when")?.textContent ?? "",
    firstKind: first?.dataset.kind ?? "",
    stat: view.querySelector(".artifacts-stat").textContent,
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
  "the artifacts entry opens a doc tab that draws the catalog as keyed cards with kind, origin and time",
  artifactsOpen.entryHidden === false &&
    artifactsOpen.tab.includes("아티팩트") &&
    artifactsOpen.asks === 1 &&
    artifactsOpen.askedFilter !== null &&
    artifactsOpen.rows === 12 && artifactsOpen.cards > 0 && artifactsOpen.cards <= 12 &&
    artifactsOpen.kinds === 10 &&
    artifactsOpen.firstTitle !== "" &&
    artifactsOpen.firstOrigin.includes("codex") &&
    artifactsOpen.firstWhen !== "" &&
    artifactsOpen.firstKind !== "" &&
    artifactsOpen.stat.includes("12") &&
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
  const card = (id) => { selectArtifact(view, id, { reveal: true }); return view.querySelector(`.artifact-card[data-id="${id}"]`); };
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
  seen.filtered = artifactOrder.length;
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
  seen.wtOriginChip = artifactsView().querySelector(".artifacts-origin-chip")?.textContent ?? "";
  // And clearing the origin brings everything back, still without asking.
  artifactsView().querySelector(".artifacts-origin-clear").click();
  await new Promise((done) => setTimeout(done, 40));
  seen.cleared = artifactOrder.length;
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
    artifactRoundTrip.filtered === 3 &&
    artifactRoundTrip.filteredWorkers === 1 &&
    artifactRoundTrip.asksAfter === 0 &&
    /6/.test(artifactRoundTrip.wtChip) &&
    artifactRoundTrip.wtFiltered === 6 &&
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
    const origin = card.querySelector(".artifact-card-origin");
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
  "the card's title and origin keep 3:1 against an opaque card in both themes",
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
    stat: view.querySelector(".artifacts-stat").textContent,
  };
  const creationsBefore = artifactCardCreations;
  const asksBefore = window.__ARTIFACT_ASKS__;
  view.querySelector('[data-artifact-kind="screenshot"]').click();
  await new Promise((done) => setTimeout(done, 60));
  seen.screenshotNodes = view.querySelectorAll(".artifact-card").length;
  seen.screenshotOnly = [...view.querySelectorAll(".artifact-card")].every((one) => one.dataset.kind === "screenshot");
  view.querySelector('[data-artifact-kind="all"]').click();
  await new Promise((done) => setTimeout(done, 60));
  const query = view.querySelector(".artifacts-query");
  query.value = "보고서 7";
  query.dispatchEvent(new Event("input", { bubbles: true }));
  await new Promise((done) => setTimeout(done, 260));
  seen.searchNodes = view.querySelectorAll(".artifact-card").length;
  seen.searchStat = view.querySelector(".artifacts-stat").textContent;
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
  "1,000 artifacts: first paint under 150 ms (min of five), a pool of visible cards only, zero node creations across a kind switch and a search, and no second ask",
  artifactScale.firstPaint < 150 &&
    artifactScale.askedPerOpen &&
    artifactScale.rows === 1000 &&
    artifactScale.nodes < 120 &&
    artifactScale.stat.includes("1000") &&
    artifactScale.screenshotOnly &&
    artifactScale.screenshotNodes > 0 &&
    artifactScale.searchNodes >= 1 &&
    artifactScale.searchStat.startsWith("33 / 1000") &&
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
    out.korean = view.querySelector(".artifacts-studio-title").textContent === "아이디어를, 완성된 화면으로."
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
