import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { installBoardWaits } from "./board-waits.mjs";
import { boardScheduleFixture, installBoardSchedule, openBoardSchedule, SCHEDULE_NOW } from "./board-schedule.mjs";

/* The window's chrome in the passed design 「또렷한 Geist」 (p-6893560125c34552,
 * the person 2026-10-28 02:5x; t-78107). Each scene below is one item of the
 * design's 「무엇이 바뀌나」 list, measured on a laid-out page — computed
 * style and boxes, not pictures. */

const frames = (page) => page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));

/* Geist ships with the window: both faces load from the window's own files
 * and the chrome draws in them, not in the system face the stack falls to. */
async function testGeistShips(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const drawn = await page.evaluate(async () => {
      await Promise.all([document.fonts.load("13px Geist"), document.fonts.load("13px 'Geist Mono'")]);
      await document.fonts.ready;
      const loaded = (family) => [...document.fonts].some((face) => face.family.replace(/"/g, "") === family && face.status === "loaded");
      // A face that is not there draws as the generic after it; a monospace
      // after the sans face and a serif after the mono one make that measurable.
      const width = (font) => {
        const pen = document.createElement("canvas").getContext("2d");
        pen.font = font;
        return pen.measureText("Geist 지금 누가 나를 필요로 하는가 0123456789").width;
      };
      return {
        sans: loaded("Geist"),
        mono: loaded("Geist Mono"),
        sansDiffers: width("13px Geist, monospace") !== width("13px monospace"),
        monoDiffers: width("13px 'Geist Mono', serif") !== width("13px serif"),
        chrome: getComputedStyle(document.querySelector(".titlebar")).fontFamily,
      };
    });
    ok("Geist and Geist Mono load from the window's own files", drawn.sans && drawn.mono, JSON.stringify(drawn));
    ok("the faces draw, not their fallbacks", drawn.sansDiffers && drawn.monoDiffers, JSON.stringify(drawn));
    ok("the chrome asks for Geist first", /^"?Geist"?,/.test(drawn.chrome), drawn.chrome);
    ok("the font scene raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* The person's 10-27 window, in fixtures: a project at work with checkouts
 * whose commits are not in main, five projects resting, one more at work,
 * and agents in three tabs. Nobody's pane or file is read. */
async function standChrome(page) {
  await page.waitForFunction(() => projectsRead);
  await page.evaluate(async () => {
    setLocale("ko", { persist: false });
    const tell = (name, payload) => { for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload }); };
    const now = Date.now();
    const unlanded = (ahead, over = {}) => ({ state: "unlanded", detached: false, ahead, dirty: false, dirty_checked_ms: now,
      compare_ref: "origin/main", ref_updated_ms: now - 3_600_000, ...over });
    const checkout = (path, branch, over = {}) => ({ path, branch, is_main: false, active: false, is_folder: false,
      ownership: "zerocode-managed", external_hidden: false, last_activity_ms: now, ...over });
    const resting = ["newbcr", "Obsidian", "ReguProof", "y", "zerocode-landing"];
    window.__ANSWER__.project_catalog = () => [
      { name: "zerocode", path: "/z", worktrees: [
        checkout("/z", "main", { is_main: true, active: true }),
        checkout("/z/wt/judgment", "wt/t-1/trace-judgment-provider-a-long-name-that-the-chips-used-to-cut", { landing: unlanded(3) }),
        checkout("/z/wt/stack", "zo/v1199-stack", { landing: unlanded(6, { behind: 20, far_behind: true }) }),
      ] },
      ...resting.map((name) => ({ name, path: `/p/${name}`, worktrees: [checkout(`/p/${name}`, "main", { is_main: true })] })),
      { name: "work", path: "/w", worktrees: [checkout("/w", "main", { is_main: true })] },
    ];
    window.__ANSWER__.set_sidebar_view = () => null;
    setEveryProjectClosed(false);
    startupProjectFoldsReady = true;
    for (const name of resting) closedProjects.add(`/p/${name}`);
    setSidebarView({ groupBy: "repo" });
    for (const [worktree, agent] of [["/z", "claude"], ["/z", "zo"], ["/w", "zo"]]) {
      const term = await openTermTab({ placement: "tab" });
      tabOfTerm(term).worktree = worktree;
      paneAgents.set(term, agent);
      tell("hook:agent", { term, state: "working", agent, session: `s-${term}` });
    }
    await refreshWorktrees();
    renderTabs();
    await window.__PAINTED__();
  });
  await frames(page);
}

/* 2 · 3 · 5 · 6 · 7 · 10 of the design, read off one laid-out window. */
async function testChromeSidebarAndTabs(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await standChrome(page);
    const seen = await page.evaluate(() => {
      const box = (node) => node.getBoundingClientRect();
      const token = (name) => {
        const probe = document.createElement("i");
        probe.style.color = `var(${name})`;
        document.body.appendChild(probe);
        const value = getComputedStyle(probe).color;
        probe.remove();
        return value;
      };
      const nav = document.querySelector(".nav-rows");
      const rows = [...nav.querySelectorAll(":scope > .nav-row")].filter((row) => !row.hidden);
      const lines = new Set(rows.map((row) => Math.round(box(row).top)));
      const tab = [...document.querySelectorAll(".tab")].find((one) => one.querySelector(".tab-age")?.textContent);
      const label = tab?.querySelector(".tab-label");
      const age = tab?.querySelector(".tab-age");
      const dot = tab?.querySelector(".tab-dot");
      const row = document.querySelector('.wt-row[data-worktree-path="/z/wt/judgment"]');
      const title = row?.querySelector(".wt-title");
      const chip = row?.querySelector(".wt-landing");
      const chipStyle = chip ? getComputedStyle(chip) : null;
      const resting = document.querySelector(".proj-resting");
      return {
        navLines: lines.size,
        navRows: rows.length,
        navHeight: Math.round(box(nav).height),
        tabAge: age?.textContent ?? null,
        ageAfterName: label && age ? Math.round(box(age).left - box(label).right) : null,
        dotShown: dot ? getComputedStyle(dot).visibility === "visible" : false,
        dotInk: dot ? getComputedStyle(dot).backgroundColor : null,
        ready: token("--signal-ready"),
        titleWhole: title ? title.scrollWidth <= title.clientWidth + 1 || box(title).width > box(row.querySelector(".wt-titlebox")).width - 2 : false,
        chipUnderTitle: chip && title ? box(chip).top >= box(title).bottom - 1 : false,
        chipInk: chipStyle?.color ?? null,
        chipBorder: chipStyle?.borderTopColor ?? null,
        wait: token("--signal-wait-ink"),
        restingRows: document.querySelectorAll(".proj-resting").length,
        restingWord: resting?.querySelector(".proj-resting-word")?.textContent ?? null,
        restingHeaders: ["newbcr", "Obsidian"].filter((name) => document.querySelector(`.proj-row[data-project-path="/p/${name}"]`)).length,
        workHeader: Boolean(document.querySelector('.proj-row[data-project-path="/w"]')),
        heading: el("sidebar-title").textContent,
        chatGround: token("--chat-ground"),
        floor: token("--surface-abyss"),
      };
    });
    ok("the seven destinations stand on two lines, not seven (시안 3)",
      seen.navLines === 2 && seen.navRows >= 6, JSON.stringify(seen));
    ok("a tab with an agent wears its state dot and how long it has run, right after its name (시안 2)",
      seen.dotShown && seen.dotInk === seen.ready && /^(now|\d+[mhd])$/.test(seen.tabAge ?? "") && seen.ageAfterName !== null && seen.ageAfterName < 12,
      JSON.stringify(seen));
    ok("a checkout's name takes its whole line and its chips stand under it (시안 5)",
      seen.titleWhole && seen.chipUnderTitle, JSON.stringify(seen));
    ok("unlanded is a quiet number — not the wait ink, no box (시안 6)",
      seen.chipInk !== null && seen.chipInk !== seen.wait && seen.chipBorder === "rgba(0, 0, 0, 0)", JSON.stringify(seen));
    ok("the resting projects stand as one line, the projects at work keep their headers (시안 7)",
      seen.restingRows === 1 && seen.restingWord === "쉬는 프로젝트 5" && seen.restingHeaders === 0 && seen.workHeader,
      JSON.stringify(seen));
    ok("the sidebar heading speaks the person's language (시안 7)", seen.heading === "프로젝트", seen.heading);
    ok("the conversation stands on the floor, one step under the panels (시안 10)",
      seen.chatGround === seen.floor, JSON.stringify(seen));

    // The resting line opens with a press and with the keyboard, and says so.
    const opened = await page.evaluate(async () => {
      const row = document.querySelector(".proj-resting");
      if (!row) return { expanded: null, headers: 0, isButton: false };
      row.focus();
      row.click();
      await new Promise((done) => setTimeout(done, 50));
      await window.__PAINTED__();
      const again = document.querySelector(".proj-resting");
      return {
        expanded: again?.getAttribute("aria-expanded"),
        headers: ["newbcr", "Obsidian"].filter((name) => document.querySelector(`.proj-row[data-project-path="/p/${name}"]`)).length,
        isButton: again?.tagName === "BUTTON",
      };
    });
    ok("the resting line is a button that opens the projects it stands for",
      opened.isButton && opened.expanded === "true" && opened.headers === 2, JSON.stringify(opened));
    // Its spoken name keeps the count and the names apart: 「쉬는 프로젝트 5 ·
    // newbcr …」, not 「…5newbcr」 (review t-78487, P3-5).
    const restingName = await page.getByRole("button", { name: /^쉬는 프로젝트 5 · newbcr · Obsidian/ }).count();
    ok("the resting line's spoken name says the count, then the names", restingName === 1, String(restingName));

    // A tab waiting on the person says it in words as well as its ring (시안 2:
    // the shape and the word together; review t-78487, P3-2).
    const waiting = await page.evaluate(async () => {
      const tab = [...tabs.values()].find((one) => one.kind === "term" && paneAgents.get(tabAgentTerm(one)));
      const term = tabAgentTerm(tab);
      for (const handler of window.__LISTENERS__["hook:agent"] ?? []) {
        handler({ payload: { term, state: "needs-attention", agent: paneAgents.get(term), session: `s-${term}` } });
      }
      await window.__PAINTED__();
      const node = el("tabstrip").querySelector(`.tab[data-tab="${CSS.escape(String(tab.id))}"]`);
      return { classes: node?.className ?? null, tip: node?.dataset.tip ?? null, word: t("board.attention", "확인 필요") };
    });
    ok("a tab waiting on the person says 확인 필요 in its tip, beside its ring",
      /\bis-waiting\b/.test(waiting.classes ?? "") && (waiting.tip ?? "").split(" · ").includes(waiting.word),
      JSON.stringify(waiting));
    ok("the chrome scene raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* 5 — the line under a name is read, not cut to a syllable: the branch's own
 * words and the chips beside them share the line, so neither takes it all.
 * At 1280 px the chip 「미반영 6 · main보다 20 뒤」 took 129 px of the
 * stack row and left its words 52 of the 91 px they need — 「에이전트」 —
 * and at 1000 px 32 (review t-78487, P2-1). The share asked: the words
 * whole, or at least a third of the line — the chips' news keeps the rest
 * (sidebar-landing-state). */
async function testBranchLineRoom(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standChrome(page);
    for (const width of [1280, 1000]) {
      await page.setViewportSize({ width, height: 900 });
      await frames(page);
      const rows = await page.evaluate(() => [...document.querySelectorAll(".wt-row")].flatMap((row) => {
        const line = row.querySelector(".wt-titlebox > .wt-branch");
        if (!line || line.hidden || line.offsetParent === null) return [];
        const box = row.querySelector(".wt-titlebox").getBoundingClientRect().width;
        const chip = row.querySelector(".wt-landing");
        // What the line's words take whole: its parts give way inside it, so
        // the line itself never reads as cut.
        const wants = [...line.children].filter((part) => !part.hidden && part.offsetParent !== null).reduce((sum, part) => {
          const style = getComputedStyle(part);
          return sum + part.scrollWidth + parseFloat(style.marginLeft) + parseFloat(style.marginRight);
        }, 0);
        return [{
          path: row.dataset.worktreePath,
          line: line.clientWidth,
          wants: Math.round(wants),
          box: Math.round(box),
          chip: chip && getComputedStyle(chip).display !== "none" ? chip.clientWidth : 0,
        }];
      }));
      ok(`at ${width} px every branch line under a name keeps its words whole or a third of the line, never a syllable (시안 5)`,
        rows.length >= 2 && rows.every((one) => one.line + 1 >= Math.min(one.wants, one.box / 3)),
        JSON.stringify(rows));
    }
    ok("the branch line scene raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* 10 — one ground: the terminal on the default theme stands on the floor the
 * conversation does, not on the theme's lighter grey — the person's 10-27
 * photo showed the centre alone in another material (#1d1e24 inside #151515).
 * A theme the person picked keeps its own ground, and the shell is told the
 * ground it is drawn on (OSC 11, t-73869). */
async function testOneGround(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.setViewportSize({ width: 1280, height: 900 });
    await standChrome(page);
    // `picked`: a colour the reading resolves beside the screen's, so both
    // compare as the same computed form.
    const read = (picked = null) => page.evaluate(async (picked) => {
      await window.__PAINTED__();
      // One written form for every colour: the screen's ground is a
      // color-mix (the window's opacity), its gutter a plain var.
      const pen = document.createElement("canvas").getContext("2d");
      const ink = (css) => {
        const probe = document.createElement("i");
        probe.style.color = css ?? "";
        document.body.appendChild(probe);
        const value = getComputedStyle(probe).color;
        probe.remove();
        const mixed = value.match(/^color\(srgb ([\d.]+) ([\d.]+) ([\d.]+)\)$/);
        pen.fillStyle = mixed ? `rgb(${mixed.slice(1).map((one) => Math.round(Number(one) * 255)).join(", ")})` : value;
        return pen.fillStyle;
      };
      const screen = document.querySelector(".pane-slot > .terminal");
      const panes = document.querySelector(".panes");
      const reported = window.__REPORTED_GROUNDS__.at(-1) ?? null;
      return {
        screen: screen ? ink(getComputedStyle(screen).backgroundColor) : null,
        panes: panes ? ink(getComputedStyle(panes).backgroundColor) : null,
        chat: ink("var(--chat-ground)"),
        floor: ink("var(--surface-abyss)"),
        reported: reported ? ink(reported) : null,
        picked: picked ? ink(picked) : null,
      };
    }, picked);
    await page.evaluate(() => {
      window.__REPORTED_GROUNDS__ = [];
      window.__ANSWER__.report_terminal_colors = ({ report }) => { window.__REPORTED_GROUNDS__.push(report.background); return null; };
      // Told again, as on a fresh start: the palette reports only a change.
      terminalPaletteSignature = "";
      applyTheme();
    });
    const dark = await read();
    await page.evaluate(() => { document.documentElement.dataset.theme = "light"; applyTerminalPalette(); });
    const light = await read();
    const picked = await page.evaluate(() => {
      delete document.documentElement.dataset.theme;
      const themes = termPrefsSpec.terminal_themes;
      const fallback = themes[termPrefsSpec.theme_defaults.dark];
      const name = Object.keys(themes).find((one) => themes[one].background !== fallback.background);
      termPrefs.theme_dark = name;
      applyTerminalPalette();
      return { name, background: themes[name].background };
    });
    const chosen = await read(picked.background);
    ok("the terminal on the default theme stands on the conversation's ground, the floor, in both treatments (시안 10)",
      dark.screen === dark.chat && dark.panes === dark.chat && dark.chat === dark.floor &&
        light.screen === light.chat && light.panes === light.chat,
      JSON.stringify({ dark, light }));
    ok("the shell is told the ground the terminal is drawn on (OSC 11)",
      dark.reported === dark.screen && light.reported === light.screen, JSON.stringify({ dark, light }));
    ok("a theme the person picked keeps its own ground",
      chosen.screen === chosen.picked && chosen.reported === chosen.picked, JSON.stringify({ picked, chosen }));
    ok("the ground scene raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* 13 — a 14 px ring per provider, amber only past the hot line, never red —
 * and t-77830: the tip a focused bar item shows reaches a screen reader. */
async function testStatusGauges(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    const seen = await page.evaluate(async () => {
      const hour = 3_600_000;
      const now = Date.now();
      window.__USAGE__ = { provider: "claude", account: null, status: "ok", error: null, updated_at: now,
        session: { used_percent: 12, window_minutes: 300, resets_at: now + 4 * hour },
        weekly: { used_percent: 20, window_minutes: 10080, resets_at: now + 100 * hour } };
      window.__CODEX_USAGE__ = { provider: "codex", account: null, status: "ok", error: null, updated_at: now,
        session: { used_percent: 12, window_minutes: 300, resets_at: now + 4 * hour },
        weekly: { used_percent: 99, window_minutes: 10080, resets_at: now + 142 * hour } };
      await refreshClaudeUsage(false);
      await refreshCodexUsage(false);
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const token = (name) => {
        const probe = document.createElement("i");
        probe.style.color = `var(${name})`;
        document.body.appendChild(probe);
        const value = getComputedStyle(probe).color;
        probe.remove();
        return value;
      };
      const face = (id) => {
        const gauge = el(`${id}-bar`);
        const style = getComputedStyle(gauge);
        return {
          size: [gauge.offsetWidth, gauge.offsetHeight],
          round: style.borderRadius,
          value: gauge.style.getPropertyValue("--gauge-value"),
          sweep: style.backgroundImage,
          tone: el(id).dataset.tone ?? null,
          ink: getComputedStyle(el(`${id}-pct`)).color,
        };
      };
      return { claude: face("sb-claude"), codex: face("sb-codex"), wait: token("--signal-wait-ink"),
        halt: token("--signal-halt-ink"), size: getComputedStyle(document.documentElement).getPropertyValue("--gauge-size").trim() };
    });
    ok("each provider wears a ring the size of the gauge token, filled by its tightest window",
      seen.claude.size.join() === "14,14" && seen.size === "14px" && seen.claude.round === "50%" &&
        seen.claude.value === "20" && seen.codex.value === "99" && seen.claude.sweep.startsWith("conic-gradient"),
      JSON.stringify(seen));
    ok("only the nearly spent provider is amber, and not red even at 99 %",
      seen.claude.tone === null && seen.codex.tone === "warn" && seen.codex.ink === seen.wait &&
        seen.codex.ink !== seen.halt && seen.claude.ink !== seen.wait,
      JSON.stringify(seen));

    // t-77830: focused, the roster's tip is its description; blurred, it is not.
    const cdp = await page.context().newCDPSession(page);
    await page.evaluate(() => el("sb-usage-roster").focus());
    const { root } = await cdp.send("DOM.getDocument");
    const { nodeId } = await cdp.send("DOM.querySelector", { nodeId: root.nodeId, selector: "#sb-usage-roster" });
    const { nodes } = await cdp.send("Accessibility.getPartialAXTree", { nodeId, fetchRelatives: false });
    const described = await page.evaluate(() => ({
      by: el("sb-usage-roster").getAttribute("aria-describedby") ?? "",
      tip: document.querySelector(".tooltip")?.textContent ?? "",
    }));
    const description = nodes[0]?.description?.value ?? "";
    await page.evaluate(() => el("sb-usage-roster").blur());
    const after = await page.evaluate(() => el("sb-usage-roster").getAttribute("aria-describedby"));
    ok("a focused status-bar item's tip reaches the accessibility tree as its description (t-77830)",
      described.tip.length > 0 && description === described.tip && described.by.split(" ").includes("window-tooltip"),
      JSON.stringify({ described, description }));
    ok("the description goes with the tip (t-77830)", after === null, String(after));
    ok("the gauge scene raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* 15 — the task board's head in three layers: the search in the title row,
 * the schedule's range in the tab row, and the nav strip between them. */
async function testBoardHead(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await installBoardWaits(page);
    await page.clock.setFixedTime(new Date(SCHEDULE_NOW));
    await page.evaluate(installBoardSchedule, await boardScheduleFixture());
    await openBoardSchedule(page);
    const read = () => page.evaluate(() => {
      const box = (selector) => document.querySelector(`#board-view ${selector}`)?.getBoundingClientRect() ?? null;
      const head = box(".agent-graph-head");
      const search = box(".board-search");
      const tabs = box(".board-now-tabs");
      const zoom = box(".board-schedule-zoom");
      const content = box(".board-schedule-head + *") ?? box(".board-schedule-chart");
      const mid = (r) => r.top + r.height / 2;
      return {
        searchInHead: search.top >= head.top && search.bottom <= head.bottom,
        zoomInTabs: Math.abs(mid(zoom) - mid(tabs)) <= 1 && zoom.right <= tabs.right,
        zoomClearOfTabs: [...document.querySelectorAll("#board-view .board-now-tab")]
          .every((tab) => tab.getBoundingClientRect().right <= zoom.left),
        headToTabsEnd: Math.round(tabs.bottom - head.top),
      };
    });
    const wide = await read();
    ok("the board's search stands in its title row and the schedule's range in its tab row (시안 15)",
      wide.searchInHead && wide.zoomInTabs && wide.zoomClearOfTabs, JSON.stringify(wide));
    ok("the board's head is three layers, about 136 px from its top to the end of its tabs (시안 15)",
      wide.headToTabsEnd <= 140, JSON.stringify(wide));
    ok("the board head scene raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* 17 — a narrow window folds the tabs not on stage to their mark: the name
 * leaves the eye and stays the tab's spoken name. The draft folds at the
 * window's width (`--fold-sidebar-rail`), so the scene holds both sides of
 * it: folded at 1000 px, every name back at 1001 px. */
async function testNarrowFolds(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await standChrome(page);
    const read = () => page.evaluate(() => [...el("tabstrip").querySelectorAll(".tab")].map((tab) => {
      const label = tab.querySelector(".tab-label");
      return {
        // The tab its leaf shows: the window's active tab, or the leaf's last.
        active: tab.classList.contains("is-front"),
        shown: label.getBoundingClientRect().width > 2,
        spoken: label.textContent.length > 0 && getComputedStyle(label).display !== "none",
      };
    }));
    const wide = await read();
    await page.setViewportSize({ width: 1001, height: 900 });
    await frames(page);
    const above = await read();
    await page.setViewportSize({ width: 1000, height: 900 });
    await frames(page);
    const narrow = await read();
    ok("a wide window shows every tab's name (시안 17)", wide.every((tab) => tab.shown), JSON.stringify(wide));
    ok("a 1001 px window still shows every tab's name — the fold is the window's 1000 px, whatever the title bar's chip says (시안 17)",
      above.every((tab) => tab.shown), JSON.stringify(above));
    ok("in a 1000 px window the title bar's strip folds the tabs not on stage to their mark, keeping their spoken name; the one on stage keeps its name (시안 17)",
      narrow.some((tab) => !tab.active) && narrow.filter((tab) => !tab.active).every((tab) => !tab.shown && tab.spoken) &&
        narrow.filter((tab) => tab.active).every((tab) => tab.shown),
      JSON.stringify({ narrow }));
    ok("the fold scene raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* 17 — under 760 px the strip is the tab on stage and one menu of every tab
 * (「탭 N」); under 480 px the strip's other controls fold into that menu too.
 * The menu's rows exist only while it is open. The widths are named tokens a
 * later fold (the panels' rails) reads too, and the rules fold at those. */
async function testTabMenuFold(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.setViewportSize({ width: 1000, height: 900 });
    await standChrome(page);
    // The checkout on stage holds two of the three tabs; one of them is in front.
    await page.evaluate(() => setActiveTab(el("tabstrip").querySelector(".tab").dataset.tab));
    await frames(page);
    const read = () => page.evaluate(() => {
      const strip = el("tabstrip");
      const shown = (node) => node !== null && node.getClientRects().length > 0 && node.getBoundingClientRect().width > 2;
      const opener = strip.querySelector(".tab-menu");
      return {
        tabs: [...strip.querySelectorAll(".tab")].map((tab) => ({ active: tab.classList.contains("is-active"), shown: shown(tab) })),
        opener: shown(opener),
        openerWords: opener?.textContent.trim() ?? null,
        openerPopup: opener?.getAttribute("aria-haspopup") ?? null,
        openerExpanded: opener?.getAttribute("aria-expanded") ?? null,
        add: shown(strip.querySelector(".tab-new")),
        // The opener stands whole inside its strip, not scrolled past its end.
        openerWhole: opener !== null && opener.getBoundingClientRect().right <= strip.getBoundingClientRect().right + 1,
        // The tab in front still says its (short) name whole.
        frontNamed: ((label) => label !== null && label.getBoundingClientRect().width > 0 &&
          label.scrollWidth <= label.clientWidth + 1)(strip.querySelector(".tab.is-front .tab-label")),
        menuRows: el("sidebar-menu").childElementCount,
      };
    });
    const at1000 = await read();
    ok("at 1000 px the tabs fold to marks and no tab menu stands (시안 17)",
      !at1000.opener && at1000.tabs.length === 2 && at1000.tabs.every((tab) => tab.shown), JSON.stringify(at1000));

    await page.setViewportSize({ width: 760, height: 900 });
    await frames(page);
    const at760 = await read();
    ok("at 760 px the strip is the tab on stage and one 「탭 N」 menu (시안 17)",
      at760.opener && at760.openerWhole && at760.openerWords === "탭 2" && at760.openerPopup === "menu" && at760.openerExpanded === "false" &&
        at760.tabs.filter((tab) => tab.shown).length === 1 && at760.tabs.find((tab) => tab.active)?.shown === true,
      JSON.stringify(at760));
    ok("the tab menu has no rows while it is closed", at760.menuRows === 0, JSON.stringify(at760));

    await page.focus("#tabstrip .tab-menu");
    await page.keyboard.press("Enter");
    await frames(page);
    const open = await page.evaluate(() => {
      const menu = el("sidebar-menu");
      const rows = [...menu.querySelectorAll('[role="menuitemradio"]')];
      return {
        visible: !menu.hidden,
        rows: rows.map((row) => row.textContent.trim()),
        checked: rows.filter((row) => row.getAttribute("aria-checked") === "true").length,
        focusInside: menu.contains(document.activeElement),
        expanded: el("tabstrip").querySelector(".tab-menu").getAttribute("aria-expanded"),
      };
    });
    ok("Enter on the opener lists every tab of the strip, the one on stage checked, with the keyboard inside",
      open.visible && open.rows.length === 2 && open.checked === 1 && open.focusInside && open.expanded === "true",
      JSON.stringify(open));

    await page.keyboard.press("Escape");
    await page.waitForFunction(() => el("sidebar-menu").childElementCount === 0, null, { timeout: 2000 }).catch(() => {});
    const shut = await page.evaluate(() => ({
      rows: el("sidebar-menu").childElementCount,
      focusBack: document.activeElement === el("tabstrip").querySelector(".tab-menu"),
      expanded: el("tabstrip").querySelector(".tab-menu").getAttribute("aria-expanded"),
    }));
    ok("Escape closes the menu, empties it and hands the keyboard back to its opener",
      shut.rows === 0 && shut.focusBack && shut.expanded === "false", JSON.stringify(shut));

    await page.keyboard.press("Enter");
    await frames(page);
    const picked = await page.evaluate(() => {
      const onStage = () => el("tabstrip").querySelector(".tab.is-active")?.dataset.tab ?? null;
      const before = onStage();
      const row = [...el("sidebar-menu").querySelectorAll('[role="menuitemradio"]')].find((one) => one.getAttribute("aria-checked") === "false");
      row.click();
      return { before };
    });
    await frames(page);
    picked.after = await page.evaluate(() => el("tabstrip").querySelector(".tab.is-active")?.dataset.tab ?? null);
    ok("a row of the menu puts its tab on stage", picked.after !== null && picked.after !== picked.before, JSON.stringify(picked));

    await page.setViewportSize({ width: 480, height: 900 });
    await frames(page);
    const at480 = await read();
    await page.click("#tabstrip .tab-menu");
    await frames(page);
    const tail = await page.evaluate(() => [...el("sidebar-menu").querySelectorAll('[role="menuitem"]')].map((row) => row.textContent.trim()));
    await page.keyboard.press("Escape");
    ok("at 480 px the strip keeps the tab on stage and its menu, and the new-terminal control folds into that menu (시안 17)",
      at480.opener && at480.openerWhole && at480.frontNamed && !at480.add && at480.tabs.filter((tab) => tab.shown).length === 1 && tail.some((words) => words.startsWith("새 터미널")),
      JSON.stringify({ at480, tail }));

    // A split: the leaf without the window's active tab still shows its own.
    await page.setViewportSize({ width: 760, height: 900 });
    await frames(page);
    const split = await page.evaluate(async () => {
      const pane = splitStageBesideFocused("horizontal");
      await openTermTab({ placement: "tab" });
      const fresh = currentTab();
      fresh.pane = pane;
      await openTermTab({ placement: "tab" });
      const front = currentTab();
      front.pane = pane;
      setActiveTab(el("tabstrip").querySelector(".tab").dataset.tab);
      renderTabs();
      await window.__PAINTED__();
      return pane;
    });
    await frames(page);
    const strips = await page.evaluate(() => [...document.querySelectorAll(".tabstrip")].map((strip) => ({
      shown: [...strip.querySelectorAll(".tab")].filter((tab) => tab.getClientRects().length > 0).length,
      opener: strip.querySelector(".tab-menu")?.textContent.trim() ?? null,
    })));
    ok("in a split at 760 px each leaf's strip shows its own front tab and its own menu",
      split !== null && strips.length === 2 && strips.every((strip) => strip.shown === 1 && strip.opener === "탭 2"),
      JSON.stringify({ split, strips }));

    const widths = await page.evaluate(() => {
      const root = getComputedStyle(document.documentElement);
      const token = (name) => root.getPropertyValue(name).trim();
      const folds = new Set();
      const visit = (rules) => {
        for (const rule of rules) {
          if (rule instanceof CSSMediaRule && /\.tab-menu|\.tab-new|\.tab:not\(\.is-front\) \.tab-label/.test(rule.cssText)) folds.add(rule.conditionText);
          else if (rule.cssRules) visit(rule.cssRules);
        }
      };
      for (const sheet of document.styleSheets) {
        try { visit(sheet.cssRules); } catch { /* a sheet from elsewhere */ }
      }
      return {
        tokens: ["--fold-panel-rail", "--fold-sidebar-rail", "--fold-tab-menu", "--fold-tab-only"].map(token),
        folds: [...folds],
      };
    });
    ok("the fold widths are named tokens — 1360, 1000, 760 and 480 px — the tabs fold to marks at the second and to the tab menu at the last two",
      widths.tokens.join() === "1360px,1000px,760px,480px" &&
        widths.folds.length === 3 && widths.folds.every((when) => widths.tokens.slice(1).some((width) => when.includes(width))),
      JSON.stringify(widths));
    ok("the tab menu scene raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* Both themes: what this pass turned quiet still reads (WCAG 4.5:1 for words). */
async function testQuietContrast(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await standChrome(page);
    for (const theme of ["dark", "light"]) {
      const ratios = await page.evaluate(async (theme) => {
        // The harness holds the window's clock still, so an eased colour change
        // would be read at its first frame: the colours are read at rest.
        if (!document.getElementById("contrast-at-rest")) {
          const rest = document.createElement("style");
          rest.id = "contrast-at-rest";
          rest.textContent = "* { transition: none !important; }";
          document.head.append(rest);
        }
        document.documentElement.dataset.theme = theme;
        await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        // `color-mix()` resolves to `color(srgb r g b / a)` on a 0–1 scale.
        const rgb = (value) => {
          const [r, g, b, a = 1] = value.replace(/^color\(srgb/, "").match(/[\d.]+/g).map(Number);
          const scale = value.startsWith("color(") ? 255 : 1;
          return [r * scale, g * scale, b * scale, a];
        };
        const groundOf = (node) => {
          for (let at = node; at; at = at.parentElement) {
            const [r, g, b, a] = rgb(getComputedStyle(at).backgroundColor);
            if (a > 0.99) return [r, g, b];
          }
          return [0, 0, 0];
        };
        const lum = ([r, g, b]) => {
          const c = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; };
          return 0.2126 * c(r) + 0.7152 * c(g) + 0.0722 * c(b);
        };
        const ratio = (node) => {
          const ground = groundOf(node);
          const [r, g, b, a] = rgb(getComputedStyle(node).color);
          const ink = [r * a + ground[0] * (1 - a), g * a + ground[1] * (1 - a), b * a + ground[2] * (1 - a)];
          const [hi, lo] = [lum(ink), lum(ground)].sort((x, y) => y - x);
          return Math.round(((hi + 0.05) / (lo + 0.05)) * 100) / 100;
        };
        return {
          unlanded: ratio(document.querySelector('.wt-row[data-worktree-path="/z/wt/judgment"] .wt-landing')),
          resting: ratio(document.querySelector(".proj-resting")),
          heading: ratio(el("sidebar-title")),
        };
      }, theme);
      ok(`the quiet unlanded number, the resting line and the heading read at 4.5:1 or more (${theme})`,
        Object.values(ratios).every((value) => value >= 4.5), JSON.stringify(ratios));
    }
    ok("the contrast scene raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

export async function testChromeGeist(browser, origin, ok) {
  await testGeistShips(browser, origin, ok);
  await testChromeSidebarAndTabs(browser, origin, ok);
  await testBranchLineRoom(browser, origin, ok);
  await testOneGround(browser, origin, ok);
  await testStatusGauges(browser, origin, ok);
  await testBoardHead(browser, origin, ok);
  await testNarrowFolds(browser, origin, ok);
  await testTabMenuFold(browser, origin, ok);
  await testQuietContrast(browser, origin, ok);
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1] ?? "")).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failed = 0;
  try {
    await testChromeGeist(browser, origin, (name, pass, detail) => {
      if (!pass) failed += 1;
      console.log(`${pass ? "PASS" : "FAIL"}  ${name}${pass ? "" : "  — " + (detail ?? "")}`);
    });
  } finally {
    await browser.close();
    files.close();
  }
  process.exit(failed ? 1 : 0);
}

export { frames };
