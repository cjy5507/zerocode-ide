import { openWindowTestPage } from "./window-boot.mjs";
import { standPile } from "./stacked-work.mjs";

/* The right column and its popups in the Geist language (2026-11-10, the person:
 * 「오른쪽 네비바 깃창 파일창 찾기창등 giest 디자인토큰 룰을 어긴것과 팝업창까지」, and
 * the draft p-39057edd7d8a28d6 passed as recommended — A to G).
 *
 * The draft was measured by an audit of computed style: every visible element's
 * type pair, face, weight, case, ink, ground, edges, corners, spacing, control
 * height and glyph size, held against the values tokens.css resolves to and
 * Geist's named type styles. Before the pass the sixteen scenes held 419 such
 * findings in the dark treatment; this suite stands the same scenes and wants
 * none, apart from the few the draft names with a reason (ALLOWED). Then the
 * decisions themselves, one assertion each.
 *
 * Invented data only — nobody's files, ledger or sessions.
 *
 *   WINDOW_SUITES=right-column-geist node ui/tests/window.mjs */

/* What the audit still finds and why it stands (the draft's own list). */
const ALLOWED = [
  { scene: "search", kind: "ground", at: "mark.hit-mark", why: "the find highlight is the markdown search's token, kept" },
  { scene: "search", kind: "space", at: "#file-search", why: "the field's ends are sums of tokens: the glyph's rail (8 + 14 + 8) and the three toggles' room (3 × 28)" },
  { scene: "files", kind: "space", at: "#file-search", why: "the glyph's rail, 8 + 14 + 8" },
  { scene: "pile", kind: "control-height", at: "button.pile-cleanup", why: "a word inside a sentence, not a control" },
  { scene: "pile", kind: "ink", at: "span.history-ref", why: "the ref's lane hue leaned toward the ink so its word reads at 4.5:1 — a mix of two tokens" },
  { scene: "scm", kind: "ink", at: "span.history-ref", why: "the same badge in the history list" },
  { scene: "tooltip", kind: "float-shadow", at: "#window-tooltip", why: "the inverted pill needs no ring" },
  { scene: "toast", kind: "float-shadow", at: "div.toasts", why: "the toasts' own frame is not drawn; each toast carries the ring" },
  { scene: "menu-scm-primary", kind: "control-height", at: "button.note-pop-row.term-menu-row", why: "a row with its hint line under it" },
  { scene: "dialog-finder", kind: "tracking", at: "#qo-title", why: "Heading 14's own tracking token" },
  { scene: "dialog-finder", kind: "space", at: "#qo-row-", why: "12px less the lane rail (3px) a worktree row wears" },
  { scene: "dialog-pr", kind: "space", at: "#pr-new-base", why: "the chevron's room, 8 + 16 + 4" },
];

/* The audit, run in the page. */
function installGeistAudit() {
  const parse = (text) => {
    if (!text || text === "transparent") return null;
    let m = text.match(/^rgba?\(\s*([\d.]+)[ ,]+([\d.]+)[ ,]+([\d.]+)(?:\s*[,/]\s*([\d.]+%?))?\s*\)$/);
    if (m) return [+m[1], +m[2], +m[3], m[4] === undefined ? 1 : m[4].endsWith("%") ? parseFloat(m[4]) / 100 : parseFloat(m[4])];
    m = text.match(/^color\(srgb\s+([-\d.e]+)\s+([-\d.e]+)\s+([-\d.e]+)(?:\s*\/\s*([\d.]+%?))?\s*\)$/);
    if (m) return [m[1] * 255, m[2] * 255, m[3] * 255, m[4] === undefined ? 1 : m[4].endsWith("%") ? parseFloat(m[4]) / 100 : parseFloat(m[4])];
    return null;
  };
  const same = (a, b) => a && b && Math.abs(a[0] - b[0]) <= 2 && Math.abs(a[1] - b[1]) <= 2 && Math.abs(a[2] - b[2]) <= 2 && Math.abs(a[3] - b[3]) <= 0.02;
  const palette = () => {
    const names = new Set();
    const walk = (list) => { for (const rule of list) { if (rule.cssRules && !rule.style) walk(rule.cssRules); if (rule.style) for (let i = 0; i < rule.style.length; i++) if (rule.style[i].startsWith("--")) names.add(rule.style[i]); } };
    for (const sheet of document.styleSheets) { try { walk(sheet.cssRules); } catch { /* another origin */ } }
    const host = document.createElement("div");
    host.style.cssText = "position:absolute;left:-9999px;color:rgb(1, 2, 3)";
    const probe = host.appendChild(document.createElement("span"));
    document.body.appendChild(host);
    const colours = [];
    for (const name of names) {
      probe.style.color = `var(${name})`;
      const got = getComputedStyle(probe).color;
      if (got !== "rgb(1, 2, 3)") { const c = parse(got); if (c) colours.push(c); }
    }
    host.remove();
    return colours;
  };
  const TYPE = new Set(["24/32", "20/28", "16/24", "14/20", "13/16", "13/18", "13/20", "12/16"]);
  const SPACE = new Set([0, 1, 2, 4, 6, 8, 10, 12, 16, 20, 24, 32, 40, 48]);
  const RADIUS = new Set([0, 2, 6, 12]);
  const HEIGHT = new Set([20, 24, 28, 32, 36, 40]);
  const ICON = new Set([12, 14, 16, 20]);
  const sig = (node) => node.id ? `#${node.id}` : `${node.tagName.toLowerCase()}${[...node.classList].filter((c) => !/^is-|^has-/.test(c)).slice(0, 2).map((c) => `.${c}`).join("")}`;
  const shown = (node) => {
    const r = node.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) return false;
    for (let at = node; at && at !== document.documentElement; at = at.parentElement) {
      const s = getComputedStyle(at);
      if (s.display === "none" || s.visibility === "hidden" || s.opacity === "0") return false;
    }
    return true;
  };
  // The tokens resolve per treatment, so the palette is read again for each.
  const palettes = new Map();
  window.__geistAudit = (root, { float = false } = {}) => {
    const theme = document.documentElement.dataset.theme ?? "dark";
    if (!palettes.has(theme)) palettes.set(theme, palette());
    const colours = palettes.get(theme);
    const token = (c) => !c || c[3] === 0 || colours.some((one) => same(one, c));
    const found = [];
    const add = (kind, node, value) => found.push({ kind, at: sig(node), value });
    for (const node of [root, ...root.querySelectorAll("*")]) {
      if (node instanceof SVGElement && node.tagName.toLowerCase() !== "svg") continue;
      if (!shown(node) || node.closest(".sr")) continue;
      const s = getComputedStyle(node);
      const r = node.getBoundingClientRect();
      if (node.tagName.toLowerCase() === "svg") {
        // An outer <svg> has a layout box of its own; read it rather than the
        // painted one, which a float's entrance scale shrinks.
        const w = Math.round(node.clientWidth || r.width), h = Math.round(node.clientHeight || r.height);
        if (node.classList.contains("icon") && (!ICON.has(w) || w !== h)) add("icon-size", node, `${w}×${h}`);
        continue;
      }
      const words = [...node.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim()) || /^(INPUT|TEXTAREA|SELECT)$/.test(node.tagName);
      if (words && !(node.tagName === "INPUT" && /checkbox|radio/.test(node.type))) {
        const size = Math.round(parseFloat(s.fontSize));
        const pair = `${size}/${s.lineHeight === "normal" ? "normal" : Math.round(parseFloat(s.lineHeight))}`;
        if (!TYPE.has(pair) && !(size === 11 && /count|badge|tally|num|kbd|chord/.test(node.className))) add("type", node, pair);
        if (!/^Geist( Mono)?$/.test(s.fontFamily.split(",")[0].replace(/["']/g, "").trim())) add("family", node, s.fontFamily.split(",")[0]);
        if (![400, 500, 600].includes(+s.fontWeight)) add("weight", node, s.fontWeight);
        if (s.textTransform === "uppercase") add("uppercase", node, s.letterSpacing);
        else if (s.letterSpacing !== "normal" && Math.abs(parseFloat(s.letterSpacing)) > 0.01 && size < 16) add("tracking", node, s.letterSpacing);
        if (s.fontStyle === "italic") add("italic", node, "italic");
        if (!token(parse(s.color))) add("ink", node, s.color);
      }
      const ground = parse(s.backgroundColor);
      if (ground && ground[3] > 0 && !token(ground)) add("ground", node, s.backgroundColor);
      const sides = ["Top", "Right", "Bottom", "Left"].filter((side) => s[`border${side}Style`] !== "none" && parseFloat(s[`border${side}Width`]) > 0);
      if (sides.length === 4) add("box-border", node, s.borderTopColor);
      if (sides.some((side) => !token(parse(s[`border${side}Color`])))) add("edge-ink", node, s.borderTopColor);
      for (const corner of [s.borderTopLeftRadius, s.borderTopRightRadius, s.borderBottomRightRadius, s.borderBottomLeftRadius]) {
        if (corner.includes("%")) continue;
        const px = Math.round(parseFloat(corner));
        if (!RADIUS.has(px) && !(px > 0 && px >= Math.min(r.width, r.height) / 2 - 1)) { add("radius", node, `${px}px`); break; }
      }
      if (float && node === root) {
        if (Math.round(parseFloat(s.borderTopLeftRadius)) !== 12 && !/tooltip|toast/.test(node.className)) add("float-radius", node, s.borderTopLeftRadius);
        if (s.boxShadow === "none") add("float-shadow", node, "none");
      }
      for (const prop of ["paddingTop", "paddingRight", "paddingBottom", "paddingLeft", "rowGap", "columnGap"]) {
        const v = parseFloat(s[prop]);
        if (!Number.isNaN(v) && !SPACE.has(Math.round(v * 2) / 2)) { add("space", node, `${prop} ${v}px`); break; }
      }
      const control = /^(BUTTON|INPUT|SELECT|TEXTAREA)$/.test(node.tagName) || node.getAttribute("role") === "tab" || node.classList.contains("segment-btn");
      const tall = node.offsetHeight || r.height;
      if (control && !/checkbox|radio/.test(node.type) && !HEIGHT.has(Math.round(tall)) && tall < 48) add("control-height", node, `${Math.round(tall)}px`);
    }
    return found;
  };
}

/* The fixture: a checkout with folders git ignores, changed and staged files,
 * a review with five checks, two agents' sessions and the pile. */
function standRightColumn() {
  setLocale("ko", { persist: false });
  const dir = (name) => ({ name, is_dir: true });
  const file = (name) => ({ name, is_dir: false });
  const listings = {
    "": ["crates", "docs", "node_modules", "target", "ui"].map(dir).concat(["CHANGELOG.md", "Cargo.toml", "README.md"].map(file)),
    ui: ["tests"].map(dir).concat(["index.html", "shell.css", "tokens.css"].map(file)),
  };
  window.__ANSWER__.list_dir = ({ path }) => listings[path] ?? [];
  const entry = (path, code, added, removed) => ({ path, code, staged: code[0] !== " " && code[0] !== "?", changed: code[1] !== " ", added, removed, conflict: null, origin: null });
  window.__ANSWER__.scm_status = () => ({
    changed: [entry("ui/shell.css", " M", 120, 48), entry("ui/tokens.css", " M", 8, 2), entry("CHANGELOG.md", "M ", 4, 0), entry("ui/tests/right.mjs", "??", null, null)],
    ignored: ["docs", "node_modules", "target"],
  });
  window.__ANSWER__.scm_numstat = () => [];
  window.__ANSWER__.upstream_status = () => ({ upstream: "origin/wt/geist", ahead: 2, behind: 0, behind_commits_are_patch_equivalent: null });
  window.__ANSWER__.source_control_compare_context = () => ({ head: "refs/heads/wt/geist", base_ref: "origin/main", source: "repository-default", options: ["origin/main"], error: null, line_total: { added: 132, removed: 50 } });
  window.__ANSWER__.search_text = () => [
    { path: "ui/shell.css", line: 4211, text: "  border-radius: var(--radius-md);" },
    { path: "ui/tokens.css", line: 852, text: "  --radius: 12px;" },
  ];
  window.__ANSWER__.search_files = () => ["ui/shell.css", "ui/tokens.css"];
  const session = (id, agent, title, minutes) => ({ id, agent, session_id: id, title, cwd: activeWorktreePath, branch: "main", model: null,
    file_path: `/tmp/${id}.jsonl`, modified_at: new Date(Date.now() - minutes * 60_000).toISOString(), message_count: 4,
    preview: [{ role: "user", text: "오른쪽 열을 Geist로" }], resume: `${agent} --resume ${id}`, children: [], depth: 0 });
  window.__VAULT__ = { groups: [{ key: "today", label: "오늘", sessions: [session("s1", "claude", "오른쪽 열 감사", 4), session("s2", "codex", "CI 초록", 50)] }],
    agents: [{ slug: "claude", sessions: 1 }, { slug: "codex", sessions: 1 }], issues: [], shown: 2, total: 2, matched: 2, truncated: false,
    limits: { choices: [50, 100, 200], absolute_max: 1000, default: 200, children_per_parent: 50 } };
  window.__CHECKS__ = {
    review: { number: 7, title: "fix(ui): the right column speaks Geist", url: "https://example.invalid/acme/r/pull/7", state: "open", draft: false, head_sha: "abc", owner_repo: "acme/r" },
    checks: [
      { name: "lint", status: "completed", conclusion: "failure", url: "https://example.invalid/runs/1", check_run_id: 1, workflow_run_id: 1 },
      { name: "window", status: "in_progress", conclusion: null, url: "https://example.invalid/runs/2", check_run_id: 2, workflow_run_id: 2 },
      { name: "shell", status: "completed", conclusion: "success", url: "https://example.invalid/runs/3", check_run_id: 3, workflow_run_id: 3 },
    ],
    at_page_limit: false,
  };
}

const frames = (page, n = 3) => page.evaluate((n) => new Promise((done) => {
  let left = n;
  const step = () => (--left <= 0 ? done() : requestAnimationFrame(step));
  requestAnimationFrame(step);
}), n);

export async function testRightColumnGeist(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.evaluate(standRightColumn);
    await page.evaluate(standPile);
    await page.evaluate(installGeistAudit);
    await frames(page);

    const allowed = (scene, finding) => ALLOWED.some((one) => one.scene === scene && one.kind === finding.kind && finding.at.startsWith(one.at));
    const audit = async (scene, selector, options = {}) => {
      await frames(page);
      await page.waitForTimeout(200);
      // A float scales in (--enter-scale over --motion-fast/--motion-dialog), and
      // a box read while it does is every size times that scale — a 32px field
      // reads 31 on a slow runner. Measure once its own entrance has finished.
      await page.waitForFunction(
        (selector) => {
          const node = document.querySelector(selector);
          return !node || node.getAnimations().every((one) => one.playState !== "running");
        },
        selector,
        { timeout: 5000 },
      ).catch(() => {});
      const found = await page.evaluate(({ selector, options }) => {
        const root = document.querySelector(selector);
        return root ? window.__geistAudit(root, options) : [{ kind: "missing", at: selector, value: "" }];
      }, { selector, options });
      return found.filter((finding) => !allowed(scene, finding));
    };
    const offending = {};
    for (const theme of ["dark", "light"]) {
      await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
      const record = (scene, found) => { if (found.length) offending[`${theme}:${scene}`] = found.slice(0, 8); };

      await page.evaluate(async () => {
        revealActivity("files", "name");
        await refreshScm();
        await loadTree(fileTree, "");
        await fileTree.querySelector('.tree-row[data-tree-path="ui"]')?._treeUnfold(true);
      });
      record("files", await audit("files", "aside.files"));

      await page.evaluate(async () => {
        revealActivity("files", "text");
        setFileSearchMode("text");
        fileSearch.value = "radius";
        await runFileSearch();
      });
      record("search", await audit("search", "aside.files"));
      await page.evaluate(async () => { fileSearch.value = ""; setFileSearchMode("name"); await loadTree(fileTree, ""); });

      await page.evaluate(async () => {
        setActivityItem("scm");
        await refreshScm();
        await refreshSourceControlCompare();
        await refreshUpstream();
      });
      record("scm", await audit("scm", "aside.files"));

      await page.evaluate(async () => { document.getElementById("activity-vault-tab").click(); await refreshVaultDock(true); });
      record("vault", await audit("vault", "aside.files"));

      await page.evaluate(async () => { revealActivity("checks"); await refreshChecks(); });
      record("checks", await audit("checks", "aside.files"));

      await page.evaluate(() => setActivityItem("pile"));
      record("pile", await audit("pile", "aside.files"));

      // A menu: the file tree's.
      await page.evaluate(async () => {
        revealActivity("files", "name");
        await loadTree(fileTree, "");
        const row = fileTree.querySelector('.tree-row[data-tree-path="CHANGELOG.md"]');
        const r = row.getBoundingClientRect();
        row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: r.left + 40, clientY: r.top + 8 }));
      });
      record("menu-tree", await audit("menu-tree", "#sidebar-menu", { float: true }));
      await page.evaluate(() => closeSidebarMenu());

      // The commit's menu, which used to be another popup's rows.
      await page.evaluate(async () => { setActivityItem("scm"); await refreshScm(); });
      await frames(page);
      await page.evaluate(() => document.getElementById("scm-primary-more").click());
      record("menu-scm-primary", await audit("menu-scm-primary", "#scm-primary-menu", { float: true }));
      await page.keyboard.press("Escape");

      await page.evaluate(() => toast("커밋했습니다 · 2개 파일", "", { action: { label: "되돌리기", run: () => {} } }));
      record("toast", await audit("toast", ".toasts", { float: true }));

      await page.evaluate(() => openPalette("file"));
      await frames(page);
      await page.keyboard.type("shell");
      record("dialog-finder", await audit("dialog-finder", ".qo--finder", { float: true }));
      await page.keyboard.press("Escape");
      await page.waitForTimeout(250);

      await page.evaluate(() => { window.__ANSWER__.pull_request_seed = () => ({ base: "main", title: "fix(ui): Geist", body: "", head: "wt/geist", default_base: "main" }); });
      await page.evaluate(() => openPullRequestForm());
      record("dialog-pr", await audit("dialog-pr", "#pr-new-scrim .qo", { float: true }));
      await page.keyboard.press("Escape");
      await page.waitForTimeout(250);
    }
    ok(
      "the right column's seven tabs and their popups hold no value off Geist's tokens and type styles, in both treatments, apart from the draft's named exceptions",
      Object.keys(offending).length === 0,
      JSON.stringify(offending),
    );

    /* The decisions, one fact each. */
    await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
    const facts = await page.evaluate(async () => {
      const seen = {};
      const style = (node) => getComputedStyle(node);
      // One field: the side column's 28px, a ring, not a border; the same in both treatments.
      revealActivity("files", "name");
      const field = document.getElementById("file-search");
      // Read at rest: the reveal may hand the field the keyboard, and a focused
      // field trades its ring for the focus line.
      field.blur();
      seen.field = { h: Math.round(field.getBoundingClientRect().height), border: style(field).borderTopWidth, ring: style(field).boxShadow !== "none" };
      document.documentElement.dataset.theme = "light";
      seen.fieldLightGround = style(field).backgroundColor === style(document.querySelector("aside.files")).backgroundColor;
      document.documentElement.dataset.theme = "dark";
      // Focus is the 2px ink line, not a glow.
      field.focus();
      seen.focus = { outline: `${style(field).outlineWidth} ${style(field).outlineStyle}`, ink: style(field).outlineColor, chalk: style(document.body).color, glow: style(field).boxShadow };
      field.blur();
      // One rhythm: a tree row stands on --row-height; an ignored one is not slanted.
      await loadTree(fileTree, "");
      const row = fileTree.querySelector(".tree-row");
      const ignored = fileTree.querySelector(".tree-row.is-ignored .tree-name");
      seen.row = Math.round(row.getBoundingClientRect().height);
      seen.ignoredSlant = ignored ? style(ignored).fontStyle : "none-ignored";
      // D: the search's order — the field, then 이름/내용, then the path filters; the three
      // toggles inside the field, and only while the content question is asked.
      const toggles = document.getElementById("file-search-toggles");
      seen.togglesInField = toggles.parentElement === field.parentElement;
      seen.togglesHiddenForNames = toggles.hidden;
      setFileSearchMode("text");
      const top = (node) => node.getBoundingClientRect().top;
      seen.order = top(field) < top(document.querySelector("#activity-files > .segment"))
        && top(document.querySelector("#activity-files > .segment")) < top(document.getElementById("file-search-filters"));
      seen.togglesShownForText = !toggles.hidden;
      setFileSearchMode("name");
      // F: the history is 「커밋」.
      seen.history = document.querySelector('#scm-history-title [data-i18n="sourceControl.history"]').textContent.trim();
      // C: the commit is the ink fill only when it can act, and its menu half wears its face.
      setActivityItem("scm");
      await refreshScm();
      const commit = document.getElementById("scm-primary");
      const more = document.getElementById("scm-primary-more");
      const ink = style(document.body).color;
      seen.commitDisabled = commit.disabled;
      seen.splitQuiet = style(commit).backgroundColor !== ink && style(more).backgroundColor !== ink;
      // The vault dock speaks Geist, not the button's system face.
      document.getElementById("activity-vault-tab").click();
      await refreshVaultDock(true);
      seen.vaultFaces = [...document.querySelectorAll("#activity-vault button, #activity-vault span")]
        .map((node) => style(node).fontFamily.split(",")[0].replace(/["']/g, "").trim())
        .filter((face) => !/^Geist( Mono)?$/.test(face));
      // G: one floating material, solid, ringed.
      const menu = document.getElementById("sidebar-menu");
      openSidebarMenu(200, 200, [{ label: "하나", run: () => {} }]);
      seen.menu = { ground: style(menu).backgroundColor, blur: style(menu).backdropFilter, border: style(menu).borderTopWidth, ring: style(menu).boxShadow.includes("0px 0px 0px 1px") };
      closeSidebarMenu();
      return seen;
    });
    ok("one field: 28px, a ring and no border, the panel's own ground in light", facts.field.h === 28 && facts.field.border === "0px" && facts.field.ring && facts.fieldLightGround, JSON.stringify(facts));
    ok("a focused field wears the contract's 2px ink line, not a glow", facts.focus.outline === "2px solid" && facts.focus.ink === facts.focus.chalk && facts.focus.glow === "none", JSON.stringify(facts.focus));
    ok("a tree row stands on --row-height and an ignored entry is not slanted", facts.row === 24 && facts.ignoredSlant !== "italic", JSON.stringify(facts));
    ok("the search asks first, then chooses names or contents, then narrows the paths — its toggles inside the field, only for contents",
      facts.togglesInField && facts.togglesHiddenForNames && facts.order && facts.togglesShownForText, JSON.stringify(facts));
    ok("the history section is 「커밋」, the word the pile tab uses for the same list", facts.history === "커밋", facts.history);
    ok("a commit that cannot act is quiet, and its menu half with it", facts.commitDisabled && facts.splitQuiet, JSON.stringify(facts));
    ok("the session record speaks Geist, not the system face", facts.vaultFaces.length === 0, JSON.stringify(facts.vaultFaces));
    ok("a menu is solid, unblurred and ringed rather than bordered",
      !/^rgba|\/ 0\./.test(facts.menu.ground) && (facts.menu.blur === "none" || facts.menu.blur === "") && facts.menu.border === "0px" && facts.menu.ring,
      JSON.stringify(facts.menu));
    ok("the right column's Geist scenes raise no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
