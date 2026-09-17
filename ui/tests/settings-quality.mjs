import { mkdir, writeFile } from "node:fs/promises";
import { dirname } from "node:path";

export const SETTINGS_QUALITY_SPEC = Object.freeze({
  viewport: Object.freeze({ width: 720, height: 480 }),
  locales: Object.freeze(["ko", "en", "ja", "zh", "es"]),
  themes: Object.freeze(["dark", "light"]),
  wcagTags: Object.freeze(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"]),
  performance: Object.freeze({
    warmups: 3,
    samples: 15,
    budgetsMs: Object.freeze({
      open: 300,
      search: 150,
      pane_switch: 150,
      snapshot_apply: 250,
    }),
  }),
});

const INTERACTIVE_SELECTOR = [
  "button",
  "input",
  "select",
  "textarea",
  "a[href]",
  "[role='button']",
  "[role='tab']",
  "[role='switch']",
  "[role='checkbox']",
  "[tabindex]:not([tabindex='-1'])",
].join(",");

export async function settingsPaneIds(page) {
  return page.locator(".settings-rail-item[data-pane]").evaluateAll((items) =>
    [...new Set(items.map((item) => item.dataset.pane).filter(Boolean))]);
}

export async function setQualityTheme(page, theme) {
  await page.evaluate((code) => setTheme(code), theme);
  await settlePaint(page);
}

export async function setQualityLocale(page, locale) {
  await page.evaluate((code) => setLocale(code, { refresh: false, persist: false }), locale);
  await settlePaint(page);
}

export async function axeViolations(page, AxeBuilder, include) {
  const answer = await new AxeBuilder({ page })
    .include(include)
    .withTags([...SETTINGS_QUALITY_SPEC.wcagTags])
    .analyze();
  return answer.violations.map(({ id, impact, help, nodes }) => ({
    id,
    impact,
    help,
    nodes: nodes.map(({ target, failureSummary }) => ({ target, failureSummary })),
  }));
}

export async function inspectSettingsLayout(page) {
  return page.evaluate((interactiveSelector) => {
    const visible = (node) => {
      const style = getComputedStyle(node);
      return !node.hidden
        && style.display !== "none"
        && style.visibility !== "hidden"
        && node.getClientRects().length > 0;
    };
    const labelledBy = (node) => (node.getAttribute("aria-labelledby") ?? "")
      .split(/\s+/)
      .filter(Boolean)
      .map((id) => document.getElementById(id)?.textContent ?? "")
      .join(" ");
    const accessibleName = (node) => {
      const own = node.getAttribute("aria-label") ?? "";
      const labelled = labelledBy(node);
      const explicit = node.id
        ? document.querySelector(`label[for="${CSS.escape(node.id)}"]`)?.textContent ?? ""
        : "";
      const wrapping = node.closest("label")?.textContent ?? "";
      const text = node.textContent ?? "";
      const fallback = node.getAttribute("alt")
        ?? node.getAttribute("title")
        ?? node.dataset.tip
        ?? node.getAttribute("placeholder")
        ?? "";
      return [own, labelled, explicit, wrapping, text, fallback]
        .find((value) => value.trim() !== "")?.trim() ?? "";
    };
    const describe = (node) => {
      const id = node.id ? `#${node.id}` : "";
      const role = node.getAttribute("role");
      return `${node.tagName.toLowerCase()}${id}${role ? `[role=${role}]` : ""}`;
    };
    const roots = [
      document.getElementById("settings-view"),
      document.querySelector(".settings-rail"),
      document.querySelector(".settings-frame"),
      document.querySelector(".settings-pane:not([hidden])"),
    ].filter(Boolean);
    const overflowing = roots
      .filter((node) => node.scrollWidth > node.clientWidth + 1)
      .map((node) => ({
        node: describe(node),
        clientWidth: node.clientWidth,
        scrollWidth: node.scrollWidth,
      }));
    const pane = document.querySelector(".settings-pane:not([hidden])");
    const paneBounds = pane?.getBoundingClientRect();
    const overflowSources = overflowing.length === 0 || !paneBounds
      ? []
      : [...pane.querySelectorAll("*")]
        .filter(visible)
        .flatMap((node) => {
          const rect = node.getBoundingClientRect();
          const ownOverflow = node.scrollWidth > node.clientWidth + 1;
          const outsidePane = rect.left < paneBounds.left - 1 || rect.right > paneBounds.right + 1;
          if (!ownOverflow && !outsidePane) return [];
          const style = getComputedStyle(node);
          return [{
            node: describe(node),
            className: node.className,
            text: (node.textContent ?? "").trim().replace(/\s+/g, " ").slice(0, 100),
            clientWidth: node.clientWidth,
            scrollWidth: node.scrollWidth,
            left: Number(rect.left.toFixed(1)),
            right: Number(rect.right.toFixed(1)),
            whiteSpace: style.whiteSpace,
            minWidth: style.minWidth,
          }];
        })
        .slice(0, 20);
    const controls = [...new Set(document.querySelectorAll(interactiveSelector))]
      .filter((node) => node.closest("#settings-view") && visible(node));
    const unnamed = controls
      .filter((node) => accessibleName(node) === "")
      .map(describe);
    const outOfBounds = controls.flatMap((node) => {
      const rect = node.getBoundingClientRect();
      return rect.left < -1 || rect.right > innerWidth + 1
        ? [{ node: describe(node), left: rect.left, right: rect.right, viewport: innerWidth }]
        : [];
    });
    return { overflowing, overflowSources, unnamed, outOfBounds, controls: controls.length };
  }, INTERACTIVE_SELECTOR);
}

/* The largest finite preference surface the renderer can enumerate: every
 * action has an override, every optional shortcut is hidden, and every typed
 * terminal field is at its declared boundary. It grows automatically when a
 * registry grows; the fixture contains no second action count or field cap. */
export async function maximalSettingsFixture(page, base) {
  return page.evaluate((snapshot) => {
    const next = JSON.parse(JSON.stringify(snapshot));
    next.keybindings = Object.fromEntries(ACTIONS.map((action, index) => [
      action.id,
      [`mod+alt+shift+f${index + 1}`],
    ]));
    next.hidden_shortcuts = SHORTCUT_ROWS.slice(0, -1).map(({ name }) => name);
    next.ctrl_tab_order_mode = "sequential";
    next.terminal_shortcut_policy = "terminal-first";
    next.workspace_creation_prefs = {
      directory: "/Users/example/zerocode/workspaces/with-a-deliberately-long-directory-name",
      nest_workspaces: false,
    };
    next.floating_workspace = {
      enabled: true,
      cwd: "/Users/example/floating/workspace/with-a-deliberately-long-seat-name",
      trigger_location: "status-bar",
    };
    next.setup_script_launch_mode = "split-horizontal";
    next.open_in_applications = Array.from(
      { length: next.open_in_applications_spec.max },
      (_, index) => ({
        id: `fixture-${index + 1}`,
        label: `Editor With A Deliberately Long Name ${index + 1}`,
        command: `editor-${index + 1} --new-window --profile fixture-${index + 1}`,
      }),
    );
    next.skip_delete_worktree_confirm = true;
    next.skip_delete_automation_confirm = true;
    next.usage_percentage_display = "remaining";
    next.status_bar_usage_mode = "compact";
    next.show_titlebar_app_name = false;
    next.show_menu_bar_icon = false;
    next.minimize_to_tray_on_close = true;
    next.ui_zoom_level = next.ui_zoom_spec.max_level;
    next.app_font_family = "JetBrains Mono";
    next.compact_worktree_cards = true;
    next.show_git_ignored_files = false;
    next.source_control_group_order = "untracked-first";
    next.source_control_compare_base = "branch-upstream";
    next.refresh_local_base_ref_on_worktree_create = true;
    next.left_sidebar_appearance_mode = "tinted";
    next.left_sidebar_tint_color = "#336699";
    next.left_sidebar_tint_opacity = next.left_sidebar_appearance_spec.tint_opacity.max;
    next.browser = {
      ...next.browser,
      open_links_in_app: true,
      open_links_in_app_modifier_inverts: true,
      restore_tabs: true,
      default_zoom_level: next.browser_zoom_spec.max_level,
      open_tabs: ACTIONS.map((action, index) =>
        `https://settings-fixture.invalid/${index + 1}/${encodeURIComponent(action.id)}`),
      user_agents: [
        {
          host: "settings-fixture.invalid",
          agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.3 Safari/605.1.15 settings-fixture",
        },
        { host: "x.test", agent: "UA-x" },
      ],
    };
    const spec = next.terminal_prefs_spec;
    next.terminal_prefs = {
      ...next.terminal_prefs,
      font_size: spec.font_size.max,
      font_family: "JetBrains Mono",
      ligatures: "on",
      mac_option_as_alt: spec.mac_option_as_alt_modes.at(-1),
      jis_yen_to_backslash: true,
      windows_shell: spec.windows_shells.at(-1),
      windows_powershell_implementation:
        spec.windows_powershell_implementations.at(-1),
      theme_dark: Object.keys(spec.terminal_themes).at(-1),
      use_separate_light_theme: true,
      theme_light: Object.keys(spec.terminal_themes).at(0),
      color_overrides: Object.fromEntries(
        spec.color_override_groups
          .flatMap((group) => group.keys)
          .map((key, index) => [key, `#${(index + 1).toString(16).padStart(6, "0")}`]),
      ),
      leading: Number((spec.leading.max * spec.leading_base).toFixed(4)),
      weight: spec.weight.max,
      cursor_style: spec.cursor_styles.at(-1),
      cursor_opacity: spec.cursor_opacity.max,
      padding_x: spec.padding_x.max,
      padding_y: spec.padding_y.max,
      sensitivity: spec.sensitivity.max,
      fast_scroll_sensitivity: spec.fast_scroll_sensitivity.max,
      tui_scroll_sensitivity: spec.tui_scroll_sensitivity.max,
      scrollback: spec.scrollback.max,
      word_separators: "x".repeat(spec.word_separators_max_chars),
      cursor_blink: true,
      focus_follows_mouse: true,
      hide_mouse_while_typing: true,
      allow_osc52_clipboard: true,
      inactive_pane_opacity: spec.inactive_pane_opacity.max,
      divider_color_dark: "#123456",
      divider_color_light: "#abcdef",
      divider_thickness_px: spec.divider_thickness_px.max,
    };
    next.editing_prefs = {
      ...next.editing_prefs,
      editor_auto_save: true,
      editor_auto_save_delay_ms: next.editing_prefs_spec.auto_save_delay_ms.max,
      editor_minimap_enabled: true,
      rich_markdown_spellcheck_enabled: true,
      markdown_review_tools_enabled: true,
      editor_font_family: "JetBrains Mono",
      combined_diff_file_tree_visible_by_default: true,
      primary_selection_middle_click_paste: true,
    };
    return next;
  }, base);
}

export async function measureSettingsPerformance(page, fixture, panes) {
  const { warmups, samples, budgetsMs } = SETTINGS_QUALITY_SPEC.performance;
  await page.evaluate(() => {
    window.__SETTINGS_LONG_TASKS__ = [];
    window.__SETTINGS_LONG_TASK_OBSERVER__?.disconnect();
    window.__SETTINGS_LONG_TASK_OBSERVER__ = new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        window.__SETTINGS_LONG_TASKS__.push({
          startTime: entry.startTime,
          duration: entry.duration,
        });
      }
    });
    window.__SETTINGS_LONG_TASK_OBSERVER__.observe({ type: "longtask", buffered: false });
  });

  const operations = Object.keys(budgetsMs);
  const timings = Object.fromEntries(operations.map((name) => [name, []]));
  const spans = [];
  for (const [operationIndex, operation] of operations.entries()) {
    for (let sample = -warmups; sample < samples; sample += 1) {
      const span = await page.evaluate(async ({ fixture: full, operation: name, panes: ids, seed }) => {
        const settle = () => new Promise((done) =>
          requestAnimationFrame(() => requestAnimationFrame(done)));
        const search = document.getElementById("settings-search");
        if (name === "open") {
          setSettingsOpen(false);
          await settle();
        } else {
          setSettingsOpen(true);
          search.value = "";
          paintSettingsSearch();
          showSettingsPane(ids[seed % ids.length]);
          await settle();
        }
        const started = performance.now();
        if (name === "open") {
          setSettingsOpen(true);
        } else if (name === "search") {
          search.value = "terminal";
          search.dispatchEvent(new Event("input"));
        } else if (name === "pane_switch") {
          showSettingsPane(ids[(seed + 1) % ids.length]);
        } else if (name === "snapshot_apply") {
          applySettingsSnapshot({ ...full, revision: settingsRevision + 1 });
        } else {
          throw new Error(`unknown performance operation: ${name}`);
        }
        await settle();
        const ended = performance.now();
        return { started, ended, duration: ended - started };
      }, { fixture, operation, panes, seed: sample + warmups + operationIndex });
      spans.push({ operation, sample, ...span });
      if (sample >= 0) timings[operation].push(span.duration);
    }
  }

  const rawLongTasks = await page.evaluate(() => {
    window.__SETTINGS_LONG_TASK_OBSERVER__?.disconnect();
    return window.__SETTINGS_LONG_TASKS__ ?? [];
  });
  const longTasks = rawLongTasks
    .map((task) => {
      const context = spans.find(
        (span) => task.startTime >= span.started && task.startTime < span.ended,
      );
      return context
        ? { ...task, operation: context.operation, sample: context.sample }
        : task;
    })
    // Warmups exist to absorb JIT/font/layout cold starts and are already
    // excluded from p95. Applying a stricter long-task gate to those same
    // discarded samples made the two halves of one performance contract
    // disagree. Unattributed tasks are outside a measured operation too.
    .filter((task) => Number.isInteger(task.sample) && task.sample >= 0);
  const measured = Object.fromEntries(Object.entries(timings).map(([name, values]) => [name, {
    samples_ms: values.map((value) => Number(value.toFixed(3))),
    p95_ms: Number(percentile(values, 0.95).toFixed(3)),
    budget_ms: budgetsMs[name],
  }]));
  return {
    fixture: {
      actions: Object.keys(fixture.keybindings).length,
      hidden_shortcuts: fixture.hidden_shortcuts.length,
      restored_tabs: fixture.browser.open_tabs.length,
      open_in_applications: fixture.open_in_applications.length,
      word_separator_chars: fixture.terminal_prefs.word_separators.length,
    },
    operations: measured,
    long_tasks: longTasks,
  };
}

export async function writeQualityReport(report) {
  const path = process.env.SETTINGS_QUALITY_REPORT;
  const serialized = `${JSON.stringify(report, null, 2)}\n`;
  if (path) {
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, serialized, "utf8");
  }
  console.log(`SETTINGS_QUALITY ${JSON.stringify(report)}`);
}

export function performanceFailures(performance) {
  const overBudget = Object.entries(performance.operations).flatMap(([name, result]) =>
    result.p95_ms > result.budget_ms
      ? [{ operation: name, p95_ms: result.p95_ms, budget_ms: result.budget_ms }]
      : []);
  return { overBudget, longTasks: performance.long_tasks };
}

export async function settlePaint(page) {
  await page.evaluate(async () => {
    const nextPaint = () => new Promise((done) =>
      requestAnimationFrame(() => requestAnimationFrame(done)));
    await nextPaint();
    const finiteAnimations = document.getAnimations({ subtree: true }).filter((animation) => {
      const iterations = animation.effect?.getTiming().iterations;
      return iterations !== Infinity;
    });
    await Promise.all(finiteAnimations.map((animation) => animation.finished.catch(() => {})));
    await nextPaint();
  });
}

function percentile(values, quantile) {
  const sorted = [...values].sort((left, right) => left - right);
  if (sorted.length === 0) return 0;
  const index = Math.max(0, Math.ceil(sorted.length * quantile) - 1);
  return sorted[index];
}
