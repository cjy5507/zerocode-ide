/* Settings, driven through a stateful backend.
 *
 * This suite is intentionally separate from window.mjs. The broad window
 * suite is a renderer fixture; this one is a persistence/concurrency fixture:
 * one Node-owned settings store, two real documents, strict IPC, revisions,
 * failed writes, a lost acknowledgement, and cross-window events.
 *
 *   node ui/tests/settings.mjs
 */

import { createRequire } from "node:module";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { dirname, extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import {
  SETTINGS_QUALITY_SPEC,
  axeViolations,
  inspectSettingsLayout,
  maximalSettingsFixture,
  measureSettingsPerformance,
  performanceFailures,
  setQualityLocale,
  setQualityTheme,
  settingsPaneIds,
  settlePaint,
  writeQualityReport,
} from "./settings-quality.mjs";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const TERMINAL_THEME_CATALOG = Object.freeze(JSON.parse(
  await readFile(
    resolve(UI, "..", "crates", "zerocode-shell", "src", "terminal_themes.json"),
    "utf8",
  ),
));
/* The keychain item the TypeSafe key lives in — `typesafe_settings.rs`
 * (`SERVICE_KEYCHAIN_SERVICE_PREFIX` + `TYPESAFE_API_KEY_ENV`). */
const TYPESAFE_SERVICE = "dev.zerocode.key.TYPESAFE_API_KEY";
/* The routing row of the Jev use table (`zerocode_core::jev::ROUTING`), as
 * `typesafe_settings` answers it: each mode's word and what it does. The Rust
 * contract `the_pane_offers_the_rows_modes_and_asks_registered_commands`
 * holds this fixture to the table. */
const TYPESAFE_DECISION_MODES = Object.freeze([
  Object.freeze({ mode: "off", asks: false, applies: false, automatic: false }),
  Object.freeze({ mode: "shadow", asks: true, applies: false, automatic: false }),
  Object.freeze({ mode: "on", asks: true, applies: true, automatic: false }),
  Object.freeze({ mode: "auto", asks: true, applies: false, automatic: true }),
]);
/* Every seat of `zerocode_core::jev::JEV_USES`, in the table's order, with the
   settings key it writes and the modes it offers — a labeled seat offers all
   four, a seat nothing labels offers no `auto`. `typesafe_settings.rs` holds
   this list against the table. */
const JEV_SEATS = Object.freeze([
  Object.freeze({ id: "routing", setting: "decisionShadow", modes: "off shadow on auto" }),
  Object.freeze({ id: "recall", setting: "rerankShadow", modes: "off shadow on auto" }),
  Object.freeze({ id: "skills", setting: "skillSearch", modes: "off shadow on auto" }),
  Object.freeze({ id: "browser", setting: "browserAction", modes: "off shadow on auto" }),
  Object.freeze({ id: "desktop", setting: "desktopAction", modes: "off shadow on auto" }),
  Object.freeze({ id: "emulator", setting: "emulatorAction", modes: "off shadow on auto" }),
  Object.freeze({ id: "stall", setting: "stallCause", modes: "off shadow on auto" }),
  Object.freeze({ id: "placement", setting: "workerPlacement", modes: "off shadow on auto" }),
  Object.freeze({ id: "summon", setting: "summonChoice", modes: "off shadow on auto" }),
  Object.freeze({ id: "effort", setting: "stepEffort", modes: "off shadow on auto" }),
  Object.freeze({ id: "step_effort", setting: "zoStepEffort", modes: "off shadow on auto" }),
  Object.freeze({ id: "compaction", setting: "jevCompaction", modes: "off shadow on auto" }),
  Object.freeze({ id: "agent_tool", setting: "agentTool", modes: "off shadow on" }),
  Object.freeze({ id: "browser_read", setting: "jevBrowserRead", modes: "off shadow on auto" }),
  Object.freeze({ id: "notify", setting: "jevNotify", modes: "off shadow on auto" }),
  Object.freeze({ id: "mention_rerank", setting: "jevMentionRerank", modes: "off shadow on auto" }),
  Object.freeze({ id: "branching", setting: "jevBranching", modes: "off shadow on auto" }),
  Object.freeze({ id: "judgment_cache", setting: "jevJudgmentCache", modes: "off shadow on auto" }),
  Object.freeze({ id: "challenger", setting: "jevChallenger", modes: "off shadow on auto" }),
  Object.freeze({ id: "patch_review", setting: "jevPatchReview", modes: "off shadow on auto" }),
]);
const jevSeat = (id) => JEV_SEATS.find((seat) => seat.id === id) ?? null;
/* The model pin every Jev request names (`zerocode_core::jev::MODEL_SETTING`)
   and the alias an unpinned request asks (`DEFAULT_MODEL`), as
   `typesafe_settings` answers them — `typesafe_settings.rs` holds these to
   the core's. A pin is one word: blank is no pin, as the door reads it. */
const JEV_MODEL_SETTING = "jevModel";
const JEV_MODEL_ALIAS = "jev-latest";
const jevModelPin = (given) => {
  const word = String(given ?? "").trim();
  return word && !/[\s\p{Cc}]/u.test(word) ? word : null;
};
/* The routing classifier's four words (`zerocode_core::jev::ClassifierMode`)
   and what each one does. Only the probing word calls a probe, and the routing
   seat is asked nothing under the other three — `typesafe_settings.rs` holds
   this fixture to the table and to zo's own source. */
const CLASSIFIER_SETTING = "autoClassifier";
const CLASSIFIER_MODES = Object.freeze([
  Object.freeze({ mode: "off", runs: false, markers: false, probes: false }),
  Object.freeze({ mode: "deterministic", runs: true, markers: false, probes: false }),
  Object.freeze({ mode: "assisted", runs: true, markers: true, probes: false }),
  Object.freeze({ mode: "probed", runs: true, markers: false, probes: true }),
]);
/* An absent key is the probing word; anything nobody reads is the
   provider-free one (`ClassifierMode::of`). */
const classifierMode = (given) => {
  if (given === undefined) return CLASSIFIER_MODES[3];
  const word = String(given ?? "").trim().toLowerCase();
  return CLASSIFIER_MODES.find((choice) => choice.mode === word) ?? CLASSIFIER_MODES[1];
};
const jevSeatModes = (seat) =>
  seat.modes.split(" ").map((word) =>
    TYPESAFE_DECISION_MODES.find((choice) => choice.mode === word));
const ROUTER_PRESETS_CATALOG = Object.freeze(JSON.parse(
  await readFile(
    resolve(UI, "..", "crates", "zerocode-shell", "src", "api-routers.json"),
    "utf8",
  ),
));
const ORCA_SETTINGS_CONTRACT = Object.freeze(JSON.parse(
  await readFile(resolve(UI, "tests", "orca-settings-1.4.180.json"), "utf8"),
));

let chromium;
let AxeBuilder;
try {
  const require = createRequire(import.meta.url);
  let entry;
  try {
    entry = require.resolve("playwright");
  } catch {
    const root = await new Promise((done) => {
      const npm = spawn("npm", ["root", "-g"], { stdio: ["ignore", "pipe", "ignore"] });
      let out = "";
      npm.stdout.on("data", (chunk) => (out += chunk));
      npm.on("close", () => done(out.trim()));
      npm.on("error", () => done(""));
    });
    if (!root) throw new Error("no global npm root");
    entry = require.resolve(resolve(root, "playwright"));
  }
  ({ chromium } = require(entry));
  const axeEntry = require.resolve("@axe-core/playwright");
  const axeModule = require(axeEntry);
  AxeBuilder = axeModule.default ?? axeModule;
} catch (error) {
  console.error("FAIL  Playwright and @axe-core/playwright are required for the settings browser gate.");
  console.error("      Install it with: npm ci && npx playwright install chromium");
  console.error(`      ${error?.message ?? error}`);
  process.exit(1);
}

const clone = (value) => JSON.parse(JSON.stringify(value));
const same = (left, right) => JSON.stringify(left) === JSON.stringify(right);
const UI_TIMEOUT = 2_500;

// These two JSON blocks are the browser backend's serialized Rust contract.
// A Rust source test compares them to SettingsDocument::default() and
// TerminalPrefsSpec::default(), so this fixture cannot quietly invent a fresh
// install that production never returns.
const RUST_DEFAULT_DOCUMENT_JSON = String.raw`{
  "explorer": {},
  "skills": {},
  "checks": {},
  "readiness": {},
  "crash": {},
  "locale": "system",
  "theme": "system",
  "ui_zoom_level": 0.0,
  "app_font_family": "Geist",
  "status_bar_items": ["claude", "codex", "antigravity", "kimi", "grok", "opencode-go", "resource-usage", "ports"],
  "status_bar_items_seen": ["claude", "codex", "antigravity", "kimi", "grok", "opencode-go", "resource-usage", "ports"],
  "usage_percentage_display": "used",
  "status_bar_usage_mode": "verbose",
  "usage_analytics_off": [],
  "source_control_view_mode": "list",
  "show_titlebar_app_name": true,
  "show_menu_bar_icon": true,
  "minimize_to_tray_on_close": false,
  "compact_worktree_cards": false,
  "second_brain_weekly_review": false,
  "agent_activity_display": "compact",
  "show_git_ignored_files": true,
  "source_control_group_order": "changes-first",
  "source_control_compare_base": "repository-default",
  "refresh_local_base_ref_on_worktree_create": false,
  "left_sidebar_appearance_mode": "default",
  "left_sidebar_tint_color": "#18181b",
  "left_sidebar_tint_opacity": 0.08,
  "panel_widths": { "sidebar": 280, "aside": 350 },
  "editing_prefs": {
    "editor_auto_save": false,
    "editor_auto_save_delay_ms": 1000,
    "editor_minimap_enabled": false,
    "editor_word_wrap": true,
    "diff_word_wrap": false,
    "rich_markdown_spellcheck_enabled": true,
    "markdown_review_tools_enabled": true,
    "editor_font_family": "",
    "combined_diff_file_tree_visible_by_default": false,
    "primary_selection_middle_click_paste": null,
    "editor_font_zoom": 0
  },
  "update": {
    "policy": "ask",
    "channel": "stable",
    "last_checked": null,
    "skipped_version": null
  },
  "terminal_prefs": {
    "font_size": 14,
    "font_family": "",
    "ligatures": "auto",
    "mac_option_as_alt": "auto",
    "jis_yen_to_backslash": false,
    "allow_osc52_clipboard": true,
    "windows_shell": "powershell.exe",
    "windows_powershell_implementation": "auto",
    "theme_dark": "Ghostty Default Style Dark",
    "use_separate_light_theme": true,
    "theme_light": "Builtin Tango Light",
    "custom_themes": [],
    "color_overrides": {},
    "leading": 1.15,
    "weight": 500,
    "cursor_blink": true,
    "cursor_style": "block",
    "cursor_opacity": 1.0,
    "padding_x": 4,
    "padding_y": 4,
    "sensitivity": 1.15,
    "scrollback": 5000,
    "word_separators": " ()[]{}',\"\u0060",
    "fast_scroll_sensitivity": 5.0,
    "tui_scroll_sensitivity": 1,
    "focus_follows_mouse": false,
    "hide_mouse_while_typing": false,
    "copy_on_select": false,
    "right_click_paste": false,
    "inactive_pane_opacity": 0.9,
    "divider_color_dark": "#3f3f46",
    "divider_color_light": "#d4d4d8",
    "divider_thickness_px": 3
  },
  "terminal_command": "",
  "setup_script_launch_mode": "new-tab",
  "terminal_shortcut_policy": "orca-first",
  "hidden_shortcuts": [],
  "keybindings": {},
  "hidden_task_sources": [],
  "default_task_source": "jira",
  "hide_automation_workspaces": false,
  "hide_default_branch_workspaces": false,
  "hide_detached_head_workspaces": false,
  "hide_sleeping_workspaces": false,
  "keep_default_branch_awake": true,
  "hide_agent_scratch_workspaces": false,
  "worktree_card_properties": ["branch", "ports", "agents", "default-badge"],
  "workspace_board": {
    "statuses": [
      { "id": "todo", "label": "Todo", "color": "neutral", "icon": "circle" },
      { "id": "in-progress", "label": "In progress", "color": "conductor-progress", "icon": "conductor-progress" },
      { "id": "in-review", "label": "In review", "color": "conductor-review", "icon": "conductor-review" },
      { "id": "completed", "label": "Done", "color": "conductor-done", "icon": "conductor-done" }
    ],
    "column_width": 308
  },
  "sidebar_view": { "group_by": "repo", "sort_by": "default", "project_order": "default" },
  "confirm_close_pinned": true,
  "skip_close_terminal_with_running_process_confirm": false,
  "ctrl_tab_order_mode": "mru",
  "workspace_creation_prefs": {
    "directory": "~/zerocode/workspaces",
    "nest_workspaces": true
  },
  "floating_workspace": {
    "enabled": true,
    "cwd": "~",
    "trigger_location": "floating-button"
  },
  "open_in_applications": [
    { "id": "vscode", "label": "VS Code", "command": "code" }
  ],
  "skip_delete_worktree_confirm": false,
  "skip_delete_automation_confirm": false,
  "vault.sessionLimit": 200,
  "artifacts_retention_days": 0,
  "diff_side_by_side": true,
  "conversation_focus_view": false,
  "window_material": { "terminal_opacity": 1.0, "blur": false },
  "default_agent": { "kind": "auto" },
  "agent_teams_mode": "panes",
  "worktree_prefs": { "branch_prefix": "git-username" },
  "notifications": { "enabled": true, "agent_attention": true, "agent_completion": true },
  "computer_awake_mode": "off",
  "computer_confirm_payment": true,
  "computer_confirm_transfer": true,
  "computer_confirm_delete": true,
  "emulator.keepBooted": true,
  "emulator.prebootLastUsed": true,
  "emulator.idleShutdownMinutes": 30,
  "browser": {
    "home_page": "",
    "search_engine": "google",
    "restore_tabs": false,
    "open_links_in_app": false,
    "open_links_in_app_modifier_inverts": false,
    "open_links_in_app_prompted": false,
    "terminal_link_action_popover": true,
    "default_zoom_level": 0.0,
    "open_tabs": [],
    "visits": [],
    "user_agents": []
  },
  "opencode_cookie_configured": false
}`;
const RUST_DEFAULT_DOCUMENT = Object.freeze(JSON.parse(RUST_DEFAULT_DOCUMENT_JSON));

const RUST_UI_ZOOM_SPEC_JSON = String.raw`{
  "min_level": -3.0,
  "max_level": 5.0,
  "step": 0.5,
  "default_level": 0.0,
  "scale_base": 1.2
}`;
const UI_ZOOM_SPEC = Object.freeze(JSON.parse(RUST_UI_ZOOM_SPEC_JSON));

const RUST_BROWSER_ZOOM_SPEC_JSON = String.raw`{
  "min_level": -3.0,
  "max_level": 5.0,
  "step": 0.5,
  "default_level": 0.0,
  "scale_base": 1.2
}`;
const BROWSER_ZOOM_SPEC = Object.freeze(JSON.parse(RUST_BROWSER_ZOOM_SPEC_JSON));

const RUST_LEFT_SIDEBAR_APPEARANCE_SPEC_JSON = String.raw`{
  "tint_color_default": "#18181b",
  "tint_opacity": { "min": 0.0, "max": 0.35, "step": 0.01, "default": 0.08 }
}`;
const LEFT_SIDEBAR_APPEARANCE_SPEC = Object.freeze(
  JSON.parse(RUST_LEFT_SIDEBAR_APPEARANCE_SPEC_JSON),
);

const RUST_EDITING_PREFS_SPEC_JSON = String.raw`{
  "auto_save_delay_ms": { "min": 250, "max": 10000, "step": 250 },
  "editor_font_zoom": {
    "min_level": -6, "max_level": 18, "step": 1, "default_level": 0,
    "px_min": 8.0, "px_max": 32.0
  }
}`;
const EDITING_PREFS_SPEC = Object.freeze(JSON.parse(RUST_EDITING_PREFS_SPEC_JSON));

// The update clock and the pickers' words (t-3191), as `SettingsSnapshot.update_spec`
// carries them: the window arms one boot timer from these numbers and offers
// exactly these policies and channels — never a copy of its own.
const RUST_UPDATE_SPEC_JSON = String.raw`{"check_after_boot_secs":60,"check_every_secs":21600,"history_ttl_secs":21600,"policies":["ask","auto","off"],"channels":["stable","beta"]}`;
const UPDATE_SPEC = Object.freeze(JSON.parse(RUST_UPDATE_SPEC_JSON));

const RUST_OPEN_IN_APPLICATIONS_SPEC_JSON = String.raw`{
  "max": 8,
  "presets": [
    { "id": "vscode", "label": "VS Code", "command": "code" },
    { "id": "cursor", "label": "Cursor", "command": "cursor" },
    { "id": "zed", "label": "Zed", "command": "zed" }
  ]
}`;
const OPEN_IN_APPLICATIONS_SPEC = Object.freeze(
  JSON.parse(RUST_OPEN_IN_APPLICATIONS_SPEC_JSON),
);

const RUST_TERMINAL_PREFS_SPEC_JSON = String.raw`{
  "font_size": { "min": 10, "max": 24, "step": 1 },
  "font_family_defaults": {
    "macos": "SF Mono",
    "windows": "Cascadia Mono",
    "linux": "DejaVu Sans Mono"
  },
  "ligature_modes": ["auto", "on", "off"],
  "mac_option_as_alt_modes": ["auto", "true", "left", "right", "false"],
  "windows_shells": ["powershell.exe", "cmd.exe", "git-bash"],
  "windows_powershell_implementations": ["auto", "powershell.exe", "pwsh.exe"],
  "ligature_font_tokens": [
    "fira code", "fira mono", "jetbrains mono", "jetbrainsmono",
    "cascadia code", "cascadia mono", "iosevka", "victor mono",
    "hasklig", "monoid", "operator mono", "dank mono", "mononoki",
    "pragmatapro", "recursive", "monolisa", "commit mono", "geist mono",
    "maple mono", "departure mono"
  ],
  "theme_defaults": {
    "dark": "Ghostty Default Style Dark",
    "light": "Builtin Tango Light"
  },
  "color_override_groups": [
    {
      "id": "base",
      "keys": [
        "foreground", "background", "cursor", "cursorAccent",
        "selectionBackground", "selectionForeground", "bold"
      ]
    },
    {
      "id": "normal",
      "keys": ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"]
    },
    {
      "id": "bright",
      "keys": [
        "brightBlack", "brightRed", "brightGreen", "brightYellow",
        "brightBlue", "brightMagenta", "brightCyan", "brightWhite"
      ]
    }
  ],
  "leading": { "min": 1.0, "max": 3.0, "step": 0.1 },
  "leading_base": 1.15,
  "weight": { "min": 100, "max": 900, "step": 100 },
  "bold_weight_floor": 700,
  "bold_weight_offset": 200,
  "cursor_styles": ["block", "bar", "underline"],
  "cursor_opacity": { "min": 0.0, "max": 1.0, "step": 0.05 },
  "padding_x": { "min": 0, "max": 512, "step": 1 },
  "padding_y": { "min": 0, "max": 512, "step": 1 },
  "sensitivity": { "min": 0.5, "max": 3.0, "step": 0.05 },
  "fast_scroll_sensitivity": { "min": 1.0, "max": 10.0, "step": 0.5 },
  "tui_scroll_sensitivity": { "min": 1, "max": 10, "step": 1 },
  "scrollback": { "min": 1000, "max": 50000, "step": 100 },
  "scrollback_presets": [5000, 10000, 25000, 50000],
  "word_separators_max_chars": 128,
  "inactive_pane_opacity": { "min": 0.0, "max": 1.0, "step": 0.05 },
  "divider_color_defaults": { "dark": "#3f3f46", "light": "#d4d4d8" },
  "divider_thickness_px": { "min": 1, "max": 32, "step": 1 },
  "divider_hit_padding_px": 6,
  "max_custom_terminal_themes": 200,
  "custom_theme_selection_prefix": "custom:"
}`;
const TERMINAL_PREFS_SPEC = Object.freeze({
  ...JSON.parse(RUST_TERMINAL_PREFS_SPEC_JSON),
  terminal_themes: TERMINAL_THEME_CATALOG,
});

const RUST_CRASH_LIMITS_JSON = String.raw`{"watchdog":true,"first_ms":2000,"second_ms":6000,"ping_ms":250,"warmup_ms":15000,"threshold_max_ms":120000}`;
const CRASH_LIMITS = Object.freeze(JSON.parse(RUST_CRASH_LIMITS_JSON));

const DEFAULT_SETTINGS = Object.freeze({
  schema_version: 1,
  revision: 0,
  settings_health: "healthy",
  settings_error: null,
  ...RUST_DEFAULT_DOCUMENT,
  crash_limits: CRASH_LIMITS,
  vault_limits: { choices: [50, 100, 200], absolute_max: 1000, default: 200, children_per_parent: 50 },
  editing_prefs_spec: EDITING_PREFS_SPEC,
  update_spec: UPDATE_SPEC,
  // The artifact table's cap, as `SettingsSnapshot.artifacts_retention_spec`
  // carries it (t-2720): the field's bounds come from here, never from the UI.
  artifacts_retention_spec: { min: 0, max: 365, step: 1 },
  terminal_prefs_spec: TERMINAL_PREFS_SPEC,
  left_sidebar_appearance_spec: LEFT_SIDEBAR_APPEARANCE_SPEC,
  ui_zoom_spec: UI_ZOOM_SPEC,
  explorer_policy: JSON.parse(await readFile(resolve(UI, "..", "crates/zerocode-shell/src/explorer_policy.json"), "utf8")),
  open_in_applications_spec: OPEN_IN_APPLICATIONS_SPEC,
  browser_zoom_spec: BROWSER_ZOOM_SPEC,
  terminal_opacity: RUST_DEFAULT_DOCUMENT.window_material.terminal_opacity,
  window_blur: RUST_DEFAULT_DOCUMENT.window_material.blur,
});

const BOOT_BASE = Object.freeze({
  project_root: "/tmp/zerocode-settings-test",
  active_root: "/tmp/zerocode-settings-test",
  project: "zerocode-settings-test",
  branch: "main",
  hidden_shortcuts: [],
  keybindings: {
    "tab.close": [],
    "view.toggleAside": [],
    "terminal.newTab": [],
    "terminal.toggle": [],
  },
  panel_widths: { sidebar: 280, aside: 350 },
  hide_automation_workspaces: false,
  confirm_close_pinned: true,
  skip_close_terminal_with_running_process_confirm: false,
  ctrl_tab_order_mode: "mru",
  workspace_creation_prefs: {
    directory: "~/zerocode/workspaces",
    nest_workspaces: true,
  },
  skip_delete_worktree_confirm: false,
  skip_delete_automation_confirm: false,
  diff_side_by_side: true,
  zo: "/usr/local/bin/zo",
  lanes: [],
  worktrees: [],
  recent_projects: [],
  serve: "down",
  system_locale: "ko",
  window_blur_active: false,
  onboarding: {
    flow_version: 1,
    closed_at: 1,
    outcome: "completed",
    last_completed_step: 4,
    checklist: { chose_agent: true, dismissed: false },
  },
});

const AGENTS = Object.freeze([
  {
    id: "claude", name: "Claude", favicon_domain: "claude.ai",
    homepage_url: "https://docs.anthropic.com/claude/docs/claude-code", installed: true,
    found_as: "claude", unsupported_here: false, missing_requirement: null,
    takes_a_paste: false, ready: "quiet", glyph: "✻", busy_word: "Pondering…", models_provider: "claude", model_command: "/model", model_command_takes_id: true, permission_road: "shift-tab",
  },
  {
    id: "codex", name: "Codex", favicon_domain: "openai.com",
    homepage_url: "https://github.com/openai/codex", installed: true,
    found_as: "codex", unsupported_here: false, missing_requirement: null,
    takes_a_paste: true, ready: "composer-prompt", glyph: "◎", busy_word: "Thinking…", models_provider: "openai", model_command: "/model", model_command_takes_id: false, permission_road: "/permissions",
  },
]);

const DEFAULT_AGENT_LAUNCH_PLANS = Object.freeze([
  {
    agent: "claude", args: "--dangerously-skip-permissions", env: [],
    permission: "unattended", has_switch: true, is_default: true,
  },
  {
    agent: "codex", args: "--dangerously-bypass-approvals-and-sandbox", env: [],
    permission: "unattended", has_switch: true, is_default: true,
  },
  {
    agent: "goose", args: "", env: [["GOOSE_MODE", "auto"]],
    permission: "unattended", has_switch: true, is_default: true,
  },
]);

const PROJECT_CATALOG = Object.freeze([
  {
    name: "zerocode-settings-test",
    path: "/tmp/zerocode-settings-test",
    worktrees: [
      {
        path: "/tmp/zerocode-settings-test",
        branch: "main",
        is_main: true,
        active: true,
        is_folder: false,
        ownership: "zerocode-managed",
        external_hidden: false,
      },
    ],
    external: {
      visibility: "show", legacy: true, authoritative: true, shown: 0,
      hidden: [], prompt: false, inbox: [],
    },
  },
]);

const JIRA_SITES = Object.freeze([
  {
    id: "jira-a",
    site_url: "https://alpha.atlassian.net",
    email: "hana@alpha.example",
    display_name: "Hana",
    account_id: "acct-a",
    auth_type: "cloud",
    credential_error: null,
  },
  {
    id: "jira-b",
    site_url: "https://beta.atlassian.net",
    email: "mira@beta.example",
    display_name: "Mira",
    account_id: "acct-b",
    auth_type: "cloud",
    credential_error: null,
  },
]);

const GITHUB_ACCOUNTS = Object.freeze([
  {
    id: "gh-a", host: "github.com", login: "hana", active: true,
    standing: "connected", selectable: true, disconnectable: true,
    credential_source: "gh",
  },
  {
    id: "gh-b", host: "github.com", login: "mira", active: false,
    standing: "connected", selectable: true, disconnectable: true,
    credential_source: "gh",
  },
  {
    id: "gh-env", host: "enterprise.example", login: "ci", active: true,
    standing: "connected", selectable: false, disconnectable: false,
    credential_source: "environment",
  },
]);

/* What the native side answers once it actually asks macOS (t-5587). Five of
 * the nine are readable facts, two are answers nobody gave yet, and two — the
 * microphone and automation — are the rows a dialog can still be raised for.
 * Camera is granted, so its prompt is spent and the row opens the pane. */
const DEVELOPER_PERMISSION_STATES = Object.freeze([
  { id: "microphone", status: "ready", action: "trigger-prompt" },
  { id: "camera", status: "granted", action: "open-settings" },
  { id: "screen", status: "granted", action: "open-settings" },
  { id: "accessibility", status: "denied", action: "open-settings" },
  { id: "full-disk-access", status: "denied", action: "open-settings" },
  { id: "automation", status: "ready", action: "trigger-prompt" },
  { id: "local-network", status: "unknown", action: "trigger-prompt" },
  { id: "usb", status: "ready", action: "open-settings" },
  { id: "bluetooth", status: "denied", action: "open-settings" },
]);

/* What `glab` on this machine says. Three facts and no lifecycle: the GitLab
 * card reads a standing, and `glab auth` offers nothing for it to mutate. */
function gitlabFixture() {
  return { installed: true, authenticated: true, hosts: ["gitlab.com"] };
}

function githubFixture() {
  return {
    availability: "available",
    connected: true,
    credential_protection: "external_cli",
    repo_host: "github.com",
    repo_standing: "connected",
    active_account_id: "gh-a",
    accounts: clone(GITHUB_ACCOUNTS),
  };
}

/* `scm_tree_rows`의 픽스처판.
 *
 * 디렉터리 먼저·이름순, 파일은 들어온 순서 그대로. 자식이 디렉터리 하나뿐인
 * 사슬은 한 행으로 접힌다 — 러스트 쪽과 같은 계약이다. */
function scmTreeRows(area, paths, folded) {
  const roots = [];
  paths.forEach((path, at) => {
    const parts = path.split("/").filter((part) => part && part !== ".");
    if (!parts.length) return;
    let children = roots;
    let prefix = "";
    parts.forEach((part, depth) => {
      prefix = prefix ? `${prefix}/${part}` : part;
      if (depth === parts.length - 1) {
        children.push({ dir: false, name: part, path, at });
        return;
      }
      let held = children.find((child) => child.dir && child.name === part);
      if (!held) {
        held = { dir: true, name: part, path: prefix, children: [] };
        children.push(held);
      }
      children = held.children;
    });
  });
  const arrange = (children) => {
    children.sort((left, right) => {
      if (left.dir && right.dir) return left.name < right.name ? -1 : left.name > right.name ? 1 : 0;
      if (left.dir !== right.dir) return left.dir ? -1 : 1;
      return left.at - right.at;
    });
    children.filter((child) => child.dir).forEach((child) => arrange(child.children));
  };
  arrange(roots);
  const compact = (node) => {
    if (!node.dir) return node;
    let { name, path, children } = node;
    while (children.length === 1 && children[0].dir) {
      const only = children[0];
      name = `${name}/${only.name}`;
      path = only.path;
      children = only.children;
    }
    return { dir: true, name, path, children: children.map(compact) };
  };
  const under = (node) => (node.dir ? node.children.flatMap(under) : [node.path]);
  const rows = [];
  const walk = (node, depth) => {
    if (!node.dir) {
      rows.push({ type: "file", key: `${area}::${node.path}`, name: node.name, path: node.path, depth, at: node.at });
      return;
    }
    const held = under(node);
    const key = `dir::${area}::${node.path}`;
    rows.push({ type: "directory", key, name: node.name, path: node.path, depth, file_count: held.length, paths: held });
    if (folded.has(key)) return;
    node.children.forEach((child) => walk(child, depth + 1));
  };
  roots.map(compact).forEach((root) => walk(root, 0));
  return rows;
}

const SETTINGS_MUTATION_COMMANDS = new Set([
  "set_crash_watchdog", "set_theme", "set_locale", "set_ui_zoom", "set_app_font_family", "set_show_titlebar_app_name", "set_show_menu_bar_icon", "set_minimize_to_tray_on_close", "set_compact_worktree_cards", "set_show_git_ignored_files", "set_source_control_group_order", "set_source_control_compare_base", "set_refresh_local_base_ref_on_worktree_create", "patch_left_sidebar_appearance", "set_status_bar_item", "set_usage_percentage_display", "set_status_bar_usage_mode", "set_usage_analytics_enabled", "set_source_control_view_mode", "set_terminal_command", "set_setup_script_launch_mode", "set_terminal_shortcut_policy", "set_terminal_prefs",
  "patch_terminal_prefs", "patch_editing_prefs", "patch_update_prefs", "apply_ghostty_import", "set_panel_width",
  "set_notification_preference", "set_browser_home_page", "set_browser_search_engine",
  "patch_browser_link_routing", "patch_browser_user_agents", "set_browser_restore_tabs", "set_browser_default_zoom", "set_browser_open_tabs",
  "set_terminal_opacity",
  "set_window_blur", "set_agent_teams_mode", "set_default_agent",
  "set_shortcut_visibility", "set_task_source_visibility", "set_keybinding",
  "set_diff_side_by_side", "set_conversation_focus_view", "set_confirm_close_pinned",
  "set_skip_close_terminal_with_running_process_confirm", "set_ctrl_tab_order_mode",
  "patch_workspace_creation_prefs",
  "patch_floating_workspace",
  "patch_open_in_applications",
  "set_skip_delete_worktree_confirm", "set_skip_delete_automation_confirm",
  "set_vault_session_limit",
  "set_artifacts_retention_days",
  "set_second_brain_weekly_review",
  "set_second_brain_explore",
  "save_worktree_prefs",
]);

class StatefulBackend {
  constructor() {
    this.settings = clone(DEFAULT_SETTINGS);
    this.calls = [];
    this.unknown = [];
    this.deliveries = [];
    this.pages = new Map();
    this.nextTerm = 100;
    this.terminalSessions = [];
    this.routerPresets = [...ROUTER_PRESETS_CATALOG];
    this.zoSettings = { providers: [] };
    this.keychain = new Map();
    // The CLI login card's rows — a test fills them; the pane paints what
    // the backend's table says and nothing of its own.
    this.cliLogins = { rows: [] };
    // A machine with no keychain for router keys (every build but macOS):
    // `save_router` refuses a key there instead of dropping it on the floor.
    this.routerKeychainUnavailable = false;
    // What `zo decision-shadow check --json` answers next — a test swaps it.
    this.typesafeCheck = { answered: true, model: "jev-1.13.0", elapsedMs: 612 };
    // What `zo jev summary --json` answers next. `null` is a zo too old to
    // know the verb: the switches must still stand, without numbers.
    this.jevSummary = null;
    // 어느 에이전트의 전역 지시문 쓰기가 실패하는가 — 시험이 끼워 넣는다.
    this.secondBrainLinkFailure = null;
    this.secondBrain = {
      saved_path: "",
      vaults: [
        { name: "Knowledge", path: "/Users/fixture/Knowledge", open: true },
      ],
      vault: null,
      obsidian_installed: false,
      created: [],
      quick_commands_added: 0,
      skill_installs: [],
      // 볼트 밖 판이 볼트를 아는가 — 세팅 전에는 어느 에이전트도 모른다.
      guides: [
        { agent: "claude", label: "Claude", path: "/Users/fixture/.claude/CLAUDE.md",
          state: "missing", detail: "" },
        { agent: "codex", label: "Codex", path: "/Users/fixture/.codex/AGENTS.md",
          state: "missing", detail: "" },
      ],
      // 주간 리뷰 cron — 스위치는 설정, 영수증은 볼트의 zo 등록부의 답.
      weekly_review_enabled: false,
      weekly_review: null,
    };
    this.waiters = [];
    this.rejectNext = new Set();
    this.sshLinks = {};
    // 브라우저 프로필 하나가 곧 쿠키 단지 하나다. 시험이 사이에 넣고 뺄 수
    // 있도록 상태로 든다 — 설정 페이지는 올 때마다 이것을 다시 읽는다.
    this.browserProfiles = [];
    // 새 탭이 입는 기본 프로필의 id — 설정 카드가 "활성" 배지로 비춘다.
    this.defaultProfileId = null;
    this.deferredRejectNext = new Map();
    this.deferredLoseAckNext = new Map();
    this.loseAckNext = new Set();
    this.canonicalNext = new Map();
    this.answerNext = new Map();
    this.holds = new Map();
    this.beforeHolds = new Map();
    this.snapshotHolds = new Map();
    this.droppedDeliveries = new Set();
    /* The scheduled jobs and their runs, empty until a test sets them — the
     * Automations page reads the catalog, the selected job's runs, and one
     * run's evidence folder through three separate commands. */
    this.automations = [];
    this.automationRuns = [];
    this.runEvidence = { dir: "", files: [], more: 0 };
    /* The Flow roster (t-4260), empty until a test sets it. `policies` and
     * `levels` are the two closed sets the backend draws from the core enums
     * (`Policy::ALL`, `EvidenceLevel::ALL`) — `guarded` carries the answer of
     * `Policy::runnable`, so the card can say it cannot run yet. */
    this.flows = {
      flows: [],
      policies: [
        { value: "dry", runnable: { ok: true } },
        { value: "guarded", runnable: { ok: false, refusal: "flow_policy_unbuilt" } },
      ],
      levels: ["full", "verdict-only", "off"],
    };
    this.quickCommands = [
      {
        id: "qc-global",
        label: "Run checks",
        workspace: null,
        body: "just verify",
        agent: null,
        append_enter: true,
      },
      {
        id: "qc-project",
        label: "Review",
        workspace: BOOT_BASE.project_root,
        body: "Review the current diff",
        agent: "codex",
        append_enter: true,
      },
    ];
    this.jira = {
      connected: true,
      sites: clone(JIRA_SITES),
      active_site_id: "jira-a",
      selected_site_id: "all",
      credential_error: null,
      credential_protection: "native",
    };
    this.github = githubFixture();
    this.gitlab = gitlabFixture();
    this.sshHosts = [];
    this.nextSshHost = 1;
    this.sshTargets = [];
    this.nextSshTarget = 1;
    /* What `~/.ssh/config` says, and which of its aliases somebody deleted.
     * The real suppression list lives in the settings document and the Rust
     * tests own its rules; this is enough of it for the window's half — that
     * the silent pass respects a deletion and the button forgives it. */
    this.sshConfigHosts = [];
    this.sshSuppressed = [];
    this.remoteWorkspaces = [];
    this.nextRemoteWorkspace = 1;
    this.remoteServers = [];
    this.nextRemoteServer = 1;
    this.nextRemoteTerm = 900;
    this.keyboardLayout = { category: "us" };
    this.pwshAvailable = true;
    this.gitBashAvailable = true;
    this.agentLaunchPlans = clone(DEFAULT_AGENT_LAUNCH_PLANS);
    this.computerUsePermissions = [
      { id: "accessibility", status: "not-granted" },
      { id: "screenshots", status: "not-granted" },
    ];
    this.computerUseSkill = {
      installed: false,
      install_command: "npx skills add https://github.com/cjy5507/zerocode-ide --skill computer-use --global",
      update_command: "npx skills update computer-use --global",
      agents: [{ agent: "codex", label: "Codex", installed: false }],
    };
    this.ghosttyPreview = {
      found: false,
      configPaths: [],
      patch: {},
      changes: [],
      unsupportedKeys: [],
    };
    // The Google login this window makes itself: one row on the provider
    // accounts pane, and a store path the row shows.
    this.googleAccount = {
      signed_in: false,
      expired: false,
      renewable: false,
      scoped_for_agents: true,
      store: "/home/tester/.zo/credentials.json",
    };
    this.googleConsentUrl = "https://accounts.google.com/o/oauth2/v2/auth?state=fixture";
  }

  snapshot() {
    const limits = { ...CRASH_LIMITS, ...this.settings.crash };
    limits.first_ms = Math.max(limits.ping_ms, Math.min(limits.threshold_max_ms - limits.ping_ms, limits.first_ms));
    limits.second_ms = Math.max(limits.first_ms + limits.ping_ms, Math.min(limits.threshold_max_ms, limits.second_ms));
    return clone({ ...this.settings, crash_limits: limits });
  }

  bootReport() {
    return {
      ...clone(BOOT_BASE),
      ...this.snapshot(),
      window_blur_active: this.settings.window_blur,
    };
  }

  attach(id, page) {
    this.pages.set(id, page);
  }

  count(id, command) {
    return this.calls.filter((call) => call.window_id === id && call.command === command).length;
  }

  last(id, command) {
    return this.calls.findLast((call) => call.window_id === id && call.command === command) ?? null;
  }

  record(windowId, command, args) {
    const call = {
      at: this.calls.length,
      window_id: windowId,
      command,
      args: clone(args ?? {}),
    };
    this.calls.push(call);
    const waiting = this.waiters.filter(
      (one) => one.window_id === windowId && one.command === command && call.at >= one.after,
    );
    this.waiters = this.waiters.filter((one) => !waiting.includes(one));
    for (const one of waiting) {
      clearTimeout(one.timer);
      one.resolve(call);
    }
    return call;
  }

  waitForCall(windowId, command, after = 0, timeout = UI_TIMEOUT) {
    const held = this.calls.find(
      (call) => call.window_id === windowId && call.command === command && call.at >= after,
    );
    if (held) return Promise.resolve(held);
    return new Promise((resolveCall, rejectCall) => {
      const waiter = { window_id: windowId, command, after, resolve: resolveCall, timer: null };
      waiter.timer = setTimeout(() => {
        this.waiters = this.waiters.filter((one) => one !== waiter);
        rejectCall(new Error(`${windowId} did not invoke ${command}`));
      }, timeout);
      this.waiters.push(waiter);
    });
  }

  rejectOnce(command) {
    this.rejectNext.add(command);
  }

  deferRejectionOnce(command, message = `deferred refusal: ${command}`) {
    if (this.deferredRejectNext.has(command)) {
      throw new Error(`${command} already has a deferred rejection`);
    }
    let release;
    const waiting = new Promise((done) => (release = done));
    this.deferredRejectNext.set(command, { waiting, message });
    return () => release();
  }

  loseAcknowledgementOnce(command) {
    this.loseAckNext.add(command);
  }

  deferLostAcknowledgementOnce(command) {
    if (this.deferredLoseAckNext.has(command)) {
      throw new Error(`${command} already has a deferred lost acknowledgement`);
    }
    let release;
    let arrive;
    const waiting = new Promise((done) => (release = done));
    const reached = new Promise((done) => (arrive = done));
    this.deferredLoseAckNext.set(command, { waiting, arrive });
    return { release, reached };
  }

  canonicalizeOnce(command, patch) {
    this.canonicalNext.set(command, clone(patch));
  }

  answerOnce(command, patch) {
    this.answerNext.set(command, clone(patch));
  }

  holdNext(command) {
    if (this.holds.has(command)) throw new Error(`${command} is already held`);
    let release;
    let arrive;
    const promise = new Promise((done) => (release = done));
    const reached = new Promise((done) => (arrive = done));
    this.holds.set(command, { promise, release, arrive });
    return reached;
  }

  holdBeforeNext(command) {
    if (this.beforeHolds.has(command)) {
      throw new Error(`${command} already has a held request entrance`);
    }
    let release;
    let arrive;
    const waiting = new Promise((done) => (release = done));
    const reached = new Promise((done) => (arrive = done));
    this.beforeHolds.set(command, { waiting, arrive });
    return { release, reached };
  }

  holdNextSnapshot(windowId) {
    if (this.snapshotHolds.has(windowId)) {
      throw new Error(`${windowId} already has a held settings snapshot`);
    }
    let release;
    const promise = new Promise((done) => (release = done));
    this.snapshotHolds.set(windowId, { promise });
    return release;
  }

  release(command) {
    const held = this.holds.get(command);
    if (!held) throw new Error(`${command} is not held`);
    this.holds.delete(command);
    held.release();
  }

  /* One connection state moves, and every window hears the whole report —
   * the same story the Rust side tells over `ssh:link-changed`. */
  async moveSshLink(id, move) {
    const report = { id, state: move.state, ...(move.error ? { error: move.error } : {}) };
    this.sshLinks[id] = report;
    for (const [, page] of this.pages) {
      if (page.isClosed() || page.url() === "about:blank") continue;
      await page.evaluate((detail) => window.__SETTINGS_TEST_EMIT__("ssh:link-changed", detail), report);
    }
  }

  async broadcast(origin, keys) {
    const payload = { revision: this.settings.revision, keys: [...keys] };
    for (const [windowId, page] of this.pages) {
      if (windowId === origin || page.isClosed() || page.url() === "about:blank") continue;
      if (this.droppedDeliveries.delete(windowId)) continue;
      this.deliveries.push({ window_id: windowId, payload: clone(payload) });
      await page.evaluate((detail) => window.__SETTINGS_TEST_EMIT__("settings:changed", detail), payload);
    }
  }

  dropNextDeliveryFor(windowId) {
    this.droppedDeliveries.add(windowId);
  }

  async externalPatch(patch, keys = Object.keys(patch)) {
    Object.assign(this.settings, clone(patch));
    this.settings.revision += 1;
    await this.broadcast(null, keys);
    return this.snapshot();
  }

  async mutate(windowId, command, args) {
    const deferred = this.deferredRejectNext.get(command);
    if (deferred) {
      // Consume it before waiting: the next same-command mutation is the
      // newer request and must be free to complete first.
      this.deferredRejectNext.delete(command);
      await deferred.waiting;
      throw new Error(deferred.message);
    }
    if (this.rejectNext.delete(command)) throw new Error(`refused: ${command}`);

    let keys;
    switch (command) {
      case "set_crash_watchdog":
        this.settings.crash = { ...this.settings.crash, ...args.patch };
        keys = ["crash"];
        break;
      case "set_theme":
        this.settings.theme = args.code;
        keys = ["theme"];
        break;
      case "set_locale":
        this.settings.locale = args.code;
        keys = ["locale"];
        break;
      case "set_ui_zoom": {
        const spec = this.settings.ui_zoom_spec;
        const rounded = Math.round(Number(args.zoomLevel) / spec.step) * spec.step;
        this.settings.ui_zoom_level = Math.min(
          spec.max_level,
          Math.max(spec.min_level, rounded),
        );
        keys = ["ui_zoom_level"];
        break;
      }
      case "set_status_bar_item": {
        const available = ["claude", "codex", "antigravity", "kimi", "grok", "opencode-go", "resource-usage", "ports"];
        if (!available.includes(args.item)) throw new Error(`unknown status-bar item: ${args.item}`);
        const enabled = new Set(this.settings.status_bar_items);
        if (args.enabled === true) enabled.add(args.item);
        else enabled.delete(args.item);
        this.settings.status_bar_items = available.filter((item) => enabled.has(item));
        keys = ["status_bar_items"];
        break;
      }
      case "set_usage_percentage_display":
        if (!["used", "remaining"].includes(args.display)) {
          throw new Error(`unknown usage percentage display: ${args.display}`);
        }
        this.settings.usage_percentage_display = args.display;
        keys = ["usage_percentage_display"];
        break;
      case "set_usage_analytics_enabled":
        if (!["claude", "codex"].includes(args.provider)) {
          throw new Error(`unknown usage analytics provider: ${args.provider}`);
        }
        this.settings.usage_analytics_off = this.settings.usage_analytics_off
          .filter((one) => one !== args.provider);
        if (!args.enabled) this.settings.usage_analytics_off.push(args.provider);
        keys = ["usage_analytics_off"];
        break;
      case "set_source_control_view_mode":
        if (!["list", "tree"].includes(args.mode)) {
          throw new Error(`unknown source control view mode: ${args.mode}`);
        }
        this.settings.source_control_view_mode = args.mode;
        keys = ["source_control_view_mode"];
        break;
      case "set_status_bar_usage_mode":
        if (!["verbose", "compact"].includes(args.mode)) {
          throw new Error(`unknown status-bar usage mode: ${args.mode}`);
        }
        this.settings.status_bar_usage_mode = args.mode;
        keys = ["status_bar_usage_mode"];
        break;
      case "set_show_titlebar_app_name":
        this.settings.show_titlebar_app_name = args.visible !== false;
        keys = ["show_titlebar_app_name"];
        break;
      case "set_show_menu_bar_icon":
        this.settings.show_menu_bar_icon = args.visible !== false;
        keys = ["show_menu_bar_icon"];
        break;
      case "set_minimize_to_tray_on_close":
        this.settings.minimize_to_tray_on_close = args.enabled === true;
        keys = ["minimize_to_tray_on_close"];
        break;
      case "set_app_font_family":
        this.settings.app_font_family = args.family.trim() || "Geist";
        keys = ["app_font_family"];
        break;
      case "set_compact_worktree_cards":
        this.settings.compact_worktree_cards = args.compact === true;
        keys = ["compact_worktree_cards"];
        break;
      case "set_show_git_ignored_files":
        this.settings.show_git_ignored_files = args.visible !== false;
        keys = ["show_git_ignored_files"];
        break;
      case "set_source_control_group_order":
        if (!["changes-first", "staged-first", "untracked-first"].includes(args.order)) {
          throw new Error("invalid source-control group order");
        }
        this.settings.source_control_group_order = args.order;
        keys = ["source_control_group_order"];
        break;
      case "set_source_control_compare_base":
        if (!["repository-default", "branch-upstream"].includes(args.mode)) {
          throw new Error("invalid source-control compare base");
        }
        this.settings.source_control_compare_base = args.mode;
        keys = ["source_control_compare_base"];
        break;
      case "set_refresh_local_base_ref_on_worktree_create":
        this.settings.refresh_local_base_ref_on_worktree_create = args.enabled === true;
        keys = ["refresh_local_base_ref_on_worktree_create"];
        break;
      case "patch_left_sidebar_appearance": {
        const field = {
          mode: "left_sidebar_appearance_mode",
          tint_color: "left_sidebar_tint_color",
          tint_opacity: "left_sidebar_tint_opacity",
        }[args.patch.kind];
        if (!field) throw new Error(`unknown left-sidebar patch: ${args.patch.kind}`);
        this.settings[field] = clone(args.patch.value);
        keys = [field];
        break;
      }
      case "set_terminal_command":
        this.settings.terminal_command = String(args.command ?? "").trim();
        keys = ["terminal_command"];
        break;
      case "set_setup_script_launch_mode":
        if (!["new-tab", "split-vertical", "split-horizontal"].includes(args.mode)) {
          throw new Error(`unknown setup script launch mode: ${args.mode}`);
        }
        this.settings.setup_script_launch_mode = args.mode;
        keys = ["setup_script_launch_mode"];
        break;
      case "set_terminal_shortcut_policy":
        if (!["orca-first", "terminal-first"].includes(args.policy)) {
          throw new Error(`unknown terminal shortcut policy: ${args.policy}`);
        }
        this.settings.terminal_shortcut_policy = args.policy;
        keys = ["terminal_shortcut_policy"];
        break;
      case "set_terminal_prefs":
        this.settings.terminal_prefs = { ...this.settings.terminal_prefs, ...clone(args.prefs) };
        keys = ["terminal_prefs"];
        break;
      case "apply_ghostty_import": {
        const patch = clone(args.patch ?? {});
        const terminal = { ...this.settings.terminal_prefs };
        const assign = (wire, stored) => {
          if (patch[wire] !== null && patch[wire] !== undefined) terminal[stored] = patch[wire];
        };
        assign("macOptionAsAlt", "mac_option_as_alt");
        assign("dividerColorDark", "divider_color_dark");
        assign("dividerColorLight", "divider_color_light");
        assign("inactivePaneOpacity", "inactive_pane_opacity");
        assign("paddingX", "padding_x");
        assign("paddingY", "padding_y");
        assign("hideMouseWhileTyping", "hide_mouse_while_typing");
        assign("cursorOpacity", "cursor_opacity");
        assign("fontFamily", "font_family");
        assign("fontSize", "font_size");
        assign("fontWeight", "weight");
        assign("cursorStyle", "cursor_style");
        assign("cursorBlink", "cursor_blink");
        assign("focusFollowsMouse", "focus_follows_mouse");
        if (patch.lineHeight !== null && patch.lineHeight !== undefined) {
          terminal.leading = Number((patch.lineHeight
            * this.settings.terminal_prefs_spec.leading_base).toFixed(4));
        }
        terminal.color_overrides = {
          ...terminal.color_overrides,
          ...clone(patch.colorOverrides ?? {}),
        };
        this.settings.terminal_prefs = terminal;
        if (patch.terminalOpacity !== null && patch.terminalOpacity !== undefined) {
          this.settings.terminal_opacity = patch.terminalOpacity;
        }
        if (patch.windowBlur !== null && patch.windowBlur !== undefined) {
          this.settings.window_blur = patch.windowBlur;
        }
        if (
          patch.primarySelectionMiddleClickPaste !== null
          && patch.primarySelectionMiddleClickPaste !== undefined
        ) {
          this.settings.editing_prefs = {
            ...this.settings.editing_prefs,
            primary_selection_middle_click_paste: patch.primarySelectionMiddleClickPaste,
          };
        }
        keys = ["terminal_prefs", "window_material", "editing_prefs"];
        break;
      }
      case "patch_terminal_prefs":
        if (args.patch.kind === "color_override") {
          const overrides = { ...this.settings.terminal_prefs.color_overrides };
          if (args.patch.value.value === null) delete overrides[args.patch.value.key];
          else overrides[args.patch.value.key] = clone(args.patch.value.value);
          this.settings.terminal_prefs = {
            ...this.settings.terminal_prefs,
            color_overrides: overrides,
          };
        } else if (args.patch.kind === "reset_color_overrides") {
          this.settings.terminal_prefs = {
            ...this.settings.terminal_prefs,
            color_overrides: {},
          };
        } else {
          this.settings.terminal_prefs = {
            ...this.settings.terminal_prefs,
            [args.patch.kind]: clone(args.patch.value),
          };
        }
        keys = ["terminal_prefs"];
        break;
      case "patch_editing_prefs":
        let editingValue = clone(args.patch.value);
        if (args.patch.kind === "editor_auto_save_delay_ms") {
          const spec = this.settings.editing_prefs_spec.auto_save_delay_ms;
          editingValue = Math.min(spec.max, Math.max(spec.min, Math.round(editingValue)));
        } else if (args.patch.kind === "editor_font_family") {
          editingValue = String(editingValue ?? "").trim();
        }
        this.settings.editing_prefs = {
          ...this.settings.editing_prefs,
          [args.patch.kind]: editingValue,
        };
        keys = ["editing_prefs"];
        break;
      case "patch_update_prefs":
        // One field of `settings.update` (t-3191): the policy, the channel or
        // the skipped version. The clock has no patch road.
        if (!["policy", "channel", "skipped_version"].includes(args.patch.kind)) {
          throw new Error(`patch_update_prefs refuses ${args.patch.kind}`);
        }
        this.settings.update = {
          ...this.settings.update,
          [args.patch.kind]: clone(args.patch.value),
        };
        keys = ["update"];
        break;
      case "set_diff_side_by_side":
        this.settings.diff_side_by_side = args.on === true;
        keys = ["diff_side_by_side"];
        break;
      case "set_conversation_focus_view":
        this.settings.conversation_focus_view = args.on === true;
        keys = ["conversation_focus_view"];
        break;
      case "set_confirm_close_pinned":
        this.settings.confirm_close_pinned = args.on === true;
        keys = ["confirm_close_pinned"];
        break;
      case "set_skip_close_terminal_with_running_process_confirm":
        this.settings.skip_close_terminal_with_running_process_confirm = args.skip === true;
        keys = ["skip_close_terminal_with_running_process_confirm"];
        break;
      case "set_ctrl_tab_order_mode":
        this.settings.ctrl_tab_order_mode = args.mode === "sequential" ? "sequential" : "mru";
        keys = ["ctrl_tab_order_mode"];
        break;
      case "patch_workspace_creation_prefs":
        if (args.patch.kind === "directory") {
          const directory = String(args.patch.value ?? "").trim();
          if (!directory) throw new Error("workspace directory is required");
          this.settings.workspace_creation_prefs = {
            ...this.settings.workspace_creation_prefs,
            directory,
          };
        } else if (args.patch.kind === "nest_workspaces") {
          this.settings.workspace_creation_prefs = {
            ...this.settings.workspace_creation_prefs,
            nest_workspaces: args.patch.value === true,
          };
        } else {
          throw new Error(`unknown workspace creation patch: ${args.patch.kind}`);
        }
        keys = ["workspace_creation_prefs"];
        break;
      // The Rust contract: `enabled` lands as a bool, an empty or `~` cwd
      // stores the literal default `~`, any other cwd must be a directory
      // (this fixture takes the picker's word for that), and the trigger
      // location is one of the two wire words.
      case "patch_floating_workspace":
        if (args.patch.kind === "enabled") {
          this.settings.floating_workspace = {
            ...this.settings.floating_workspace,
            enabled: args.patch.value === true,
          };
        } else if (args.patch.kind === "cwd") {
          const cwd = String(args.patch.value ?? "").trim();
          this.settings.floating_workspace = {
            ...this.settings.floating_workspace,
            cwd: cwd === "" ? "~" : cwd,
          };
        } else if (args.patch.kind === "trigger_location") {
          if (!["floating-button", "status-bar"].includes(args.patch.value)) {
            throw new Error(`unknown trigger location: ${args.patch.value}`);
          }
          this.settings.floating_workspace = {
            ...this.settings.floating_workspace,
            trigger_location: args.patch.value,
          };
        } else {
          throw new Error(`unknown floating workspace patch: ${args.patch.kind}`);
        }
        keys = ["floating_workspace"];
        break;
      case "patch_open_in_applications": {
        const patch = clone(args.patch);
        if (patch.kind === "upsert") {
          const application = {
            id: String(patch.value?.id ?? "").trim(),
            label: String(patch.value?.label ?? "").trim(),
            command: String(patch.value?.command ?? "").trim(),
          };
          if (!application.id || !application.label || !application.command) {
            throw new Error("open-in application fields are required");
          }
          const at = this.settings.open_in_applications.findIndex(
            (row) => row.id === application.id,
          );
          if (at === -1) {
            if (this.settings.open_in_applications.length >= this.settings.open_in_applications_spec.max) {
              throw new Error("open-in application limit reached");
            }
            this.settings.open_in_applications.push(application);
          } else {
            this.settings.open_in_applications[at] = application;
          }
        } else if (patch.kind === "remove") {
          this.settings.open_in_applications = this.settings.open_in_applications.filter(
            (row) => row.id !== String(patch.value ?? "").trim(),
          );
        } else {
          throw new Error(`unknown open-in application patch: ${patch.kind}`);
        }
        keys = ["open_in_applications"];
        break;
      }
      case "set_skip_delete_worktree_confirm":
        this.settings.skip_delete_worktree_confirm = args.skip === true;
        keys = ["skip_delete_worktree_confirm"];
        break;
      case "set_skip_delete_automation_confirm":
        this.settings.skip_delete_automation_confirm = args.skip === true;
        keys = ["skip_delete_automation_confirm"];
        break;
      case "set_second_brain_weekly_review": {
        // The backend registers with `zo cron` first and writes the setting
        // only then; the fixture answers what the vault's registry would.
        const enabled = args.enabled === true;
        this.settings.second_brain_weekly_review = enabled;
        this.secondBrain = {
          ...this.secondBrain,
          weekly_review_enabled: enabled,
          weekly_review: enabled
            ? {
                registered: true,
                cron_id: "cron_fixture_1",
                schedule: "0 21 * * 6",
                next_due_at: 1_800_000_000,
                scheduler: "session_idle",
                registry: "/Users/fixture/Knowledge/.zo/registries/crons.json",
                outcome: "created",
                error: null,
              }
            : null,
        };
        keys = ["second_brain_weekly_review"];
        break;
      }
      case "set_second_brain_explore": {
        // t-4140 S2: the knowledge graph's last exploration, one JSON line per
        // vault beside the scenes — an empty line removes the vault's entry.
        const lines = { ...(this.settings.second_brain_explore ?? {}) };
        const line = String(args.explore ?? "").trim();
        if (line === "" || line === "{}") delete lines[args.vault];
        else lines[args.vault] = line;
        this.settings.second_brain_explore = lines;
        keys = ["second_brain_explore"];
        break;
      }
      case "set_vault_session_limit": {
        this.settings["vault.sessionLimit"] = args.limit;
        keys = ["vault.sessionLimit"];
        break;
      }
      case "set_artifacts_retention_days": {
        const spec = this.settings.artifacts_retention_spec;
        const asked = Number(args.days);
        this.settings.artifacts_retention_days = Math.min(spec.max, Math.max(spec.min, Math.round(asked)));
        keys = ["artifacts_retention_days"];
        break;
      }
      case "save_worktree_prefs":
        this.settings.worktree_prefs = {
          branch_prefix: args.mode,
          custom_prefix: args.custom ?? null,
        };
        keys = ["worktree_prefs"];
        break;
      case "set_panel_width":
        this.settings.panel_widths = {
          ...this.settings.panel_widths,
          [args.side]: args.width,
        };
        keys = ["panel_widths"];
        break;
      case "set_notification_preference":
        this.settings.notifications = {
          ...this.settings.notifications,
          [args.kind]: args.on === true,
        };
        keys = ["notifications"];
        break;
      case "set_browser_home_page":
        this.settings.browser = { ...this.settings.browser, home_page: args.homePage };
        keys = ["browser"];
        break;
      case "set_browser_search_engine":
        this.settings.browser = { ...this.settings.browser, search_engine: args.searchEngine };
        keys = ["browser"];
        break;
      case "patch_browser_link_routing":
        this.settings.browser = {
          ...this.settings.browser,
          [args.patch.kind]: args.patch.value === true,
        };
        keys = ["browser"];
        break;
      // One ROW at a time, the Rust patch's own validation mirrored: a host
      // is a bare lowercase name, an agent one printable ASCII line.
      case "patch_browser_user_agents": {
        const rows = clone(this.settings.browser.user_agents ?? []);
        const { kind, value } = args.patch;
        const rawHost = kind === "set" ? value.host : value;
        const host = String(rawHost).trim().replace(/\.$/, "").toLowerCase();
        if (!host || /[\s/:@?#]/.test(host)) {
          throw new Error(`호스트는 example.com 처럼 스킴·포트·경로 없는 이름이어야 합니다: ${rawHost}`);
        }
        if (kind === "set") {
          const agent = String(value.agent).trim();
          if (!agent || !/^[ -~]+$/.test(agent)) {
            throw new Error("사용자 에이전트는 출력 가능한 ASCII 한 줄이어야 합니다");
          }
          const held = rows.find((row) => row.host === host);
          if (held) held.agent = agent;
          else rows.push({ host, agent });
        } else {
          rows.splice(0, rows.length, ...rows.filter((row) => row.host !== host));
        }
        this.settings.browser = { ...this.settings.browser, user_agents: rows };
        keys = ["browser"];
        break;
      }
      case "set_browser_restore_tabs":
        this.settings.browser = { ...this.settings.browser, restore_tabs: args.on === true };
        keys = ["browser"];
        break;
      case "set_browser_default_zoom": {
        const spec = this.settings.browser_zoom_spec;
        const rounded = Math.round(Number(args.zoomLevel) / spec.step) * spec.step;
        this.settings.browser = {
          ...this.settings.browser,
          default_zoom_level: Math.min(spec.max_level, Math.max(spec.min_level, rounded)),
        };
        keys = ["browser"];
        break;
      }
      case "set_browser_open_tabs":
        this.settings.browser = { ...this.settings.browser, open_tabs: clone(args.urls) };
        keys = ["browser"];
        break;
      case "set_terminal_opacity":
        this.settings.terminal_opacity = Math.max(0, Math.min(1, Number(args.value)));
        keys = ["terminal_opacity"];
        break;
      case "set_window_blur":
        this.settings.window_blur = args.on === true;
        keys = ["window_blur"];
        break;
      case "set_agent_teams_mode":
        this.settings.agent_teams_mode = args.mode;
        keys = ["agent_teams_mode"];
        break;
      case "set_default_agent":
        this.settings.default_agent = clone(args.preference);
        keys = ["default_agent"];
        break;
      case "set_shortcut_visibility": {
        const hidden = new Set(this.settings.hidden_shortcuts ?? []);
        if (args.visible === true) hidden.delete(args.name);
        else hidden.add(args.name);
        this.settings.hidden_shortcuts = [...hidden].sort();
        keys = ["hidden_shortcuts"];
        break;
      }
      case "set_task_source_visibility": {
        const hidden = new Set(this.settings.hidden_task_sources ?? []);
        if (args.visible === true) {
          hidden.delete(args.source);
        } else {
          const other = args.source === "jira" ? "github" : "jira";
          if (hidden.has(other)) throw new Error("at least one task source must remain visible");
          hidden.add(args.source);
        }
        this.settings.hidden_task_sources = [...hidden].sort();
        keys = ["hidden_task_sources"];
        break;
      }
      case "set_keybinding": {
        const defaults = {
          "lane.new": "mod+n",
          "tab.close": "mod+w",
          "file.save": "mod+s",
          "terminal.equalizePaneSizes": null,
          "terminal.setTitle": null,
        };
        const effective = (actionId) => {
          const stored = this.settings.keybindings?.[actionId];
          if (Array.isArray(stored)) return stored;
          const fallback = defaults[actionId];
          return fallback === null || fallback === undefined ? [] : [fallback];
        };
        const next = args.bindings === null ? null : [...(args.bindings ?? [])];
        const candidate = next ?? (defaults[args.actionId] ? [defaults[args.actionId]] : []);
        for (const chord of candidate) {
          for (const actionId of Object.keys(defaults)) {
            if (actionId !== args.actionId && effective(actionId).includes(chord)) {
              throw new Error(`${chord} is already owned by ${actionId}`);
            }
          }
        }
        if (next === null) delete this.settings.keybindings[args.actionId];
        else this.settings.keybindings[args.actionId] = next;
        keys = ["keybindings"];
        break;
      }
      default:
        throw new Error(`not a settings mutation: ${command}`);
    }

    const canonicalPatch = this.canonicalNext.get(command);
    this.canonicalNext.delete(command);
    if (canonicalPatch) Object.assign(this.settings, canonicalPatch);
    this.settings.revision += 1;
    const answer = this.snapshot();
    const answerPatch = this.answerNext.get(command);
    this.answerNext.delete(command);
    if (answerPatch) Object.assign(answer, answerPatch);
    await this.broadcast(windowId, keys);

    const deferredLostAck = this.deferredLoseAckNext.get(command);
    if (deferredLostAck) {
      this.deferredLoseAckNext.delete(command);
      deferredLostAck.arrive();
      await deferredLostAck.waiting;
      throw new Error(`lost acknowledgement after commit: ${command}`);
    }
    if (this.loseAckNext.delete(command)) {
      throw new Error(`lost acknowledgement after commit: ${command}`);
    }
    const held = this.holds.get(command);
    if (held) {
      held.arrive();
      await held.promise;
    }
    return answer;
  }

  jiraId(args) {
    return args.id ?? args.siteId ?? args.site_id ?? null;
  }

  jiraStatus() {
    return clone(this.jira);
  }

  jiraLifecycle(command, change) {
    if (this.rejectNext.delete(command)) throw new Error(`refused: ${command}`);
    change();
    const answer = this.jiraStatus();
    if (this.loseAckNext.delete(command)) {
      throw new Error(`lost acknowledgement after commit: ${command}`);
    }
    return answer;
  }

  githubId(args) {
    return args.accountId ?? args.account_id ?? null;
  }

  githubStatus() {
    return clone(this.github);
  }

  gitlabStatus() {
    return clone(this.gitlab);
  }

  githubLifecycle(command, change) {
    if (this.rejectNext.delete(command)) throw new Error(`refused: ${command}`);
    change();
    const answer = this.githubStatus();
    if (this.loseAckNext.delete(command)) {
      throw new Error(`lost acknowledgement after commit: ${command}`);
    }
    return answer;
  }

  sshHostsReport() {
    return { hosts: clone(this.sshHosts) };
  }

  remoteWorkspacesReport() {
    return {
      workspaces: this.remoteWorkspaces.map((workspace) => ({
        ...clone(workspace),
        hostLabel: this.sshHosts.find((host) => host.id === workspace.hostId)?.label ?? "",
      })),
    };
  }

  remoteServersReport() {
    return { servers: clone(this.remoteServers) };
  }

  async sshHostLifecycle(windowId, command, change) {
    if (this.rejectNext.delete(command)) throw new Error(`refused: ${command}`);
    change();
    const answer = this.sshHostsReport();
    await this.broadcast(windowId, ["sshHosts", "remoteWorkspaces"]);
    if (this.loseAckNext.delete(command)) {
      throw new Error(`lost acknowledgement after commit: ${command}`);
    }
    return answer;
  }

  /* The SSH-target store, kept to the same rules Rust enforces: an empty id
   * mints one, a label falls back to where the target goes, and the answer is
   * the whole list. The renderer paints from that answer, so a fixture that
   * returned less would prove the wrong thing. */
  async sshTargetLifecycle(windowId, command, change) {
    if (this.rejectNext.delete(command)) throw new Error(`refused: ${command}`);
    change();
    const answer = clone(this.sshTargets);
    await this.broadcast(windowId, ["sshTargets"]);
    if (this.loseAckNext.delete(command)) {
      throw new Error(`lost acknowledgement after commit: ${command}`);
    }
    return answer;
  }

  async remoteWorkspaceLifecycle(windowId, command, change) {
    if (this.rejectNext.delete(command)) throw new Error(`refused: ${command}`);
    change();
    const answer = this.remoteWorkspacesReport();
    await this.broadcast(windowId, ["remoteWorkspaces"]);
    if (this.loseAckNext.delete(command)) {
      throw new Error(`lost acknowledgement after commit: ${command}`);
    }
    return answer;
  }

  async remoteServerLifecycle(windowId, command, change) {
    if (this.rejectNext.delete(command)) throw new Error(`refused: ${command}`);
    change();
    const answer = this.remoteServersReport();
    await this.broadcast(windowId, ["remoteServers"]);
    if (this.loseAckNext.delete(command)) {
      throw new Error(`lost acknowledgement after commit: ${command}`);
    }
    return answer;
  }

  async invoke(windowId, command, args = {}) {
    this.record(windowId, command, args);
    const before = this.beforeHolds.get(command);
    if (before) {
      this.beforeHolds.delete(command);
      before.arrive();
      await before.waiting;
    }

    if (SETTINGS_MUTATION_COMMANDS.has(command)) {
      return this.mutate(windowId, command, args);
    }

    switch (command) {
      case "vault_sessions": return { groups: [], agents: [], issues: [], shown: 0, total: 0, truncated: false };
      case "settings_snapshot": {
        // Capture before waiting: this is a real in-flight stale read, not a
        // delayed read that happens to observe whatever committed last.
        const answer = this.snapshot();
        const held = this.snapshotHolds.get(windowId);
        if (held) {
          this.snapshotHolds.delete(windowId);
          await held.promise;
        }
        return answer;
      }
      case "apply_ui_zoom": return null;
      // 창 안 경로 브라우저(t-2982)의 두 물음. 폴더 필드의 「폴더 선택」은 이제
      // OS 패널이 아니라 이 브라우저를 열고, 시험은 브라우저를 답으로 닫는다.
      case "browse_places": return { home: "/Users/tester", places: [
        { kind: "home", name: "tester", path: "/Users/tester" },
      ] };
      case "browse_dir": return { path: args.path, parent: null, entries: [], total: 0, truncated: false };
      case "second_brain_status": {
        const path = args.path || this.secondBrain.saved_path || this.secondBrain.vaults[0]?.path;
        return clone({
          ...this.secondBrain,
          vault: path ? {
            path,
            exists: true,
            setup_complete: path === this.secondBrain.saved_path && Boolean(path),
            raw_items: 4,
            wiki_pages: 7,
            last_ingested: "2026-09-03 — source → [[Concept]]",
            counts_capped: false,
          } : null,
        });
      }
      case "second_brain_setup": {
        const path = args.path;
        this.settings.second_brain_vault = path;
        this.secondBrain = {
          ...this.secondBrain,
          saved_path: path,
          vault: {
            path,
            exists: true,
            setup_complete: true,
            raw_items: 4,
            wiki_pages: 7,
            last_ingested: "2026-09-03 — source → [[Concept]]",
            counts_capped: false,
          },
          created: ["raw/README.md", "wiki/index.md", "wiki/log.md", "AGENTS.md", "CLAUDE.md"],
          quick_commands_added: 3,
          skill_installs: [{ agent: "codex", state: "written" }],
          guides: this.secondBrain.guides.map((row) => ({ ...row, state: "linked" })),
        };
        for (const [suffix, label] of [
          ["ingest", "raw 취합"], ["ask", "위키에 질문"], ["weekly", "주간 리뷰"],
        ]) {
          const id = `second-brain-fixture-${suffix}`;
          if (!this.quickCommands.some((row) => row.id === id)) {
            this.quickCommands.push({
              id, label, workspace: path, body: label, agent: "codex", append_enter: true,
            });
          }
        }
        return clone(this.secondBrain);
      }
      case "second_brain_link": {
        // 다시 연결: 저장된 볼트를 각 에이전트의 전역 지시문에 다시 쓴다.
        // 하나가 실패하면 카드가 그 파일 이름을 문장으로 말해야 한다.
        this.secondBrain = {
          ...this.secondBrain,
          guides: this.secondBrain.guides.map((row) => (
            row.agent === this.secondBrainLinkFailure
              ? { ...row, state: "failed", detail: "Permission denied (os error 13)" }
              : { ...row, state: "linked", detail: "" }
          )),
        };
        return clone(this.secondBrain);
      }
      case "second_brain_open": return null;
      case "open_project": return args.path;
      case "floating_workspace_seat": {
        // The Rust resolution in miniature: `~` answers as this fixture's
        // home, a chosen directory answers as itself.
        const cwd = this.settings.floating_workspace.cwd;
        return cwd === "" || cwd === "~" ? "/Users/fixture" : cwd;
      }
      case "open_terminal": return "~";
      case "list_system_fonts": return ["Fira Code", "JetBrains Mono", "Menlo"];
      case "terminal_keyboard_layout": return clone(this.keyboardLayout);
      case "terminal_windows_status": return {
        supported: true,
        pwsh_available: this.pwshAvailable,
        git_bash_available: this.gitBashAvailable,
      };
      case "preview_ghostty_import": return clone(this.ghosttyPreview);
      case "boot_report": return this.bootReport();
      case "project_catalog": return clone(PROJECT_CATALOG);
      // 창이 보이는 동안 2초마다 묻는 도장 — 이 픽스처의 체크아웃은 움직이지
      // 않으므로 늘 같은 답이고, 목록은 다시 읽히지 않는다.
      case "worktree_stamp": return "";
      // The release lane's two files, read once at boot and on the usage
      // gauge's period (t-3005). This fixture's machine has no lane: two
      // nulls and no notice, which is what a fresh install answers.
      case "release_status": return { status: null, installed: null, notice: null };
      // The update feed (t-3191): this fixture's build stands idle — the
      // backend judges the knock and this machine has nothing published.
      case "update_check":
      case "update_download":
      case "update_install":
        return {
          phase: { phase: "idle" },
          next: null,
          running: { version: "0.1.0", commit: "9b576e43aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", ui_digest: "ui" },
          prefs: clone(this.settings.update),
        };
      case "update_history": return { releases: [], fetched_at: null, source: "none", failure: null };
      case "scm_status": return { changed: [], ignored: [] };
      case "list_dir": return [];
      case "stage_layouts": return null;
      case "pane_layouts": return [];
      // 원장이 이 체크아웃에 앉힌 마지막 에이전트 — 이 픽스처의 워크스페이스는
      // 어느 워커도 잘라 준 적이 없으니, 실제 백엔드가 그럴 때 답하는 그대로.
      case "worktree_last_agent": return null;
      case "default_tabs": return { tabs: [], applied: false };
      case "open_term_tab": return ++this.nextTerm;
      case "terminal_sessions": {
        const answer = clone(this.terminalSessions);
        const held = this.holds.get(command);
        if (held) {
          held.arrive();
          await held.promise;
        }
        return answer;
      }
      case "end_terminal_session": {
        const before = this.terminalSessions.length;
        this.terminalSessions = this.terminalSessions.filter((session) => session.term !== args.term);
        return this.terminalSessions.length !== before;
      }
      case "end_all_terminal_sessions": {
        const count = this.terminalSessions.length;
        this.terminalSessions = [];
        return count;
      }
      case "term_resize": return null;
      case "set_watched_terms": return null;
      // Mirrors `term_pull` (board.rs): the bytes of what this window's
      // declared shells owe it. A settings document declares no shell, so
      // nothing is owed and nothing is missing — the answer the window decodes.
      case "term_pull": return new TextEncoder().encode(JSON.stringify({ screens: [], missing: [] }));
      case "term_snapshot": return null;
      case "term_key": return null;
      case "save_pane_layouts": return null;
      case "save_stage_layouts": return null;
      case "list_agents": return clone(AGENTS);
      case "agent_terms": return [];
      case "list_claude_sessions": return [];
      case "agent_icon": return null;
      case "agent_launch_plans": return clone(this.agentLaunchPlans);
      case "zo_integration_details": return args.export ? { redacted: "redacted-fixture" }
        : clone(this.zoIntegration ?? { records: [], installed: false, window: {} });
      case "set_agent_permission_mode": {
        if (this.rejectNext.delete(command)) throw new Error(`refused: ${command}`);
        const defaults = new Map(DEFAULT_AGENT_LAUNCH_PLANS.map((row) => [row.agent, row]));
        this.agentLaunchPlans = this.agentLaunchPlans.map((row) => {
          if (!row.has_switch || row.permission === "mixed") return row;
          const measured = defaults.get(row.agent);
          if (args.mode === "yolo") return clone(measured);
          return {
            ...row,
            args: measured.args ? "" : row.args,
            env: measured.env.length > 0 ? [] : row.env,
            permission: "asks",
            is_default: false,
          };
        });
        const answer = clone(this.agentLaunchPlans);
        if (this.loseAckNext.delete(command)) {
          throw new Error(`lost acknowledgement after commit: ${command}`);
        }
        return answer;
      }
      // 모든 게이지가 한 문으로 물어 온다 — 창을 보는 순간과 에이전트가 조용해진
      // 뒤에. 넷만 정의해 두면 나머지가 "정의되지 않은 IPC"로 잡힌다.
      case "claude_usage":
      case "codex_usage":
      case "antigravity_usage":
      case "kimi_usage":
      case "grok_usage":
      case "opencode_usage":
        return { usage: null, fetching: false };
      // 토큰 원장은 게이지와 다른 물음이라 답의 모양도 다르다 — 아직 스캔이
      // 없는 상태가 이 픽스처의 기본값이다.
      // 머리의 세 figure는 파생이 아니라 센 값이라 자기 문으로 온다.
      case "stats_summary":
        return { first_event_at_ms: null, agents_spawned: 0, agent_time_ms: 0, prs_created: 0 };
      case "claude_usage_stats":
      case "codex_usage_stats":
      case "opencode_usage_stats":
        return { enabled: true, report: null, scanning: false, scanned_at: null, files: 0, capped: false };
      // 트리 행은 `zerocode_core::scm_tree`가 정하고, 규칙의 주인은 그쪽이다
      // — 여기서는 창이 그리는 모양을 확인할 수 있을 만큼만 같은 규칙을
      // 따라 접어 준다.
      case "scm_tree_rows":
        return scmTreeRows(args.area, args.paths, new Set(args.folded ?? []));
      case "workspace_cleanup_scan":
        return { rows: [], scanned_at_ms: 1_700_000_000_000, classifier_version: 1 };
      case "pane_sessions": return [];
      case "pane_agents": return [];
      // 원장이 아는 좌석 — 이 창이 판을 쥐지 않은 워커까지 보드가 그리려면
      // 세 번째 출처가 필요하다(1-t1103). 카드 없는 창에서는 빈 목록이다.
      case "ledger_agents": return [];
      case "board_columns": return [];
      // 그래프 보드가 묻는 스냅샷 — 열 없음·주의 0·전체 0은 실제 백엔드가 카드
      // 없는 창에 답하는 그대로(`zerocode_core::board::snapshot`).
      case "board_snapshot": return { columns: [], attention_count: 0, total_count: 0 };
      case "pane_subagents": return [];
      case "pane_activities": return [];
      case "process_memory": return 0;
      case "listening_ports": return { rows: [], unavailable: false };
      case "github_status": return this.githubStatus();
      case "github_select_account": {
        const id = this.githubId(args);
        return this.githubLifecycle(command, () => {
          const account = this.github.accounts.find((candidate) => candidate.id === id);
          if (!account) throw new Error(`unknown GitHub account: ${id}`);
          if (!account.selectable) throw new Error(`GitHub account is not selectable: ${id}`);
          for (const candidate of this.github.accounts) {
            if (candidate.host === account.host) candidate.active = candidate.id === id;
          }
          if (account.host === this.github.repo_host) {
            this.github.active_account_id = id;
            this.github.repo_standing = account.standing;
          }
        });
      }
      case "github_test_connection": {
        const id = this.githubId(args);
        const account = this.github.accounts.find((candidate) => candidate.id === id);
        if (!account?.active) throw new Error(`GitHub account is not active: ${id}`);
        if (this.rejectNext.delete(command)) throw new Error(`refused: ${command}`);
        return { account_id: id, standing: "connected" };
      }
      case "github_disconnect": {
        const id = this.githubId(args);
        return this.githubLifecycle(command, () => {
          const account = this.github.accounts.find((candidate) => candidate.id === id);
          if (!account) throw new Error(`unknown GitHub account: ${id}`);
          if (!account.disconnectable) throw new Error(`GitHub account is not removable: ${id}`);
          this.github.accounts = this.github.accounts.filter((candidate) => candidate.id !== id);
          if (account.active) {
            const replacement = this.github.accounts.find((candidate) =>
              candidate.host === account.host && candidate.standing === "connected");
            for (const candidate of this.github.accounts) {
              if (candidate.host === account.host) candidate.active = candidate === replacement;
            }
          }
          const active = this.github.accounts.find((candidate) =>
            candidate.host === this.github.repo_host && candidate.active);
          this.github.active_account_id = active?.id ?? null;
          this.github.repo_standing = active?.standing ?? "auth_error";
          this.github.connected = this.github.accounts.some((candidate) =>
            candidate.standing === "connected");
        });
      }
      case "github_login_intent": return "gh auth login --hostname github.com --web";
      case "gitlab_status": return this.gitlabStatus();
      /* The window's one external-link door. Answered rather than refused so a
       * card that sends somebody to an install page is testable at all. */
      case "open_url": return null;
      case "ssh_hosts": return this.sshHostsReport();
      case "probe_ssh_host": {
        const held = this.holds.get(command);
        if (held) {
          held.arrive();
          await held.promise;
        }
        return {
          keyAlgorithm: "ssh-ed25519",
          encodedKey: "AAAAC3NzaC1lZDI1NTE5AAAAIFixturePublicKeyOnly",
          fingerprint: "SHA256:fixture-host-key",
        };
      }
      case "save_ssh_host": return this.sshHostLifecycle(windowId, command, () => {
        const input = clone(args.input);
        const existing = this.sshHosts.find((host) => host.id === input.id);
        const id = existing?.id
          ?? `00000000-0000-4000-8000-${String(this.nextSshHost++).padStart(12, "0")}`;
        const credentialStatus = input.authentication === "agent"
          ? "not_required"
          : input.password || existing?.credential_status === "available"
            ? "available"
            : "missing";
        const host = {
          id,
          label: input.label,
          host: input.host,
          port: input.port,
          user: input.user,
          authentication: input.authentication,
          keyAlgorithm: input.keyAlgorithm,
          encodedKey: input.encodedKey,
          fingerprint: "SHA256:fixture-host-key",
          credentialStatus,
        };
        if (existing) Object.assign(existing, host);
        else this.sshHosts.push(host);
      });
      case "remove_ssh_host": return this.sshHostLifecycle(windowId, command, () => {
        this.sshHosts = this.sshHosts.filter((host) => host.id !== args.id);
        this.remoteWorkspaces = this.remoteWorkspaces.filter(
          (workspace) => workspace.hostId !== args.id,
        );
      });
      case "ssh_targets": return clone(this.sshTargets);
      case "ssh_link_states": return Object.values(clone(this.sshLinks));
      case "ssh_open_remote_term": {
        if (this.rejectNext.delete(command)) throw new Error(`refused: ${command}`);
        if (!this.sshTargets.some((target) => target.id === args.id)) {
          throw new Error("대상을 찾을 수 없습니다");
        }
        return ++this.nextTerm;
      }
      case "ssh_link_connect": {
        if (this.rejectNext.delete(command)) throw new Error(`refused: ${command}`);
        await this.moveSshLink(args.id, { state: "connecting" });
        const held = this.holds.get(command);
        if (held) {
          held.arrive();
          await held.promise;
        }
        const outcome = this.sshLinkNext ?? { state: "connected" };
        this.sshLinkNext = null;
        await this.moveSshLink(args.id, outcome);
        return clone(this.sshLinks[args.id]);
      }
      case "ssh_link_disconnect": {
        await this.moveSshLink(args.id, { state: "disconnected" });
        return null;
      }
      case "ssh_probe_target": {
        if (this.rejectNext.delete(command)) throw new Error(`refused: ${command}`);
        // Arrival is a fact before validity is a verdict: a held test must
        // see every probe reach the backend, even one that will be refused.
        const held = this.holds.get(command);
        if (held) {
          held.arrive();
          await held.promise;
        }
        if (!this.sshTargets.some((target) => target.id === args.id)) {
          throw new Error("대상을 찾을 수 없습니다");
        }
        return null;
      }
      case "ssh_save_target": return this.sshTargetLifecycle(windowId, command, () => {
        const draft = clone(args.target);
        const host = draft.configHost || draft.host;
        if (host === "") throw new Error("host_required");
        const existing = this.sshTargets.find((target) => target.id === draft.id);
        const target = {
          ...draft,
          id: existing?.id ?? `ssh-1700000000000-${String(this.nextSshTarget++).padStart(6, "0")}`,
          label: draft.label || (draft.username ? `${draft.username}@${host}` : host),
          source: "manual",
        };
        if (existing) Object.assign(existing, target);
        else this.sshTargets.push(target);
      });
      case "ssh_remove_target": return this.sshTargetLifecycle(windowId, command, () => {
        const removed = this.sshTargets.find((target) => target.id === args.id);
        // Only a row the file wrote leaves a memory: nothing was going to
        // bring a hand-typed one back.
        if (removed?.source === "ssh-config" && removed.configHost) {
          this.sshSuppressed.push(removed.configHost);
        }
        this.sshTargets = this.sshTargets.filter((target) => target.id !== args.id);
      });
      case "ssh_import_config": {
        if (this.rejectNext.delete(command)) throw new Error(`refused: ${command}`);
        // The amnesty, first: 가져오기 forgives every alias somebody deleted
        // and the same read then adopts them back.
        if (args.reAdopt) this.sshSuppressed = [];
        let changed = 0;
        for (const host of this.sshConfigHosts) {
          if (this.sshSuppressed.includes(host.alias)) continue;
          const existing = this.sshTargets.find((target) =>
            target.configHost === host.alias
            || (target.configHost === "" && target.host === host.alias));
          // A row somebody typed is theirs, and the file does not speak
          // for it.
          if (existing && existing.source !== "ssh-config") continue;
          const adopted = {
            id: existing?.id
              ?? `ssh-1700000000000-${String(this.nextSshTarget++).padStart(6, "0")}`,
            label: host.username ? `${host.username}@${host.alias}` : host.alias,
            host: host.hostname ?? "",
            configHost: host.alias,
            port: host.port ?? 22,
            username: host.username ?? "",
            identityFile: host.identityFile ?? "",
            proxyCommand: "",
            jumpHost: "",
            systemSshConnectionReuse: true,
            relayGracePeriodSeconds: 0,
            relayKeepAliveUntilReset: true,
            source: "ssh-config",
          };
          if (!existing) {
            this.sshTargets.push(adopted);
            changed += 1;
          } else if (!same(clone(existing), adopted)) {
            Object.assign(existing, adopted);
            changed += 1;
          }
        }
        // A sync that moved nothing is not news the other window repaints for.
        if (changed > 0) await this.broadcast(windowId, ["sshTargets"]);
        return { changed };
      }
      case "test_ssh_host": return null;
      case "open_ssh_terminal": return ++this.nextRemoteTerm;
      case "remote_workspaces": return this.remoteWorkspacesReport();
      case "probe_remote_workspace": return {
        root: args.input.root === "/srv/project-link" ? "/srv/project" : args.input.root,
      };
      case "save_remote_workspace": return this.remoteWorkspaceLifecycle(windowId, command, () => {
        const input = clone(args.input);
        const existing = this.remoteWorkspaces.find((workspace) => workspace.id === input.id);
        const id = existing?.id
          ?? `10000000-0000-4000-8000-${String(this.nextRemoteWorkspace++).padStart(12, "0")}`;
        const workspace = {
          id,
          label: input.label,
          hostId: input.hostId,
          root: input.root,
        };
        if (existing) Object.assign(existing, workspace);
        else this.remoteWorkspaces.push(workspace);
      });
      case "remove_remote_workspace": return this.remoteWorkspaceLifecycle(windowId, command, () => {
        this.remoteWorkspaces = this.remoteWorkspaces.filter(
          (workspace) => workspace.id !== args.id,
        );
      });
      case "test_remote_workspace": return null;
      case "open_remote_workspace_terminal": return ++this.nextRemoteTerm;
      case "remote_servers": return this.remoteServersReport();
      case "save_remote_server": return this.remoteServerLifecycle(windowId, command, () => {
        const input = clone(args.input);
        const existing = this.remoteServers.find((server) => server.id === input.id);
        const id = existing?.id
          ?? `20000000-0000-4000-8000-${String(this.nextRemoteServer++).padStart(12, "0")}`;
        const endpoint = new URL(input.accessLink).searchParams.get("endpoint") ?? "";
        const server = {
          id,
          name: input.name,
          endpoint,
          credentialStatus: "available",
        };
        if (existing) Object.assign(existing, server);
        else this.remoteServers.push(server);
      });
      case "remove_remote_server": return this.remoteServerLifecycle(windowId, command, () => {
        this.remoteServers = this.remoteServers.filter((server) => server.id !== args.id);
      });
      case "test_remote_server": return null;
      case "open_remote_server_session": return ++this.nextRemoteTerm;
      case "developer_permission_statuses": return clone(DEVELOPER_PERMISSION_STATES);
      case "request_developer_permission": return {
        id: args.id,
        status: DEVELOPER_PERMISSION_STATES.find((row) => row.id === args.id)?.status ?? "unknown",
        openedSystemSettings: false,
      };
      case "open_developer_permission_settings": return null;
      case "test_local_network_permission":
        return args.host === "127.0.0.1" && args.port === 43123
          ? {
              ok: true,
              host: args.host,
              port: args.port,
              testedAt: 1_700_000_000_000,
            }
          : { ok: false, failure: "unreachable", testedAt: 1_700_000_086_400 };
      case "term_text": return null;
      case "setup_guide":
        return { steps: [], complete: true, dismissed: false, entry: false };
      case "reopen_onboarding":
        return {
          ...clone(BOOT_BASE.onboarding),
          closed_at: null,
          outcome: null,
          last_completed_step: 2,
        };
      case "tip_verdict": return { verdict: "skip", id: null, settle: [] };
      case "tour_decision": return "NotReady";
      case "mark_onboarding":
        return {
          ...clone(BOOT_BASE.onboarding),
          checklist: { ...clone(BOOT_BASE.onboarding.checklist), [args.mark]: true },
        };
      case "terminal_command": return this.settings.terminal_command;
      case "terminal_command_argv": return [this.settings.terminal_command];
      case "agent_teams_mode": return this.settings.agent_teams_mode;
      case "project_scripts":
        return {
          root: BOOT_BASE.project_root,
          file: `${BOOT_BASE.project_root}/zerocode.yaml`,
          exists: false,
          unreadable: false,
          runs_setup: true,
          source: "shared-only",
          setup_run_policy: "run-by-default",
        };
      case "worktree_prefs": return { branch_prefix: "none", custom_prefix: null };
      case "claude_accounts": return clone(this.claudeAccounts ?? { accounts: [], can_add: true });
      case "resolve_claude_account_identity": {
        const held = this.claudeAccounts.accounts.find((row) => row.id === args.id);
        const { pending } = held;
        delete held.pending;
        if (args.choice === "add") this.claudeAccounts.accounts.push({ ...pending, id: "new", signed_in: true });
        else Object.assign(held, pending);
        return clone(this.claudeAccounts);
      }
      case "verify_claude_accounts": return [];
      case "verify_codex_accounts": return [];
      case "codex_account_list": return { accounts: [], can_add: true };
      case "cli_login_list": return clone(this.cliLogins ?? { rows: [] });
      // The three doors that MOVE a login answer with the table re-read, the
      // way the backend does: the row the test asked about flips and the
      // rest stand.
      case "cli_login_start":
      case "cli_login_wait": {
        const held = this.cliLogins.rows.find((row) => row.agent === args.agent);
        if (held) held.signed_in = args.signedIn ?? true;
        return clone(this.cliLogins);
      }
      case "cli_login_logout": {
        const held = this.cliLogins.rows.find((row) => row.agent === args.agent);
        if (held) held.signed_in = false;
        return clone(this.cliLogins);
      }
      case "relogin_codex_login": return { accounts: [], can_add: true };
      case "logout_codex_login": return { accounts: [], can_add: true };
      case "google_account": return clone(this.googleAccount);
      case "google_login_start": return this.googleConsentUrl;
      // The consent screen is a person at Google, so the sign-in that lands
      // here is the one this fixture grants: `google_login_finish` is what
      // answers AFTER the redirect, and it answers with the login on file.
      case "google_login_finish":
        this.googleAccount = {
          ...this.googleAccount,
          signed_in: true,
          email: "tester@example.com",
          renewable: true,
          scoped_for_agents: true,
        };
        return clone(this.googleAccount);
      case "cancel_google_login": return null;
      case "google_logout":
        this.googleAccount = {
          ...this.googleAccount,
          signed_in: false,
          email: undefined,
          renewable: false,
        };
        return clone(this.googleAccount);
      case "hooks_report": return { enabled: true, listening: true, agents: [] };
      case "list_skills": return { skills: [], sources: [] };
      case "skills_list":
      case "skills_rescan": return { families: [], sources: [], agents: [], required: [], policy: { terminal_rows: 24, terminal_cols: 96 }, evidence_scope: "hooks-only" };
      case "orchestration_report": return null;
      case "computer_use_permission_status":
        return {
          platform: this.computerUsePlatform ?? "darwin",
          helper_app_path: "/Applications/ZeroCode Computer Use.app",
          helper_unavailable_reason: null,
          permissions: clone(this.computerUsePermissions),
        };
      case "open_computer_use_permission":
        return {
          platform: this.computerUsePlatform ?? "darwin",
          helper_app_path: "/Applications/ZeroCode Computer Use.app",
          permission_id: args.id,
          opened_settings: true,
          launched_helper: true,
          permissions: clone(this.computerUsePermissions),
          next_step: null,
        };
      case "reset_computer_use_permissions":
        this.computerUsePermissions = this.computerUsePermissions.map((permission) => ({
          ...permission, status: "not-granted",
        }));
        return {
          platform: this.computerUsePlatform ?? "darwin",
          helper_app_path: "/Applications/ZeroCode Computer Use.app",
          helper_unavailable_reason: null,
          permissions: clone(this.computerUsePermissions),
          bundle_id: "dev.zerocode.app.computer-use",
        };
      case "install_bundled_skill": {
        const rows = (this.computerUseSkill?.agents ?? []).map((row) => ({ ...row, installed: true }));
        if (args.name === "computer-use") {
          this.computerUseSkill = { ...this.computerUseSkill, installed: true, agents: rows };
        }
        return rows.map((row) => ({
          agent: row.agent,
          label: row.label,
          path: `/home/.${row.agent}/skills/${args.name}/SKILL.md`,
          state: "written",
          detail: "",
        }));
      }
      case "computer_use_skill_report": return clone(this.computerUseSkill);
      // Where the operator stands (C4): idle, nothing stopped, no actions yet.
      case "computer_guard_status":
        return { active: false, stopped: null, actions: 0, hotkey: "control+option+escape", helper: "idle" };
      case "computer_use_capabilities":
        return { platform: "darwin", provider: "zerocode-computer-use-macos", protocolVersion: 1 };
      case "list_automations": return clone(this.automations);
      case "list_automation_runs":
        // Newest first, as the backend answers: it stores oldest-first and
        // reverses on the way out.
        return clone(this.automationRuns.filter((run) => run.automation_id === args.automationId).reverse());
      case "list_run_evidence": return clone(this.runEvidence);
      case "reveal_run_evidence": return null;
      // The Flow roster, and the setter that rewrites a Flow's two sections and
      // answers the fresh roster — the pattern `save_automation` uses.
      case "flow_list": return clone(this.flows);
      case "flow_set": {
        const flow = this.flows.flows.find((row) => row.slug === args.slug);
        if (!flow) throw new Error(`unknown flow: ${args.slug}`);
        if (args.policy) flow.policy = args.policy;
        if (args.evidence) flow.evidence = args.evidence;
        if (flow.policy === "guarded") flow.money = true; else delete flow.money;
        return clone(this.flows);
      }
      case "list_quick_commands": return clone(this.quickCommands);
      case "save_quick_command":
        this.quickCommands = [
          ...this.quickCommands.filter((command) => command.id !== args.command.id),
          clone(args.command),
        ];
        return null;
      case "delete_quick_command":
        this.quickCommands = this.quickCommands.filter((command) => command.id !== args.id);
        return null;
      case "jira_status": return this.jiraStatus();
      case "jira_issues": return [];
      case "linear_status": return { connected: false, credential: "missing", connection: null };
      case "linear_issues": return [];
      case "linear_search_issues": return [];
      case "jira_connect": return this.jiraLifecycle(command, () => {
        const siteUrl = String(args.siteUrl ?? args.site_url ?? "").replace(/\/$/, "");
        const email = String(args.email ?? "").trim();
        const existing = this.jira.sites.find((site) =>
          site.site_url === siteUrl && site.email.toLowerCase() === email.toLowerCase());
        const id = existing?.id ?? `jira-${this.jira.sites.length + 1}`;
        const site = {
          id,
          site_url: siteUrl,
          email,
          display_name: email || new URL(siteUrl).hostname,
          account_id: `acct-${id}`,
          auth_type: args.authType ?? args.auth_type ?? "cloud",
          credential_error: null,
        };
        if (existing) Object.assign(existing, site);
        else this.jira.sites.push(site);
        this.jira.active_site_id = id;
        this.jira.selected_site_id = id;
        this.jira.connected = true;
      });
      case "jira_select_site": {
        const id = this.jiraId(args);
        return this.jiraLifecycle(command, () => {
          if (id !== "all" && !this.jira.sites.some((site) => site.id === id)) {
            throw new Error(`unknown Jira site: ${id}`);
          }
          if (id !== "all") this.jira.active_site_id = id;
          this.jira.selected_site_id = id;
        });
      }
      case "jira_test_connection": {
        const id = this.jiraId(args);
        if (!this.jira.sites.some((site) => site.id === id)) throw new Error(`unknown Jira site: ${id}`);
        return this.jiraStatus();
      }
      case "jira_disconnect": {
        const id = this.jiraId(args);
        return this.jiraLifecycle(command, () => {
          this.jira.sites = this.jira.sites.filter((site) => site.id !== id);
          if (this.jira.active_site_id === id) {
            this.jira.active_site_id = this.jira.sites[0]?.id ?? null;
          }
          if (this.jira.selected_site_id === id) {
            this.jira.selected_site_id = this.jira.active_site_id ?? "all";
          }
          this.jira.connected = this.jira.sites.length > 0;
        });
      }
      case "browser_profiles":
        // 실제 백엔드의 뷰처럼, 저장된 프로필에 파생 상태(스테이징·기본)를
        // 입혀 건넨다 — 창은 `staged`("적용 대기")와 `isDefault`("활성")를 읽는다.
        return this.browserProfiles.map((profile) => ({
          ...clone(profile),
          staged: profile.staged === true,
          isDefault: profile.id === this.defaultProfileId,
        }));
      case "create_browser_profile": {
        const born = {
          id: String(this.browserProfiles.length + 1).padStart(32, "0"),
          name: args.name,
        };
        this.browserProfiles.push(born);
        return clone(born);
      }
      case "delete_browser_profile": {
        this.browserProfiles = this.browserProfiles.filter((profile) => profile.id !== args.id);
        if (this.defaultProfileId === args.id) this.defaultProfileId = null;
        return null;
      }
      case "set_browser_default_profile": {
        this.defaultProfileId = args.id ?? null;
        return null;
      }
      // 감지된 소스 브라우저 — 이 하니스엔 없다(빈 목록). 그래서 카드의
      // 가져오기 메뉴엔 "파일에서 가져오기…" 하나만 선다.
      case "cookie_sources": return [];
      // 파일에서 가져오기 — 대상 프로필에 표를 남기고 요약을 돌려준다.
      // null이면 파일 선택 취소(원본 계약).
      case "import_cookie_file": {
        if (this.cancelFileImport) return null;
        const one = this.browserProfiles.find((profile) => profile.id === args.target);
        if (one) one.source = { family: "file", label: "cookies.txt" };
        return {
          totalCookies: 12, importedCookies: 9, skippedCookies: 3,
          googleCookiesSkipped: 2, partitionSkippedCookies: 0,
          domains: ["a.example", "b.example"], warning: null,
        };
      }
      case "notification_probe": return "ok";
      // 재시작을 건너온 판들의 부팅 질문 (P0-13) — 이 하니스의 창들은
      // 설정 화면이 주제라 지난 세션이 없다: 빈 목록이 정답이다.
      case "last_statuses": return [];
      // 카페인 세그먼트의 부팅 질문. 실제 백엔드처럼 설정된 모드를 그대로
      // 비추고, 켜 둔(on) 모드만 즉시 활동으로 친다 — keeper의 pane 추적은
      // 이 하니스 밖의 일이다.
      // The settings field consumes the native picker's selected paths.
      case "choose_paths": return clone(this.pickedPaths ?? []);
      case "computer_awake_status": return {
        mode: this.settings.computer_awake_mode ?? "off",
        active: (this.settings.computer_awake_mode ?? "off") === "on",
      };
      case "api_router_keys_kept": return !this.routerKeychainUnavailable;
      case "api_router_presets": return clone(this.routerPresets);
      case "api_router_providers": return clone(this.zoSettings.providers);
      case "test_router_connection": {
        /* `api_routers::test_router_endpoint` hands a refusal back as one
           string: its own prefix, the status, and whatever the server put in
           the body — which is the shape the pane has to keep off its first
           line. */
        if (this.routerProbeRefusal) throw new Error(this.routerProbeRefusal);
        const url = `${args.baseUrl.replace(/\/+$/, "")}${args.modelsPath || "/models"}`;
        const headers = { ...(args.headers ?? {}) };
        if (args.key) {
          const scheme = args.auth?.scheme ? `${args.auth.scheme} ` : "";
          headers[args.auth?.header || "Authorization"] = `${scheme}${args.key}`;
        }
        const res = await fetch(url, { headers });
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const body = await res.json();
        const idKey = args.idField || "id";
        const ctxKey = args.contextField || "context_length";
        return (body.data ?? []).map((item) => ({
          id: item[idKey],
          context_length: item[ctxKey] ?? null,
        }));
      }
      case "save_api_router": {
        // Mirrors `assign_auth_env`: the row's variable is the backend's. A
        // row already standing under this name keeps its own; a new one is
        // spelled from its preset id, else its name, and made unique against
        // every other row. The keychain item is named by that variable.
        const rows = this.zoSettings.providers;
        const own = rows.find((p) => p.name === args.name)?.auth_env;
        let authEnv = own?.startsWith("ZEROCODE_ROUTER_") ? own : null;
        if (!authEnv) {
          const word = (args.presetId || args.name).toUpperCase()
            .replace(/[^A-Z0-9]+/g, "_").replace(/^_+|_+$/g, "") || "CUSTOM";
          const taken = (env) => rows.some((p) => p.name !== args.name && p.auth_env === env);
          authEnv = `ZEROCODE_ROUTER_${word}_KEY`;
          for (let n = 2; taken(authEnv); n++) authEnv = `ZEROCODE_ROUTER_${word}_${n}_KEY`;
        }
        const service = `dev.zerocode.router.${authEnv}`;
        if (args.key && this.routerKeychainUnavailable) {
          // Mirrors `RouterRefusal::keychain_unavailable`: a kind the pane
          // translates, and nothing written.
          throw { kind: "keychain-unavailable", message: "no keychain for router keys" };
        }
        // Mirrors `save_router`'s `requires_auth`: a key given now, or one
        // already stored for a row's kept variable; a new keyless row clears
        // whatever stale item sat under its variable.
        if (args.key) {
          this.keychain.set(service, args.key);
        } else if (!own) {
          this.keychain.delete(service);
        }
        const entry = {
          name: args.name,
          base_url: args.baseUrl,
          // Mirrors `ProviderModel::plain_when_silent`: a model states its
          // own window when the probe read one, else it is a plain id.
          models: args.models.map((model) => (typeof model === "object" && model?.context_window > 0
            ? { id: model.id, context_window: model.context_window }
            : (typeof model === "object" ? model.id : model))),
          requires_auth: this.keychain.has(service),
          auth_env: authEnv,
        };
        // Mirrors `save_api_router`: a row that names a client (its preset's
        // data) is written under that identity, and only such a row.
        if (args.clientFingerprint) {
          entry.client_fingerprint = args.clientFingerprint;
        }
        if (args.headers) {
          entry.headers = args.headers;
        }
        const existingIdx = this.zoSettings.providers.findIndex((p) => p.name === args.name);
        if (existingIdx >= 0) {
          // Mirrors `resave_row`: the window's keys are replaced in place, a
          // provider-wide window is cleared, every other key stays.
          const { context_window: _superseded, ...kept } = this.zoSettings.providers[existingIdx];
          this.zoSettings.providers[existingIdx] = { ...kept, ...entry };
        } else {
          this.zoSettings.providers.push(entry);
        }
        return clone(this.zoSettings.providers);
      }
      case "remove_api_router": {
        // Mirrors `remove_router`: the key is found on the row itself and
        // deleted only when no other row still reads it.
        const own = this.zoSettings.providers.find((p) => p.name === args.name)?.auth_env;
        this.zoSettings.providers = this.zoSettings.providers.filter((p) => p.name !== args.name);
        if (own?.startsWith("ZEROCODE_ROUTER_")
          && !this.zoSettings.providers.some((p) => p.auth_env === own)) {
          this.keychain.delete(`dev.zerocode.router.${own}`);
        }
        return clone(this.zoSettings.providers);
      }
      // Mirrors `typesafe_settings.rs`: the key is kept trimmed under the one
      // item every zo reads and never sent back; the switch is zo's
      // `smart.decisionShadow`, one of the routing row's modes.
      case "typesafe_settings": return this.typesafeSettings();
      case "save_typesafe_key": {
        const key = String(args.key ?? "").trim();
        if (!key) throw { kind: "failed", message: "the TypeSafe key is empty" };
        if (this.routerKeychainUnavailable) {
          throw { kind: "keychain-unavailable", message: "no keychain for keys" };
        }
        this.keychain.set(TYPESAFE_SERVICE, key);
        return this.typesafeSettings();
      }
      case "remove_typesafe_key":
        this.keychain.delete(TYPESAFE_SERVICE);
        return this.typesafeSettings();
      case "set_jev_mode": {
        if (this.typesafeSetFailure) {
          throw { kind: "failed", message: "fixture refused the switch" };
        }
        const seat = jevSeat(String(args.use ?? ""));
        if (!seat) throw { kind: "failed", message: `not a seat: ${args.use}` };
        if (!seat.modes.split(" ").includes(args.mode)) {
          throw { kind: "failed", message: `not a mode of ${seat.id}: ${args.mode}` };
        }
        this.zoSettings.smart = { ...(this.zoSettings.smart ?? {}), [seat.setting]: args.mode };
        return this.typesafeSettings();
      }
      case "set_route_classifier": {
        if (this.typesafeSetFailure) {
          throw { kind: "failed", message: "fixture refused the classifier" };
        }
        if (!CLASSIFIER_MODES.some((choice) => choice.mode === args.mode)) {
          throw { kind: "failed", message: `not a classifier mode: ${args.mode}` };
        }
        this.zoSettings.smart = {
          ...(this.zoSettings.smart ?? {}),
          [CLASSIFIER_SETTING]: args.mode,
        };
        return this.typesafeSettings();
      }
      case "set_jev_model": {
        // Mirrors `typesafe_settings::set_model`: an empty word unpins (the
        // key leaves the file), a pin the door would not read is refused.
        if (this.typesafeSetFailure) {
          throw { kind: "failed", message: "fixture refused the model" };
        }
        const pin = jevModelPin(args.model);
        if (!pin && String(args.model ?? "").trim()) {
          throw { kind: "failed", message: `not a model: ${args.model}` };
        }
        const smart = { ...(this.zoSettings.smart ?? {}) };
        if (pin) smart[JEV_MODEL_SETTING] = pin;
        else delete smart[JEV_MODEL_SETTING];
        this.zoSettings.smart = smart;
        return this.typesafeSettings();
      }
      case "check_typesafe_key": return clone(this.typesafeCheck);
      case "jev_summary":
        if (this.jevSummary === null) throw new Error("unknown argument 'jev'");
        return clone(this.jevSummary);
      default:
        this.unknown.push({ window_id: windowId, command, args: clone(args) });
        throw new Error(`unknown command: ${command}`);
    }
  }

  typesafeSettings() {
    /* Every switch reads its own row's words and anything else is off:
       `typesafe_settings.rs` `mode_in`. */
    const mode = (seat, given) => {
      const word = String(given ?? "").trim().toLowerCase();
      const offered = seat.modes.split(" ");
      return offered.includes(word) ? word : offered[0];
    };
    const pin = jevModelPin(this.zoSettings.smart?.[JEV_MODEL_SETTING]);
    return {
      keysKeptHere: !this.routerKeychainUnavailable,
      keySaved: Boolean(this.keychain.get(TYPESAFE_SERVICE)?.trim()),
      model: {
        setting: JEV_MODEL_SETTING,
        model: pin ?? JEV_MODEL_ALIAS,
        pinned: pin !== null,
        alias: JEV_MODEL_ALIAS,
      },
      switches: JEV_SEATS.map((seat) => ({
        id: seat.id,
        setting: seat.setting,
        mode: mode(seat, this.zoSettings.smart?.[seat.setting]),
        modes: clone(jevSeatModes(seat)),
      })),
      classifier: {
        setting: CLASSIFIER_SETTING,
        mode: classifierMode(this.zoSettings.smart?.[CLASSIFIER_SETTING]).mode,
        probes: classifierMode(this.zoSettings.smart?.[CLASSIFIER_SETTING]).probes,
        gates: JEV_SEATS[0].id,
        modes: clone(CLASSIFIER_MODES),
      },
    };
  }
}

const backend = new StatefulBackend();

const files = createServer(async (request, response) => {
  const asked = decodeURIComponent((request.url ?? "/").split("?")[0]);
  if (asked === "/models" || asked === "/stub/models") {
    response.writeHead(200, {
      "content-type": "application/json; charset=utf-8",
      "access-control-allow-origin": "*",
      "access-control-allow-headers": "*",
    });
    response.end(JSON.stringify({
      data: [
        { id: "model-alpha", context_length: 128000 },
        { id: "model-beta", context_length: 64000 },
        { id: "model-gamma", context_length: 32000 },
        { id: "model-delta", context_length: 16000 },
      ],
    }));
    return;
  }
  const path = resolve(UI, `.${asked === "/" ? "/index.html" : asked}`);
  if (path !== UI && !path.startsWith(`${UI}${sep}`)) {
    response.writeHead(403).end();
    return;
  }
  try {
    const types = {
      ".html": "text/html; charset=utf-8",
      ".js": "text/javascript; charset=utf-8",
      ".css": "text/css; charset=utf-8",
      ".png": "image/png",
      ".ico": "image/x-icon",
    };
    response.writeHead(200, { "content-type": types[extname(path)] ?? "application/octet-stream" });
    response.end(await readFile(path));
  } catch {
    response.writeHead(404).end();
  }
});
await new Promise((done) => files.listen(0, "127.0.0.1", done));
const origin = `http://127.0.0.1:${files.address().port}`;

let browser;
try {
  browser = await chromium.launch();
} catch (error) {
  console.error("FAIL  Chromium is required for the settings browser gate.");
  console.error("      Install it with: npx playwright install chromium");
  console.error(`      ${error?.message ?? error}`);
  files.close();
  process.exit(1);
}
const context = await browser.newContext({ viewport: { width: 1280, height: 860 } });
await context.exposeBinding("__SETTINGS_INVOKE__", (source, request) => {
  backend.attach(request.window_id, source.page);
  return backend.invoke(request.window_id, request.command, request.args ?? {});
});

const installTauri = async (page, windowId) => {
  backend.attach(windowId, page);
  await page.addInitScript((id) => {
    window.__SETTINGS_TEST_WINDOW__ = id;
    window.__SETTINGS_TEST_LISTENERS__ = {};
    window.__SETTINGS_TEST_EMIT__ = (name, payload) => {
      for (const handler of window.__SETTINGS_TEST_LISTENERS__[name] ?? []) {
        handler({ payload });
      }
    };
    window.__TAURI__ = {
      core: {
        invoke: (command, args = {}) => window.__SETTINGS_INVOKE__({
          window_id: id,
          command,
          args,
        }),
      },
      event: {
        listen: async (name, handler) => {
          (window.__SETTINGS_TEST_LISTENERS__[name] ??= []).push(handler);
          return () => {
            const held = window.__SETTINGS_TEST_LISTENERS__[name] ?? [];
            window.__SETTINGS_TEST_LISTENERS__[name] = held.filter((one) => one !== handler);
          };
        },
      },
      clipboardManager: {
        readText: async () => "",
        writeText: async () => {},
      },
    };
  }, windowId);
};

const addReportedPlatform = (page, platform) => page.addInitScript((reported) => {
  Object.defineProperty(navigator, "userAgentData", {
    configurable: true,
    get: () => ({ platform: reported }),
  });
}, platform);

const pageA = await context.newPage();
const pageB = await context.newPage();
await installTauri(pageA, "A");
await installTauri(pageB, "B");

const faults = [];
const attachFaultRecorder = (page, id) => {
  page.on("pageerror", (error) => faults.push(`${id}: ${String(error)}`));
};
for (const [id, page] of [["A", pageA], ["B", pageB]]) attachFaultRecorder(page, id);

const results = [];
const pass = (name, detail = "") => results.push({ name, pass: true, detail });
const fail = (name, detail) => results.push({ name, pass: false, detail: String(detail) });

async function test(name, run) {
  try {
    const detail = await run();
    pass(name, detail ?? "");
  } catch (error) {
    fail(name, error?.stack ?? error);
  }
}

function assert(condition, message, detail = undefined) {
  if (!condition) {
    throw new Error(detail === undefined ? message : `${message}: ${JSON.stringify(detail)}`);
  }
}

function assertEqual(actual, expected, message) {
  assert(same(actual, expected), message, { actual, expected });
}

async function renderSettled(page) {
  await page.evaluate(() => new Promise((done) => {
    requestAnimationFrame(() => requestAnimationFrame(done));
  }));
}

async function bootPage(page, id) {
  const from = backend.calls.length;
  await page.goto(origin, { waitUntil: "domcontentloaded" });
  await backend.waitForCall(id, "open_term_tab", from);
  await backend.waitForCall(id, "setup_guide", from);
  await renderSettled(page);
}

async function openSettings(page, pane = null) {
  await page.evaluate((wanted) => {
    setSettingsOpen(true);
    if (wanted) showSettingsPane(wanted);
  }, pane);
  await renderSettled(page);
}

async function reopenSettings(page, id, pane) {
  const before = backend.count(id, "settings_snapshot");
  const from = backend.calls.length;
  await page.evaluate(() => setSettingsOpen(false));
  await page.evaluate((wanted) => {
    setSettingsOpen(true);
    showSettingsPane(wanted);
  }, pane);
  await backend.waitForCall(id, "settings_snapshot", from);
  await renderSettled(page);
  assert(
    backend.count(id, "settings_snapshot") > before,
    "reopening settings did not refetch settings_snapshot",
  );
}

async function gestureAndWait(page, id, command, gesture) {
  const from = backend.calls.length;
  await gesture();
  return backend.waitForCall(id, command, from);
}

const waitForSettingsMutations = (page, remaining) => page.waitForFunction(
  (count) => settingsMutationsInFlight === count,
  remaining,
  { timeout: UI_TIMEOUT },
);
const waitForSettingsIdle = (page) => waitForSettingsMutations(page, 0);

// A no-IPC assertion belongs to its gesture, not the repository's ambient
// stamp beat. Park that existing poller through its target predicate and
// restore each document even when the assertion fails; settings broadcasts,
// reads and writes keep their real stateful backend throughout.
async function withoutWorktreePolling(pages, run) {
  const setRead = (page, value) => page.evaluate((next) => {
    const previous = projectsRead;
    projectsRead = next;
    worktreePoll.sync();
    return previous;
  }, value);
  const held = [];
  try {
    for (const page of pages) held.push({ page, previous: await setRead(page, false) });
    return await run();
  } finally {
    for (const { page, previous } of held) await setRead(page, previous);
  }
}

const controlValue = async (page, kind) => page.evaluate((name) => {
  const pressed = (agent) =>
    document.querySelector(`.agent-pill[data-agent="${agent}"]`)?.getAttribute("aria-pressed");
  return {
    theme: () => document.getElementById("app-theme")?.value,
    locale: () => document.getElementById("app-locale")?.value,
    ui_zoom: () => Number(document.documentElement.dataset.uiZoomLevel),
    app_font_family: () => document.getElementById("app-font-family")?.value,
    titlebar_app_name: () => document.getElementById("show-titlebar-app-name")?.checked,
    menu_bar_icon: () => document.getElementById("show-menu-bar-icon")?.checked,
    minimize_to_tray: () => document.getElementById("minimize-to-tray-on-close")?.checked,
    workspace_card_layout: () => document.documentElement.dataset.worktreeCardLayout,
    show_git_ignored_files: () => document.getElementById("show-git-ignored-files")?.checked,
    source_control_group_order: () => document.getElementById("source-control-group-order")?.value,
    source_control_compare_base: () => document.getElementById(
      "source-control-compare-base",
    )?.value,
    refresh_local_base_ref: () => document.getElementById(
      "refresh-local-base-ref-on-worktree-create",
    )?.checked,
    left_sidebar_appearance: () => document.documentElement.dataset.leftSidebarAppearance,
    left_sidebar_tint_color: () => document.getElementById("left-sidebar-tint-color")?.value,
    left_sidebar_tint_opacity: () => Number(
      document.getElementById("left-sidebar-tint-opacity")?.value,
    ),
    terminal: () => document.getElementById("terminal-command")?.value,
    terminal_prefs: () => Number(document.getElementById("term-font-size")?.value),
    editing_auto_save: () => document.getElementById("editing-auto-save")?.checked,
    opacity: () => Number(document.getElementById("window-opacity")?.value),
    blur: () => document.getElementById("window-blur")?.checked,
    teams: () => document.getElementById("orch-teams-mode")?.value,
    setup_script_launch_mode: () => document.querySelector(
      "[data-setup-launch-mode][aria-pressed='true']",
    )?.dataset.setupLaunchMode,
    terminal_shortcut_policy: () => document.getElementById("terminal-shortcut-policy")?.value,
    tab_order: () => document.getElementById("ctrl-tab-order-mode")?.value,
    workspace_directory: () => document.getElementById("workspace-directory")?.value,
    nest_workspaces: () => document.getElementById("nest-workspaces")?.checked,
    delete_worktree_confirm: () => document.getElementById("ask-before-delete-worktree")?.checked,
    delete_automation_confirm: () => document.getElementById("ask-before-delete-automation")?.checked,
    artifacts_retention: () => Number(document.getElementById("artifacts-retention-days")?.value),
    default_agent: () => ({
      auto: pressed("auto"), blank: pressed("blank"), codex: pressed("codex"),
    }),
  }[name]();
}, kind);

async function changeField(page, selector, value) {
  return page.$eval(selector, (field, next) => {
    field.value = String(next);
    field.dispatchEvent(new Event("change", { bubbles: true }));
  }, value);
}

async function blurField(page, selector, value) {
  await page.fill(selector, String(value));
  return page.$eval(selector, (field) => field.blur());
}

const chooseControl = async (page, kind, value) => {
  switch (kind) {
    case "theme": return page.selectOption("#app-theme", value);
    case "locale": return page.selectOption("#app-locale", value);
    case "artifacts_retention": return blurField(page, "#artifacts-retention-days", value);
    case "ui_zoom": return page.evaluate((level) => setUiZoomLevel(level), value);
    case "app_font_family":
      await page.fill("#app-font-family", value);
      return page.$eval("#app-font-family", (field) =>
        field.dispatchEvent(new Event("change")));
    case "titlebar_app_name": return page.$eval(
      "#show-titlebar-app-name",
      (field, checked) => {
        if (field.checked !== checked) field.click();
      },
      value,
    );
    case "menu_bar_icon": return page.$eval(
      "#show-menu-bar-icon",
      (field, checked) => {
        if (field.checked !== checked) field.click();
      },
      value,
    );
    case "minimize_to_tray": return page.$eval(
      "#minimize-to-tray-on-close",
      (field, checked) => {
        if (field.checked !== checked) field.click();
      },
      value,
    );
    case "workspace_card_layout": return page.click(`#workspace-card-layout-${value}`);
    case "show_git_ignored_files": return page.$eval(
      "#show-git-ignored-files",
      (field, checked) => {
        if (field.checked !== checked) field.click();
      },
      value,
    );
    case "source_control_group_order": return page.selectOption(
      "#source-control-group-order",
      value,
    );
    case "source_control_compare_base": return page.selectOption(
      "#source-control-compare-base",
      value,
    );
    case "refresh_local_base_ref": return page.$eval(
      "#refresh-local-base-ref-on-worktree-create",
      (field, checked) => {
        if (field.checked !== checked) field.click();
      },
      value,
    );
    // The persistence matrix is deliberately sequential. The Window gate
    // exercises the real controls; here, await the one shared patch owner so
    // three full-document answers cannot overlap the next field's draft.
    case "left_sidebar_appearance": return page.evaluate(
      (mode) => patchLeftSidebarAppearance("mode", mode),
      value,
    );
    case "left_sidebar_tint_color": return page.evaluate(
      (color) => patchLeftSidebarAppearance("tint_color", color),
      value,
    );
    case "left_sidebar_tint_opacity": return page.evaluate(
      (opacity) => patchLeftSidebarAppearance("tint_opacity", opacity),
      value,
    );
    case "terminal":
      await page.fill("#terminal-command", value);
      return page.$eval("#terminal-command", (field) => field.dispatchEvent(new Event("change")));
    case "terminal_prefs": return changeField(page, "#term-font-size", value);
    case "editing_auto_save": return page.$eval(
      "#editing-auto-save",
      (field, checked) => {
        if (field.checked !== checked) field.click();
      },
      value,
    );
    case "opacity":
      await page.fill("#window-opacity", String(value));
      return page.$eval("#window-opacity", (field) => field.dispatchEvent(new Event("change")));
    case "blur": return page.$eval(
      "#window-blur",
      (field, checked) => {
        if (field.checked !== checked) field.click();
      },
      value,
    );
    case "teams": return page.selectOption("#orch-teams-mode", value);
    case "setup_script_launch_mode": return page.click(
      `[data-setup-launch-mode="${value}"]`,
    );
    case "terminal_shortcut_policy": return page.selectOption(
      "#terminal-shortcut-policy",
      value,
    );
    case "tab_order": return page.selectOption("#ctrl-tab-order-mode", value);
    case "workspace_directory":
      await page.fill("#workspace-directory", value);
      return page.$eval("#workspace-directory", (field) => field.blur());
    case "nest_workspaces": return page.$eval(
      "#nest-workspaces",
      (field, checked) => {
        if (field.checked !== checked) field.click();
      },
      value,
    );
    case "delete_worktree_confirm": return page.$eval(
      "#ask-before-delete-worktree",
      (field, checked) => {
        if (field.checked !== checked) field.click();
      },
      value,
    );
    case "delete_automation_confirm": return page.$eval(
      "#ask-before-delete-automation",
      (field, checked) => {
        if (field.checked !== checked) field.click();
      },
      value,
    );
    case "default_agent": return page.click(`.agent-pill[data-agent="${value}"]`);
    default: throw new Error(`unknown control: ${kind}`);
  }
};

await bootPage(pageA, "A");

await test("zo integration is shared, on demand, escaped, and copies only the backend export", async () => {
  const record = { pane: "term-2516", reason: "verified", stale: false, updated_at: 1,
    source_differs: true, installed_changed: true, launch_path: "/local/zo",
    receipt: { process: { build: { id: "zo-build", git_sha: "zo-sha" } }, protocol: { major: 1 },
      launch: { requested: { model: "<img src=x onerror=alert(1)>" }, effective: { model: "fixture", effort: "high" } },
      mode: { requested: "panes", effective: "panes", available: false, reason: "missing-team-binding" } } };
  backend.zoIntegration = { installed: true, installed_path: "/local/zo", window: { git_sha: "window-sha", ui_digest: "window-ui" }, records: [record] };
  const before = backend.count("A", "zo_integration_details");
  await pageA.evaluate((record) => {
    rememberZoIntegration(record);
    const host = document.createElement("section"); host.id = "zo-integration-test";
    host.style.cssText = "position:fixed;inset:0 auto auto 0;width:320px;z-index:999999;background:var(--surface);";
    host.appendChild(zoIntegrationNode(record.pane)); document.body.appendChild(host);
  }, record);
  assertEqual(backend.count("A", "zo_integration_details"), before, "collapsed details made an RPC");
  await pageA.evaluate(() => document.querySelector("#zo-integration-test details").open = true);
  await pageA.waitForFunction(() => document.querySelector("#zo-integration-test").textContent.includes("window-ui"));
  const rendered = await pageA.evaluate(() => {
    const host = document.querySelector("#zo-integration-test");
    return { text: host.textContent, injected: host.querySelectorAll("img").length, overflow: host.scrollWidth > host.clientWidth + 1 };
  });
  assert(rendered.text.includes("window-sha") && rendered.text.includes("zo-sha"), "independent builds were not both shown");
  assert(rendered.injected === 0 && !rendered.overflow, "details injected markup or overflowed a narrow panel", rendered);
  const beforeReopen = backend.count("A", "zo_integration_details");
  await pageA.evaluate(() => { const details = document.querySelector("#zo-integration-test details"); details.open = false; details.open = true; });
  await renderSettled(pageA);
  assertEqual(backend.count("A", "zo_integration_details"), beforeReopen + 1, "reopening should refresh local installed-file evidence once");
  await pageA.evaluate(() => {
    window.__TAURI__.clipboardManager.writeText = async (text) => { window.__ZO_DIAGNOSTICS_COPY__ = text; };
    document.querySelector("#zo-integration-test .zo-integration-body").lastElementChild.click();
  });
  await pageA.waitForFunction(() => window.__ZO_DIAGNOSTICS_COPY__);
  assert((await pageA.evaluate(() => window.__ZO_DIAGNOSTICS_COPY__)).includes("redacted-fixture"), "copy did not use the backend export");
  await pageA.evaluate(() => document.querySelector("#zo-integration-test").remove());
  backend.zoIntegration = null;
});


await test("Orca 1.4.180 settings parity is a versioned pane contract, not a menu claim", async () => {
  assertEqual(
    ORCA_SETTINGS_CONTRACT.reference,
    {
      product: "Orca",
      version: "1.4.180",
      platform: "macOS",
      measured_at: "2026-08-14",
    },
    "settings parity reference drifted",
  );
  assertEqual(ORCA_SETTINGS_CONTRACT.panes.length, 33, "measured Orca pane inventory drifted");
  const terminalThemeContract = ORCA_SETTINGS_CONTRACT.terminal_theme_contract;
  assertEqual(
    {
      dark_default: TERMINAL_PREFS_SPEC.theme_defaults.dark,
      light_default: TERMINAL_PREFS_SPEC.theme_defaults.light,
      separate_light_default: RUST_DEFAULT_DOCUMENT.terminal_prefs.use_separate_light_theme,
      built_in_theme_count: Object.keys(TERMINAL_THEME_CATALOG).length,
      bold_override_application: "preserved_not_applied",
      color_override_keys: TERMINAL_PREFS_SPEC.color_override_groups
        .flatMap((group) => group.keys),
    },
    terminalThemeContract,
    "terminal theme contract drifted from Orca 1.4.180",
  );
  const optionAsAltContract = ORCA_SETTINGS_CONTRACT.terminal_option_as_alt_contract;
  assertEqual(
    {
      default: RUST_DEFAULT_DOCUMENT.terminal_prefs.mac_option_as_alt,
      modes: TERMINAL_PREFS_SPEC.mac_option_as_alt_modes,
    },
    { default: optionAsAltContract.default, modes: optionAsAltContract.modes },
    "Option as Alt contract drifted from Orca 1.4.180",
  );
  assertEqual(
    RUST_DEFAULT_DOCUMENT.terminal_prefs.jis_yen_to_backslash,
    ORCA_SETTINGS_CONTRACT.terminal_jis_yen_to_backslash_contract.default,
    "JIS Yen-to-Backslash default drifted from Orca 1.4.180",
  );
  assertEqual(
    RUST_DEFAULT_DOCUMENT.terminal_prefs.allow_osc52_clipboard,
    ORCA_SETTINGS_CONTRACT.terminal_osc52_clipboard_contract.default,
    "OSC 52 clipboard default drifted from Orca 1.4.180",
  );
  assertEqual(
    {
      default: RUST_DEFAULT_DOCUMENT.terminal_prefs.windows_shell,
      shells: TERMINAL_PREFS_SPEC.windows_shells,
    },
    {
      default: ORCA_SETTINGS_CONTRACT.terminal_windows_shell_contract.default,
      shells: ORCA_SETTINGS_CONTRACT.terminal_windows_shell_contract.shells,
    },
    "Windows default-shell contract drifted from Orca 1.4.180",
  );
  assertEqual(
    {
      default: RUST_DEFAULT_DOCUMENT.terminal_prefs.windows_powershell_implementation,
      implementations: TERMINAL_PREFS_SPEC.windows_powershell_implementations,
    },
    {
      default: ORCA_SETTINGS_CONTRACT.terminal_windows_powershell_contract.default,
      implementations:
        ORCA_SETTINGS_CONTRACT.terminal_windows_powershell_contract.implementations,
    },
    "Windows PowerShell contract drifted from Orca 1.4.180",
  );
  assertEqual(
    {
      default: RUST_DEFAULT_DOCUMENT.setup_script_launch_mode,
      modes: ["new-tab", "split-vertical", "split-horizontal"],
    },
    {
      default: ORCA_SETTINGS_CONTRACT.setup_script_launch_contract.default,
      modes: ORCA_SETTINGS_CONTRACT.setup_script_launch_contract.modes,
    },
    "Setup Script Location contract drifted from Orca 1.4.180",
  );
  assertEqual(
    {
      default: RUST_DEFAULT_DOCUMENT.terminal_shortcut_policy,
      modes: ["orca-first", "terminal-first"],
    },
    {
      default: ORCA_SETTINGS_CONTRACT.terminal_shortcut_policy_contract.default,
      modes: ORCA_SETTINGS_CONTRACT.terminal_shortcut_policy_contract.modes,
    },
    "Shortcuts in Terminal contract drifted from Orca 1.4.180",
  );
  const mapped = ORCA_SETTINGS_CONTRACT.panes.filter((pane) => pane.zerocode !== null);
  assert(
    mapped.every((pane) => pane.state === "live" || pane.state === "partial"),
    "a mapped pane was labelled blocked",
    mapped,
  );
  assert(
    ORCA_SETTINGS_CONTRACT.panes
      .filter((pane) => pane.state === "blocked")
      .every((pane) => pane.zerocode === null),
    "a blocked capability shipped a dead pane",
  );

  const shape = await pageA.evaluate(() => ({
    rail: [...document.querySelectorAll(".settings-rail-item[data-pane]")]
      .map((item) => item.dataset.pane),
    sections: [...document.querySelectorAll(".settings-pane[data-pane]")]
      .map((section) => section.dataset.pane),
    active: settingsPane,
  }));
  for (const pane of mapped) {
    assertEqual(
      shape.rail.filter((id) => id === pane.zerocode).length,
      1,
      `${pane.orca} has no single rail owner`,
    );
    assertEqual(
      shape.sections.filter((id) => id === pane.zerocode).length,
      1,
      `${pane.orca} has no single pane owner`,
    );
  }
  assertEqual(
    shape.rail.filter((id) => !shape.sections.includes(id)),
    [],
    "a Settings rail item has no pane",
  );
  assertEqual(shape.active, "general", "Settings did not start on Orca's General pane");
  return `${mapped.length}/${ORCA_SETTINGS_CONTRACT.panes.length} panes have real ZeroCode owners`;
});

await test("Computer Use owns permissions, skill setup, and built-in routing on one live pane", async () => {
  const from = backend.calls.length;
  await openSettings(pageA, "computer-use");
  await backend.waitForCall("A", "computer_use_permission_status", from);
  await backend.waitForCall("A", "computer_use_skill_report", from);
  assert(
    await pageA.locator('.settings-pane[data-pane="computer-use"]').isVisible(),
    "Computer Use pane did not open",
  );
  assertEqual(
    await pageA.locator(".computer-use-permission-state.is-on").count(),
    0,
    "ungranted permissions looked ready",
  );
  assert(
    (await pageA.locator("#computer-use-skill-command").textContent()).includes("--skill computer-use"),
    "the skill card lost its install command",
  );
  assert(
    (await pageA.locator(".computer-use-routing").textContent()).includes("zerocode-browser"),
    "web work is not routed to the built-in browser",
  );

  // The install button writes the bundled skill from inside the app and
  // re-reads the report; the outcome names the agent it wrote for.
  {
    const installedAt = backend.calls.length;
    await pageA.click("#computer-use-skill-install");
    const installed = await backend.waitForCall("A", "install_bundled_skill", installedAt);
    assertEqual(installed.args.name, "computer-use", "the wrong skill was installed");
    await backend.waitForCall("A", "computer_use_skill_report", installedAt);
    await pageA.waitForFunction(
      () => document.querySelector("#computer-use-skill-pill")?.classList.contains("is-on") === true,
      undefined,
      { timeout: 5000 },
    );
    assert(
      (await pageA.locator("#computer-use-skill-install-note").textContent()).includes("Codex"),
      "the install outcome did not name the agent it wrote for",
    );
  }

  assertEqual(await pageA.locator('[data-permission-check]').count(), 2, "both permission rows need a recheck button");
  assertEqual(await pageA.locator('[data-permission-reset]').count(), 2, "both permission rows need a targeted reset");
  let askedAt = backend.calls.length;
  await pageA.click('[data-permission-reset="screenshots"]');
  const reset = await backend.waitForCall("A", "open_computer_use_permission", askedAt);
  assertEqual(reset.args, { id: "screenshots", reset: true }, "reset must request only ScreenCapture again");
  askedAt = backend.calls.length;
  await pageA.click('[data-permission-check="screenshots"]');
  await backend.waitForCall("A", "computer_use_permission_status", askedAt);
  askedAt = backend.calls.length;
  await pageA.click('[data-permission-open="accessibility"]');
  const opened = await backend.waitForCall("A", "open_computer_use_permission", askedAt);
  assertEqual(opened.args.id, "accessibility", "the wrong permission setup opened");

  backend.computerUsePermissions = backend.computerUsePermissions.map((permission) => ({
    ...permission,
    status: "granted",
  }));
  askedAt = backend.calls.length;
  // Returning from System Settings automatically rechecks after the toggle.
  await pageA.evaluate(() => window.dispatchEvent(new Event("focus")));
  await backend.waitForCall("A", "computer_use_permission_status", askedAt);
  await pageA.waitForFunction(
    () => document.querySelectorAll(".computer-use-permission-state.is-on").length === 2,
    undefined,
    { timeout: UI_TIMEOUT },
  );
  assert(
    !(await pageA.locator("#computer-use-ready-pill").isHidden()),
    "both grants did not make the summary ready",
  );

  // The typed npx road is the second button now; the first installs in-app.
  askedAt = backend.calls.length;
  const expectedTerm = backend.nextTerm + 1;
  await pageA.click("#computer-use-skill-install-terminal");
  await backend.waitForCall("A", "open_term_tab", askedAt);
  const typed = await backend.waitForCall("A", "term_text", askedAt);
  assertEqual(typed.args.term, expectedTerm, "install command targeted another terminal");
  // Installed a moment ago in-app, so the typed road now offers the update
  // command; either way it names the skill and carries no Enter.
  assert(
    /npx skills (add .*--skill computer-use|update computer-use)/.test(typed.args.text) &&
      !typed.args.text.includes("\n"),
    "the skill command was run or changed instead of only typed",
  );
  assert(await pageA.locator("#settings-view").isHidden(), "skill setup left Settings over the terminal");

  backend.computerUsePermissions = backend.computerUsePermissions.map((permission) => ({
    ...permission,
    status: "not-granted",
  }));
});

// The Flow card (t-4260): its policy and evidence controls are the core enums
// the backend hands back, not values fixed in the window; and a guarded Flow —
// money, whose contracts are not built — says it cannot run in the enum's own
// terms (`Policy::runnable`).
await test("the_flow_card_draws_policy_and_evidence_from_the_core_enums_and_says_when_guarded_cannot_run", async () => {
  backend.flows.flows = [
    {
      slug: "wallet-transfer",
      name: "Wallet 송금 QA",
      policy: "dry",
      evidence: "full",
      fingerprint: { apps: ["com.apple.Safari"], hosts: ["stg-admin.example.internal"] },
      checks: { n: 2, required: 1 },
      last: { at: 5000, pass: true, dir: "/s/1", report: "/s/1/report.html" },
      file: "/r/wallet-transfer.md",
    },
    {
      slug: "wallet-payout",
      name: "Wallet 지급 (운영)",
      policy: "guarded",
      evidence: "full",
      fingerprint: { apps: [], hosts: ["stg-admin.example.internal"] },
      checks: { n: 3, required: 2 },
      money: true,
      last: null,
      file: "/r/wallet-payout.md",
    },
  ];
  const from = backend.calls.length;
  await openSettings(pageA, "computer-use");
  await backend.waitForCall("A", "flow_list", from);
  await renderSettled(pageA);

  assert(await pageA.locator("#flow-card").isVisible(), "the Flow card did not show");
  assertEqual(await pageA.locator(".flow-row").count(), 2, "the roster did not list both Flows");

  // The controls are the two closed sets the backend handed back — the core
  // enums — drawn in order, never fixed in the window.
  const policyValues = await pageA.$$eval("#flow-policy-seg .segment-btn", (nodes) => nodes.map((node) => node.dataset.value));
  assertEqual(policyValues, backend.flows.policies.map((policy) => policy.value), "the policy segment is not the core policy enum");
  const levelValues = await pageA.$$eval("#flow-evidence-seg .segment-btn", (nodes) => nodes.map((node) => node.dataset.value));
  assertEqual(levelValues, backend.flows.levels, "the evidence segment is not the core evidence enum");

  // The guarded Flow cannot run yet, and the card says so and holds the button.
  await pageA.click('.flow-row[data-slug="wallet-payout"]');
  await renderSettled(pageA);
  assertEqual(await pageA.$eval("#flow-policy-seg .segment-btn.is-active", (node) => node.dataset.value), "guarded", "the guarded Flow's chosen policy is not shown");
  assert(await pageA.locator("#flow-guarded-note").isVisible(), "guarded did not say it cannot run");
  assert(await pageA.locator("#flow-run").isDisabled(), "guarded left the run button live");

  // The dry Flow runs, and the warning is gone.
  await pageA.click('.flow-row[data-slug="wallet-transfer"]');
  await renderSettled(pageA);
  assert(await pageA.locator("#flow-guarded-note").isHidden(), "the dry Flow still warned it cannot run");
  assert(!(await pageA.locator("#flow-run").isDisabled()), "the dry Flow held the run button");

  // Choosing an evidence level rewrites the document through flow_set — only
  // the level changes — and the fresh roster makes the choice stick.
  const setAt = backend.calls.length;
  await pageA.click('#flow-evidence-seg .segment-btn[data-value="verdict-only"]');
  const set = await backend.waitForCall("A", "flow_set", setAt);
  assertEqual(set.args, { slug: "wallet-transfer", policy: null, evidence: "verdict-only" }, "flow_set did not carry only the evidence change");
  await renderSettled(pageA);
  assertEqual(await pageA.$eval("#flow-evidence-seg .segment-btn.is-active", (node) => node.dataset.value), "verdict-only", "the chosen evidence level did not stick");

  // The card's first paint over a full roster (§5 numbers): fifty Flows painted
  // in the loaded page, median of five, in ms.
  const paintMs = await pageA.evaluate(() => {
    flowData = {
      ...flowData,
      flows: Array.from({ length: 50 }, (_, i) => ({
        slug: `flow-${i}`,
        name: `Flow ${i}`,
        policy: i % 2 ? "guarded" : "dry",
        evidence: "full",
        fingerprint: { apps: [`app.example.${i}`], hosts: [`host-${i}.example`] },
        checks: { n: 3, required: 2 },
        last: i % 3 ? { at: 1000 + i, pass: i % 2 === 0, dir: `/s/${i}` } : null,
        file: `/r/flow-${i}.md`,
      })),
    };
    const runs = [];
    for (let k = 0; k < 5; k += 1) {
      const t0 = performance.now();
      paintFlows();
      runs.push(performance.now() - t0);
    }
    runs.sort((a, b) => a - b);
    return runs[2];
  });

  backend.flows.flows = [];
  // Leave Settings closed, as the next test's arrival refresh expects.
  await pageA.evaluate(() => setSettingsOpen(false));
  return `policy/evidence from the core enums; card first paint over 50 Flows median ${paintMs.toFixed(2)}ms`;
});

// The report names its platform, and on Windows a grant is not something a
// person opens System Settings for: both rows read granted, the summary says
// what that means there, and neither the open buttons nor the reset offer a
// door that does not exist.
await test("Computer Use on Windows reads ready and offers no permission door to open", async () => {
  backend.computerUsePlatform = "windows";
  backend.computerUsePermissions = backend.computerUsePermissions.map((permission) => ({
    ...permission,
    status: "granted",
  }));
  try {
    const from = backend.calls.length;
    await openSettings(pageA, "computer-use");
    await backend.waitForCall("A", "computer_use_permission_status", from);
    const refreshedAt = backend.calls.length;
    await pageA.click("#computer-use-refresh");
    await backend.waitForCall("A", "computer_use_permission_status", refreshedAt);
    await pageA.waitForFunction(
      () => document.querySelectorAll(".computer-use-permission-state.is-on").length === 2,
      undefined,
      { timeout: UI_TIMEOUT },
    );
    assert(
      !(await pageA.locator("#computer-use-ready-pill").isHidden()),
      "Windows grants did not make the summary ready",
    );
    assert(
      (await pageA.locator("#computer-use-summary-description").textContent()).includes("Windows"),
      "the summary did not say what a Windows grant means",
    );
    assert(
      await pageA.locator('[data-permission-open="accessibility"]').isDisabled(),
      "Windows offered a permission door to open",
    );
    assert(
      await pageA.locator('[data-permission-open="screenshots"]').isDisabled(),
      "Windows offered a screenshot permission door to open",
    );
    assert(
      await pageA.locator("#computer-use-reset").isDisabled(),
      "Windows offered a reset with nothing to reset",
    );
  } finally {
    backend.computerUsePlatform = "darwin";
    backend.computerUsePermissions = backend.computerUsePermissions.map((permission) => ({
      ...permission,
      status: "not-granted",
    }));
  }
});

await test("Setup Script Location owns Orca's three choices and survives a canonical reopen", async () => {
  await openSettings(pageA, "terminal");
  const choices = await pageA.locator("[data-setup-launch-mode]").evaluateAll((buttons) =>
    buttons.map((button) => ({
      mode: button.dataset.setupLaunchMode,
      pressed: button.getAttribute("aria-pressed"),
    })));
  assertEqual(
    choices,
    [
      { mode: "new-tab", pressed: "true" },
      { mode: "split-vertical", pressed: "false" },
      { mode: "split-horizontal", pressed: "false" },
    ],
    "Setup Script Location did not open on Orca's New Tab default",
  );

  for (const mode of ["split-vertical", "split-horizontal"]) {
    const call = await gestureAndWait(
      pageA,
      "A",
      "set_setup_script_launch_mode",
      () => pageA.click(`[data-setup-launch-mode="${mode}"]`),
    );
    assertEqual(call.args, { mode }, `${mode} sent a different wire value`);
  }
  await reopenSettings(pageA, "A", "terminal");
  assertEqual(
    await controlValue(pageA, "setup_script_launch_mode"),
    "split-horizontal",
    "Setup Script Location did not survive a canonical refetch",
  );
});

await test("Manage Sessions lists, refreshes, and deliberately ends the native terminal pool", async () => {
  backend.terminalSessions = [
    { term: 41, running: true, agent: "codex", program: "codex" },
    { term: 42, running: false, agent: null, program: "zsh" },
  ];
  const revision = backend.settings.revision;
  await pageA.evaluate(() => setSettingsOpen(false));
  let from = backend.calls.length;
  await openSettings(pageA, "terminal");
  await backend.waitForCall("A", "terminal_sessions", from);
  await pageA.waitForFunction(() =>
    document.querySelectorAll("#term-session-list .settings-command-row").length === 2);

  const initial = await pageA.evaluate(() => ({
    count: document.getElementById("term-session-count")?.textContent,
    names: [...document.querySelectorAll("#term-session-list .settings-command-name")]
      .map((node) => node.textContent),
    meta: [...document.querySelectorAll("#term-session-list .settings-command-meta")]
      .map((node) => node.textContent),
    empty: document.getElementById("term-session-empty")?.hidden,
  }));
  assertEqual(initial.count, "2", "Manage Sessions did not count the native pool");
  assert(
    initial.names.includes("터미널 41")
      && initial.meta.some((text) => text.includes("Codex") && text.includes("실행 중"))
      && initial.meta.some((text) => text.includes("zsh") && text.includes("셸 대기 중"))
      && initial.empty,
    "Manage Sessions hid a session identity or native foreground fact",
    initial,
  );

  backend.terminalSessions.push({ term: 43, running: null, agent: null, program: null });
  from = backend.calls.length;
  await pageA.click("#term-session-refresh");
  await backend.waitForCall("A", "terminal_sessions", from);
  await pageA.waitForFunction(() =>
    document.querySelectorAll("#term-session-list .settings-command-row").length === 3);

  const staleReached = backend.holdNext("terminal_sessions");
  from = backend.calls.length;
  await pageA.click("#term-session-refresh");
  await staleReached;
  backend.terminalSessions = backend.terminalSessions.filter((session) => session.term !== 43);
  await pageA.evaluate(() => window.__SETTINGS_TEST_EMIT__("term:exited", { term: 43 }));
  backend.release("terminal_sessions");
  await backend.waitForCall("A", "terminal_sessions", from + 1);
  await pageA.waitForFunction(() =>
    document.querySelectorAll("#term-session-list .settings-command-row").length === 2);
  assert(
    !(await pageA.locator("#term-session-list").innerText()).includes("43"),
    "a stale list response resurrected an exited terminal",
  );

  const oneCalls = backend.count("A", "end_terminal_session");
  await pageA.locator("#term-session-list .settings-command-delete").first().click();
  await pageA.waitForSelector("#ask-scrim:not([hidden])");
  await pageA.click("#ask-cancel");
  assertEqual(
    backend.count("A", "end_terminal_session"),
    oneCalls,
    "cancel ended a terminal session",
  );

  from = backend.calls.length;
  await pageA.locator("#term-session-list .settings-command-delete").first().click();
  await pageA.click("#ask-yes");
  const ended = await backend.waitForCall("A", "end_terminal_session", from);
  assertEqual(ended.args, { term: 41 }, "one-session termination changed terminal identity");
  await backend.waitForCall("A", "terminal_sessions", ended.at + 1);
  await pageA.waitForFunction(() =>
    document.querySelectorAll("#term-session-list .settings-command-row").length === 1);

  from = backend.calls.length;
  await pageA.click("#term-session-end-all");
  await pageA.click("#ask-yes");
  const endedAll = await backend.waitForCall("A", "end_all_terminal_sessions", from);
  await backend.waitForCall("A", "terminal_sessions", endedAll.at + 1);
  await pageA.waitForFunction(() =>
    document.querySelectorAll("#term-session-list .settings-command-row").length === 0);
  const empty = await pageA.evaluate(() => ({
    count: document.getElementById("term-session-count")?.textContent,
    empty: document.getElementById("term-session-empty")?.hidden,
    disabled: document.getElementById("term-session-end-all")?.disabled,
  }));
  assertEqual(
    empty,
    { count: "0", empty: false, disabled: true },
    "the empty native pool did not settle into one disabled state",
  );
  assertEqual(
    backend.settings.revision,
    revision,
    "session lifecycle was incorrectly persisted as a Settings mutation",
  );
  return "list + refresh + cancel + end one + end all";
});

await test("Shortcuts in Terminal owns Orca's two policies and survives a canonical reopen", async () => {
  await openSettings(pageA, "shortcuts");
  assertEqual(
    await controlValue(pageA, "terminal_shortcut_policy"),
    "orca-first",
    "Shortcuts in Terminal did not open on Orca's app-first default",
  );
  const choices = await pageA.locator("#terminal-shortcut-policy option").evaluateAll(
    (options) => options.map((option) => option.value),
  );
  assertEqual(
    choices,
    ["orca-first", "terminal-first"],
    "Shortcuts in Terminal did not expose Orca's exact choices",
  );
  const call = await gestureAndWait(
    pageA,
    "A",
    "set_terminal_shortcut_policy",
    () => pageA.selectOption("#terminal-shortcut-policy", "terminal-first"),
  );
  assertEqual(call.args, { policy: "terminal-first" }, "policy sent a different wire value");
  await reopenSettings(pageA, "A", "shortcuts");
  assertEqual(
    await controlValue(pageA, "terminal_shortcut_policy"),
    "terminal-first",
    "Shortcuts in Terminal did not survive a canonical refetch",
  );
});

await test("the first Orca parity panes are real destinations with one active owner", async () => {
  await openSettings(pageA, "general");
  for (const pane of [
    "provider-accounts",
    "api-routers",
    "onboarding",
    "automations",
    "git",
    "quick-commands",
  ]) {
    await pageA.evaluate((wanted) => showSettingsPane(wanted), pane);
    await renderSettled(pageA);
    assert(
      await pageA.locator(`.settings-pane[data-pane="${pane}"]`).isVisible(),
      `${pane} did not become the visible pane`,
    );
    assertEqual(
      await pageA.locator(`.settings-rail-item[data-pane="${pane}"]`).getAttribute("aria-current"),
      "page",
      `${pane} did not own rail selection`,
    );
  }
  return "provider accounts, api routers, onboarding, automations, git, quick commands";
});

await test("the provider accounts pane carries a Google card whose login the window makes", async () => {
  // The third card on that pane, and the one whose sign-in has no CLI behind
  // it: `google_login_start` hands back the consent URL and the window opens
  // it in its own browser tab, so the surface a person touches is the row's
  // three states rather than a spawned program's output.
  const openedAt = backend.calls.length;
  await openSettings(pageA, "provider-accounts");
  await backend.waitForCall("A", "google_account", openedAt);
  const row = pageA.locator("#google-account-list .account-row");
  assertEqual(await row.count(), 1, "the Google card is not one row");
  // The words come through `t()` rather than written out here: this suite
  // switches languages in another test, and a hard-coded Korean sentence
  // would make this one depend on the order they run in.
  const words = await pageA.evaluate(() => ({
    signIn: t("settings.googleAccount.signIn", "로그인"),
    signedOut: t("settings.googleAccount.signedOut", "로그인 안 됨"),
    relogin: t("settings.accounts.relogin", "다시 로그인"),
    logout: t("settings.accounts.logout", "로그아웃"),
  }));
  assertEqual(
    await row.locator(".agent-row-name").textContent(),
    words.signedOut,
    "a machine with no Google login did not say so",
  );
  // There is one Google login per machine, so there is nothing to choose —
  // and a pressed-state control here would be a radio with one option.
  assertEqual(
    await pageA.locator("#google-account-list .account-pick").count(),
    0,
    "the Google row grew a radio it has nothing to choose between",
  );
  assertEqual(
    await row.locator("button").allTextContents(),
    [words.signIn],
    "the signed-out row did not offer exactly one way in",
  );
  // Signing in: Google's desktop OAuth refuses embedded user agents, so the
  // consent URL goes to the system browser (`open_url`) and never to a tab of
  // the window's own; the loopback listener owns completion, and the login the
  // backend reports afterwards names who arrived.
  await pageA.evaluate(() => {
    window.__CONSENT_TABS__ = [];
    window.__REAL_OPEN_BROWSER_TAB__ = window.openBrowserTab;
    window.openBrowserTab = async (url) => {
      window.__CONSENT_TABS__.push(url);
      return true;
    };
  });
  const signingIn = backend.calls.length;
  await row.locator("button").click();
  await backend.waitForCall("A", "google_login_start", signingIn);
  const consentOpened = await backend.waitForCall("A", "open_url", signingIn);
  await backend.waitForCall("A", "google_login_finish", signingIn);
  // The forced Antigravity re-read is what keeps the status bar from standing
  // under a fresh login saying 「Antigravity 로그인 필요」.
  await backend.waitForCall("A", "antigravity_usage", signingIn);
  await renderSettled(pageA);
  assertEqual(
    consentOpened.args.url,
    "https://accounts.google.com/o/oauth2/v2/auth?state=fixture",
    "the consent screen did not go to the system browser",
  );
  assertEqual(
    await pageA.evaluate(() => window.__CONSENT_TABS__),
    [],
    "the consent screen opened in the window's own browser tab, which Google refuses",
  );
  await pageA.evaluate(() => {
    window.openBrowserTab = window.__REAL_OPEN_BROWSER_TAB__;
  });
  await renderSettled(pageA);
  assertEqual(
    await row.locator(".agent-row-name").textContent(),
    "tester@example.com",
    "the finished sign-in did not name the account it belongs to",
  );
  assertEqual(
    await row.locator("button").allTextContents(),
    [words.relogin, words.logout],
    "a signed-in Google row did not offer the repair and the way out",
  );
  return "signed out → consent tab → named login";
});

await test("CLI 로그인 카드는 행이 말하는 길만 걷고, 못 읽는 로그인은 못 읽는다고 말한다", async () => {
  // The card is the backend's table painted: every button, caption and
  // command below comes off a row, and the pane knows no agent's name. The
  // rows here wear the five shapes the table has — a headless verb, a verb
  // typed into a shell, a slash command at an agent's own pane, the same for
  // a CLI the launch catalog does not know, and a login kept where this
  // window may not look.
  const row = (fields) => ({
    homepage_url: "https://example.com",
    installed: true,
    signed_in: false,
    known: true,
    proof: "file",
    opens: "agent",
    home: "~/.fixture",
    road: "verb",
    logout_road: "verb",
    ...fields,
  });
  backend.cliLogins = {
    rows: [
      row({ agent: "verb-row", name: "Verb", program: "verbcli" }),
      row({
        agent: "pane-row",
        name: "Pane",
        program: "panecli",
        road: "pane-verb",
        pane_command: "PANE_HOME=/Users/dev/.pane panecli login",
        logout_road: "pane-verb",
        logout_command: "PANE_HOME=/Users/dev/.pane panecli logout",
      }),
      row({
        agent: "tui-row",
        name: "Tui",
        program: "tuicli",
        signed_in: true,
        account: "person@example.com",
        road: "tui",
        tui_login: "/login",
        logout_road: "tui",
        tui_logout: "/logout",
      }),
      row({
        agent: "outside-row",
        name: "Outside",
        program: "outsidecli",
        opens: "shell",
        road: "tui",
        tui_login: "/auth",
        pane_command: "OUTSIDE_HOME=/Users/dev outsidecli",
        logout_road: "none",
      }),
      row({
        agent: "keyring-row",
        name: "Keyring",
        program: "keyringcli",
        known: false,
        proof: "none",
        road: "pane-verb",
        pane_command: "KEYRING_HOME=/Users/dev/.keyring keyringcli configure",
        logout_road: "none",
      }),
      row({ agent: "absent-row", name: "Absent", program: "absentcli", installed: false }),
    ],
  };
  // Arriving at the pane is what re-reads the table, and a test before this
  // one may have left the panel standing on it: close it first, so this
  // arrival is one.
  await pageA.evaluate(() => setSettingsOpen(false));
  const openedAt = backend.calls.length;
  await openSettings(pageA, "provider-accounts");
  await backend.waitForCall("A", "cli_login_list", openedAt);
  await renderSettled(pageA);
  const rows = pageA.locator("#cli-login-list .account-row");
  assertEqual(await rows.count(), 6, "the card did not paint one row per table row");

  const words = await pageA.evaluate(() => ({
    signIn: t("usage.signIn", "로그인"),
    relogin: t("settings.accounts.relogin", "다시 로그인"),
    logout: t("settings.accounts.logout", "로그아웃"),
    install: t("settings.agents.install", "설치"),
    unreadable: t(
      "settings.cliLogins.unreadable",
      "{{program}}은(는) 로그인을 창이 읽을 수 없는 곳에 둡니다 — 로그인은 여기서 열고, 결과는 그 CLI에서 확인하세요",
      { program: "keyringcli" },
    ),
    notInstalled: t(
      "settings.cliLogins.notInstalled",
      "{{program}}을(를) PATH에서 찾지 못했습니다 — 설치 안내는 오른쪽 링크",
      { program: "absentcli" },
    ),
  }));
  const under = async (index) => rows.nth(index).locator(".agent-row-cmd").textContent();
  const buttons = async (index) => rows.nth(index).locator("button").allTextContents();

  // A signed-in row names who, and offers the repair and the way out.
  assertEqual(await under(2), "person@example.com", "a signed-in row did not name its account");
  assertEqual(await buttons(2), [words.relogin, words.logout], "a signed-in row's verbs");
  // A row whose CLI documents no logout offers none.
  assertEqual(await buttons(3), [words.signIn], "a row with no way out grew one");
  // A login this window cannot read says so, and is not called signed out.
  assertEqual(await under(4), words.unreadable, "a keyring row did not say it cannot be read");
  assertEqual(await buttons(4), [words.signIn], "an unreadable row offered a logout it cannot judge");
  // A CLI that is not on this machine names what was looked for — not the id.
  assertEqual(await under(5), words.notInstalled, "an absent CLI did not name the command");
  assertEqual(await buttons(5), [words.install], "an absent CLI offered something to press besides its page");

  // The headless road: one door, and the table comes back.
  const signingIn = backend.calls.length;
  await rows.nth(0).locator("button").click();
  const started = await backend.waitForCall("A", "cli_login_start", signingIn);
  assertEqual(started.args.agent, "verb-row", "the door was asked about another row");
  await renderSettled(pageA);
  assertEqual(await buttons(0), [words.relogin, words.logout], "the finished login did not move the row");

  // The road that needs a screen: the row's own line, typed into a plain
  // shell of this window, and then the backend watches.
  const typing = backend.calls.length;
  await rows.nth(1).locator("button").click();
  await backend.waitForCall("A", "open_term_tab", typing);
  const typed = await backend.waitForCall("A", "term_text", typing);
  assertEqual(
    typed.args.text,
    "PANE_HOME=/Users/dev/.pane panecli login\r",
    "the window typed something other than the row's line",
  );
  const waited = await backend.waitForCall("A", "cli_login_wait", typing);
  assertEqual(waited.args.signedIn, true, "the watch was not for a login");

  // A CLI the catalog does not launch opens in a shell too, with the slash
  // command left for the person — and a login the window cannot read is
  // never waited on. (The pane-verb road above closed the panel so the
  // person could see the terminal; this opens it again.)
  await openSettings(pageA, "provider-accounts");
  await renderSettled(pageA);
  const opening = backend.calls.length;
  await rows.nth(4).locator("button").click();
  const opened = await backend.waitForCall("A", "term_text", opening);
  assertEqual(
    opened.args.text,
    "KEYRING_HOME=/Users/dev/.keyring keyringcli configure\r",
    "the unreadable row typed something else",
  );
  await renderSettled(pageA);
  assertEqual(
    backend.calls.slice(opening).filter((call) => call.name === "cli_login_wait").length,
    0,
    "the window waited for an answer this CLI never gives",
  );

  // And the card's own cost, measured: the table is thirty rows now.
  const painting = await pageA.evaluate(() => {
    const one = cliLogins;
    const many = { rows: [] };
    for (let i = 0; i < 30; i += 1) {
      many.rows.push({ ...one.rows[0], agent: `row-${i}`, name: `Row ${i}` });
    }
    const timed = (table) => {
      cliLogins = table;
      const began = performance.now();
      paintCliLogins();
      return performance.now() - began;
    };
    const few = timed(one);
    const lots = timed(many);
    cliLogins = one;
    paintCliLogins();
    return { few, lots, rows: many.rows.length };
  });
  console.log(
    `    measured: the CLI login card painted ${backend.cliLogins.rows.length} rows in ` +
      `${painting.few.toFixed(1)} ms and ${painting.rows} rows in ${painting.lots.toFixed(1)} ms`,
  );
  assert(painting.lots < 60, `painting ${painting.rows} rows took ${painting.lots} ms`);
  backend.cliLogins = { rows: [] };
  await pageA.evaluate(() => setSettingsOpen(false));
  return "six shapes, three roads, thirty rows painted";
});

await test("라우터 추가→연결 시험→모델 셋 켬→settings.json providers 한 항목", async () => {
  await openSettings(pageA, "api-routers");

  assert(
    await pageA.locator('.settings-pane[data-pane="api-routers"]').isVisible(),
    "API routers pane is not visible",
  );

  // Fill in router configuration pointing to the stub models endpoint
  await pageA.selectOption("#router-preset-select", "openrouter");
  assertEqual(await pageA.inputValue("#router-name-input"), "OpenRouter", "preset name did not fill");

  // Point baseUrl to the stub models endpoint served by the test harness
  await pageA.fill("#router-base-url-input", origin);
  await pageA.fill("#router-key-input", "sk-or-test-secret-98765");

  // Click '연결 시험' (test connection)
  await pageA.click("#router-test-btn");

  // Wait for models section to become visible
  await pageA.waitForSelector("#router-models-section:not([hidden])", { timeout: UI_TIMEOUT });
  const rows = pageA.locator("#router-models-tbody tr");
  assertEqual(await rows.count(), 4, "stub returned 4 models");

  // 모델 셋 켬: turn on three models (model-alpha, model-beta, model-gamma), uncheck model-delta
  const checkboxes = pageA.locator(".router-model-check");
  assertEqual(await checkboxes.count(), 4, "4 model checkboxes rendered");

  // Uncheck the 4th model (model-delta) so exactly three models are enabled
  await checkboxes.nth(3).uncheck();

  // Ensure save button is enabled
  assert(!await pageA.locator("#router-save-btn").isDisabled(), "save button should be enabled");

  // Click '저장' (Save)
  await pageA.click("#router-save-btn");

  // Wait for save to complete and confirmation
  await pageA.waitForFunction(() => {
    const status = document.getElementById("router-test-status");
    return status && status.textContent.includes("저장");
  });

  // Verify ~/.zo/settings.json providers has ONE entry
  assertEqual(backend.zoSettings.providers.length, 1, "providers must have exactly one entry");
  const saved = backend.zoSettings.providers[0];
  assertEqual(saved.name, "OpenRouter", "saved provider name matches");
  assertEqual(saved.base_url, origin, "saved provider base_url matches");
  assertEqual(saved.requires_auth, true, "requires_auth must be true");
  assertEqual(saved.auth_env, "ZEROCODE_ROUTER_OPENROUTER_KEY", "auth_env name matches");
  // Each checked model carries the window the probe read for it — not the
  // first model's number written once for all of them.
  assertEqual(
    saved.models,
    [
      { id: "model-alpha", context_window: 128000 },
      { id: "model-beta", context_window: 64000 },
      { id: "model-gamma", context_window: 32000 },
    ],
    "the 3 checked models were not saved with their own context windows",
  );
  assertEqual(saved.context_window, undefined, "one model's window was written for every model");
  // OpenRouter accepts any client: its row names none, so zo is never told to
  // present itself as some other client there.
  assertEqual(saved.client_fingerprint, undefined, "a generic router row carries no client identity");

  // Verify secret is NOT in settings.json
  assertEqual(JSON.stringify(saved).includes("sk-or-test-secret-98765"), false, "secret must never be in settings.json");

  // Verify secret IS stored in fake keychain under the service named by the
  // entry's auth_env — the name a zo launch reads it back by.
  const keychainService = "dev.zerocode.router.ZEROCODE_ROUTER_OPENROUTER_KEY";
  assertEqual(backend.keychain.get(keychainService), "sk-or-test-secret-98765", "secret stored in keychain under the service named by auth_env");

  // Verify saved provider is rendered in the list using accountRow styling
  const connectedRows = pageA.locator("#router-provider-list .account-row");
  assertEqual(await connectedRows.count(), 1, "connected router appears in list");
  assertEqual(await connectedRows.locator(".agent-row-name").textContent(), "OpenRouter", "connected router name matches");
  const cmdText = await connectedRows.locator(".agent-row-cmd").textContent();
  assert(cmdText.includes("모델 3개"), `cmdText should include '모델 3개': ${cmdText}`);

  // AgentRouter refuses an unknown client before it weighs the key, so its
  // preset row names the Claude client: the connection test asks for that
  // identity and the saved entry carries it for zo. Only this row does.
  await pageA.selectOption("#router-preset-select", "agentrouter");
  assertEqual(await pageA.inputValue("#router-name-input"), "AgentRouter", "agentrouter preset name did not fill");
  await pageA.fill("#router-base-url-input", origin);
  await pageA.fill("#router-key-input", "sk-ar-test-secret-24680");
  const testedAt = backend.calls.length;
  await pageA.click("#router-test-btn");
  await backend.waitForCall("A", "test_router_connection", testedAt);
  const probe = backend.calls.slice(testedAt).find((call) => call.command === "test_router_connection");
  assertEqual(probe?.args?.clientFingerprint, "claude", "the AgentRouter probe asks for the Claude client identity");
  await pageA.waitForSelector("#router-models-section:not([hidden])", { timeout: UI_TIMEOUT });
  await pageA.click("#router-save-btn");
  await pageA.waitForFunction(
    () => document.querySelectorAll("#router-provider-list .account-row").length === 2,
    null,
    { timeout: UI_TIMEOUT },
  );
  const agentRouter = backend.zoSettings.providers.find((p) => p.name === "AgentRouter");
  assertEqual(agentRouter?.client_fingerprint, "claude", "the AgentRouter entry carries the client identity zo presents");
  assertEqual(
    backend.zoSettings.providers.find((p) => p.name === "OpenRouter")?.client_fingerprint,
    undefined,
    "saving AgentRouter did not lend its identity to OpenRouter",
  );

  return "stub models endpoint, fake keychain, 3 models enabled, providers upsert verified; AgentRouter alone carries the client identity";
});

await test("라우터 연결 변경은 이전 모델 목록과 늦게 도착한 응답을 무효화한다", async () => {
  await openSettings(pageA, "api-routers");
  await pageA.selectOption("#router-preset-select", "custom");
  await pageA.fill("#router-name-input", "probe-generation");
  await pageA.fill("#router-base-url-input", origin);
  await pageA.click("#router-test-btn");
  await pageA.waitForSelector("#router-models-section:not([hidden])", { timeout: UI_TIMEOUT });
  assert(!await pageA.locator("#router-save-btn").isDisabled(), "current models can be saved");
  await pageA.fill("#router-key-input", "changed-credential");
  assert(await pageA.locator("#router-save-btn").isDisabled(), "changing credentials invalidates the tested models");
  assertEqual(await pageA.locator(".router-model-check").count(), 0, "old checkboxes are removed");

  await pageA.evaluate(() => {
    const held = { original: fetchRouterModels };
    fetchRouterModels = () => new Promise((resolve) => { held.release = resolve; });
    held.pending = runRouterTestConnection();
    window.__routerProbeTest = held;
  });
  try {
    await pageA.selectOption("#router-preset-select", "openrouter");
    await pageA.evaluate(async () => {
      const held = window.__routerProbeTest;
      held.release([{ id: "stale-model", context_length: 32_000 }]);
      await held.pending;
    });
    assert(await pageA.locator("#router-save-btn").isDisabled(), "a stale response cannot enable save");
    assertEqual(await pageA.locator(".router-model-check").count(), 0, "a stale response cannot restore models");
    const before = backend.calls.filter((call) => call.command === "save_api_router").length;
    await pageA.evaluate(() => saveApiRouterEntry());
    assertEqual(backend.calls.filter((call) => call.command === "save_api_router").length, before, "the handler also refuses a stale save");
  } finally {
    await pageA.evaluate(() => {
      fetchRouterModels = window.__routerProbeTest.original;
      delete window.__routerProbeTest;
    });
  }
  return "credentials and preset edits invalidate models; a delayed result cannot repopulate or save them";
});

await test("라우터 저장 중 이름이나 모델을 바꾸면 옛 저장 응답이 새 편집을 저장됐다고 하지 않는다", async () => {
  await openSettings(pageA, "api-routers");
  await pageA.selectOption("#router-preset-select", "custom");
  await pageA.fill("#router-name-input", "saved-draft");
  await pageA.fill("#router-base-url-input", origin);
  await pageA.click("#router-test-btn");
  await pageA.waitForSelector("#router-models-section:not([hidden])", { timeout: UI_TIMEOUT });
  for (const change of ["name", "model"]) {
    const held = backend.holdBeforeNext("save_api_router");
    const saving = pageA.evaluate(() => saveApiRouterEntry());
    await held.reached;
    try {
      if (change === "name") await pageA.fill("#router-name-input", "unsaved-draft");
      else await pageA.locator(".router-model-check").last().uncheck();
    } finally { held.release(); }
    await saving;
    const message = await pageA.locator("#router-test-status").textContent();
    const saved = await pageA.evaluate(() => t("settings.apiRouters.saved", "저장되었습니다"));
    assert(message !== saved, `${change} edit was falsely reported as saved`);
    assert(!await pageA.locator("#router-save-btn").isDisabled(), "the current draft can still be saved without another probe");
  }
});

/* One router row as a person adds it: preset (or custom), name, key, test,
 * save — and the save call it made. */
/* What a card's status line says in OUR words: the sentence, not the standing
 * badge beside it and not the server's own words folded under it. */
const statusSaid = (page, id) => page.locator(`#${id} .settings-status-said`).textContent();

/* The four words a card's head can wear, as the one table spells them. */
const standingWord = (page, id) => page.evaluate((wanted) => {
  const row = SETTINGS_STANDINGS.find((held) => held.id === wanted);
  return t(row.key, row.word);
}, id);

async function addRouterRow(page, { preset, name, key }) {
  await page.selectOption("#router-preset-select", preset);
  await page.fill("#router-name-input", name);
  await page.fill("#router-base-url-input", origin);
  if (await page.locator("#router-key-input").isEnabled()) {
    await page.fill("#router-key-input", key);
  } else {
    assertEqual(key, "", "a key typed where the pane offers no key field");
  }
  await page.click("#router-test-btn");
  await page.waitForSelector("#router-models-section:not([hidden])", { timeout: UI_TIMEOUT });
  const savedAt = backend.calls.length;
  await page.click("#router-save-btn");
  const call = await backend.waitForCall("A", "save_api_router", savedAt);
  // The test step left its own words in the status line; the save's replace
  // them once the save has landed and the list is repainted.
  await page.waitForFunction(
    () => document.querySelector("#router-test-status .settings-status-said")?.textContent
      === t("settings.apiRouters.saved", "저장되었습니다"),
    null,
    { timeout: UI_TIMEOUT },
  );
  return call;
}

await test("라우터 줄마다 제 키: 한글 이름 둘·같은 프리셋 둘이 서로의 키를 덮지 않고, 지우기는 제 키만", async () => {
  await openSettings(pageA, "api-routers");
  backend.zoSettings.providers = [];
  backend.keychain.clear();
  await pageA.evaluate(() => refreshApiRouters());
  await renderSettled(pageA);

  // The pane names the preset and the name — never an id it derived itself.
  const corp = await addRouterRow(pageA, { preset: "custom", name: "사내 게이트웨이", key: "sk-corp" });
  assertEqual(corp.args.rowId, undefined, "the pane still derives a row id from the name");
  assertEqual(corp.args.presetId, null, "a custom row named a preset");
  await addRouterRow(pageA, { preset: "custom", name: "개인 게이트웨이", key: "sk-mine" });
  const second = await addRouterRow(pageA, { preset: "openrouter", name: "OpenRouter 회사", key: "sk-or-corp" });
  assertEqual(second.args.presetId, "openrouter", "a preset row did not name its preset");
  await addRouterRow(pageA, { preset: "openrouter", name: "OpenRouter", key: "sk-or-mine" });

  const envs = backend.zoSettings.providers.map((p) => p.auth_env);
  assertEqual(new Set(envs).size, 4, `two rows share one key variable: ${envs.join(", ")}`);
  const keyOf = (name) => {
    const row = backend.zoSettings.providers.find((p) => p.name === name);
    return backend.keychain.get(`dev.zerocode.router.${row.auth_env}`);
  };
  assertEqual(keyOf("사내 게이트웨이"), "sk-corp", "the second Hangul row overwrote the first one's key");
  assertEqual(keyOf("개인 게이트웨이"), "sk-mine", "the second Hangul row's key is missing");
  assertEqual(keyOf("OpenRouter 회사"), "sk-or-corp", "a second OpenRouter row overwrote the first one's key");
  assertEqual(keyOf("OpenRouter"), "sk-or-mine", "the second OpenRouter row's key is missing");

  // 지우기 on the renamed preset row sends its name alone; the key it read is
  // gone and every other row keeps its own.
  const renamed = backend.zoSettings.providers.find((p) => p.name === "OpenRouter 회사");
  const droppedAt = backend.calls.length;
  const rows = pageA.locator("#router-provider-list .account-row");
  const index = (await rows.locator(".agent-row-name").allTextContents()).indexOf("OpenRouter 회사");
  await rows.nth(index).locator(".account-drop").click();
  const removal = await backend.waitForCall("A", "remove_api_router", droppedAt);
  assertEqual(Object.keys(removal.args), ["name"], "the removal still guesses a row id");
  await pageA.waitForFunction(
    () => document.querySelectorAll("#router-provider-list .account-row").length === 3,
    null,
    { timeout: UI_TIMEOUT },
  );
  assertEqual(backend.keychain.has(`dev.zerocode.router.${renamed.auth_env}`), false, "the renamed row's key outlived it");
  assertEqual(keyOf("OpenRouter"), "sk-or-mine", "removing one OpenRouter row took the other's key");
  return envs.join(", ");
});

await test("키체인이 없는 컴퓨터: 판이 키체인을 약속하지 않고 키 칸을 내주지 않으며, 키 없는 저장은 된다", async () => {
  await openSettings(pageA, "api-routers");
  backend.zoSettings.providers = [];
  backend.keychain.clear();
  backend.routerKeychainUnavailable = true;
  let reason;
  try {
    await pageA.evaluate(() => refreshApiRouters());
    // Before anyone types a key, the pane says where one would go — and on
    // this machine that is nowhere.
    assert(await pageA.locator("#router-no-keychain-hint").isVisible(), "the pane did not say this machine keeps no key");
    assert(!(await pageA.locator("#router-keychain-hint").isVisible()), "the pane still promised the macOS Keychain");
    assert(await pageA.locator("#router-key-input").isDisabled(), "a key field was offered on a machine that cannot keep a key");
    // The save's refusal stays the backstop, in the pane's own words — never
    // the backend's raw sentence.
    reason = await pageA.evaluate(() => routerSaveRefusal({
      kind: "keychain-unavailable",
      message: "no keychain for router keys",
    }));
    assert(reason !== "" && reason !== "no keychain for router keys", "the refusal is not spoken through the catalog", reason);

    // A router that needs no key saves, keyless.
    await addRouterRow(pageA, { preset: "ollama", name: "Ollama", key: "" });
    assertEqual(backend.zoSettings.providers.map((p) => [p.name, p.requires_auth]), [["Ollama", false]],
      "a keyless save was refused too");
  } finally {
    backend.routerKeychainUnavailable = false;
  }
  // The same pane on a machine with a keychain offers the field again.
  await pageA.evaluate(() => refreshApiRouters());
  assert(!(await pageA.locator("#router-key-input").isDisabled()), "a machine with a keychain was left without the key field");
  assert(await pageA.locator("#router-keychain-hint").isVisible(), "a machine with a keychain lost its hint");
  assert(!(await pageA.locator("#router-no-keychain-hint").isVisible()), "a machine with a keychain still said it had none");
  return reason;
});

await test("TypeSafe 키는 키체인에만 가고 확인·판단 모드·되돌리기가 백엔드의 답을 그린다", async () => {
  await openSettings(pageA, "api-routers");
  backend.keychain.delete(TYPESAFE_SERVICE);
  delete backend.zoSettings.smart;
  backend.zoSettings.providers = [{ name: "OpenRouter", base_url: origin, models: [], requires_auth: false, auth_env: "ZEROCODE_ROUTER_OPENROUTER_KEY" }];
  await pageA.evaluate(() => refreshApiRouters());
  await renderSettled(pageA);
  const said = (key, fallback, vars) => pageA.evaluate(([one, words, values]) => t(one, words, values), [key, fallback, vars]);
  const status = () => statusSaid(pageA, "typesafe-status");

  // Nothing saved: the badge says so, and nothing but typing a key is offered.
  assertEqual(
    await pageA.locator("#typesafe-key-state").textContent(),
    await standingWord(pageA, "unchecked"),
    "a card with nothing checked does not wear the table's word for it",
  );
  for (const id of ["#typesafe-check-btn", "#typesafe-remove-btn", "#typesafe-save-btn"]) {
    assert(await pageA.locator(id).isDisabled(), `${id} was offered with no key saved`);
  }
  assertEqual(await pageA.locator("#typesafe-routing-select").inputValue(), "off", "the switch did not read zo's default");
  // The options are the backend's modes, in its order, each named by what it does.
  assertEqual(
    await pageA.locator("#typesafe-routing-select option").evaluateAll((options) => options.map((option) => option.value)),
    TYPESAFE_DECISION_MODES.map((choice) => choice.mode),
    "the switch did not list the backend's modes",
  );
  assertEqual(
    await pageA.locator("#typesafe-routing-select option").evaluateAll((options) => options.map((option) => option.textContent)),
    [
      await said("settings.typesafe.modeOff", "끔"),
      await said("settings.typesafe.modeRecord", "기록만"),
      await said("settings.typesafe.modeApply", "항상 적용"),
      await said("settings.typesafe.modeAuto", "자동 (근거가 쌓이면 적용)"),
    ],
    "an option is not named by what its mode does",
  );

  // A save sends the trimmed key once, clears the field, and paints the answer.
  await pageA.fill("#typesafe-key-input", "  apikey_fixture  ");
  assert(await pageA.locator("#typesafe-save-btn").isEnabled(), "a typed key could not be saved");
  const savedAt = backend.calls.length;
  await pageA.click("#typesafe-save-btn");
  const save = await backend.waitForCall("A", "save_typesafe_key", savedAt);
  assertEqual(save.args.key, "apikey_fixture", "the key left the page untrimmed");
  await pageA.waitForFunction(
    (words) => document.getElementById("typesafe-key-state")?.textContent === words,
    await standingWord(pageA, "keySaved"),
    { timeout: UI_TIMEOUT },
  );
  assertEqual(backend.keychain.get(TYPESAFE_SERVICE), "apikey_fixture");
  assertEqual(await pageA.inputValue("#typesafe-key-input"), "", "the key stayed in the field after the save");
  assert(
    !(await pageA.locator('.settings-pane[data-pane="api-routers"]').innerHTML()).includes("apikey_fixture"),
    "the key came back to the page",
  );
  assertEqual(await status(), await said("settings.typesafe.saved", "키를 키체인에 저장했습니다. zo가 다음에 필요할 때 읽습니다."));

  // The check is zo's answer: a model and its latency, or the failure named.
  await pageA.click("#typesafe-check-btn");
  await pageA.waitForFunction(() => document.querySelector("#typesafe-status .settings-status-said")?.textContent.includes("jev-1.13.0"), null, { timeout: UI_TIMEOUT });
  assertEqual(await status(), await said("settings.typesafe.answered", "응답했습니다 — {{model}}, {{ms}} ms", { model: "jev-1.13.0", ms: 612 }));
  backend.typesafeCheck = { answered: false, failure: "unauthorized", elapsedMs: 515 };
  const refused = await said("settings.typesafe.unauthorized", "키가 거절되었습니다 — 키를 다시 확인하세요.");
  await pageA.click("#typesafe-check-btn");
  await pageA.waitForFunction((words) => document.querySelector("#typesafe-status .settings-status-said")?.textContent === words, refused, { timeout: UI_TIMEOUT });
  backend.typesafeCheck = { answered: true, model: "jev-1.13.0", elapsedMs: 612 };

  // The switch writes zo's word and moves nothing else of zo's settings.
  const routers = clone(backend.zoSettings.providers);
  const switchedAt = backend.calls.length;
  await pageA.selectOption("#typesafe-routing-select", "shadow");
  assertEqual((await backend.waitForCall("A", "set_jev_mode", switchedAt)).args.mode, "shadow");
  await pageA.waitForFunction(
    (words) => document.querySelector("#typesafe-status .settings-status-said")?.textContent === words,
    await said("settings.typesafe.turnedRecord", "기록만 켰습니다. 다음 판단부터 기록하되 실제 동작에는 적용하지 않습니다."),
    { timeout: UI_TIMEOUT },
  );
  assertEqual(backend.zoSettings.smart.decisionShadow, "shadow");
  assertEqual(backend.zoSettings.providers, routers, "turning the shadow on moved the router rows");
  await pageA.evaluate(() => refreshApiRouters());
  assertEqual(await pageA.locator("#typesafe-routing-select").inputValue(), "shadow", "a reopened pane lost record-only mode");

  const activeAt = backend.calls.length;
  await pageA.selectOption("#typesafe-routing-select", "on");
  assertEqual((await backend.waitForCall("A", "set_jev_mode", activeAt)).args.mode, "on");
  await pageA.waitForFunction(
    (words) => document.querySelector("#typesafe-status .settings-status-said")?.textContent === words,
    await said("settings.typesafe.turnedApply", "항상 적용을 켰습니다. 다음 판단부터 실제 동작에 반영합니다."),
    { timeout: UI_TIMEOUT },
  );
  assertEqual(backend.zoSettings.smart.decisionShadow, "on");
  assertEqual(backend.zoSettings.providers, routers, "turning actual use on moved the router rows");
  await pageA.evaluate(() => refreshApiRouters());
  assertEqual(await pageA.locator("#typesafe-routing-select").inputValue(), "on", "a reopened pane lost actual-use mode");

  // The ledgers' numbers stand under the seat they belong to, and a zo too
  // old to count them leaves the switches standing without any.
  const numbersOf = (seat) => pageA.evaluate(
    (id) => document.querySelector(`[data-jev-seat="${id}"]`)
      ?.closest("[data-jev-row]")?.querySelector("[data-jev-numbers]")?.textContent ?? null,
    seat,
  );
  assertEqual(await numbersOf("routing"), null, "an unanswered summary drew numbers anyway");
  const window7 = (rows, answered, p95Ms) => ({
    rows, answered, answeredShare: rows ? answered / rows : null,
    answeredLowerBound: rows ? 0.71 : null, called: rows, requests: rows,
    redactedLines: 0, inputTokens: 0, p50Ms: p95Ms, p95Ms, failures: [],
  });
  backend.jevSummary = JEV_SEATS.map((seat) => ({
    id: seat.id, setting: seat.setting, mode: "auto", ledger: `${seat.id}.jsonl`,
    found: null, today: window7(0, 0, null), week: window7(0, 0, null),
    costUsd: 0, riseFloorPermille: null, clearsRiseFloor: null,
    rowsToNextJudgment: null, stand: "recording", applies: false, verdict: null,
  }));
  const routingNumbers = backend.jevSummary.find((seat) => seat.id === "routing");
  routingNumbers.today = window7(1, 1, 616);
  routingNumbers.week = window7(26, 23, 4847);
  routingNumbers.riseFloorPermille = 950;
  routingNumbers.rowsToNextJudgment = 14;
  routingNumbers.verdict = { verdict: "hold", line: "answered" };
  await pageA.evaluate(() => refreshApiRouters());
  const held = [
    await said("settings.typesafe.seatCounts", "오늘 {{today}}건 · 7일 {{rows}}건 중 {{answered}}건 응답",
      { today: 1, rows: 26, answered: 23 }),
    // Counts are grouped the way the language in force groups them (t-6243).
    await said("settings.typesafe.seatP95", "느릴 때 {{ms}} ms", { ms: "4,847" }),
    await said("settings.typesafe.seatHolding", "기록만 하는 중 — {{because}}",
      { because: await said("settings.typesafe.lineAnswered", "응답률이 기준에 못 미칩니다") }),
  ].join(" · ");
  await pageA.waitForFunction(
    (words) => document.querySelector('[data-jev-seat="routing"]')
      ?.closest("[data-jev-row]")?.querySelector("[data-jev-numbers]")?.textContent === words,
    held, { timeout: UI_TIMEOUT },
  );
  assertEqual(
    await numbersOf("summon"),
    await said("settings.typesafe.seatNeverAsked", "아직 사용된 적이 없습니다."),
    "a seat nothing has asked read as a seat that answered nothing",
  );

  // The id asked and the version that answered, one line with the window's
  // rows and the version a change cut away (t-6187) — before the verdict,
  // which stays the line's last words.
  routingNumbers.askedModel = JEV_MODEL_ALIAS;
  routingNumbers.model = "jev-1.13.0";
  routingNumbers.judged = { window: window7(25, 25, 400), windowWanted: 73,
    agreement: { compared: 0, agreed: 0, lowerBound: null, controlRows: 0 } };
  routingNumbers.verdict = { verdict: "hold", line: "answered", cutModel: "jev-1.12.0" };
  await pageA.evaluate(() => refreshApiRouters());
  const versioned = [
    await said("settings.typesafe.seatCounts", "오늘 {{today}}건 · 7일 {{rows}}건 중 {{answered}}건 응답",
      { today: 1, rows: 26, answered: 23 }),
    await said("settings.typesafe.seatP95", "느릴 때 {{ms}} ms", { ms: "4,847" }),
    [
      await said("settings.typesafe.seatVersion", "모델 {{model}}",
        { asked: JEV_MODEL_ALIAS, model: "jev-1.13.0" }),
      await said("settings.typesafe.seatWindowRows", "판정 표본 {{rows}}건", { rows: 25 }),
      await said("settings.typesafe.seatCut", "이전 버전 {{cut}}의 기록은 제외", { cut: "jev-1.12.0" }),
    ].join(" · "),
    await said("settings.typesafe.seatHolding", "기록만 하는 중 — {{because}}",
      { because: await said("settings.typesafe.lineAnswered", "응답률이 기준에 못 미칩니다") }),
  ].join(" · ");
  await pageA.waitForFunction(
    (words) => document.querySelector('[data-jev-seat="routing"]')
      ?.closest("[data-jev-row]")?.querySelector("[data-jev-numbers]")?.textContent === words,
    versioned, { timeout: UI_TIMEOUT },
  );
  delete routingNumbers.askedModel;
  delete routingNumbers.model;
  delete routingNumbers.judged;
  routingNumbers.verdict = { verdict: "hold", line: "answered" };

  // The model pin (`smart.jevModel`): unpinned, the field is empty with the
  // alias behind it; a version pins it where the door reads it; an empty
  // field unpins — the key leaves the file — and a pin the door would not
  // read is refused and writes nothing.
  const modelInput = pageA.locator("#typesafe-model-input");
  assertEqual(await modelInput.inputValue(), "", "an unpinned card showed a pin");
  assertEqual(await modelInput.getAttribute("placeholder"), JEV_MODEL_ALIAS);
  const pinAt = backend.calls.length;
  await modelInput.fill("jev-1.13.0");
  await modelInput.dispatchEvent("change");
  assertEqual((await backend.waitForCall("A", "set_jev_model", pinAt)).args.model, "jev-1.13.0");
  await pageA.waitForFunction(
    (words) => document.querySelector("#typesafe-status .settings-status-said")?.textContent === words,
    await said("settings.typesafe.modelPinned", "{{model}} 버전으로 고정했습니다. 다음 요청부터 이 버전을 씁니다.",
      { model: "jev-1.13.0" }),
    { timeout: UI_TIMEOUT },
  );
  assertEqual(backend.zoSettings.smart[JEV_MODEL_SETTING], "jev-1.13.0");
  // A pinned seat names its version as the pin, not as the newest answer
  // (t-6243 D0): the line says the model is held there.
  routingNumbers.askedModel = "jev-1.13.0";
  routingNumbers.model = "jev-1.13.0";
  await pageA.evaluate(() => refreshApiRouters());
  assertEqual(await modelInput.inputValue(), "jev-1.13.0", "a reopened card lost the pin");
  const pinnedLine = await said("settings.typesafe.seatVersionPinned", "고정 모델 {{model}}", { model: "jev-1.13.0" });
  await pageA.waitForFunction(
    (words) => document.querySelector('[data-jev-seat="routing"]')
      ?.closest("[data-jev-row]")?.querySelector("[data-jev-numbers]")?.textContent.includes(words),
    pinnedLine, { timeout: UI_TIMEOUT },
  );
  delete routingNumbers.askedModel;
  delete routingNumbers.model;
  const slipAt = backend.calls.length;
  await modelInput.fill("jev 1.13");
  await modelInput.dispatchEvent("change");
  await backend.waitForCall("A", "set_jev_model", slipAt);
  assertEqual(backend.zoSettings.smart[JEV_MODEL_SETTING], "jev-1.13.0", "a refused pin was written");
  const unpinAt = backend.calls.length;
  await modelInput.fill("");
  await modelInput.dispatchEvent("change");
  await backend.waitForCall("A", "set_jev_model", unpinAt);
  await pageA.waitForFunction(
    (words) => document.querySelector("#typesafe-status .settings-status-said")?.textContent === words,
    await said("settings.typesafe.modelUnpinned", "고정을 풀었습니다. 늘 최신 버전을 씁니다."),
    { timeout: UI_TIMEOUT },
  );
  assertEqual(JEV_MODEL_SETTING in backend.zoSettings.smart, false, "unpinned left the key behind");

  const autoAt = backend.calls.length;
  await pageA.selectOption("#typesafe-routing-select", "auto");
  assertEqual((await backend.waitForCall("A", "set_jev_mode", autoAt)).args.mode, "auto");
  await pageA.waitForFunction(
    (words) => document.querySelector("#typesafe-status .settings-status-said")?.textContent === words,
    await said("settings.typesafe.turnedAuto", "자동 모드입니다. 정확도 근거가 충분히 쌓일 때까지는 기록만 하고 실제 동작에는 적용하지 않습니다."),
    { timeout: UI_TIMEOUT },
  );
  assertEqual(backend.zoSettings.smart.decisionShadow, "auto");
  await pageA.evaluate(() => refreshApiRouters());
  assertEqual(await pageA.locator("#typesafe-routing-select").inputValue(), "auto", "a reopened pane lost auto mode");
  // Back to actual use, answered before the refusal below is tried: a switch
  // still waiting on its answer takes no second one.
  const backToOn = backend.calls.length;
  await pageA.selectOption("#typesafe-routing-select", "on");
  await backend.waitForCall("A", "set_jev_mode", backToOn);
  await pageA.waitForFunction(
    (words) => document.querySelector("#typesafe-status .settings-status-said")?.textContent === words,
    await said("settings.typesafe.turnedApply", "항상 적용을 켰습니다. 다음 판단부터 실제 동작에 반영합니다."),
    { timeout: UI_TIMEOUT },
  );

  // The window's own switch: its own key under `smart`, and the router's
  // untouched — one card, two questions.
  assertEqual(
    await pageA.locator("#typesafe-browser-select").inputValue(),
    "off",
    "browser recovery did not start off",
  );
  const browserAt = backend.calls.length;
  await pageA.selectOption("#typesafe-browser-select", "on");
  assertEqual((await backend.waitForCall("A", "set_jev_mode", browserAt)).args.mode, "on");
  await pageA.waitForFunction(
    (words) => document.querySelector("#typesafe-status .settings-status-said")?.textContent === words,
    await said("settings.typesafe.turnedApply", "항상 적용을 켰습니다. 다음 판단부터 실제 동작에 반영합니다."),
    { timeout: UI_TIMEOUT },
  );
  assertEqual(backend.zoSettings.smart.browserAction, "on");
  assertEqual(backend.zoSettings.smart.decisionShadow, "on", "the browser switch moved the router's");
  assertEqual(backend.zoSettings.providers, routers, "the browser switch moved the router rows");
  await pageA.evaluate(() => refreshApiRouters());
  assertEqual(
    await pageA.locator("#typesafe-browser-select").inputValue(),
    "on",
    "a reopened pane lost browser recovery",
  );

  backend.typesafeSetFailure = true;
  try {
    const browserRefusedAt = backend.calls.length;
    await pageA.selectOption("#typesafe-browser-select", "off");
    await backend.waitForCall("A", "set_jev_mode", browserRefusedAt);
    await pageA.waitForFunction(
      () => document.getElementById("typesafe-browser-select")?.value === "on",
      null,
      { timeout: UI_TIMEOUT },
    );
    assertEqual(
      backend.zoSettings.smart.browserAction,
      "on",
      "a refused browser change mutated the backend",
    );
  } finally {
    delete backend.typesafeSetFailure;
  }
  // The browser switch's own `auto` records and never presses — named so.
  assertEqual(
    await pageA.locator("#typesafe-browser-select option").evaluateAll((options) => options.map((option) => option.textContent)),
    [
      await said("settings.typesafe.modeOff", "끔"),
      await said("settings.typesafe.modeRecord", "기록만"),
      await said("settings.typesafe.modeApply", "항상 적용"),
      await said("settings.typesafe.modeAuto", "자동 (근거가 쌓이면 적용)"),
    ],
    "a browser option is not named by what its mode does",
  );

  // Every seat Jev sits in has a row on this one card, offering its own row's
  // modes and no others — a seat with no apply stage offers no way to apply.
  //
  // How many each offers is read off the table rather than typed a second
  // time. It WAS typed a second time, and the copy went stale the day `recall`
  // gained an apply stage: the fixture said four words, the list beside it
  // still said three, and this suite was red on main for it.
  for (const seat of JEV_SEATS) {
    assertEqual(
      await pageA.locator(`#typesafe-${seat.id}-select option`).count(),
      seat.modes.split(" ").length,
      `the ${seat.id} switch does not offer its row's modes`,
    );
    assert(
      await pageA.locator(`#typesafe-${seat.id}-select`).isVisible(),
      `the ${seat.id} switch is not on the card`,
    );
  }
  const recallAt = backend.calls.length;
  await pageA.selectOption("#typesafe-recall-select", "shadow");
  const asked = await backend.waitForCall("A", "set_jev_mode", recallAt);
  assertEqual(asked.args.use, "recall", "the card did not say which seat moved");
  assertEqual(asked.args.mode, "shadow");
  assertEqual(backend.zoSettings.smart.rerankShadow, "shadow");
  assertEqual(
    backend.zoSettings.smart.decisionShadow,
    "on",
    "the recall switch moved the router's",
  );
  const browserAutoAt = backend.calls.length;
  await pageA.selectOption("#typesafe-browser-select", "auto");
  assertEqual((await backend.waitForCall("A", "set_jev_mode", browserAutoAt)).args.mode, "auto");
  await pageA.waitForFunction(
    (words) => document.querySelector("#typesafe-status .settings-status-said")?.textContent === words,
    await said("settings.typesafe.turnedAuto", "자동 모드입니다. 정확도 근거가 충분히 쌓일 때까지는 기록만 하고 실제 동작에는 적용하지 않습니다."),
    { timeout: UI_TIMEOUT },
  );
  assertEqual(backend.zoSettings.smart.browserAction, "auto");
  assertEqual(backend.zoSettings.smart.decisionShadow, "on", "the browser switch moved the router's");

  backend.typesafeSetFailure = true;
  try {
    const refusedAt = backend.calls.length;
    await pageA.selectOption("#typesafe-routing-select", "off");
    await backend.waitForCall("A", "set_jev_mode", refusedAt);
    await pageA.waitForFunction(
      () => document.getElementById("typesafe-routing-select")?.value === "on",
      null,
      { timeout: UI_TIMEOUT },
    );
    assertEqual(backend.zoSettings.smart.decisionShadow, "on", "a refused change mutated the backend");
  } finally {
    delete backend.typesafeSetFailure;
  }

  const offAt = backend.calls.length;
  await pageA.selectOption("#typesafe-routing-select", "off");
  await backend.waitForCall("A", "set_jev_mode", offAt);
  await pageA.waitForFunction(
    (words) => document.querySelector("#typesafe-status .settings-status-said")?.textContent === words,
    await said("settings.typesafe.turnedOff", "껐습니다. 더는 판단을 요청하지 않습니다."),
    { timeout: UI_TIMEOUT },
  );
  assertEqual(backend.zoSettings.smart.decisionShadow, "off");

  // A removal forgets the key and takes back what only a saved key offers.
  const removedAt = backend.calls.length;
  await pageA.click("#typesafe-remove-btn");
  await backend.waitForCall("A", "remove_typesafe_key", removedAt);
  await pageA.waitForFunction(
    (words) => document.getElementById("typesafe-key-state")?.textContent === words,
    await standingWord(pageA, "unchecked"),
    { timeout: UI_TIMEOUT },
  );
  assert(!backend.keychain.has(TYPESAFE_SERVICE), "the key outlived its removal");
  assert(await pageA.locator("#typesafe-check-btn").isDisabled(), "a check was offered with no key");

  // A machine with no keychain offers no key field and says why a save cannot be.
  backend.routerKeychainUnavailable = true;
  try {
    await pageA.evaluate(() => refreshApiRouters());
    assert(await pageA.locator("#typesafe-key-input").isDisabled(), "a key field was offered with no keychain");
    assert(await pageA.locator("#typesafe-no-keychain-hint").isVisible(), "the pane did not say this machine keeps no key");
    const reason = await pageA.evaluate(() => typesafeRefusal({ kind: "keychain-unavailable", message: "no keychain for keys" }));
    assert(reason !== "" && reason !== "no keychain for keys", "the refusal is not spoken through the catalog", reason);
  } finally {
    backend.routerKeychainUnavailable = false;
    delete backend.zoSettings.smart;
  }
  await pageA.evaluate(() => refreshApiRouters());
  assert(!(await pageA.locator("#typesafe-key-input").isDisabled()), "a machine with a keychain was left without the key field");
});

await test("카드가 제시하는 판단은 실제로 불릴 수 있어야 한다 — 분류기가 라우팅 자리 앞에 선다", async () => {
  await openSettings(pageA, "api-routers");
  backend.keychain.set(TYPESAFE_SERVICE, "apikey_fixture");
  delete backend.zoSettings.smart;
  await pageA.evaluate(() => refreshApiRouters());
  await renderSettled(pageA);
  const said = (key, fallback) => pageA.evaluate(([one, words]) => t(one, words), [key, fallback]);
  const unreachable = pageA.locator("[data-jev-unreachable]");

  // The gate is on the card, and it offers the four words the classifier has —
  // named by what each does, never by the word it writes.
  assertEqual(
    await pageA.locator("#route-classifier-select option").evaluateAll((options) =>
      options.map((option) => option.value)),
    CLASSIFIER_MODES.map((choice) => choice.mode),
    "the card does not offer the classifier's own words",
  );
  assertEqual(
    await pageA.locator("#route-classifier-select option").evaluateAll((options) =>
      options.map((option) => option.textContent)),
    [
      await said("settings.classifier.modeOff", "끔 — 자동 라우팅을 쓰지 않음"),
      await said("settings.classifier.modeWords", "낱말만"),
      await said("settings.classifier.modeMarkers", "낱말 + 과업에 적힌 표식"),
      await said("settings.classifier.modeProbed", "낱말 + 모델에게도 물음"),
    ],
    "a classifier option is not named by what it does",
  );
  // Nothing written means the probing word, which is zo's own reading of an
  // absent key — so an untouched machine reaches the seat.
  assertEqual(
    await pageA.locator("#route-classifier-select").inputValue(),
    CLASSIFIER_MODES.at(-1).mode,
    "an untouched settings file did not read as the probing word",
  );

  // A seat that asks while nothing calls a probe says so, on the row whose
  // mode it is about — and which row that is, is the backend's answer.
  const quiet = CLASSIFIER_MODES.find((choice) => choice.runs && !choice.probes).mode;
  const chosenAt = backend.calls.length;
  await pageA.selectOption("#route-classifier-select", quiet);
  assertEqual((await backend.waitForCall("A", "set_route_classifier", chosenAt)).args.mode, quiet);
  await pageA.waitForFunction(
    (words) => document.querySelector("#typesafe-status .settings-status-said")?.textContent === words,
    await said("settings.classifier.nowQuiet", "바꿨습니다 — 이 방식은 어디에도 묻지 않습니다."),
    { timeout: UI_TIMEOUT },
  );
  assertEqual(backend.zoSettings.smart.autoClassifier, quiet, "the choice was not written to zo's settings");
  assert(await unreachable.count() === 1, "exactly one row carries this warning");
  assert(
    await unreachable.isHidden(),
    "a seat that asks nothing yet was warned about a mode it is not in",
  );
  assertEqual(
    await unreachable.evaluate((node) =>
      node.closest("[data-jev-row]").querySelector("[data-jev-seat]").dataset.jevSeat),
    JEV_SEATS[0].id,
    "the warning does not stand on the seat the classifier gates",
  );

  // Turn the gated seat on: now the card offers a judgment nothing can make,
  // and says so.
  const onAt = backend.calls.length;
  await pageA.selectOption(`#typesafe-${JEV_SEATS[0].id}-select`, "on");
  await backend.waitForCall("A", "set_jev_mode", onAt);
  await pageA.waitForFunction(() => !document.querySelector("[data-jev-unreachable]").hidden, null, { timeout: UI_TIMEOUT });
  assertEqual(
    (await unreachable.textContent()).trim(),
    await said("settings.typesafe.routingUnreachable", "지금 분류 방식이 모델에게 묻지 않아 이 기능은 판단을 요청하지 않습니다. 위의 「라우팅 분류기」를 모델에게도 묻는 방식으로 바꾸세요."),
    "the row does not say why its mode cannot be reached",
  );

  // Put the probing word back and the warning goes with it.
  const probing = CLASSIFIER_MODES.at(-1).mode;
  const backAt = backend.calls.length;
  await pageA.selectOption("#route-classifier-select", probing);
  await backend.waitForCall("A", "set_route_classifier", backAt);
  await pageA.waitForFunction(() => document.querySelector("[data-jev-unreachable]").hidden, null, { timeout: UI_TIMEOUT });
  assertEqual(
    await statusSaid(pageA, "typesafe-status"),
    await said("settings.classifier.nowProbes", "이제 모델에게도 묻습니다 — 「모델 선택 판단」도 이제 판단을 요청할 수 있습니다."),
  );

  // A refused change keeps the card as it stood.
  backend.typesafeSetFailure = true;
  try {
    const refusedAt = backend.calls.length;
    await pageA.selectOption("#route-classifier-select", quiet);
    await backend.waitForCall("A", "set_route_classifier", refusedAt);
    await pageA.waitForFunction(
      (word) => document.getElementById("route-classifier-select")?.value === word,
      probing,
      { timeout: UI_TIMEOUT },
    );
    assertEqual(backend.zoSettings.smart.autoClassifier, probing, "a refused change mutated the backend");
  } finally {
    delete backend.typesafeSetFailure;
  }

  backend.keychain.delete(TYPESAFE_SERVICE);
  delete backend.zoSettings.smart;
  await pageA.evaluate(() => refreshApiRouters());
  return `${CLASSIFIER_MODES.length} words · gate on ${JEV_SEATS[0].id}`;
});

await test("SSH targets are added by parsing what was typed, edited back, and removed on confirmation", async () => {
  // 1-g55a is the CRUD only: no target here can be connected to, so what this
  // proves is that the list, the form and the removal agree with the store.
  const openedAt = backend.calls.length;
  await openSettings(pageA, "ssh");
  await backend.waitForCall("A", "ssh_targets", openedAt);
  assert(
    await pageA.locator('.settings-pane[data-pane="ssh"]').isVisible(),
    "the SSH pane did not open from the rail",
  );
  assertEqual(
    await pageA.locator('.settings-rail-item[data-pane="ssh"]').getAttribute("aria-current"),
    "page",
    "the SSH pane did not own rail selection",
  );
  assert(
    await pageA.locator("#ssh-target-empty").isVisible(),
    "an empty list did not say it was empty",
  );
  // The pane already read `~/.ssh/config` on the way in — there is none here
  // — and 가져오기 is that same read with the amnesty on, so it is live.
  assert(await pageA.locator("#ssh-target-import").isEnabled(), "가져오기 was drawn refused");

  await pageA.locator("#ssh-target-add").click();
  await pageA.locator("#ssh-target-scrim").waitFor({ state: "visible", timeout: UI_TIMEOUT });

  // Every shape Orca's own blur handler reads, asked of the pure function
  // rather than through five round trips of the form. The `@` split is the
  // one with a trap in it: a username may contain one, so the LAST wins.
  assertEqual(
    await pageA.evaluate(() => [
      "ssh://deploy@server:2222",
      "ssh://server",
      "ssh://server:99999",
      "de@ploy@server:2222",
      "server:2222",
      "server",
      "[::1]:2222",
    ].map((typed) => {
      const parsed = parseSshHostInput(typed);
      return [typed, parsed.host, parsed.username, parsed.port, parsed.error];
    })),
    [
      ["ssh://deploy@server:2222", "server", "deploy", 2222, null],
      ["ssh://server", "server", null, null, null],
      ["ssh://server:99999", "ssh://server:99999", null, null, "url"],
      ["de@ploy@server:2222", "server", "de@ploy", 2222, null],
      ["server:2222", "server", null, 2222, null],
      ["server", "server", null, null, null],
      ["[::1]:2222", "::1", null, 2222, null],
    ],
    "the host field stopped reading what people actually type",
  );

  // A port already chosen outranks one arriving in a paste: the draft is
  // overwritten only while it is the default nobody picked.
  await blurField(pageA, "#ssh-target-port", "2200");
  await blurField(pageA, "#ssh-target-host", "other:2222");
  assertEqual(
    await pageA.locator("#ssh-target-port").inputValue(),
    "2200",
    "a pasted port overwrote one that was typed",
  );
  await pageA.fill("#ssh-target-port", "22");

  // The whole endpoint typed into the host field, which is how a person who
  // already knows the machine writes it. The blur splits it into the three
  // fields the store keeps.
  await blurField(pageA, "#ssh-target-host", "deploy@server:2222");
  assertEqual(await pageA.locator("#ssh-target-host").inputValue(), "server", "host after parse");
  assertEqual(await pageA.locator("#ssh-target-username").inputValue(), "deploy", "user after parse");
  assertEqual(await pageA.locator("#ssh-target-port").inputValue(), "2222", "port after parse");
  await pageA.fill("#ssh-target-label", "Build box");

  const savedAt = backend.calls.length;
  await pageA.locator("#ssh-target-save").click();
  const saved = await backend.waitForCall("A", "ssh_save_target", savedAt);
  assertEqual(
    {
      id: saved.args.target.id,
      host: saved.args.target.host,
      username: saved.args.target.username,
      port: saved.args.target.port,
      label: saved.args.target.label,
      source: saved.args.target.source,
      keepAlive: saved.args.target.relayKeepAliveUntilReset,
      grace: saved.args.target.relayGracePeriodSeconds,
    },
    {
      id: "",
      host: "server",
      username: "deploy",
      port: 2222,
      label: "Build box",
      source: "manual",
      keepAlive: true,
      grace: 0,
    },
    "the saved target did not carry what the form parsed",
  );
  await pageA.locator("#ssh-target-scrim").waitFor({ state: "hidden", timeout: UI_TIMEOUT });

  const targetId = backend.sshTargets[0].id;
  const card = pageA.locator(`#ssh-target-list [data-target-id="${targetId}"]`);
  await card.waitFor({ state: "visible", timeout: UI_TIMEOUT });
  assertEqual(
    await card.locator(".ssh-target-label").textContent(),
    "Build box",
    "the card did not carry the label",
  );
  assertEqual(
    await card.locator(".ssh-target-endpoint").textContent(),
    "deploy@server:2222",
    "the card did not carry where the target goes",
  );
  assert(await pageA.locator("#ssh-target-empty").isHidden(), "the empty state outlived the list");

  // Editing reopens the SAME row rather than starting a second one.
  await card.locator(".ssh-target-edit").click();
  await pageA.locator("#ssh-target-scrim").waitFor({ state: "visible", timeout: UI_TIMEOUT });
  assertEqual(
    {
      id: await pageA.locator("#ssh-target-id").inputValue(),
      host: await pageA.locator("#ssh-target-host").inputValue(),
      user: await pageA.locator("#ssh-target-username").inputValue(),
      port: await pageA.locator("#ssh-target-port").inputValue(),
      label: await pageA.locator("#ssh-target-label").inputValue(),
    },
    { id: targetId, host: "server", user: "deploy", port: "2222", label: "Build box" },
    "editing opened an empty form instead of the saved target",
  );
  await pageA.locator("#ssh-target-cancel").click();
  await pageA.locator("#ssh-target-scrim").waitFor({ state: "hidden", timeout: UI_TIMEOUT });

  // A removal is asked before it happens, and the question is what commits it.
  const removedAt = backend.calls.length;
  await card.locator(".ssh-target-remove").click();
  await pageA.locator("#ask-scrim").waitFor({ state: "visible", timeout: UI_TIMEOUT });
  assertEqual(
    backend.count("A", "ssh_remove_target"),
    0,
    "the target went before the question was answered",
  );
  await pageA.locator("#ask-yes").click();
  const removed = await backend.waitForCall("A", "ssh_remove_target", removedAt);
  assertEqual(removed.args, { id: targetId }, "the removal did not name the saved target");
  await pageA.locator("#ssh-target-empty").waitFor({ state: "visible", timeout: UI_TIMEOUT });
  assertEqual(backend.sshTargets.length, 0, "the store kept a removed target");
  return "add · parse · card · edit · remove";
});

await test("the SSH pane reads ~/.ssh/config on its own, and 가져오기 forgives what it deleted", async () => {
  // The list has two authors, and this is the other one. What the file may
  // rewrite — and what it may never touch — is Rust's rule and the Rust tests
  // own it; what this proves is the window's half: the pass that runs unasked
  // says nothing, the button asks for the amnesty and reports either way.
  backend.sshConfigHosts = [
    { alias: "web", hostname: "10.0.0.5", username: "deploy", port: 2222 },
  ];
  const clearToasts = () => pageA.evaluate(() => {
    for (const note of document.querySelectorAll(".toasts .toast")) note.remove();
  });
  const toastTexts = () => pageA.evaluate(() =>
    [...document.querySelectorAll(".toasts .toast")].map((note) => note.textContent));
  // Read from the catalog in force rather than written out here: this suite
  // switches locales, and the words are the catalog's either way.
  const words = (key, korean, vars) =>
    pageA.evaluate(([asked, fallback, held]) => t(asked, fallback, held), [key, korean, vars]);

  await clearToasts();
  const openedAt = backend.calls.length;
  // Reopened rather than shown: the pane is already the one on screen, and
  // the read that matters is the one ARRIVING on it does.
  await reopenSettings(pageA, "A", "ssh");
  const silent = await backend.waitForCall("A", "ssh_import_config", openedAt);
  assertEqual(silent.args, { reAdopt: false }, "the pass nobody asked for asked for the amnesty");

  // It found a machine, so the list is re-read and the card is simply there.
  const card = pageA.locator("#ssh-target-list .ssh-target-row").first();
  await card.waitFor({ state: "visible", timeout: UI_TIMEOUT });
  assertEqual(
    {
      label: await card.locator(".ssh-target-label").textContent(),
      endpoint: await card.locator(".ssh-target-endpoint").textContent(),
      source: backend.sshTargets[0].source,
    },
    { label: "deploy@web", endpoint: "deploy@web:2222", source: "ssh-config" },
    "the card the file wrote is not the machine it describes",
  );
  assertEqual(await toastTexts(), [], "the silent sync announced itself");

  // 가져오기 with nothing new in the file: it still answers, because somebody
  // pressed it.
  const syncedAt = backend.calls.length;
  await pageA.locator("#ssh-target-import").click();
  const readopt = await backend.waitForCall("A", "ssh_import_config", syncedAt);
  assertEqual(readopt.args, { reAdopt: true }, "the button did not ask for the amnesty");
  await pageA.locator(".toasts .toast").first().waitFor({ state: "visible", timeout: UI_TIMEOUT });
  assertEqual(
    await toastTexts(),
    [await words("settings.ssh.importSynced", "~/.ssh/config와 이미 동기화되어 있습니다")],
    "an import that moved nothing claimed work",
  );
  await clearToasts();

  // A deletion is remembered: the pass that runs on the next visit leaves it
  // deleted rather than painting it back within the second.
  const removedAt = backend.calls.length;
  await card.locator(".ssh-target-remove").click();
  await pageA.locator("#ask-scrim").waitFor({ state: "visible", timeout: UI_TIMEOUT });
  await pageA.locator("#ask-yes").click();
  await backend.waitForCall("A", "ssh_remove_target", removedAt);
  await pageA.locator("#ssh-target-empty").waitFor({ state: "visible", timeout: UI_TIMEOUT });
  const reopenedAt = backend.calls.length;
  await reopenSettings(pageA, "A", "ssh");
  await backend.waitForCall("A", "ssh_import_config", reopenedAt);
  assertEqual(backend.sshTargets.length, 0, "a machine somebody deleted came straight back");
  assertEqual(await toastTexts(), [], "the silent sync announced itself");

  // And 가져오기 is the amnesty: the alias is forgiven and the card returns,
  // counted.
  const forgivenAt = backend.calls.length;
  await pageA.locator("#ssh-target-import").click();
  await backend.waitForCall("A", "ssh_import_config", forgivenAt);
  await card.waitFor({ state: "visible", timeout: UI_TIMEOUT });
  assertEqual(
    await toastTexts(),
    [await words("settings.ssh.importSyncedN", "{{n}}개 서버를 동기화했습니다", { n: 1 })],
    "the re-adoption did not say how many machines it synced",
  );
  await clearToasts();

  // A failure of the read is the button's to report and the silent pass's to
  // swallow — nobody asked that one to run.
  backend.rejectOnce("ssh_import_config");
  const refusedAt = backend.calls.length;
  await pageA.locator("#ssh-target-import").click();
  await backend.waitForCall("A", "ssh_import_config", refusedAt);
  await pageA.locator(".toasts .toast").first().waitFor({ state: "visible", timeout: UI_TIMEOUT });
  assertEqual(
    await toastTexts(),
    [await words("settings.ssh.importFailed", "가져오기에 실패했습니다")],
    "a failed import said nothing",
  );
  await clearToasts();

  // Taken back out so the tests after this one open onto the list they wrote.
  backend.sshConfigHosts = [];
  backend.sshSuppressed = [];
  backend.sshTargets = [];
  return "silent pass · 이미 동기화 · 삭제 기억 · 사면 · 실패";
});


await test("the Test button asks ssh once and the card keeps the answer", async () => {
  // 1-g55c's first slice: a probe, not a connection. The dot stays where it
  // was — measured, Orca's bare test suppresses lifecycle broadcasts — so
  // what the button learns is a line under the endpoint, on the one card
  // that asked.
  backend.sshTargets = [{
    id: "ssh-probe1", label: "Build box", host: "server", configHost: "",
    port: 22, username: "deploy", source: "manual",
  }];
  backend.sshConfigHosts = [];
  backend.sshSuppressed = [];
  // The pane is standing from the test before this one; the list is read on
  // the way IN, so leave and come back.
  await pageA.evaluate(() => setSettingsOpen(false));
  const openedAt = backend.calls.length;
  await openSettings(pageA, "ssh");
  await backend.waitForCall("A", "ssh_targets", openedAt);
  const card = pageA.locator('#ssh-target-list [data-target-id="ssh-probe1"]');
  await card.waitFor({ state: "visible", timeout: UI_TIMEOUT });

  // While the probe is out the card says so, and the button will not stack a
  // second question on the first.
  const reached = backend.holdNext("ssh_probe_target");
  const askedAt = backend.calls.length;
  await card.locator(".ssh-target-test").click();
  await reached;
  await card.locator('.ssh-target-probe[data-tone="wait"]')
    .waitFor({ state: "visible", timeout: UI_TIMEOUT });
  assert(
    await card.locator(".ssh-target-test").isDisabled(),
    "a second probe could stack on the first",
  );
  backend.release("ssh_probe_target");
  const asked = await backend.waitForCall("A", "ssh_probe_target", askedAt);
  assertEqual(asked.args, { id: "ssh-probe1" }, "the probe did not name the card's own target");
  await card.locator('.ssh-target-probe[data-tone="ready"]')
    .waitFor({ state: "visible", timeout: UI_TIMEOUT });

  // A test is not a connection: the state dot did not move.
  assertEqual(
    await card.locator(".ssh-target-dot").getAttribute("data-state"),
    "disconnected",
    "a bare probe moved the lifecycle dot",
  );

  // A refusal reads as ssh's own sentence, on the card rather than a toast.
  backend.rejectNext.add("ssh_probe_target");
  await card.locator(".ssh-target-test").click();
  await card.locator('.ssh-target-probe[data-tone="halt"]')
    .waitFor({ state: "visible", timeout: UI_TIMEOUT });
  assert(
    (await card.locator(".ssh-target-probe").textContent()).includes("refused: ssh_probe_target"),
    "the refusal lost ssh's own sentence",
  );

  backend.sshTargets = [];
  return "probe · wait · answer · refusal";
});

await test("Connect walks the card through the lifecycle and Disconnect brings it home", async () => {
  // 1-g55c-2: the first five of Orca's eight states, worn by the card. The
  // face is measured: a dial under way offers only Edit/Remove, a standing
  // connection offers Disconnect, everything else may Test or Connect.
  backend.sshTargets = [{
    id: "ssh-link1", label: "Build box", host: "server", configHost: "",
    port: 22, username: "deploy", source: "manual",
  }];
  backend.sshConfigHosts = [];
  await pageA.evaluate(() => setSettingsOpen(false));
  const openedAt = backend.calls.length;
  await openSettings(pageA, "ssh");
  await backend.waitForCall("A", "ssh_targets", openedAt);
  const card = pageA.locator('#ssh-target-list [data-target-id="ssh-link1"]');
  await card.waitFor({ state: "visible", timeout: UI_TIMEOUT });
  assertEqual(
    await card.locator(".ssh-target-dot").getAttribute("data-state"),
    "disconnected",
    "a fresh card did not start disconnected",
  );
  assertEqual(
    await card.locator(".ssh-target-term").count(),
    0,
    "a remote terminal was offered before the connection stood",
  );

  // While the dial is out: the dot says connecting and the face folds to
  // Edit/Remove alone.
  const reached = backend.holdNext("ssh_link_connect");
  await card.locator(".ssh-target-connect").click();
  await reached;
  await card.locator('.ssh-target-dot[data-state="connecting"]')
    .waitFor({ state: "visible", timeout: UI_TIMEOUT });
  assertEqual(await card.locator(".ssh-target-connect").count(), 0, "Connect stayed offered mid-dial");
  assertEqual(await card.locator(".ssh-target-test").count(), 0, "Test stayed offered mid-dial");
  backend.release("ssh_link_connect");
  await card.locator('.ssh-target-dot[data-state="connected"]')
    .waitFor({ state: "visible", timeout: UI_TIMEOUT });
  await card.locator(".ssh-target-disconnect").waitFor({ state: "visible", timeout: UI_TIMEOUT });

  // A standing connection offers a terminal ON the target, and the ask names
  // the card's own id.
  const termAskedAt = backend.calls.length;
  await card.locator(".ssh-target-term").click();
  const termAsked = await backend.waitForCall("A", "ssh_open_remote_term", termAskedAt);
  assertEqual(termAsked.args.id, "ssh-link1", "the remote terminal did not name its target");
  // The sheet stepped aside for the tab; reopen it to keep walking the card.
  await openSettings(pageA, "ssh");
  await card.waitFor({ state: "visible", timeout: UI_TIMEOUT });

  // Down again, from the card, and the resting face returns.
  await card.locator(".ssh-target-disconnect").click();
  await card.locator('.ssh-target-dot[data-state="disconnected"]')
    .waitFor({ state: "visible", timeout: UI_TIMEOUT });
  await card.locator(".ssh-target-connect").waitFor({ state: "visible", timeout: UI_TIMEOUT });

  // A fresh dial retires the old probe note: prove there IS one first.
  await card.locator(".ssh-target-test").click();
  await card.locator('.ssh-target-probe[data-tone="ready"]')
    .waitFor({ state: "visible", timeout: UI_TIMEOUT });

  // A turned-away key is its own state, and the sentence lands on the card —
  // not the stale "연결 성공" the probe just wrote.
  backend.sshLinkNext = {
    state: "auth-failed",
    error: "인증에 실패했습니다 — 사용자 이름과 키를 확인하세요",
  };
  await card.locator(".ssh-target-connect").click();
  await card.locator('.ssh-target-dot[data-state="auth-failed"]')
    .waitFor({ state: "visible", timeout: UI_TIMEOUT });
  assert(
    (await card.locator(".ssh-target-probe").textContent()).includes("인증에 실패했습니다"),
    "the refusal sentence did not land on the card",
  );

  backend.sshTargets = [];
  return "connecting face · connected · disconnect · auth-failed";
});

await test("a falling link climbs the ladder on the card and lands on its last word", async () => {
  // 1-g55c-3: reconnecting and reconnection-failed, driven purely by the
  // event stream — the ladder itself is Rust's and Rust-tested; what this
  // proves is the card wearing each move, attempt count included.
  backend.sshTargets = [{
    id: "ssh-lad1", label: "Build box", host: "server", configHost: "",
    port: 22, username: "deploy", source: "manual",
  }];
  backend.sshConfigHosts = [];
  await pageA.evaluate(() => setSettingsOpen(false));
  const openedAt = backend.calls.length;
  await openSettings(pageA, "ssh");
  await backend.waitForCall("A", "ssh_targets", openedAt);
  const card = pageA.locator('#ssh-target-list [data-target-id="ssh-lad1"]');
  await card.waitFor({ state: "visible", timeout: UI_TIMEOUT });

  // Mid-climb: the dot pulses, the count rides along, and the face folds to
  // Edit/Remove — no Connect, no Disconnect, exactly the measured folding.
  await pageA.evaluate(() => window.__SETTINGS_TEST_EMIT__("ssh:link-changed",
    { id: "ssh-lad1", state: "reconnecting", attempt: 3 }));
  await card.locator('.ssh-target-dot[data-state="reconnecting"]')
    .waitFor({ state: "visible", timeout: UI_TIMEOUT });
  assert(
    (await card.locator(".ssh-target-state").textContent()).includes("(3/9)"),
    "the attempt count did not ride the state word",
  );
  assertEqual(await card.locator(".ssh-target-connect").count(), 0, "Connect stayed offered mid-climb");
  assertEqual(await card.locator(".ssh-target-disconnect").count(), 0, "Disconnect appeared mid-climb");

  // The ladder ran out: its own state, its sentence, and the road back open.
  await pageA.evaluate(() => window.__SETTINGS_TEST_EMIT__("ssh:link-changed",
    { id: "ssh-lad1", state: "reconnection-failed", error: "재연결 시도 한도에 도달했습니다" }));
  await card.locator('.ssh-target-dot[data-state="reconnection-failed"]')
    .waitFor({ state: "visible", timeout: UI_TIMEOUT });
  assert(
    (await card.locator(".ssh-target-probe").textContent()).includes("한도"),
    "the last word did not land on the card",
  );
  await card.locator(".ssh-target-connect").waitFor({ state: "visible", timeout: UI_TIMEOUT });

  backend.sshTargets = [];
  return "reconnecting (3/9) · folded face · reconnection-failed · road back";
});
await test("SSH Hosts verifies the server key and drives one native-backed remote terminal flow", async () => {
  const secret = "fixture-password-never-rendered";
  const openedAt = backend.calls.length;
  await openSettings(pageA, "ssh-hosts");
  await backend.waitForCall("A", "ssh_hosts", openedAt);
  await pageA.locator("#ssh-host-add").click();
  await pageA.locator("#ssh-host-label").fill("Build box");
  await pageA.locator("#ssh-host-address").fill("build.example.test");
  await pageA.locator("#ssh-host-user").fill("builder");
  await pageA.locator("#ssh-host-port").fill("2222");
  await pageA.locator('#ssh-host-authentication [data-value="password"]').click();
  await pageA.locator("#ssh-host-password").fill(secret);

  const probeReached = backend.holdNext("probe_ssh_host");
  const firstProbeAt = backend.calls.length;
  await pageA.locator("#ssh-host-probe").click();
  const firstProbe = await backend.waitForCall("A", "probe_ssh_host", firstProbeAt);
  await probeReached;
  assertEqual(
    firstProbe.args.input,
    { host: "build.example.test", port: 2222, user: "builder" },
    "host-key discovery did not use the exact endpoint",
  );
  await pageA.locator("#ssh-host-address").fill("other.example.test");
  assertEqual(await pageA.locator("#ssh-host-fingerprint").inputValue(), "", "stale key survived an endpoint edit");
  assertEqual(
    await pageA.locator("#ssh-host-probe").isDisabled(),
    false,
    "cancelling an in-flight key probe left the dialog stuck",
  );
  backend.release("probe_ssh_host");
  await pageA.locator("#ssh-host-address").fill("build.example.test");
  await pageA.locator("#ssh-host-probe").click();
  await pageA.waitForFunction(() =>
    document.querySelector("#ssh-host-fingerprint")?.value === "SHA256:fixture-host-key");
  assertEqual(
    await pageA.locator("#ssh-host-save").isDisabled(),
    false,
    "a confirmed endpoint was not saveable",
  );

  // A key belongs to one exact endpoint. Editing that endpoint after a
  // successful probe must also make the old confirmation unusable.
  await pageA.locator("#ssh-host-address").fill("other.example.test");
  assertEqual(await pageA.locator("#ssh-host-fingerprint").inputValue(), "", "stale key survived an endpoint edit");
  assert(await pageA.locator("#ssh-host-save").isDisabled(), "stale key still enabled Save");
  await pageA.locator("#ssh-host-address").fill("build.example.test");
  await pageA.locator("#ssh-host-probe").click();
  await pageA.waitForFunction(() =>
    document.querySelector("#ssh-host-fingerprint")?.value === "SHA256:fixture-host-key");

  backend.loseAcknowledgementOnce("save_ssh_host");
  const savedAt = backend.calls.length;
  await pageA.locator("#ssh-host-save").click();
  const saved = await backend.waitForCall("A", "save_ssh_host", savedAt);
  await backend.waitForCall("A", "ssh_hosts", saved.at + 1);
  assertEqual(saved.args.input.password, secret, "the password did not cross the one save boundary");
  await pageA.waitForSelector('#ssh-host-list [data-host-id="00000000-0000-4000-8000-000000000001"]');
  await pageA.locator("#ssh-host-scrim").waitFor({ state: "hidden" });
  assert(await pageA.locator("#ssh-host-error").isHidden(), "a recovered save reported a false failure");
  assert(
    !JSON.stringify(backend.sshHosts).includes(secret),
    "the SSH report persisted a password",
  );
  assert(
    !(await pageA.locator("body").innerText()).includes(secret),
    "the password reached rendered text",
  );

  const testedAt = backend.calls.length;
  await pageA.locator("#ssh-host-list .ssh-host-test").click();
  await backend.waitForCall("A", "test_ssh_host", testedAt);
  await pageA.waitForFunction(() =>
    document.querySelector("#ssh-host-list")?.textContent.includes("연결됨"));

  const terminalAt = backend.calls.length;
  await pageA.locator("#ssh-host-list .ssh-host-open").click();
  const terminal = await backend.waitForCall("A", "open_ssh_terminal", terminalAt);
  assertEqual(
    terminal.args,
    { id: "00000000-0000-4000-8000-000000000001", rows: 24, cols: 96 },
    "the terminal did not use the saved host identity and canonical grid",
  );
  await pageA.waitForFunction(() => currentTab()?.remoteHost === "00000000-0000-4000-8000-000000000001");
  const hostTerminalSurface = await pageA.evaluate(() => {
    const current = currentTab();
    const visible = activeTabIn(focusedPane);
    return {
      owner: current?.worktree,
      local: activeWorktreePath,
      current: current?.id,
      visible: visible?.id,
      keyboard: keyboardTarget(),
      pane: visible ? activePaneOf(visible) : null,
    };
  });
  assertEqual(
    hostTerminalSurface.owner,
    hostTerminalSurface.local,
    "the remote terminal was filed outside the workspace displaying it",
  );
  assertEqual(
    hostTerminalSurface.current,
    hostTerminalSurface.visible,
    "the remote terminal became active without becoming visible",
  );
  assertEqual(
    hostTerminalSurface.keyboard,
    { kind: "term", term: hostTerminalSurface.pane },
    "the keyboard target did not match the visible remote terminal pane",
  );
  const remoteTermWasPersisted = (after, term) => backend.calls
    .slice(after + 1)
    .filter((call) => call.window_id === "A" && call.command === "save_pane_layouts")
    .some((call) => (call.args.layouts ?? []).some((layout) =>
      Object.values(layout.terms ?? {}).includes(term)));
  assert(
    !remoteTermWasPersisted(terminal.at, hostTerminalSurface.pane),
    "the remote terminal was persisted as a restorable local shell",
  );

  const reopenedAt = backend.calls.length;
  await openSettings(pageA, "ssh-hosts");
  await backend.waitForCall("A", "ssh_hosts", reopenedAt);
  await backend.waitForCall("A", "remote_workspaces", reopenedAt);

  await pageA.locator("#remote-workspace-add").click();
  // 이 칸의 select는 형제 input과 같은 얼굴이어야 한다. 오래 `settings-select`를
  // 입고 있었는데 이 창의 어떤 스타일시트도 그 이름을 선언하지 않는다 — 밑줄도
  // 얼굴도 없는 네이티브 셀렉트로 서고, 그 옆에는 `.settings-field:has(select)`가
  // 그린 셰브런만 떠 있었다. 재는 것은 클래스 목록이 아니라 그려진 결과다.
  assertEqual(
    await pageA.evaluate(() => {
      const face = (id) => {
        const paint = getComputedStyle(document.getElementById(id));
        return [
          paint.borderBottomWidth,
          paint.borderBottomColor,
          paint.backgroundColor,
          paint.fontFamily,
        ].join(" | ");
      };
      return { same: face("remote-workspace-host") === face("remote-workspace-label") };
    }),
    { same: true },
    "the host select is not wearing the field face its siblings wear",
  );
  await pageA.locator("#remote-workspace-label").fill("Project checkout");
  await pageA.locator("#remote-workspace-root").fill("/srv/project-link");
  const rootProbeAt = backend.calls.length;
  await pageA.locator("#remote-workspace-probe").click();
  const rootProbe = await backend.waitForCall("A", "probe_remote_workspace", rootProbeAt);
  assertEqual(
    rootProbe.args.input,
    {
      hostId: "00000000-0000-4000-8000-000000000001",
      root: "/srv/project-link",
    },
    "the workspace probe did not use its stable host identity and exact root",
  );
  await pageA.waitForFunction(() =>
    document.querySelector("#remote-workspace-root")?.value === "/srv/project");

  backend.loseAcknowledgementOnce("save_remote_workspace");
  const workspaceSavedAt = backend.calls.length;
  await pageA.locator("#remote-workspace-save").click();
  const workspaceSaved = await backend.waitForCall(
    "A",
    "save_remote_workspace",
    workspaceSavedAt,
  );
  await backend.waitForCall("A", "remote_workspaces", workspaceSaved.at + 1);
  assertEqual(
    workspaceSaved.args.input,
    {
      id: null,
      label: "Project checkout",
      hostId: "00000000-0000-4000-8000-000000000001",
      root: "/srv/project",
    },
    "the workspace did not persist the verified canonical root",
  );
  await pageA.waitForSelector(
    '#remote-workspace-list [data-workspace-id="10000000-0000-4000-8000-000000000001"]',
  );
  await pageA.locator("#remote-workspace-scrim").waitFor({ state: "hidden" });
  assert(
    await pageA.locator("#remote-workspaces-error").isHidden(),
    "a recovered workspace save reported a false failure",
  );

  const workspaceTestedAt = backend.calls.length;
  await pageA.locator("#remote-workspace-list .remote-workspace-test").click();
  await backend.waitForCall("A", "test_remote_workspace", workspaceTestedAt);
  await pageA.waitForFunction(() =>
    document.querySelector("#remote-workspace-list")?.textContent.includes("연결됨"));

  const workspaceTerminalAt = backend.calls.length;
  await pageA.locator("#remote-workspace-list .remote-workspace-open").click();
  const workspaceTerminal = await backend.waitForCall(
    "A",
    "open_remote_workspace_terminal",
    workspaceTerminalAt,
  );
  assertEqual(
    workspaceTerminal.args,
    { id: "10000000-0000-4000-8000-000000000001", rows: 24, cols: 96 },
    "the remote workspace did not open by its persisted identity",
  );
  await pageA.waitForFunction(() =>
    currentTab()?.remoteWorkspace === "10000000-0000-4000-8000-000000000001");
  const workspaceTerminalSurface = await pageA.evaluate(() => {
    const current = currentTab();
    const visible = activeTabIn(focusedPane);
    return {
      owner: current?.worktree,
      local: activeWorktreePath,
      current: current?.id,
      visible: visible?.id,
      keyboard: keyboardTarget(),
      pane: visible ? activePaneOf(visible) : null,
    };
  });
  assertEqual(
    workspaceTerminalSurface.owner,
    workspaceTerminalSurface.local,
    "the remote workspace terminal was filed outside the workspace displaying it",
  );
  assertEqual(
    workspaceTerminalSurface.current,
    workspaceTerminalSurface.visible,
    "the remote workspace terminal became active without becoming visible",
  );
  assertEqual(
    workspaceTerminalSurface.keyboard,
    { kind: "term", term: workspaceTerminalSurface.pane },
    "the keyboard target did not match the visible remote workspace pane",
  );
  assert(
    !remoteTermWasPersisted(workspaceTerminal.at, workspaceTerminalSurface.pane),
    "the remote workspace terminal was persisted as a restorable local shell",
  );

  const workspaceReopenedAt = backend.calls.length;
  await openSettings(pageA, "ssh-hosts");
  await backend.waitForCall("A", "remote_workspaces", workspaceReopenedAt);
  backend.loseAcknowledgementOnce("remove_remote_workspace");
  const workspaceRemovedAt = backend.calls.length;
  await pageA.locator("#remote-workspace-list .remote-workspace-remove").click();
  const workspaceRemoved = await backend.waitForCall(
    "A",
    "remove_remote_workspace",
    workspaceRemovedAt,
  );
  await backend.waitForCall("A", "remote_workspaces", workspaceRemoved.at + 1);
  await pageA.waitForFunction(() =>
    document.querySelectorAll("#remote-workspace-list [data-workspace-id]").length === 0);

  backend.loseAcknowledgementOnce("remove_ssh_host");
  const removedAt = backend.calls.length;
  await pageA.locator("#ssh-host-list .ssh-host-remove").click();
  const removed = await backend.waitForCall("A", "remove_ssh_host", removedAt);
  await backend.waitForCall("A", "ssh_hosts", removed.at + 1);
  await pageA.waitForFunction(() => document.querySelectorAll("#ssh-host-list [data-host-id]").length === 0);
  assert(await pageA.locator("#ssh-host-error").isHidden(), "a recovered removal reported a false failure");
  return "host pin/vault + SFTP canonical workspace + two real remote PTY roots";
});

await test("Remote Servers pairs a tunnel, hides its bearer, and opens one persistent session", async () => {
  const accessLink = "zerocode://pair?endpoint=127.0.0.1%3A43123&token=fixture-bearer-never-rendered";
  const openedAt = backend.calls.length;
  await openSettings(pageA, "remote-servers");
  await backend.waitForCall("A", "remote_servers", openedAt);
  await pageA.locator("#remote-server-add").click();
  await pageA.locator("#remote-server-name").fill("Build runtime");
  await pageA.locator("#remote-server-access-link").fill(accessLink);

  backend.loseAcknowledgementOnce("save_remote_server");
  const savedAt = backend.calls.length;
  await pageA.locator("#remote-server-save").click();
  const saved = await backend.waitForCall("A", "save_remote_server", savedAt);
  await backend.waitForCall("A", "remote_servers", saved.at + 1);
  assertEqual(
    saved.args.input,
    { id: null, name: "Build runtime", accessLink },
    "pairing did not cross the one native access-link boundary",
  );
  await pageA.waitForSelector(
    '#remote-server-list [data-server-id="20000000-0000-4000-8000-000000000001"]',
  );
  await pageA.locator("#remote-server-scrim").waitFor({ state: "hidden" });
  assert(await pageA.locator("#remote-servers-error").isHidden(), "a recovered pair reported a false failure");
  assertEqual(
    await pageA.locator("#remote-server-access-link").inputValue(),
    "",
    "the access link survived dialog close",
  );
  assert(
    !JSON.stringify(backend.remoteServers).includes("fixture-bearer-never-rendered"),
    "the remote server report persisted its bearer",
  );
  assert(
    !(await pageA.locator("body").innerText()).includes("fixture-bearer-never-rendered"),
    "the bearer reached rendered text",
  );

  const testedAt = backend.calls.length;
  await pageA.locator("#remote-server-list .remote-server-test").click();
  await backend.waitForCall("A", "test_remote_server", testedAt);
  await pageA.waitForFunction(() =>
    document.querySelector("#remote-server-list")?.textContent.includes("연결됨"));

  const sessionAt = backend.calls.length;
  await pageA.locator("#remote-server-list .remote-server-open").click();
  const session = await backend.waitForCall("A", "open_remote_server_session", sessionAt);
  assertEqual(
    session.args,
    { id: "20000000-0000-4000-8000-000000000001", rows: 24, cols: 96 },
    "the persistent session did not open by its saved server identity",
  );
  await pageA.waitForFunction(() =>
    currentTab()?.remoteServer === "20000000-0000-4000-8000-000000000001");
  assertEqual(
    await pageA.evaluate(() => currentTab()?.worktree),
    "zerocode://127.0.0.1:43123",
    "the tunneled server session was labelled as a local workspace",
  );

  const reopenedAt = backend.calls.length;
  await openSettings(pageA, "remote-servers");
  await backend.waitForCall("A", "remote_servers", reopenedAt);
  backend.loseAcknowledgementOnce("remove_remote_server");
  const removedAt = backend.calls.length;
  await pageA.locator("#remote-server-list .remote-server-remove").click();
  const removed = await backend.waitForCall("A", "remove_remote_server", removedAt);
  await backend.waitForCall("A", "remote_servers", removed.at + 1);
  await pageA.waitForFunction(() =>
    document.querySelectorAll("#remote-server-list [data-server-id]").length === 0);
  assert(await pageA.locator("#remote-servers-error").isHidden(), "a recovered removal reported a false failure");
  return "native bearer isolation + exact loopback identity + persistent zo attach";
});

await test("macOS Permissions exposes one closed native inventory and a private-LAN-only test", async () => {
  if (!await pageA.evaluate(() => usesCommandModifier)) {
    assert(
      await pageA.locator("#settings-security-group").isHidden(),
      "macOS Permissions was reachable on a non-Apple platform",
    );
    return "hidden off macOS; native command returns Unsupported";
  }
  const openedAt = backend.calls.length;
  await openSettings(pageA, "macos-permissions");
  await backend.waitForCall("A", "developer_permission_statuses", openedAt);
  await pageA.waitForFunction(() =>
    document.querySelectorAll("#developer-permission-list [data-permission-id]").length === 9);
  assertEqual(
    await pageA.locator("#developer-permission-list [data-permission-id]").evaluateAll(
      (rows) => rows.map((row) => row.dataset.permissionId),
    ),
    DEVELOPER_PERMISSION_STATES.map((row) => row.id),
    "the renderer permission inventory diverged from the native closed list",
  );

  /* t-5587: every row wears the status the native side answered, and the
   * summary counts the same nine — not a constant the page carries. */
  const SAID = {
    granted: "허용됨", denied: "거부됨", unknown: "알 수 없음", ready: "요청 가능",
  };
  assertEqual(
    await pageA.locator("#developer-permission-list .settings-state").evaluateAll(
      (rows) => rows.map((row) => row.textContent),
    ),
    DEVELOPER_PERMISSION_STATES.map((row) => SAID[row.status]),
    "a permission row said something other than what macOS answered",
  );
  assertEqual(
    await pageA.locator("#developer-permissions-summary").textContent(),
    "5 / 9 사용 가능",
    "the availability count was not recounted from the live answers",
  );

  /* The answered prompt is spent: macOS raises each dialog once per process
   * identity, so camera (granted) points at the pane while microphone
   * (nobody asked yet) still offers the dialog. */
  assertEqual(
    await pageA.locator('[data-permission-id="microphone"] button').textContent(),
    "권한 요청 표시",
    "an unasked capture permission stopped offering its macOS dialog",
  );
  assertEqual(
    await pageA.locator('[data-permission-id="camera"] button').textContent(),
    "시스템 설정 열기",
    "an answered capture permission offered a dialog macOS will not raise",
  );

  const promptAt = backend.calls.length;
  await pageA.locator('[data-permission-id="automation"] button').click();
  const prompt = await backend.waitForCall("A", "request_developer_permission", promptAt);
  assertEqual(prompt.args, { id: "automation" }, "automation did not use the closed prompt command");
  await backend.waitForCall("A", "developer_permission_statuses", prompt.at + 1);

  const settingsAt = backend.calls.length;
  await pageA.locator('[data-permission-id="camera"] button').click();
  const settings = await backend.waitForCall(
    "A",
    "open_developer_permission_settings",
    settingsAt,
  );
  assertEqual(settings.args, { id: "camera" }, "camera opened an arbitrary settings target");
  await backend.waitForCall("A", "developer_permission_statuses", settings.at + 1);

  await pageA.locator("#local-network-host").fill("127.0.0.1");
  await pageA.locator("#local-network-port").fill("43123");
  const localAt = backend.calls.length;
  await pageA.locator("#local-network-test").click();
  const local = await backend.waitForCall("A", "test_local_network_permission", localAt);
  assertEqual(
    local.args,
    { host: "127.0.0.1", port: 43123 },
    "the local test changed its exact target",
  );
  await pageA.waitForFunction(() =>
    document.querySelector("#local-network-test-status")?.textContent.includes("43123"));
  assert(
    (await pageA.locator('[data-permission-evidence="local-network"]').textContent())
      .startsWith("최근 연결 테스트 성공 · "),
    "the row with no status API did not quote the test that stands in for one",
  );
  assertEqual(
    await pageA.locator('[data-permission-id="local-network"] .settings-state').textContent(),
    "알 수 없음",
    "the connection test invented a tenth status word",
  );

  await pageA.locator("#local-network-host").fill("8.8.8.8");
  const publicAt = backend.calls.length;
  await pageA.locator("#local-network-test").click();
  const publicAttempt = await backend.waitForCall("A", "test_local_network_permission", publicAt);
  assertEqual(
    publicAttempt.args,
    { host: "8.8.8.8", port: 43123 },
    "the public-address denial was not enforced behind the webview boundary",
  );
  await pageA.waitForFunction(() =>
    document.querySelector("#local-network-test-status")?.textContent.includes("연결할 수 없습니다"));
  assert(
    (await pageA.locator('[data-permission-evidence="local-network"]').textContent())
      .startsWith("최근 연결 테스트 실패 · "),
    "a refused test left the row quoting a stale success",
  );

  const focusedAt = backend.calls.length;
  await pageA.evaluate(() => window.dispatchEvent(new Event("focus")));
  await backend.waitForCall("A", "developer_permission_statuses", focusedAt);
  return "9 native IDs + 5/9 counted from live answers + spent prompts open the pane";
});

await test("Quick Commands settings and the terminal menu mutate one repository", async () => {
  await pageA.evaluate(() => setSettingsOpen(false));
  const openedAt = backend.calls.length;
  await openSettings(pageA, "quick-commands");
  await backend.waitForCall("A", "list_quick_commands", openedAt);
  await pageA.waitForSelector("#settings-quick-list .settings-command-row");
  assertEqual(
    await pageA.locator("#settings-quick-list .settings-command-row").count(),
    2,
    "Settings did not paint the stored quick commands",
  );

  const deletedAt = backend.calls.length;
  await pageA.locator("#settings-quick-list .settings-command-delete").first().click();
  await backend.waitForCall("A", "delete_quick_command", deletedAt);
  await pageA.waitForFunction(() =>
    document.querySelectorAll("#settings-quick-list .settings-command-row").length === 1);

  await pageA.click("#settings-quick-add");
  assert(await pageA.locator("#qc-scrim").isVisible(), "Settings did not open the canonical composer");
  await pageA.fill("#qc-label", "Status");
  await pageA.fill("#qc-body", "git status --short");
  await pageA.selectOption("#qc-scope", "global");
  const savedAt = backend.calls.length;
  await pageA.click("#qc-save");
  await backend.waitForCall("A", "save_quick_command", savedAt);
  await pageA.waitForFunction(() =>
    [...document.querySelectorAll("#settings-quick-list .settings-command-name")]
      .some((node) => node.textContent === "Status"));
  assertEqual(backend.quickCommands.length, 2, "composer and Settings diverged");
  return "list, delete, canonical composer, refresh";
});

await test("Onboarding settings reopens the real wizard and Automations opens the real page", async () => {
  await openSettings(pageA, "onboarding");
  const reopenedAt = backend.calls.length;
  await pageA.click("#settings-onboarding-open");
  await backend.waitForCall("A", "reopen_onboarding", reopenedAt);
  assert(await pageA.locator("#settings-view").isHidden(), "wizard left Settings active underneath");
  assert(await pageA.locator("#onb-scrim").isVisible(), "the real onboarding wizard did not open");
  await pageA.evaluate(() => {
    document.getElementById("onb-scrim").hidden = true;
  });

  await openSettings(pageA, "automations");
  await pageA.click("#settings-open-automations");
  assert(await pageA.locator("#settings-view").isHidden(), "Automations left Settings active");
  assert(await pageA.locator("#auto-view").isVisible(), "the real Automations page did not open");
  await pageA.evaluate(() => setAutoOpen(false));
  return "wizard + scheduled-work page";
});

await test("a completed run shows the agent's result and opens its evidence folder on demand", async () => {
  const job = {
    id: "auto-qa",
    name: "Daily emulator QA",
    enabled: true,
    workspace: BOOT_BASE.project_root,
    workspace_mode: "existing",
    base_branch: null,
    agent: "codex",
    prompt: "Run the core flows and keep screenshots.",
    command: "",
    reuse_session: false,
    close_when_done: true,
    evidence: "prompt",
    leaves_evidence: true,
    schedule: { cadence: "daily", time: "09:30", day_of_week: 1, custom: "" },
    precheck: "",
    precheck_timeout_seconds: 60,
    missed_run_grace_minutes: 60,
    last_run_at: 29_000_000,
    next_run_at: 29_001_440,
    runs: 1,
  };
  backend.automations = [job];
  backend.automationRuns = [{
    id: "1756700000000-5",
    automation_id: "auto-qa",
    scheduled_for_epoch_minutes: 28_998_560,
    at_epoch_ms: 1_756_700_000_000,
    root: BOOT_BASE.project_root,
    term: 5,
    made_worktree: null,
    skipped: null,
    failure: null,
    ended_at_epoch_ms: null,
    exit_code: null,
    completed_at_epoch_ms: 1_756_700_900_000,
    result: "성공했습니다.",
    evidence_dir: "/data/automations/auto-qa/runs/1756700000000",
    evidence_files: 0,
  }, {
    id: "1756800000000-7",
    automation_id: "auto-qa",
    scheduled_for_epoch_minutes: 29_000_000,
    at_epoch_ms: 1_756_800_000_000,
    root: BOOT_BASE.project_root,
    term: 7,
    made_worktree: null,
    skipped: null,
    failure: null,
    ended_at_epoch_ms: null,
    exit_code: null,
    completed_at_epoch_ms: 1_756_800_900_000,
    result: "12 flows passed, 1 regression on checkout; screenshots saved.",
    evidence_dir: "/data/automations/auto-qa/runs/1756800000000",
    evidence_files: 2,
  }];
  backend.runEvidence = {
    dir: "/data/automations/auto-qa/runs/1756800000000",
    files: [
      { name: "001-emulator-tap.png", bytes: 20_480, kind: "image", preview: "data:image/png;base64,iVBORw0KGgo=" },
      { name: "report.md", bytes: 96, kind: "text", text: "# QA\n- checkout: regression" },
      { name: "run.log", bytes: 96, kind: "text" },
    ],
    more: 3,
    steps: [
      { n: 1, at_epoch_ms: 1_756_800_100_000, tool: "emulator", verb: "tap", argv: ["tap", "--x", "0.5", "--y", "0.7"], ok: true, shot: "001-emulator-tap.png" },
      { n: 2, at_epoch_ms: 1_756_800_200_000, tool: "emulator", verb: "text", argv: ["text", "--text", "[6 chars]"], ok: false, error: "no focused field" },
    ],
  };
  try {
    await openSettings(pageA, "automations");
    const listed = backend.calls.length;
    await pageA.click("#settings-open-automations");
    await backend.waitForCall("A", "list_automations", listed);
    await pageA.locator("#auto-list .auto-row").first().click();
    await backend.waitForCall("A", "list_automation_runs", listed);
    const word = (key, fallback, vars) => pageA.evaluate(
      ([k, f, v]) => t(k, f, v), [key, fallback, vars ?? null]);
    assertEqual(
      await pageA.locator("#auto-detail-after").textContent(),
      await word("auto.closeWhenDone", "완료되면 터미널 닫기"),
      "the overview did not say the job closes its terminal on completion",
    );
    assertEqual(
      await pageA.locator("#auto-detail-evidence").textContent(),
      await word("auto.evidencePromptYes", "프롬프트에 따라 — 이 프롬프트는 남김"),
      "the overview did not say what the prompt decided about evidence",
    );
    await pageA.click("#auto-runs-tab");
    // The state span wears its verdict as its class, so the row is found by
    // the verdict: a run whose agent reported done is painted completed.
    const completed = pageA.locator("#auto-runs-list .auto-run-completed");
    assertEqual(await completed.count(), 2, "runs whose agents reported done were not painted as completed");
    // Newest first: the proven run leads, the one that only said so follows.
    const state = completed.nth(0);
    const unproven = completed.nth(1);
    assert((await unproven.getAttribute("class")).includes("auto-run-noproof"), "a run asked for proof that left none was not marked");
    assertEqual(
      await unproven.textContent(),
      await word("auto.runCompletedNoEvidence", "완료 — 증거 없음"),
      "the unproven run's word",
    );
    assertEqual(
      await state.textContent(),
      `${await word("auto.runCompleted", "완료 — 터미널 열림")} · ${await word("auto.evidenceCount", "증거 {{n}}개", { n: 2 })}`,
      "completion label with its evidence count",
    );
    assert(!(await state.getAttribute("class")).includes("auto-run-noproof"), "a run with evidence was painted as unproven");
    assert(
      (await pageA.locator("#auto-runs-list .auto-run-result").first().textContent()).includes("1 regression on checkout"),
      "the run row did not show what the agent said",
    );
    assert(await pageA.locator("#auto-runs-list .auto-run-evidence").first().isHidden(), "the gallery was fetched before anyone asked");
    const asked = backend.calls.length;
    await pageA.locator("#auto-runs-list .auto-run-evidence-toggle").first().click();
    const call = await backend.waitForCall("A", "list_run_evidence", asked);
    assertEqual(call.args, { automationId: "auto-qa", runId: "1756800000000-7" }, "evidence was asked for a different run");
    await pageA.locator("#auto-runs-list .auto-run-evidence").first().waitFor({ state: "visible" });
    assertEqual(await pageA.locator("#auto-runs-list .auto-run-shot img").getAttribute("alt"), "001-emulator-tap.png", "the screenshot preview");
    assert(
      (await pageA.locator("#auto-runs-list .auto-run-shot figcaption").textContent()).startsWith("1. emulator tap"),
      "the frame was not captioned by the step it proves",
    );
    assert(
      (await pageA.locator("#auto-runs-list .auto-run-text summary").textContent()).startsWith("report.md"),
      "the run's report was not offered in place",
    );
    assert(
      (await pageA.locator("#auto-runs-list .auto-run-text pre").textContent()).includes("checkout: regression"),
      "the report's text was not carried",
    );
    assert(
      (await pageA.locator("#auto-runs-list .auto-run-steps li").textContent()).includes("2. emulator text — no focused field"),
      "the failed step was not listed in words",
    );
    assert(
      (await pageA.locator("#auto-runs-list .auto-run-file").textContent()).startsWith("run.log"),
      "the log file was not listed",
    );
    assertEqual(
      await pageA.locator("#auto-runs-list .auto-run-evidence-empty").textContent(),
      await word("auto.evidenceMore", "외 {{n}}개", { n: 3 }),
      "the overflow count",
    );
    const revealed = backend.calls.length;
    await pageA.locator("#auto-runs-list .auto-run-evidence-head .btn").click();
    await backend.waitForCall("A", "reveal_run_evidence", revealed);
    await pageA.locator("#auto-runs-list .auto-run-evidence-toggle").first().click();
    assert(await pageA.locator("#auto-runs-list .auto-run-evidence").first().isHidden(), "folding the gallery did not hide it");
    assertEqual(await pageA.locator("#auto-runs-list .auto-run-shot").count(), 0, "a folded gallery kept its previews in the document");
    await pageA.evaluate(() => setAutoOpen(false));
  } finally {
    backend.automations = [];
    backend.automationRuns = [];
    backend.runEvidence = { dir: "", files: [], more: 0 };
  }
  return "completed chip + result + on-demand evidence gallery";
});

await test("Git settings persist the branch-prefix contract used by new workspaces", async () => {
  await openSettings(pageA, "git");
  await pageA.selectOption("#worktree-prefix", "custom");
  await pageA.fill("#worktree-prefix-custom", "review");
  const savedAt = backend.calls.length;
  await pageA.$eval("#worktree-prefix-custom", (field) =>
    field.dispatchEvent(new Event("change")));
  await backend.waitForCall("A", "save_worktree_prefs", savedAt);
  assertEqual(
    backend.settings.worktree_prefs,
    { branch_prefix: "custom", custom_prefix: "review" },
    "Git pane did not commit the canonical worktree preference",
  );
  return "custom/review";
});

await test("settings open fetches one versioned canonical snapshot", async () => {
  const before = backend.count("A", "settings_snapshot");
  await openSettings(pageA, "appearance");
  await backend.waitForCall("A", "settings_snapshot", backend.calls.length - 30);
  const answer = backend.snapshot();
  assert(backend.count("A", "settings_snapshot") > before, "settings_snapshot was not requested");
  assert(answer.schema_version === 1 && Number.isInteger(answer.revision), "snapshot lacks schema/revision", answer);
  assert(await pageA.locator("#settings-health").isHidden(), "healthy storage painted a warning");
  return `revision ${answer.revision}`;
});

await test("scrollback choices state their measured worst-case memory cost", async () => {
  await openSettings(pageA, "terminal");
  const shown = await pageA.evaluate(() => ({
    formula: [scrollbackWorstCaseMiB(5000), scrollbackWorstCaseMiB(50000)],
    caption: document.getElementById("term-scrollback-cost")?.textContent,
    presets: [...document.querySelectorAll("[data-scrollback-rows]")].map((button) => ({
      rows: button.dataset.scrollbackRows,
      label: button.textContent,
      aria: button.getAttribute("aria-label"),
      title: button.dataset.tip,
    })),
  }));
  assertEqual(shown.formula, [25, 246], "scrollback memory formula drifted from the measurement");
  assertEqual(
    shown.caption,
    "5,000줄 · 판당 ≤25MiB — 95칸 최악",
    "the current scrollback cost was hidden behind interaction",
  );
  assertEqual(shown.presets, [
    {
      rows: "5000",
      label: "5k",
      aria: "5,000줄 · 판당 ≤25MiB",
      title: "95칸 기준 최악 · 판당 ≤25MiB",
    },
    {
      rows: "10000",
      label: "10k",
      aria: "10,000줄 · 판당 ≤50MiB",
      title: "95칸 기준 최악 · 판당 ≤50MiB",
    },
    {
      rows: "25000",
      label: "25k",
      aria: "25,000줄 · 판당 ≤123MiB",
      title: "95칸 기준 최악 · 판당 ≤123MiB",
    },
    {
      rows: "50000",
      label: "50k",
      aria: "50,000줄 · 판당 ≤246MiB",
      title: "95칸 기준 최악 · 판당 ≤246MiB",
    },
  ], "scrollback preset tooltips hid their per-pane cost or its 95-column basis");

  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    pageA.click('[data-scrollback-rows="50000"]'));
  assertEqual(
    await pageA.locator("#term-scrollback-cost").textContent(),
    "50,000줄 · 판당 ≤246MiB — 95칸 최악",
    "the visible scrollback caption did not follow a preset selection",
  );
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    pageA.click('[data-scrollback-rows="5000"]'));

  await pageA.click("#term-scrollback-presets [data-scrollback-custom]");
  await pageA.fill("#term-scrollback", "12300");
  assertEqual(
    await pageA.locator("#term-scrollback-cost").textContent(),
    "12,300줄 · 판당 ≤61MiB — 95칸 최악",
    "custom scrollback cost did not follow the live draft",
  );
  await pageA.evaluate(() => {
    termScrollbackCustomRequested = false;
    termScrollbackDraft = String(termScrollbackCanonical);
    paintScrollbackSetting();
  });
});

await test("IDE, editor, and terminal font suggestions share one lazy native inventory", async () => {
  await openSettings(pageA, "appearance");
  assert(
    backend.count("A", "list_system_fonts") === 0,
    "font inventory ran before its control was used",
  );
  await pageA.focus("#app-font-family");
  await backend.waitForCall("A", "list_system_fonts", backend.calls.length - 10);
  await pageA.locator("#app-font-family").blur();
  await pageA.evaluate(() => showSettingsPane("editing"));
  await pageA.focus("#editing-editor-font");
  await pageA.waitForFunction(
    () => document.getElementById("system-font-options")?.children.length === 3,
    undefined,
    { timeout: UI_TIMEOUT },
  );
  assert(
    backend.count("A", "list_system_fonts") === 1,
    "reopening the font picker reran native inventory",
  );
  const options = await pageA.locator("#system-font-options option")
    .evaluateAll((rows) => rows.map((row) => row.value));
  assertEqual(options, ["Fira Code", "JetBrains Mono", "Menlo"], "font suggestions drifted");
  await pageA.evaluate(() => showSettingsPane("terminal"));
  await pageA.focus("#term-font-family");
  await pageA.locator("#term-font-family").blur();
  assert(
    backend.count("A", "list_system_fonts") === 1,
    "terminal font picker did not reuse the native inventory",
  );
  return options.join(", ");
});

await test("the editor minimap starts off and persists its switch", async () => {
  await openSettings(pageA, "editing");
  assert(
    !await pageA.locator("#editing-editor-minimap").isChecked(),
    "fresh settings did not mirror Orca's disabled minimap default",
  );
  await gestureAndWait(pageA, "A", "patch_editing_prefs", () =>
    pageA.check("#editing-editor-minimap"));
  assert(
    backend.settings.editing_prefs.editor_minimap_enabled === true,
    "the minimap switch did not reach the canonical editing document",
  );
  await gestureAndWait(pageA, "A", "patch_editing_prefs", () =>
    pageA.uncheck("#editing-editor-minimap"));
  return "Off → On → Off";
});

await test("middle-click selection paste follows the platform until explicitly changed", async () => {
  await openSettings(pageA, "editing");
  const platformDefault = await pageA.evaluate(() => !usesWindowsPlatform);
  assertEqual(
    await pageA.locator("#editing-primary-selection-middle-click-paste").isChecked(),
    platformDefault,
    "the unresolved setting did not follow its platform default",
  );
  assertEqual(
    backend.settings.editing_prefs.primary_selection_middle_click_paste,
    null,
    "painting the platform default persisted a guessed boolean",
  );

  const explicit = !platformDefault;
  await gestureAndWait(pageA, "A", "patch_editing_prefs", () =>
    pageA.locator("#editing-primary-selection-middle-click-paste").setChecked(explicit));
  assertEqual(
    backend.settings.editing_prefs.primary_selection_middle_click_paste,
    explicit,
    "the explicit middle-click policy did not reach the canonical editing document",
  );
  await gestureAndWait(pageA, "A", "patch_editing_prefs", () =>
    pageA.locator("#editing-primary-selection-middle-click-paste").setChecked(platformDefault));
  return `${platformDefault ? "On" : "Off"} by platform → explicit → restored`;
});

await test("Markdown spelling and review tools start on and persist independently", async () => {
  await openSettings(pageA, "editing");
  assert(
    await pageA.locator("#editing-markdown-spellcheck").isChecked()
      && await pageA.locator("#editing-markdown-review-tools").isChecked(),
    "fresh settings did not mirror Orca's enabled Markdown defaults",
  );
  await gestureAndWait(pageA, "A", "patch_editing_prefs", () =>
    pageA.uncheck("#editing-markdown-spellcheck"));
  assert(
    backend.settings.editing_prefs.rich_markdown_spellcheck_enabled === false
      && backend.settings.editing_prefs.markdown_review_tools_enabled === true,
    "the spellcheck patch changed its review-tools sibling",
  );
  await gestureAndWait(pageA, "A", "patch_editing_prefs", () =>
    pageA.uncheck("#editing-markdown-review-tools"));
  assert(
    backend.settings.editing_prefs.markdown_review_tools_enabled === false,
    "the review-tools switch did not reach the canonical editing document",
  );
  await gestureAndWait(pageA, "A", "patch_editing_prefs", () =>
    pageA.check("#editing-markdown-spellcheck"));
  await gestureAndWait(pageA, "A", "patch_editing_prefs", () =>
    pageA.check("#editing-markdown-review-tools"));
  return "On → independently Off → On";
});

await test("the combined diff file tree starts hidden and stores Shown or Hidden", async () => {
  await openSettings(pageA, "editing");
  assertEqual(
    await pageA.locator("#editing-diff-file-tree").inputValue(),
    "hidden",
    "fresh settings did not mirror Orca's hidden default",
  );
  await gestureAndWait(pageA, "A", "patch_editing_prefs", () =>
    pageA.selectOption("#editing-diff-file-tree", "shown"));
  assert(
    backend.settings.editing_prefs.combined_diff_file_tree_visible_by_default === true,
    "Shown did not reach the canonical editing document",
  );
  await gestureAndWait(pageA, "A", "patch_editing_prefs", () =>
    pageA.selectOption("#editing-diff-file-tree", "hidden"));
  return "Hidden → Shown → Hidden";
});

await test("boot recovery and errors remain visible after a healthy settings refetch", async () => {
  const savedHealth = backend.settings.settings_health;
  const savedError = backend.settings.settings_error;
  const cases = [
    { health: "recovered", error: null, id: "health-recovered", role: "status" },
    {
      health: "error",
      error: "preferences.json could not be read safely",
      id: "health-error",
      role: "alert",
    },
  ];
  try {
    for (const row of cases) {
      backend.settings.settings_health = row.health;
      backend.settings.settings_error = row.error;
      const healthPage = await context.newPage();
      try {
        await installTauri(healthPage, row.id);
        await bootPage(healthPage, row.id);
        assert(await healthPage.locator("#settings-view").isHidden(), "health fixture opened settings at boot");

        // The repository is readable on the next request. That does not erase
        // the unhealthy boot before the person has had a chance to see it.
        backend.settings.settings_health = "healthy";
        backend.settings.settings_error = null;
        const from = backend.calls.length;
        await healthPage.evaluate(() => setSettingsOpen(true));
        await backend.waitForCall(row.id, "settings_snapshot", from);
        await healthPage.waitForSelector("#settings-health:not([hidden])", { timeout: UI_TIMEOUT });
        const visible = await healthPage.locator("#settings-health").evaluate((banner) => ({
          health: banner.dataset.health,
          role: banner.getAttribute("role"),
          title: document.getElementById("settings-health-title")?.textContent,
          detail: document.getElementById("settings-health-detail")?.textContent,
        }));
        assert(visible.health === row.health, "healthy refetch erased boot health", visible);
        assert(visible.role === row.role && visible.title, "health banner lacks accessible severity", visible);
        if (row.error) assert(visible.detail === row.error, "safe backend detail was not shown", visible);
      } finally {
        await healthPage.close();
      }
    }
  } finally {
    backend.settings.settings_health = savedHealth;
    backend.settings.settings_error = savedError;
  }
});

await test("a stale settings snapshot still reports storage health without replacing data", async () => {
  const healthPage = await context.newPage();
  try {
    await installTauri(healthPage, "stale-health");
    await bootPage(healthPage, "stale-health");
    await openSettings(healthPage, "appearance");
    const observed = await healthPage.evaluate(() => {
      const seeded = applySettingsSnapshot({ revision: 100, theme: "light" });
      const before = { revision: settingsRevision, theme };
      const accepted = applySettingsSnapshot({
        revision: 0,
        settings_health: "error",
        settings_error: "future settings value is incompatible with this build",
        theme: "dark",
      });
      return {
        seeded,
        accepted,
        before,
        revision: settingsRevision,
        theme,
        health: settingsHealth,
        error: settingsHealthError,
      };
    });
    await healthPage.waitForSelector("#settings-health:not([hidden])", { timeout: UI_TIMEOUT });
    const banner = await healthPage.locator("#settings-health").evaluate((node) => ({
      health: node.dataset.health,
      role: node.getAttribute("role"),
    }));

    assert(
      observed.seeded === true && observed.before.revision === 100 && observed.before.theme === "light",
      "revision-100 settings fixture was not established",
      observed,
    );
    assert(observed.accepted === false, "revision-0 settings data was accepted", observed);
    assert(
      observed.revision === observed.before.revision && observed.theme === observed.before.theme,
      "stale snapshot replaced canonical preference data",
      observed,
    );
    assert(
      observed.health === "error" && observed.error?.includes("incompatible"),
      "stale snapshot storage evidence was discarded",
      observed,
    );
    assertEqual(banner, { health: "error", role: "alert" }, "stale health was not announced");
  } finally {
    await healthPage.close();
  }
});

await test("Option as Alt auto follows the native input source and fails safe", async () => {
  await openSettings(pageA, "terminal");
  const isMac = await pageA.evaluate(() => usesCommandModifier);
  if (!isMac) {
    const hidden = await pageA.evaluate(() => ({
      option: document.getElementById("term-option-as-alt-field")?.hidden,
      jisYen: document.getElementById("term-jis-yen-to-backslash-field")?.hidden,
    }));
    assert(hidden.option && hidden.jisYen, "macOS terminal controls were reachable off macOS", hidden);
    return "hidden off macOS";
  }
  await pageA.waitForFunction(
    () => document.getElementById("term-option-as-alt-status")?.textContent.includes("US"),
    undefined,
    { timeout: UI_TIMEOUT },
  );
  const initial = await pageA.evaluate(() => ({
    hidden: document.getElementById("term-option-as-alt-field")?.hidden,
    value: document.getElementById("term-option-as-alt")?.value,
    options: [...(document.getElementById("term-option-as-alt")?.options ?? [])]
      .map((option) => option.value),
    jisYenHidden: document.getElementById("term-jis-yen-to-backslash-field")?.hidden,
    jisYenChecked: document.getElementById("term-jis-yen-to-backslash")?.checked,
  }));
  assertEqual(initial, {
    hidden: false,
    value: ORCA_SETTINGS_CONTRACT.terminal_option_as_alt_contract.default,
    options: ORCA_SETTINGS_CONTRACT.terminal_option_as_alt_contract.modes,
    jisYenHidden: false,
    jisYenChecked: ORCA_SETTINGS_CONTRACT.terminal_jis_yen_to_backslash_contract.default,
  }, "macOS Option-as-Alt control did not expose Orca's exact five choices");

  backend.keyboardLayout = { category: "non_us" };
  let from = backend.calls.length;
  await pageA.evaluate(() => window.dispatchEvent(new Event("focus")));
  await backend.waitForCall("A", "terminal_keyboard_layout", from);
  await pageA.waitForFunction(
    () => document.getElementById("term-option-as-alt-status")?.textContent.includes("문자 조합"),
    undefined,
    { timeout: UI_TIMEOUT },
  );

  backend.keyboardLayout = { category: "unknown" };
  from = backend.calls.length;
  await pageA.evaluate(() => window.dispatchEvent(new Event("focus")));
  await backend.waitForCall("A", "terminal_keyboard_layout", from);
  await pageA.waitForFunction(
    () => document.getElementById("term-option-as-alt-status")?.textContent.includes("읽지 못해"),
    undefined,
    { timeout: UI_TIMEOUT },
  );
  backend.keyboardLayout = { category: "us" };
});

await test("Windows shells detect Git Bash and PowerShell 7 while removed choices fail soft", async () => {
  const first = await context.newPage();
  await addReportedPlatform(first, "Win32");
  await installTauri(first, "PW1");
  attachFaultRecorder(first, "PW1");
  try {
    const from = backend.calls.length;
    await bootPage(first, "PW1");
    await backend.waitForCall("PW1", "terminal_windows_status", from);
    await openSettings(first, "terminal");
    await first.waitForFunction(
      () => document.getElementById("term-windows-powershell-status")?.textContent
        .includes("PowerShell 7+"),
      undefined,
      { timeout: UI_TIMEOUT },
    );
    const initial = await first.evaluate(() => {
      const shell = document.getElementById("term-windows-shell");
      const powershell = document.getElementById("term-windows-powershell");
      return {
        shellHidden: document.getElementById("term-windows-shell-field")?.hidden,
        shellValue: shell?.value,
        shellOptions: [...(shell?.options ?? [])].map((option) => ({
          value: option.value,
          disabled: option.disabled,
        })),
        powershellHidden: document.getElementById("term-windows-powershell-field")?.hidden,
        powershellValue: powershell?.value,
        powershellOptions: [...(powershell?.options ?? [])].map((option) => ({
          value: option.value,
          disabled: option.disabled,
        })),
      };
    });
    assertEqual(initial, {
      shellHidden: false,
      shellValue: "powershell.exe",
      shellOptions: [
        { value: "powershell.exe", disabled: false },
        { value: "cmd.exe", disabled: false },
        { value: "git-bash", disabled: false },
      ],
      powershellHidden: false,
      powershellValue: "auto",
      powershellOptions: [
        { value: "auto", disabled: false },
        { value: "powershell.exe", disabled: false },
        { value: "pwsh.exe", disabled: false },
      ],
    }, "Win32 did not expose Orca's exact shell and PowerShell choices");

    await gestureAndWait(first, "PW1", "patch_terminal_prefs", () =>
      first.selectOption("#term-windows-powershell", "pwsh.exe"));
    assertEqual(
      backend.settings.terminal_prefs.windows_powershell_implementation,
      "pwsh.exe",
      "PowerShell 7+ was not stored canonically",
    );
    await gestureAndWait(first, "PW1", "patch_terminal_prefs", () =>
      first.selectOption("#term-windows-shell", "cmd.exe"));
    assert(
      await first.$eval("#term-windows-powershell-field", (field) => field.hidden),
      "PowerShell implementation remained visible for Command Prompt",
    );
    await gestureAndWait(first, "PW1", "patch_terminal_prefs", () =>
      first.selectOption("#term-windows-shell", "git-bash"));
    assertEqual(
      backend.settings.terminal_prefs.windows_shell,
      "git-bash",
      "Git Bash was not stored canonically",
    );
  } finally {
    await first.close();
  }

  backend.pwshAvailable = false;
  backend.gitBashAvailable = false;
  const second = await context.newPage();
  await addReportedPlatform(second, "Win32");
  await installTauri(second, "PW2");
  attachFaultRecorder(second, "PW2");
  try {
    const from = backend.calls.length;
    await bootPage(second, "PW2");
    await backend.waitForCall("PW2", "terminal_windows_status", from);
    await openSettings(second, "terminal");
    await second.waitForFunction(
      () => document.querySelector('#term-windows-shell option[value="git-bash"]')?.disabled,
      undefined,
      { timeout: UI_TIMEOUT },
    );
    const removedGitBash = await second.evaluate(() => ({
      value: document.getElementById("term-windows-shell")?.value,
      status: document.getElementById("term-windows-shell-status")?.textContent,
      powershellHidden: document.getElementById("term-windows-powershell-field")?.hidden,
    }));
    assert(
      removedGitBash.value === "git-bash"
        && removedGitBash.status.includes("PowerShell")
        && removedGitBash.powershellHidden,
      "removed Git Bash lost intent or hid its runtime fallback",
      removedGitBash,
    );
    await gestureAndWait(second, "PW2", "patch_terminal_prefs", () =>
      second.selectOption("#term-windows-shell", "powershell.exe"));
    await second.waitForFunction(
      () => document.querySelector('#term-windows-powershell option[value="pwsh.exe"]')?.disabled,
      undefined,
      { timeout: UI_TIMEOUT },
    );
    const restoredPowerShell = await second.evaluate(() => ({
      value: document.getElementById("term-windows-powershell")?.value,
      status: document.getElementById("term-windows-powershell-status")?.textContent,
    }));
    assert(
      restoredPowerShell.value === "pwsh.exe"
        && restoredPowerShell.status.includes("Windows PowerShell"),
      "removed PowerShell 7 lost intent or hid its runtime fallback",
      restoredPowerShell,
    );
    await gestureAndWait(second, "PW2", "patch_terminal_prefs", () =>
      second.selectOption("#term-windows-powershell", "auto"));
  } finally {
    backend.pwshAvailable = true;
    backend.gitBashAvailable = true;
    await second.close();
  }
});

await test("Ghostty previews and atomically applies every compatible settings family", async () => {
  const original = backend.snapshot();
  const patch = {
    macOptionAsAlt: "right",
    terminalOpacity: 0.64,
    windowBlur: true,
    colorOverrides: { foreground: "#d0d1d2", red: "#aa1122" },
    dividerColorDark: "#334455",
    dividerColorLight: "#ccddee",
    inactivePaneOpacity: 0.42,
    paddingX: 12,
    paddingY: 8,
    lineHeight: 1.25,
    hideMouseWhileTyping: true,
    cursorOpacity: 0.55,
    fontFamily: "Commit Mono",
    fontSize: 19,
    fontWeight: 600,
    cursorStyle: "bar",
    cursorBlink: false,
    focusFollowsMouse: true,
    primarySelectionMiddleClickPaste: true,
  };
  backend.ghosttyPreview = {
    found: true,
    configPaths: ["/Users/test/.config/ghostty/config"],
    patch,
    changes: [
      { key: "terminalFontFamily", value: "Commit Mono" },
      { key: "terminalBackgroundOpacity", value: "0.64" },
      { key: "primarySelectionMiddleClickPaste", value: "true" },
    ],
    unsupportedKeys: ["selection-word-chars"],
  };

  await pageA.evaluate(() => showSettingsPane("terminal"));
  const opener = pageA.locator("#term-ghostty-import");
  await opener.focus();
  let from = backend.calls.length;
  await opener.click();
  await backend.waitForCall("A", "preview_ghostty_import", from);
  await pageA.waitForFunction(
    () => !document.getElementById("term-ghostty-import-apply")?.disabled,
    undefined,
    { timeout: UI_TIMEOUT },
  );
  const preview = await pageA.evaluate(() => ({
    open: !document.getElementById("term-ghostty-import-scrim")?.hidden,
    paths: document.getElementById("term-ghostty-import-paths")?.textContent,
    changes: document.getElementById("term-ghostty-import-changes")?.textContent,
    unsupported: document.getElementById("term-ghostty-import-unsupported-list")?.textContent,
  }));
  assert(
    preview.open
      && preview.paths.includes(".config/ghostty/config")
      && preview.changes.includes("Commit Mono")
      && preview.unsupported.includes("selection-word-chars"),
    "Ghostty preview hid its source, compatible changes, or unsupported keys",
    preview,
  );

  const revision = backend.settings.revision;
  from = backend.calls.length;
  await pageA.click("#term-ghostty-import-apply");
  await backend.waitForCall("A", "apply_ghostty_import", from);
  await pageA.waitForFunction(
    () => document.getElementById("term-ghostty-import-message")?.textContent.includes("완료"),
    undefined,
    { timeout: UI_TIMEOUT },
  );
  assertEqual(
    backend.calls.slice(from).filter((call) => call.command === "apply_ghostty_import").length,
    1,
    "Ghostty import split one gesture into duplicate settings transactions",
  );
  assertEqual(backend.settings.revision, revision + 1, "Ghostty import was not one revision");
  assert(
    backend.settings.terminal_prefs.font_family === "Commit Mono"
      && backend.settings.terminal_prefs.font_size === 19
      && backend.settings.terminal_prefs.leading === 1.4375
      && backend.settings.terminal_prefs.color_overrides.foreground === "#d0d1d2"
      && backend.settings.terminal_prefs.mac_option_as_alt === "right"
      && backend.settings.terminal_opacity === 0.64
      && backend.settings.window_blur === true
      && backend.settings.editing_prefs.primary_selection_middle_click_paste === true,
    "Ghostty import left a settings family behind",
    backend.settings,
  );
  const live = await pageA.evaluate(() => ({
    font: document.documentElement.style.getPropertyValue("--term-font-size"),
    leading: document.documentElement.style.getPropertyValue("--term-leading"),
    foreground: document.documentElement.style.getPropertyValue("--term-screen-fg"),
    opacity: document.documentElement.style.getPropertyValue("--term-screen-alpha"),
  }));
  assertEqual(
    live,
    { font: "19px", leading: "1.4375", foreground: "#d0d1d2", opacity: "0.64" },
    "Ghostty canonical snapshot did not reach the live terminal",
  );

  const restarted = await context.newPage();
  await installTauri(restarted, "GHOSTTY-RESTART");
  attachFaultRecorder(restarted, "GHOSTTY-RESTART");
  try {
    await bootPage(restarted, "GHOSTTY-RESTART");
    await openSettings(restarted, "terminal");
    const booted = await restarted.evaluate(() => ({
      font: document.getElementById("term-font-size")?.value,
      family: document.getElementById("term-font-family")?.value,
      cursor: document.getElementById("term-cursor-style")?.value,
    }));
    assertEqual(
      booted,
      { font: "19", family: "Commit Mono", cursor: "bar" },
      "Ghostty settings did not survive a second window boot",
    );
  } finally {
    await restarted.close();
  }

  await pageA.click("#term-ghostty-import-apply");
  await pageA.waitForFunction(
    () => document.getElementById("term-ghostty-import-scrim")?.hidden,
    undefined,
    { timeout: UI_TIMEOUT },
  );
  assert(
    await pageA.evaluate(() => document.activeElement?.id === "term-ghostty-import"),
    "closing a completed Ghostty import did not restore opener focus",
  );

  backend.ghosttyPreview = {
    ...backend.ghosttyPreview,
    patch: { fontSize: 20 },
    changes: [{ key: "terminalFontSize", value: "20" }],
    unsupportedKeys: [],
  };
  await opener.click();
  await pageA.waitForFunction(
    () => !document.getElementById("term-ghostty-import-apply")?.disabled,
    undefined,
    { timeout: UI_TIMEOUT },
  );
  backend.rejectOnce("apply_ghostty_import");
  await pageA.click("#term-ghostty-import-apply");
  await pageA.waitForFunction(
    () => !document.getElementById("term-ghostty-import-error")?.hidden,
    undefined,
    { timeout: UI_TIMEOUT },
  );
  assert(
    backend.settings.terminal_prefs.font_size === 19
      && !await pageA.$eval("#term-ghostty-import-scrim", (scrim) => scrim.hidden),
    "a rejected Ghostty import changed canonical state or dismissed recovery",
  );
  await pageA.click("#term-ghostty-import-cancel");

  await backend.externalPatch({
    terminal_prefs: original.terminal_prefs,
    terminal_opacity: original.terminal_opacity,
    window_blur: original.window_blur,
    editing_prefs: original.editing_prefs,
  }, ["terminal_prefs", "window_material", "editing_prefs"]);
  await waitForSettingsIdle(pageA);
  backend.ghosttyPreview = {
    found: false,
    configPaths: [],
    patch: {},
    changes: [],
    unsupportedKeys: [],
  };
});

await test("non-default writes become canonical state before the second window boots", async () => {
  const writes = [
    ["theme", "set_theme", "light", "appearance"],
    ["locale", "set_locale", "ja", "appearance"],
    ["ui_zoom", "set_ui_zoom", 1.5, "appearance"],
    ["app_font_family", "set_app_font_family", "Inter", "appearance"],
    ["titlebar_app_name", "set_show_titlebar_app_name", false, "appearance"],
    ["menu_bar_icon", "set_show_menu_bar_icon", false, "appearance"],
    ["minimize_to_tray", "set_minimize_to_tray_on_close", true, "appearance"],
    ["workspace_card_layout", "set_compact_worktree_cards", "compact", "appearance"],
    ["show_git_ignored_files", "set_show_git_ignored_files", false, "appearance"],
    ["source_control_group_order", "set_source_control_group_order", "untracked-first", "git"],
    ["source_control_compare_base", "set_source_control_compare_base", "branch-upstream", "git"],
    ["refresh_local_base_ref", "set_refresh_local_base_ref_on_worktree_create", true, "git"],
    ["left_sidebar_appearance", "patch_left_sidebar_appearance", "tinted", "appearance"],
    ["left_sidebar_tint_color", "patch_left_sidebar_appearance", "#336699", "appearance"],
    ["left_sidebar_tint_opacity", "patch_left_sidebar_appearance", 0.27, "appearance"],
    ["terminal", "set_terminal_command", "/usr/local/bin/zo --resume", "terminal"],
    ["opacity", "set_terminal_opacity", 0.7, "appearance"],
    ["blur", "set_window_blur", true, "appearance"],
    ["teams", "set_agent_teams_mode", "off", "orchestration"],
    ["setup_script_launch_mode", "set_setup_script_launch_mode", "split-horizontal", "terminal"],
    ["terminal_shortcut_policy", "set_terminal_shortcut_policy", "terminal-first", "shortcuts"],
    ["workspace_directory", "patch_workspace_creation_prefs", "/tmp/zerocode-workspaces", "general"],
    ["nest_workspaces", "patch_workspace_creation_prefs", false, "general"],
    ["default_agent", "set_default_agent", "blank", "agents"],
  ];
  for (const [kind, command, value, pane] of writes) {
    await pageA.evaluate((wanted) => showSettingsPane(wanted), pane);
    await gestureAndWait(pageA, "A", command, () => chooseControl(pageA, kind, value));
  }
  const terminalPatchStart = backend.calls.length;
  await pageA.evaluate(() => showSettingsPane("terminal"));
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    changeField(pageA, "#term-font-size", 16));
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", async () => {
    await changeField(pageA, "#term-font-family", "JetBrains Mono");
  });
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    pageA.selectOption("#term-ligatures", "on"));
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    pageA.$eval("#term-option-as-alt", (field) => {
      field.value = "left";
      field.dispatchEvent(new Event("change", { bubbles: true }));
    }));
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    pageA.$eval("#term-jis-yen-to-backslash", (field) => {
      field.checked = true;
      field.dispatchEvent(new Event("change", { bubbles: true }));
    }));
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    pageA.selectOption("#term-theme-dark", "Dracula"));
  await waitForSettingsIdle(pageA);
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    pageA.selectOption("#term-theme-light", "Solarized Light"));
  await waitForSettingsIdle(pageA);
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    pageA.uncheck("#term-separate-light-theme"));
  await waitForSettingsIdle(pageA);
  await pageA.$eval("#term-color-overrides", (details) => { details.open = true; });
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", async () => {
    await changeField(pageA, '[data-terminal-color-text="background"]', "#102030");
  });
  await waitForSettingsIdle(pageA);
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", async () => {
    await changeField(pageA, '[data-terminal-color-text="brightBlue"]', "#abcdef");
  });
  await waitForSettingsIdle(pageA);
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    changeField(pageA, "#term-leading", 1.2));
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", async () => {
    await changeField(pageA, "#term-word-separators", "/");
  });
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    changeField(pageA, "#term-fast-scroll", 7.5));
  await pageA.click("#term-scrollback-presets [data-scrollback-custom]");
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    blurField(pageA, "#term-scrollback", 12300));
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    pageA.selectOption("#term-cursor-style", "underline"));
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    blurField(pageA, "#term-cursor-opacity", 0.45));
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    blurField(pageA, "#term-padding-x", 17));
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    blurField(pageA, "#term-padding-y", 9));
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    changeField(pageA, "#term-inactive-pane-opacity", 0.55));
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    changeField(pageA, "#term-divider-thickness", 7));
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", async () => {
    await changeField(pageA, "#term-divider-color-dark", "#123456");
  });
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", async () => {
    await changeField(pageA, "#term-divider-color-light", "#abcdef");
  });
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    pageA.check("#term-copy-on-select"));
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    pageA.check("#term-right-click-paste"));
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    pageA.uncheck("#term-allow-osc52-clipboard"));
  await gestureAndWait(pageA, "A", "patch_terminal_prefs", () =>
    pageA.check("#term-hide-mouse-while-typing"));
  await pageA.evaluate(() => showSettingsPane("editing"));
  await gestureAndWait(pageA, "A", "patch_editing_prefs", () =>
    pageA.check("#editing-auto-save"));
  await gestureAndWait(pageA, "A", "patch_editing_prefs", () =>
    blurField(pageA, "#editing-auto-save-delay", 1750));
  await gestureAndWait(pageA, "A", "patch_editing_prefs", () =>
    pageA.uncheck("#editing-editor-wrap"));
  await gestureAndWait(pageA, "A", "patch_editing_prefs", () =>
    pageA.check("#editing-diff-wrap"));
  await gestureAndWait(pageA, "A", "patch_editing_prefs", () =>
    pageA.uncheck("#editing-markdown-spellcheck"));
  await gestureAndWait(pageA, "A", "patch_editing_prefs", () =>
    pageA.uncheck("#editing-markdown-review-tools"));
  await gestureAndWait(pageA, "A", "patch_editing_prefs", async () => {
    await changeField(pageA, "#editing-editor-font", "JetBrains Mono");
  });
  await gestureAndWait(pageA, "A", "patch_editing_prefs", () =>
    pageA.check("#editing-editor-minimap"));
  await gestureAndWait(pageA, "A", "patch_editing_prefs", () =>
    pageA.uncheck("#editing-primary-selection-middle-click-paste"));
  await gestureAndWait(pageA, "A", "patch_editing_prefs", () =>
    pageA.selectOption("#editing-diff-file-tree", "shown"));
  await gestureAndWait(pageA, "A", "set_diff_side_by_side", () =>
    pageA.selectOption("#editing-diff-view", "inline"));
  await pageA.evaluate(() => showSettingsPane("tasks"));
  await gestureAndWait(pageA, "A", "set_task_source_visibility", () =>
    pageA.uncheck("#show-source-github"));
  await waitForSettingsIdle(pageA);
  await renderSettled(pageA);
  const terminalPatches = backend.calls
    .slice(terminalPatchStart)
    .filter((call) => call.window_id === "A" && call.command === "patch_terminal_prefs")
    .map((call) => call.args.patch);
  for (const kind of [
    "font_family",
    "mac_option_as_alt",
    "jis_yen_to_backslash",
    "allow_osc52_clipboard",
    "theme_dark",
    "theme_light",
    "use_separate_light_theme",
    "divider_thickness_px",
    "divider_color_dark",
    "divider_color_light",
  ]) {
    assertEqual(
      terminalPatches.filter((patch) => patch.kind === kind).length,
      1,
      `${kind} gesture emitted a duplicate terminal patch`,
    );
  }
  assertEqual(
    terminalPatches.filter((patch) => patch.kind === "color_override").map((patch) => patch.value),
    [
      { key: "background", value: "#102030" },
      { key: "brightBlue", value: "#abcdef" },
    ],
    "color override gestures emitted a stale or duplicate patch",
  );

  assertEqual(backend.settings.default_agent, { kind: "blank" }, "Blank was not stored canonically");
  assert(backend.settings.terminal_prefs.font_size === 16, "terminal prefs did not persist");
  assert(
    backend.settings.terminal_prefs.font_family === "JetBrains Mono"
      && backend.settings.terminal_prefs.ligatures === "on"
      && backend.settings.terminal_prefs.mac_option_as_alt === "left"
      && backend.settings.terminal_prefs.jis_yen_to_backslash === true
      && backend.settings.terminal_prefs.theme_dark === "Dracula"
      && backend.settings.terminal_prefs.theme_light === "Solarized Light"
      && backend.settings.terminal_prefs.use_separate_light_theme === false
      && same(backend.settings.terminal_prefs.color_overrides, {
        background: "#102030",
        brightBlue: "#abcdef",
      })
      && backend.settings.terminal_prefs.leading === 1.38,
    "Orca line height did not persist through the CSS compatibility scale",
  );
  assert(backend.settings.terminal_prefs.scrollback === 12300, "custom scrollback did not persist");
  assert(
    backend.settings.terminal_prefs.cursor_style === "underline"
      && backend.settings.terminal_prefs.cursor_opacity === 0.45
      && backend.settings.terminal_prefs.padding_x === 17
      && backend.settings.terminal_prefs.padding_y === 9
      && backend.settings.terminal_prefs.inactive_pane_opacity === 0.55
      && backend.settings.terminal_prefs.divider_color_dark === "#123456"
      && backend.settings.terminal_prefs.divider_color_light === "#abcdef"
      && backend.settings.terminal_prefs.divider_thickness_px === 7
      && backend.settings.terminal_prefs.copy_on_select === true
      && backend.settings.terminal_prefs.right_click_paste === true
      && backend.settings.terminal_prefs.allow_osc52_clipboard === false
      && backend.settings.terminal_prefs.hide_mouse_while_typing === true,
    "terminal appearance prefs did not persist",
    backend.settings.terminal_prefs,
  );
  assertEqual(
    backend.settings.editing_prefs,
    {
      editor_auto_save: true,
      editor_auto_save_delay_ms: 1750,
      editor_minimap_enabled: true,
      editor_word_wrap: false,
      diff_word_wrap: true,
      rich_markdown_spellcheck_enabled: false,
      markdown_review_tools_enabled: false,
      editor_font_family: "JetBrains Mono",
      combined_diff_file_tree_visible_by_default: true,
      primary_selection_middle_click_paste: false,
      editor_font_zoom: 0,
    },
    "editing prefs did not persist",
  );
  assertEqual(backend.settings.hidden_task_sources, ["github"], "task visibility did not persist");
  assertEqual(
    backend.settings.workspace_creation_prefs,
    { directory: "/tmp/zerocode-workspaces", nest_workspaces: false },
    "workspace location prefs did not persist independently",
  );
  assertEqual(
    backend.settings.setup_script_launch_mode,
    "split-horizontal",
    "Setup Script Location did not persist independently",
  );
  return `revision ${backend.settings.revision}`;
});

await test("a successful mutation applies the backend's full canonical answer", async () => {
  await pageA.evaluate(() => showSettingsPane("appearance"));
  backend.canonicalizeOnce("set_terminal_opacity", { terminal_opacity: 0.65 });
  await gestureAndWait(pageA, "A", "set_terminal_opacity", async () => {
    await pageA.fill("#window-opacity", "0.6");
    await pageA.$eval("#window-opacity", (field) => field.dispatchEvent(new Event("change")));
  });
  await pageA.waitForFunction(
    () => document.getElementById("window-opacity")?.value === "0.65",
    undefined,
    { timeout: UI_TIMEOUT },
  );
  assert(backend.settings.terminal_opacity === 0.65, "fixture did not persist its canonical answer");
});

await test("the second boot paints every non-default before opening settings", async () => {
  await bootPage(pageB, "B");
  await pageB.waitForFunction(
    () => document.querySelector('.agent-pill[data-agent="blank"]')?.getAttribute("aria-pressed") === "true",
    undefined,
    { timeout: UI_TIMEOUT },
  );
  const shown = await pageB.evaluate(() => ({
    lang: document.documentElement.lang,
    theme: document.documentElement.dataset.theme ?? "dark",
    uiZoomLevel: document.documentElement.dataset.uiZoomLevel,
    uiZoomFactor: document.documentElement.style.getPropertyValue("--ui-zoom-factor"),
    uiZoomPercent: document.getElementById("ui-zoom-percent")?.textContent,
    appFontFamily: document.getElementById("app-font-family")?.value,
    appFontChoice: document.documentElement.style.getPropertyValue("--font-ui-choice"),
    titlebarAppNameHidden: document.getElementById("titlebar-app-name")?.hidden,
    workspaceCardLayout: document.documentElement.dataset.worktreeCardLayout,
    workspaceCardCompactPressed: document.getElementById("workspace-card-layout-compact")
      ?.getAttribute("aria-pressed"),
    showGitIgnoredFiles: document.getElementById("show-git-ignored-files")?.checked,
    sourceControlGroupOrder: document.getElementById("source-control-group-order")?.value,
    refreshLocalBaseRef: document.getElementById(
      "refresh-local-base-ref-on-worktree-create",
    )?.checked,
    leftSidebarAppearance: document.documentElement.dataset.leftSidebarAppearance,
    leftSidebarTintVisible: !document.getElementById("left-sidebar-tint-settings")?.hidden,
    leftSidebarTintColor: document.getElementById("left-sidebar-tint-color")?.value,
    leftSidebarTintOpacity: document.getElementById("left-sidebar-tint-opacity")?.value,
    leftSidebarTintBounds: [
      document.getElementById("left-sidebar-tint-opacity")?.min,
      document.getElementById("left-sidebar-tint-opacity")?.max,
      document.getElementById("left-sidebar-tint-opacity")?.step,
    ],
    leftSidebarPlane: document.documentElement.style.getPropertyValue("--nav-plane"),
    leftSidebarInk: document.documentElement.style.getPropertyValue("--nav-plane-ink"),
    command: document.getElementById("terminal-command")?.value,
    fontSize: document.getElementById("term-font-size")?.value,
    fontFamily: document.getElementById("term-font-family")?.value,
    ligatures: document.getElementById("term-ligatures")?.value,
    ligaturesEffective: document.documentElement.dataset.termLigatures,
    optionAsAlt: document.getElementById("term-option-as-alt")?.value,
    optionAsAltOptions: [...(document.getElementById("term-option-as-alt")?.options ?? [])]
      .map((option) => option.value),
    jisYenToBackslash: document.getElementById("term-jis-yen-to-backslash")?.checked,
    jisYenHidden: document.getElementById("term-jis-yen-to-backslash-field")?.hidden,
    themeDark: document.getElementById("term-theme-dark")?.value,
    themeLight: document.getElementById("term-theme-light")?.value,
    separateLightTheme: document.getElementById("term-separate-light-theme")?.checked,
    lightThemeHidden: document.getElementById("term-theme-light-field")?.hidden,
    themeCount: document.getElementById("term-theme-dark")?.options.length,
    terminalBackground: getComputedStyle(document.documentElement)
      .getPropertyValue("--term-screen-bg").trim(),
    terminalBrightBlue: getComputedStyle(document.documentElement)
      .getPropertyValue("--term-12").trim(),
    leading: document.getElementById("term-leading")?.value,
    cursorStyle: document.getElementById("term-cursor-style")?.value,
    cursorOpacity: document.getElementById("term-cursor-opacity")?.value,
    cursorOpacityToken: getComputedStyle(document.documentElement)
      .getPropertyValue("--term-cursor-opacity").trim(),
    paddingX: document.getElementById("term-padding-x")?.value,
    paddingY: document.getElementById("term-padding-y")?.value,
    paddingXBounds: [
      document.getElementById("term-padding-x")?.min,
      document.getElementById("term-padding-x")?.max,
      document.getElementById("term-padding-x")?.step,
    ],
    paddingYBounds: [
      document.getElementById("term-padding-y")?.min,
      document.getElementById("term-padding-y")?.max,
      document.getElementById("term-padding-y")?.step,
    ],
    paddingXToken: getComputedStyle(document.documentElement)
      .getPropertyValue("--term-padding-x").trim(),
    paddingYToken: getComputedStyle(document.documentElement)
      .getPropertyValue("--term-padding-y").trim(),
    cursorOptions: [...(document.getElementById("term-cursor-style")?.options ?? [])]
      .map((option) => option.value),
    inactivePaneOpacity: document.getElementById("term-inactive-pane-opacity")?.value,
    inactivePaneBounds: [
      document.getElementById("term-inactive-pane-opacity")?.min,
      document.getElementById("term-inactive-pane-opacity")?.max,
      document.getElementById("term-inactive-pane-opacity")?.step,
    ],
    dividerThickness: document.getElementById("term-divider-thickness")?.value,
    dividerColorDark: document.getElementById("term-divider-color-dark")?.value,
    dividerColorLight: document.getElementById("term-divider-color-light")?.value,
    dividerBounds: [
      document.getElementById("term-divider-thickness")?.min,
      document.getElementById("term-divider-thickness")?.max,
      document.getElementById("term-divider-thickness")?.step,
    ],
    hideMouseWhileTyping: document.getElementById("term-hide-mouse-while-typing")?.checked,
    copyOnSelect: document.getElementById("term-copy-on-select")?.checked,
    rightClickPaste: document.getElementById("term-right-click-paste")?.checked,
    allowOsc52Clipboard: document.getElementById("term-allow-osc52-clipboard")?.checked,
    cursorDataset: document.documentElement.dataset.termCursorStyle,
    inactivePaneToken: getComputedStyle(document.documentElement)
      .getPropertyValue("--term-inactive-pane-opacity").trim(),
    dividerToken: getComputedStyle(document.documentElement)
      .getPropertyValue("--term-divider-thickness").trim(),
    dividerHitToken: getComputedStyle(document.documentElement)
      .getPropertyValue("--term-divider-hit-size").trim(),
    separators: document.getElementById("term-word-separators")?.value,
    fastScroll: document.getElementById("term-fast-scroll")?.value,
    scrollback: document.getElementById("term-scrollback")?.value,
    scrollbackBounds: [
      document.getElementById("term-scrollback")?.min,
      document.getElementById("term-scrollback")?.max,
      document.getElementById("term-scrollback")?.step,
    ],
    scrollbackCustomVisible: !document.getElementById("term-scrollback-custom")?.hidden,
    scrollbackCustomPressed: document.querySelector("[data-scrollback-custom]")
      ?.getAttribute("aria-pressed"),
    scrollbackCustomLabel: document.querySelector("[data-scrollback-custom]")?.textContent,
    autoSave: document.getElementById("editing-auto-save")?.checked,
    autoSaveDelay: document.getElementById("editing-auto-save-delay")?.value,
    autoSaveDelayBounds: [
      document.getElementById("editing-auto-save-delay")?.min,
      document.getElementById("editing-auto-save-delay")?.max,
      document.getElementById("editing-auto-save-delay")?.step,
    ],
    editorWrap: document.getElementById("editing-editor-wrap")?.checked,
    editorMinimap: document.getElementById("editing-editor-minimap")?.checked,
    diffWrap: document.getElementById("editing-diff-wrap")?.checked,
    editorFont: document.getElementById("editing-editor-font")?.value,
    diffView: document.getElementById("editing-diff-view")?.value,
    diffFileTree: document.getElementById("editing-diff-file-tree")?.value,
    opacity: document.getElementById("window-opacity")?.value,
    blur: document.getElementById("window-blur")?.checked,
    glass: document.documentElement.dataset.blur,
    teams: document.getElementById("orch-teams-mode")?.value,
    setupScriptLaunchMode: document.querySelector(
      "[data-setup-launch-mode][aria-pressed='true']",
    )?.dataset.setupLaunchMode,
    workspaceDirectory: document.getElementById("workspace-directory")?.value,
    nestWorkspaces: document.getElementById("nest-workspaces")?.checked,
    githubHidden: document.getElementById("task-tab-github")?.hidden,
    blank: document.querySelector('.agent-pill[data-agent="blank"]')?.getAttribute("aria-pressed"),
  }));
  const opened = backend.last("B", "open_term_tab");
  assertEqual(shown, {
    lang: "ja",
    theme: "light",
    uiZoomLevel: "1.5",
    uiZoomFactor: String(Math.pow(1.2, 1.5)),
    uiZoomPercent: "131%",
    appFontFamily: "Inter",
    appFontChoice: "Inter",
    titlebarAppNameHidden: true,
    workspaceCardLayout: "compact",
    workspaceCardCompactPressed: "true",
    showGitIgnoredFiles: false,
    sourceControlGroupOrder: "untracked-first",
    refreshLocalBaseRef: true,
    leftSidebarAppearance: "tinted",
    leftSidebarTintVisible: true,
    leftSidebarTintColor: "#336699",
    leftSidebarTintOpacity: "0.27",
    leftSidebarTintBounds: ["0", "0.35", "0.01"],
    leftSidebarPlane: "color-mix(in srgb, #336699 27%, var(--surface-abyss))",
    leftSidebarInk: "var(--ink-chalk)",
    command: "/usr/local/bin/zo --resume",
    fontSize: "16",
    fontFamily: "JetBrains Mono",
    ligatures: "on",
    ligaturesEffective: "on",
    optionAsAlt: "left",
    optionAsAltOptions: ["auto", "true", "left", "right", "false"],
    jisYenToBackslash: true,
    jisYenHidden: process.platform !== "darwin",
    themeDark: "Dracula",
    themeLight: "Solarized Light",
    separateLightTheme: false,
    lightThemeHidden: true,
    themeCount: 30,
    terminalBackground: "#102030",
    terminalBrightBlue: "#abcdef",
    leading: "1.2",
    cursorStyle: "underline",
    cursorOpacity: "0.45",
    cursorOpacityToken: "0.45",
    paddingX: "17",
    paddingY: "9",
    paddingXBounds: ["0", "512", "1"],
    paddingYBounds: ["0", "512", "1"],
    paddingXToken: "17px",
    paddingYToken: "9px",
    cursorOptions: ["block", "bar", "underline"],
    inactivePaneOpacity: "0.55",
    inactivePaneBounds: ["0", "1", "0.05"],
    dividerThickness: "7",
    dividerColorDark: "#123456",
    dividerColorLight: "#abcdef",
    dividerBounds: ["1", "32", "1"],
    hideMouseWhileTyping: true,
    copyOnSelect: true,
    rightClickPaste: true,
    allowOsc52Clipboard: false,
    cursorDataset: "underline",
    inactivePaneToken: "0.55",
    dividerToken: "7px",
    dividerHitToken: "13px",
    separators: "/",
    fastScroll: "7.5",
    scrollback: "12300",
    scrollbackBounds: ["1000", "50000", "100"],
    scrollbackCustomVisible: true,
    scrollbackCustomPressed: "true",
    scrollbackCustomLabel: "カスタム",
    autoSave: true,
    autoSaveDelay: "1750",
    autoSaveDelayBounds: ["250", "10000", "250"],
    editorWrap: false,
    editorMinimap: true,
    diffWrap: true,
    editorFont: "JetBrains Mono",
    diffView: "inline",
    diffFileTree: "shown",
    opacity: "0.65",
    blur: true,
    glass: "on",
    teams: "off",
    setupScriptLaunchMode: "split-horizontal",
    workspaceDirectory: "/tmp/zerocode-workspaces",
    nestWorkspaces: false,
    githubHidden: true,
    blank: "true",
  }, "second boot corrected a default after its first application frame");
  assertEqual(
    backend.last("B", "apply_ui_zoom")?.args,
    { zoomLevel: 1.5 },
    "the second renderer did not apply its canonical native UI zoom",
  );
  assert(opened?.args?.plain === true, "second boot did not start Blank as a plain shell", opened);
});

await test("reopening settings refetches an externally changed revision", async () => {
  await openSettings(pageB, "appearance");
  await pageB.evaluate(() => setSettingsOpen(false));
  await backend.externalPatch({ theme: "dark" }, ["theme"]);
  const before = backend.count("B", "settings_snapshot");
  await pageB.evaluate(() => setSettingsOpen(true));
  await backend.waitForCall("B", "settings_snapshot", backend.calls.length - 20);
  await pageB.waitForFunction(
    () => document.getElementById("app-theme")?.value === "dark",
    undefined,
    { timeout: UI_TIMEOUT },
  );
  assert(backend.count("B", "settings_snapshot") > before, "reopen reused a stale snapshot");
});

const ROLLBACKS = [
  { kind: "theme", command: "set_theme", pane: "appearance", asked: "light", canonical: "dark" },
  { kind: "locale", command: "set_locale", pane: "appearance", asked: "en", canonical: "ja" },
  { kind: "ui_zoom", command: "set_ui_zoom", pane: "appearance", asked: -2, canonical: 1.5 },
  {
    kind: "terminal", command: "set_terminal_command", pane: "terminal",
    asked: "/bin/false", canonical: "/usr/local/bin/zo --resume",
  },
  { kind: "terminal_prefs", command: "patch_terminal_prefs", pane: "terminal", asked: 18, canonical: 16 },
  {
    kind: "editing_auto_save", command: "patch_editing_prefs", pane: "editing",
    asked: false, canonical: true,
  },
  { kind: "opacity", command: "set_terminal_opacity", pane: "appearance", asked: 0.2, canonical: 0.65 },
  { kind: "blur", command: "set_window_blur", pane: "appearance", asked: false, canonical: true },
  { kind: "teams", command: "set_agent_teams_mode", pane: "orchestration", asked: "panes", canonical: "off" },
  {
    kind: "setup_script_launch_mode", command: "set_setup_script_launch_mode", pane: "terminal",
    asked: "split-vertical", canonical: "split-horizontal",
  },
  {
    kind: "terminal_shortcut_policy", command: "set_terminal_shortcut_policy", pane: "shortcuts",
    asked: "orca-first", canonical: "terminal-first",
  },
  {
    kind: "tab_order", command: "set_ctrl_tab_order_mode", pane: "general",
    asked: "sequential", canonical: "mru",
  },
  {
    kind: "source_control_group_order", command: "set_source_control_group_order", pane: "git",
    asked: "staged-first", canonical: "untracked-first",
  },
  {
    kind: "source_control_compare_base", command: "set_source_control_compare_base", pane: "git",
    asked: "repository-default", canonical: "branch-upstream",
  },
  {
    kind: "refresh_local_base_ref", command: "set_refresh_local_base_ref_on_worktree_create",
    pane: "git", asked: false, canonical: true,
  },
  {
    kind: "workspace_directory", command: "patch_workspace_creation_prefs", pane: "general",
    asked: "/tmp/refused-workspaces", canonical: "/tmp/zerocode-workspaces",
  },
  {
    kind: "nest_workspaces", command: "patch_workspace_creation_prefs", pane: "general",
    asked: true, canonical: false,
  },
  {
    kind: "delete_worktree_confirm", command: "set_skip_delete_worktree_confirm", pane: "general",
    asked: false, canonical: true,
  },
  {
    kind: "delete_automation_confirm", command: "set_skip_delete_automation_confirm", pane: "general",
    asked: false, canonical: true,
  },
  {
    kind: "artifacts_retention", command: "set_artifacts_retention_days", pane: "automations",
    asked: 14, canonical: 0,
  },
  {
    kind: "default_agent", command: "set_default_agent", pane: "agents", asked: "codex",
    canonical: { auto: "false", blank: "true", codex: "false" },
  },
];

for (const row of ROLLBACKS) {
  await test(`${row.kind} rejects, visibly rolls back, and stays canonical after reopen`, async () => {
    await openSettings(pageB, row.pane);
    backend.rejectOnce(row.command);
    const snapshots = backend.count("B", "settings_snapshot");
    await gestureAndWait(pageB, "B", row.command, () => chooseControl(pageB, row.kind, row.asked));
    await pageB.waitForFunction(
      ({ kind, canonical }) => {
        const pressed = (agent) =>
          document.querySelector(`.agent-pill[data-agent="${agent}"]`)?.getAttribute("aria-pressed");
        const actual = {
          theme: () => document.getElementById("app-theme")?.value,
          locale: () => document.getElementById("app-locale")?.value,
          ui_zoom: () => Number(document.documentElement.dataset.uiZoomLevel),
          terminal: () => document.getElementById("terminal-command")?.value,
          terminal_prefs: () => Number(document.getElementById("term-font-size")?.value),
          editing_auto_save: () => document.getElementById("editing-auto-save")?.checked,
          opacity: () => Number(document.getElementById("window-opacity")?.value),
          blur: () => document.getElementById("window-blur")?.checked,
          teams: () => document.getElementById("orch-teams-mode")?.value,
          setup_script_launch_mode: () => document.querySelector(
            "[data-setup-launch-mode][aria-pressed='true']",
          )?.dataset.setupLaunchMode,
          terminal_shortcut_policy: () => document.getElementById(
            "terminal-shortcut-policy",
          )?.value,
          tab_order: () => document.getElementById("ctrl-tab-order-mode")?.value,
          source_control_group_order: () => document.getElementById(
            "source-control-group-order",
          )?.value,
          source_control_compare_base: () => document.getElementById(
            "source-control-compare-base",
          )?.value,
          refresh_local_base_ref: () => document.getElementById(
            "refresh-local-base-ref-on-worktree-create",
          )?.checked,
          workspace_directory: () => document.getElementById("workspace-directory")?.value,
          nest_workspaces: () => document.getElementById("nest-workspaces")?.checked,
          delete_worktree_confirm: () => document.getElementById("ask-before-delete-worktree")?.checked,
          delete_automation_confirm: () => document.getElementById("ask-before-delete-automation")?.checked,
    artifacts_retention: () => Number(document.getElementById("artifacts-retention-days")?.value),
          default_agent: () => ({ auto: pressed("auto"), blank: pressed("blank"), codex: pressed("codex") }),
        }[kind]();
        return JSON.stringify(actual) === JSON.stringify(canonical);
      },
      { kind: row.kind, canonical: row.canonical },
      { timeout: UI_TIMEOUT },
    );
    assert(
      backend.count("B", "settings_snapshot") > snapshots,
      `${row.command} rejection did not refetch canonical settings`,
    );
    assertEqual(await controlValue(pageB, row.kind), row.canonical, "visible value did not roll back");
    await reopenSettings(pageB, "B", row.pane);
    assertEqual(await controlValue(pageB, row.kind), row.canonical, "reopen did not preserve rollback");
  });
}

await test("Tab Order persists its Orca MRU/sequential choice", async () => {
  await openSettings(pageB, "general");
  const call = await gestureAndWait(
    pageB,
    "B",
    "set_ctrl_tab_order_mode",
    () => pageB.selectOption("#ctrl-tab-order-mode", "sequential"),
  );
  assertEqual(call.args, { mode: "sequential" }, "Tab Order sent a different wire value");
  assertEqual(backend.settings.ctrl_tab_order_mode, "sequential", "backend did not store Tab Order");
  await reopenSettings(pageB, "B", "general");
  assertEqual(
    await controlValue(pageB, "tab_order"),
    "sequential",
    "Tab Order did not survive a canonical refetch",
  );
});

await test("Workspace Directory and Nest Workspaces share one field-patched creation contract", async () => {
  await openSettings(pageB, "general");
  let from = backend.calls.length;
  await pageB.fill("#workspace-directory", ".zerocode/worktrees");
  await pageB.locator("#workspace-directory").blur();
  const directory = await backend.waitForCall("B", "patch_workspace_creation_prefs", from);
  assertEqual(
    directory.args,
    { patch: { kind: "directory", value: ".zerocode/worktrees" } },
    "Workspace Directory replaced its nesting sibling",
  );

  from = backend.calls.length;
  await pageB.check("#nest-workspaces");
  const nested = await backend.waitForCall("B", "patch_workspace_creation_prefs", from);
  assertEqual(
    nested.args,
    { patch: { kind: "nest_workspaces", value: true } },
    "Nest Workspaces replaced the directory sibling",
  );
  assertEqual(
    backend.settings.workspace_creation_prefs,
    { directory: ".zerocode/worktrees", nest_workspaces: true },
    "the two workspace creation patches did not coexist",
  );

  // The native picker is first; the field must commit its returned path.
  // window.mjs covers the picker UI and the in-window fallback.
  backend.pickedPaths = ["/tmp/browsed-workspaces"];
  from = backend.calls.length;
  await pageB.click("#workspace-directory-browse");
  await backend.waitForCall("B", "choose_paths", from);
  const browsed = await backend.waitForCall("B", "patch_workspace_creation_prefs", from);
  assertEqual(
    browsed.args,
    { patch: { kind: "directory", value: "/tmp/browsed-workspaces" } },
    "the browsed folder answer did not use the canonical directory patch",
  );
  await reopenSettings(pageB, "B", "general");
  assertEqual(
    await controlValue(pageB, "workspace_directory"),
    "/tmp/browsed-workspaces",
    "the browsed workspace directory did not survive a canonical refetch",
  );
  return "relative + nested → browsed absolute";
});

await test("Open In Apps persists presets and custom argv while stale windows merge by id", async () => {
  const defaultApps = [{ id: "vscode", label: "VS Code", command: "code" }];
  await backend.externalPatch({ open_in_applications: defaultApps }, ["open_in_applications"]);
  await openSettings(pageA, "general");
  await openSettings(pageB, "general");

  const initial = await pageB.locator("#open-in-applications-list").evaluate((list) => ({
    rows: [...list.querySelectorAll("[data-open-in-application]")].map((row) => row.dataset.openInApplication),
    count: document.getElementById("open-in-app-count")?.textContent,
    vscodeDisabled: document.querySelector('[data-open-in-preset="vscode"]')?.disabled,
  }));
  assertEqual(
    initial,
    { rows: ["vscode"], count: "1 / 8", vscodeDisabled: true },
    "fresh Settings did not derive its default and cap from the Rust snapshot",
  );

  // B deliberately misses A's event and still adds a different id. The item
  // patch must merge against the backend's latest document, not replace A.
  backend.dropNextDeliveryFor("B");
  let from = backend.calls.length;
  await pageA.click('[data-open-in-preset="cursor"]');
  const cursor = await backend.waitForCall("A", "patch_open_in_applications", from);
  assertEqual(
    cursor.args,
    { patch: { kind: "upsert", value: { id: "cursor", label: "Cursor", command: "cursor" } } },
    "Cursor preset did not use one application item patch",
  );
  from = backend.calls.length;
  await pageB.click('[data-open-in-preset="zed"]');
  await backend.waitForCall("B", "patch_open_in_applications", from);
  assertEqual(
    backend.settings.open_in_applications.map(({ id }) => id),
    ["vscode", "cursor", "zed"],
    "stale preset additions replaced one another",
  );

  await pageB.click("#open-in-add-custom");
  const draft = pageB.locator("#open-in-applications-list [data-open-in-application]").last();
  await draft.locator('[data-open-in-field="label"]').fill("Editor Preview");
  await draft.locator('[data-open-in-field="command"]').fill('"/opt/Editor Preview/bin/editor" --reuse');
  from = backend.calls.length;
  await draft.locator(".settings-open-in-actions button").first().click();
  const custom = await backend.waitForCall("B", "patch_open_in_applications", from);
  assert(
    custom.args.patch.kind === "upsert"
      && custom.args.patch.value.id.startsWith("custom-")
      && custom.args.patch.value.label === "Editor Preview"
      && custom.args.patch.value.command === '"/opt/Editor Preview/bin/editor" --reuse',
    "custom application lost its label, quoted command, or stable id",
    custom.args,
  );

  await reopenSettings(pageB, "B", "general");
  const restarted = await pageB.locator("#open-in-applications-list").evaluate((list) =>
    [...list.querySelectorAll("[data-open-in-application]")].map((row) => ({
      id: row.dataset.openInApplication,
      label: row.querySelector('[data-open-in-field="label"]')?.value,
      command: row.querySelector('[data-open-in-field="command"]')?.value,
    })));
  assertEqual(
    restarted.map(({ id }) => id),
    backend.settings.open_in_applications.map(({ id }) => id),
    "canonical Open in ordering did not survive a settings reopen",
  );
  assert(
    restarted.some((row) => row.label === "Editor Preview"
      && row.command === '"/opt/Editor Preview/bin/editor" --reuse'),
    "custom application did not survive a settings reopen",
    restarted,
  );

  const capped = Array.from({ length: backend.settings.open_in_applications_spec.max }, (_, index) => ({
    id: `cap-${index}`,
    label: `Editor ${index}`,
    command: `editor-${index}`,
  }));
  await backend.externalPatch({ open_in_applications: capped }, ["open_in_applications"]);
  await pageB.waitForFunction(() =>
    document.getElementById("open-in-app-count")?.textContent === "8 / 8");
  assert(await pageB.locator("#open-in-add-custom").isDisabled(), "the ninth custom app was offered");
  assert(
    await pageB.locator("#open-in-app-presets button:not(:disabled)").count() === 0,
    "a preset stayed enabled beyond Rust's maximum",
  );

  await backend.externalPatch({ open_in_applications: defaultApps }, ["open_in_applications"]);
  return "default + stale presets + custom + cap";
});

await test("workspace deletion confirmation persists Orca's safe-first opt-out", async () => {
  await openSettings(pageB, "general");
  const call = await gestureAndWait(
    pageB,
    "B",
    "set_skip_delete_worktree_confirm",
    () => pageB.click("#ask-before-delete-worktree"),
  );
  assertEqual(call.args, { skip: true }, "workspace deletion switch sent a different wire value");
  assertEqual(
    backend.settings.skip_delete_worktree_confirm,
    true,
    "backend did not store the workspace deletion opt-out",
  );
  await reopenSettings(pageB, "B", "general");
  assertEqual(
    await controlValue(pageB, "delete_worktree_confirm"),
    false,
    "workspace deletion confirmation did not survive a canonical refetch",
  );
});

await test("automation deletion confirmation persists Orca's run-history opt-out", async () => {
  await openSettings(pageB, "general");
  const call = await gestureAndWait(
    pageB,
    "B",
    "set_skip_delete_automation_confirm",
    () => pageB.click("#ask-before-delete-automation"),
  );
  assertEqual(call.args, { skip: true }, "automation deletion switch sent a different wire value");
  assertEqual(
    backend.settings.skip_delete_automation_confirm,
    true,
    "backend did not store the automation deletion opt-out",
  );
  await reopenSettings(pageB, "B", "general");
  assertEqual(
    await controlValue(pageB, "delete_automation_confirm"),
    false,
    "automation deletion confirmation did not survive a canonical refetch",
  );
});

await test("a committed write with a lost acknowledgement refetches canonical state", async () => {
  await openSettings(pageB, "appearance");
  const snapshots = backend.count("B", "settings_snapshot");
  backend.loseAcknowledgementOnce("set_theme");
  const mutation = await gestureAndWait(
    pageB,
    "B",
    "set_theme",
    () => pageB.selectOption("#app-theme", "light"),
  );
  await backend.waitForCall("B", "settings_snapshot", mutation.at + 1);
  await pageB.waitForFunction(
    () => document.getElementById("app-theme")?.value === "light",
    undefined,
    { timeout: UI_TIMEOUT },
  );
  assert(backend.settings.theme === "light", "backend did not commit the lost acknowledgement fixture");
  assert(
    backend.count("B", "settings_snapshot") > snapshots,
    "lost acknowledgement was mistaken for a definite rejection; canonical state was not refetched",
  );
});

await test("a lost terminal clipboard acknowledgement refetches the committed field", async () => {
  await openSettings(pageB, "terminal");
  const snapshots = backend.count("B", "settings_snapshot");
  backend.loseAcknowledgementOnce("patch_terminal_prefs");
  const mutation = await gestureAndWait(pageB, "B", "patch_terminal_prefs", () =>
    pageB.uncheck("#term-copy-on-select"));
  await backend.waitForCall("B", "settings_snapshot", mutation.at + 1);
  await pageB.waitForFunction(
    () => document.getElementById("term-copy-on-select")?.checked === false,
    undefined,
    { timeout: UI_TIMEOUT },
  );
  assert(
    backend.settings.terminal_prefs.copy_on_select === false,
    "backend did not retain the terminal clipboard commit whose answer was lost",
  );
  assert(
    backend.count("B", "settings_snapshot") > snapshots,
    "lost terminal clipboard acknowledgement did not refetch canonical settings",
  );
});

await test("a lost auto-save acknowledgement refetches the committed field", async () => {
  await openSettings(pageB, "editing");
  const snapshots = backend.count("B", "settings_snapshot");
  backend.loseAcknowledgementOnce("patch_editing_prefs");
  const mutation = await gestureAndWait(pageB, "B", "patch_editing_prefs", () =>
    pageB.uncheck("#editing-auto-save"));
  await backend.waitForCall("B", "settings_snapshot", mutation.at + 1);
  await pageB.waitForFunction(
    () => document.getElementById("editing-auto-save")?.checked === false,
    undefined,
    { timeout: UI_TIMEOUT },
  );
  assert(
    backend.settings.editing_prefs.editor_auto_save === false,
    "backend did not retain the auto-save commit whose answer was lost",
  );
  assert(
    backend.count("B", "settings_snapshot") > snapshots,
    "lost auto-save acknowledgement did not refetch canonical settings",
  );
});

await test("a stale lost-ack recovery read cannot roll back a newer clipboard mutation", async () => {
  await openSettings(pageB, "terminal");
  const oldAck = backend.deferLostAcknowledgementOnce("patch_terminal_prefs");
  const oldFrom = backend.calls.length;
  await pageB.check("#term-copy-on-select");
  await backend.waitForCall("B", "patch_terminal_prefs", oldFrom);
  await oldAck.reached;

  // Start a DIFFERENT clipboard field before the old request rejects, and
  // hold it before the backend can commit. The old recovery read therefore
  // captures right-click=true while the visible newer intent is false.
  const newer = backend.holdBeforeNext("patch_terminal_prefs");
  const newerFrom = backend.calls.length;
  await pageB.uncheck("#term-right-click-paste");
  await backend.waitForCall("B", "patch_terminal_prefs", newerFrom);
  await newer.reached;
  const optimistic = await pageB.isChecked("#term-right-click-paste");

  const releaseRecovery = backend.holdNextSnapshot("B");
  oldAck.release();
  await backend.waitForCall("B", "settings_snapshot", newerFrom);
  releaseRecovery();
  await waitForSettingsMutations(pageB, 1);
  const afterStaleRecovery = await pageB.isChecked("#term-right-click-paste");

  newer.release();
  await waitForSettingsIdle(pageB);
  await reopenSettings(pageB, "B", "terminal");
  const afterCanonicalRead = await pageB.isChecked("#term-right-click-paste");

  assert(optimistic === false, "newer clipboard intent was not painted optimistically");
  assert(
    afterStaleRecovery === false,
    "the older lost-ack recovery snapshot rolled back the newer clipboard intent",
  );
  assert(
    backend.settings.terminal_prefs.copy_on_select === true &&
      backend.settings.terminal_prefs.right_click_paste === false &&
      afterCanonicalRead === false,
    "newer clipboard intent did not converge with canonical settings",
  );
});

await test("an older full-snapshot success cannot roll back a newer clipboard mutation", async () => {
  await openSettings(pageB, "appearance");
  const nextTheme = backend.settings.theme === "light" ? "dark" : "light";
  const oldReached = backend.holdNext("set_theme");
  const oldFrom = backend.calls.length;
  await pageB.selectOption("#app-theme", nextTheme);
  await backend.waitForCall("B", "set_theme", oldFrom);
  await oldReached;

  await pageB.evaluate(() => showSettingsPane("terminal"));
  const newer = backend.holdBeforeNext("patch_terminal_prefs");
  const newerFrom = backend.calls.length;
  await pageB.check("#term-right-click-paste");
  await backend.waitForCall("B", "patch_terminal_prefs", newerFrom);
  await newer.reached;
  const optimistic = await pageB.isChecked("#term-right-click-paste");

  backend.release("set_theme");
  await waitForSettingsMutations(pageB, 1);
  const afterOlderSuccess = await pageB.isChecked("#term-right-click-paste");

  newer.release();
  await waitForSettingsIdle(pageB);
  await reopenSettings(pageB, "B", "terminal");
  const afterCanonicalRead = await pageB.isChecked("#term-right-click-paste");

  assert(optimistic === true, "newer clipboard success intent was not painted optimistically");
  assert(
    afterOlderSuccess === true,
    "an older full-snapshot success rolled back the newer clipboard intent",
  );
  assert(
    backend.settings.theme === nextTheme &&
      backend.settings.terminal_prefs.right_click_paste === true &&
      afterCanonicalRead === true,
    "held full-snapshot writes did not converge with canonical settings",
  );

  // Preserve the suite's established clipboard baseline for the later stale
  // window patch test: copy off, right-click paste on.
  const restoreFrom = backend.calls.length;
  await pageB.uncheck("#term-copy-on-select");
  await backend.waitForCall("B", "patch_terminal_prefs", restoreFrom);
  await waitForSettingsIdle(pageB);
});

await test("an unsafe generic refresh is deferred across a newer clipboard mutation", async () => {
  await openSettings(pageB, "terminal");
  const snapshots = backend.count("B", "settings_snapshot");
  const releaseStaleRead = backend.holdNextSnapshot("B");
  const staleFrom = backend.calls.length;
  await pageB.evaluate(() => void refreshSettingsSnapshot());
  await backend.waitForCall("B", "settings_snapshot", staleFrom);

  const newer = backend.holdBeforeNext("patch_terminal_prefs");
  // Make the mutation response stale on purpose. The deferred canonical read
  // must be what repairs this answer after protecting the optimistic value
  // from the older generic refresh.
  backend.answerOnce("patch_terminal_prefs", {
    terminal_prefs: {
      ...backend.settings.terminal_prefs,
      right_click_paste: true,
    },
  });
  const newerFrom = backend.calls.length;
  await pageB.uncheck("#term-right-click-paste");
  await backend.waitForCall("B", "patch_terminal_prefs", newerFrom);
  await newer.reached;
  const optimistic = await pageB.isChecked("#term-right-click-paste");

  releaseStaleRead();
  await pageB.waitForFunction(() => settingsRefreshDeferred, undefined, { timeout: UI_TIMEOUT });
  const afterStaleRefresh = await pageB.isChecked("#term-right-click-paste");

  const deferredFrom = backend.calls.length;
  newer.release();
  await backend.waitForCall("B", "settings_snapshot", deferredFrom);
  await pageB.waitForFunction(
    () => document.getElementById("term-right-click-paste")?.checked === false,
    undefined,
    { timeout: UI_TIMEOUT },
  );
  const converged = await pageB.isChecked("#term-right-click-paste");

  assert(optimistic === false, "newer clipboard intent was not painted optimistically");
  assert(
    afterStaleRefresh === false,
    "a generic full snapshot rolled back the newer clipboard intent",
  );
  assert(
    backend.count("B", "settings_snapshot") >= snapshots + 2 &&
      backend.settings.terminal_prefs.right_click_paste === false &&
      converged === false,
    "the deferred canonical refresh did not repair the stale mutation answer",
  );

  // Restore the established baseline for the later stale-window patch test.
  const restoreFrom = backend.calls.length;
  await pageB.check("#term-right-click-paste");
  await backend.waitForCall("B", "patch_terminal_prefs", restoreFrom);
  await waitForSettingsIdle(pageB);
});

await test("a later success cannot roll back an earlier clipboard mutation still in flight", async () => {
  await openSettings(pageB, "terminal");
  const clipboard = backend.holdBeforeNext("patch_terminal_prefs");
  const clipboardFrom = backend.calls.length;
  await pageB.uncheck("#term-right-click-paste");
  await backend.waitForCall("B", "patch_terminal_prefs", clipboardFrom);
  await clipboard.reached;
  const optimistic = await pageB.isChecked("#term-right-click-paste");

  // This later mutation owns the latest epoch, but its full snapshot was
  // captured before the earlier clipboard request could commit. Latest alone
  // is therefore not enough: only a sole in-flight mutation may apply one.
  await pageB.evaluate(() => showSettingsPane("appearance"));
  const nextTheme = backend.settings.theme === "light" ? "dark" : "light";
  const themeFrom = backend.calls.length;
  await pageB.selectOption("#app-theme", nextTheme);
  await backend.waitForCall("B", "set_theme", themeFrom);
  await waitForSettingsMutations(pageB, 1);
  const afterLaterSuccess = await pageB.isChecked("#term-right-click-paste");

  const deferredFrom = backend.calls.length;
  clipboard.release();
  await backend.waitForCall("B", "settings_snapshot", deferredFrom);
  await pageB.waitForFunction(
    () => document.getElementById("term-right-click-paste")?.checked === false,
    undefined,
    { timeout: UI_TIMEOUT },
  );
  const converged = await pageB.isChecked("#term-right-click-paste");

  assert(optimistic === false, "earlier clipboard intent was not painted optimistically");
  assert(
    afterLaterSuccess === false,
    "the later full-snapshot success rolled back an earlier in-flight clipboard intent",
  );
  assert(
    backend.settings.theme === nextTheme &&
      backend.settings.terminal_prefs.right_click_paste === false &&
      converged === false,
    "overlapping writes did not converge through the deferred canonical refresh",
  );

  await pageB.evaluate(() => showSettingsPane("terminal"));
  const restoreFrom = backend.calls.length;
  await pageB.check("#term-right-click-paste");
  await backend.waitForCall("B", "patch_terminal_prefs", restoreFrom);
  await waitForSettingsIdle(pageB);
});

await test("a change event updates the other open window without echoing to its origin", async () => {
  await openSettings(pageA, "appearance");
  await openSettings(pageB, "appearance");
  const revision = backend.settings.revision;
  const delivered = backend.deliveries.length;
  const bSnapshots = backend.count("B", "settings_snapshot");
  await gestureAndWait(pageA, "A", "set_theme", () => pageA.selectOption("#app-theme", "dark"));
  await pageB.waitForFunction(
    () => document.getElementById("app-theme")?.value === "dark",
    undefined,
    { timeout: UI_TIMEOUT },
  );
  const events = backend.deliveries.slice(delivered);
  assert(events.length === 1, "one mutation did not produce exactly one remote event", events);
  assert(events[0].window_id === "B", "origin window received its own settings event", events);
  assert(events[0].payload.revision === revision + 1, "event revision did not name the commit", events[0]);
  assertEqual(events[0].payload.keys, ["theme"], "event keys did not name the patch");
  assert(backend.count("B", "settings_snapshot") > bSnapshots, "remote window did not refetch canonical state");

  // A delayed event from an older commit is not another reason to read, and
  // especially not a reason to replace the canonical revision just applied.
  const afterFresh = backend.count("B", "settings_snapshot");
  await pageB.evaluate((payload) => {
    window.__SETTINGS_TEST_EMIT__("settings:changed", payload);
  }, { revision, keys: ["theme"] });
  await renderSettled(pageB);
  assert(
    backend.count("B", "settings_snapshot") === afterFresh,
    "stale settings event triggered a canonical refetch",
  );
  assert((await controlValue(pageB, "theme")) === "dark", "stale event changed the visible revision");
});

await test("stale windows preserve disjoint shortcut and task-source item patches", async () => {
  await openSettings(pageA, "appearance");
  await openSettings(pageB, "appearance");

  // Make B stale for A's first item change, then let B patch another item
  // from that stale view. Item commands must preserve both backend facts.
  backend.dropNextDeliveryFor("B");
  const shortcutA = backend.calls.length;
  await pageA.uncheck("#show-tasks");
  await backend.waitForCall("A", "set_shortcut_visibility", shortcutA);
  const shortcutB = backend.calls.length;
  await pageB.uncheck("#show-automations");
  await backend.waitForCall("B", "set_shortcut_visibility", shortcutB);
  assertEqual(
    backend.settings.hidden_shortcuts,
    ["automations", "tasks"],
    "stale shortcut patch replaced the other window's item",
  );

  // Start with Jira hidden in both windows. A brings Jira back; while B is
  // stale, B hides GitHub. The valid combined answer is only GitHub hidden —
  // A's removal and B's addition both survive.
  await backend.externalPatch({ hidden_task_sources: ["jira"] }, ["hidden_task_sources"]);
  await pageA.evaluate(() => showSettingsPane("tasks"));
  await pageB.evaluate(() => showSettingsPane("tasks"));
  await pageA.waitForFunction(() =>
    document.getElementById("show-source-jira")?.checked === false
      && document.getElementById("show-source-github")?.checked === true);
  await pageB.waitForFunction(() =>
    document.getElementById("show-source-jira")?.checked === false
      && document.getElementById("show-source-github")?.checked === true);
  backend.dropNextDeliveryFor("B");
  const sourceA = backend.calls.length;
  await pageA.check("#show-source-jira");
  await backend.waitForCall("A", "set_task_source_visibility", sourceA);
  const sourceB = backend.calls.length;
  await pageB.uncheck("#show-source-github");
  await backend.waitForCall("B", "set_task_source_visibility", sourceB);
  assertEqual(
    backend.settings.hidden_task_sources,
    ["github"],
    "stale task-source patch replaced the other window's item",
  );
});

await test("stale windows cannot assign one canonical chord to two actions", async () => {
  await openSettings(pageA, "shortcuts");
  await openSettings(pageB, "shortcuts");
  const chord = "mod+alt+k";
  const before = backend.settings.revision;

  backend.dropNextDeliveryFor("B");
  let from = backend.calls.length;
  await pageA.evaluate((binding) =>
    setKeybinding("terminal.equalizePaneSizes", [binding]), chord);
  await backend.waitForCall("A", "set_keybinding", from);
  assert(
    backend.settings.revision === before + 1,
    "first chord assignment did not commit",
  );

  const snapshots = backend.count("B", "settings_snapshot");
  from = backend.calls.length;
  await pageB.evaluate((binding) =>
    setKeybinding("terminal.setTitle", [binding]), chord);
  await backend.waitForCall("B", "set_keybinding", from);
  await renderSettled(pageB);

  assert(
    backend.settings.revision === before + 1,
    "refused stale chord assignment advanced the canonical revision",
  );
  assertEqual(
    backend.settings.keybindings,
    { "terminal.equalizePaneSizes": [chord] },
    "refused stale chord assignment changed the canonical map",
  );
  const visible = await pageB.evaluate(() => ({
    revision: settingsRevision,
    first: keybindingOverrides["terminal.equalizePaneSizes"],
    second: keybindingOverrides["terminal.setTitle"],
  }));
  assertEqual(
    visible,
    { revision: before + 1, first: [chord], second: undefined },
    "stale window did not roll back to the authoritative chord map",
  );
  assert(
    backend.count("B", "settings_snapshot") > snapshots,
    "conflict refusal did not refetch canonical settings",
  );

  backend.dropNextDeliveryFor("A");
  from = backend.calls.length;
  await pageB.evaluate(() => setKeybinding("file.save", []));
  await backend.waitForCall("B", "set_keybinding", from);
  backend.dropNextDeliveryFor("A");
  from = backend.calls.length;
  await pageB.evaluate(() => setKeybinding("terminal.setTitle", ["mod+s"]));
  await backend.waitForCall("B", "set_keybinding", from);

  const beforeReset = backend.settings.revision;
  const beforeResetMap = clone(backend.settings.keybindings);
  const resetSnapshots = backend.count("A", "settings_snapshot");
  from = backend.calls.length;
  await pageA.evaluate(() => setKeybinding("file.save", null));
  await backend.waitForCall("A", "set_keybinding", from);
  await renderSettled(pageA);
  assert(
    backend.settings.revision === beforeReset,
    "refused stale reset advanced the canonical revision",
  );
  assertEqual(
    backend.settings.keybindings,
    beforeResetMap,
    "refused stale reset changed the canonical map",
  );
  const afterReset = await pageA.evaluate(() => ({
    revision: settingsRevision,
    save: keybindingOverrides["file.save"],
    title: keybindingOverrides["terminal.setTitle"],
  }));
  assertEqual(
    afterReset,
    { revision: beforeReset, save: [], title: ["mod+s"] },
    "stale reset did not roll back to the authoritative chord map",
  );
  assert(
    backend.count("A", "settings_snapshot") > resetSnapshots,
    "conflict refusal after reset did not refetch canonical settings",
  );
});

await test("stale windows send only the terminal control each person changed", async () => {
  await openSettings(pageA, "terminal");
  await openSettings(pageB, "terminal");

  backend.dropNextDeliveryFor("B");
  let from = backend.calls.length;
  await pageA.selectOption("#term-cursor-style", "bar");
  const cursor = await backend.waitForCall("A", "patch_terminal_prefs", from);
  assertEqual(
    cursor.args,
    { patch: { kind: "cursor_style", value: "bar" } },
    "terminal gesture sent a stale full preference record",
  );

  from = backend.calls.length;
  await blurField(pageB, "#term-divider-thickness", 11);
  const divider = await backend.waitForCall("B", "patch_terminal_prefs", from);
  assertEqual(
    divider.args,
    { patch: { kind: "divider_thickness_px", value: 11 } },
    "second terminal gesture sent fields it did not change",
  );
  assert(
    backend.settings.terminal_prefs.cursor_style === "bar"
      && backend.settings.terminal_prefs.divider_thickness_px === 11,
    "disjoint terminal patches did not both survive",
    backend.settings.terminal_prefs,
  );

  from = backend.calls.length;
  await blurField(pageA, "#term-cursor-opacity", 0.4);
  const cursorOpacity = await backend.waitForCall("A", "patch_terminal_prefs", from);
  assertEqual(
    cursorOpacity.args,
    { patch: { kind: "cursor_opacity", value: 0.4 } },
    "cursor opacity sent a stale terminal record",
  );
  await waitForSettingsIdle(pageA);

  backend.dropNextDeliveryFor("B");
  from = backend.calls.length;
  await blurField(pageA, "#term-padding-x", 24);
  const paddingX = await backend.waitForCall("A", "patch_terminal_prefs", from);
  assertEqual(
    paddingX.args,
    { patch: { kind: "padding_x", value: 24 } },
    "horizontal padding sent a stale terminal record",
  );

  from = backend.calls.length;
  await blurField(pageB, "#term-padding-y", 18);
  const paddingY = await backend.waitForCall("B", "patch_terminal_prefs", from);
  assertEqual(
    paddingY.args,
    { patch: { kind: "padding_y", value: 18 } },
    "vertical padding sent a stale terminal record",
  );
  assert(
    backend.settings.terminal_prefs.padding_x === 24
      && backend.settings.terminal_prefs.padding_y === 18,
    "disjoint terminal-padding patches did not both survive",
    backend.settings.terminal_prefs,
  );

  backend.dropNextDeliveryFor("B");
  from = backend.calls.length;
  await pageA.check("#term-copy-on-select");
  const copyOnSelect = await backend.waitForCall("A", "patch_terminal_prefs", from);
  assertEqual(
    copyOnSelect.args,
    { patch: { kind: "copy_on_select", value: true } },
    "copy-on-select sent a stale terminal record",
  );

  from = backend.calls.length;
  await pageB.$eval("#term-right-click-paste", (field) => {
    field.checked = false;
    field.dispatchEvent(new Event("change", { bubbles: true }));
  });
  const rightClickPaste = await backend.waitForCall("B", "patch_terminal_prefs", from);
  assertEqual(
    rightClickPaste.args,
    { patch: { kind: "right_click_paste", value: false } },
    "right-click paste sent a stale sibling",
  );
  assert(
    backend.settings.terminal_prefs.copy_on_select === true
      && backend.settings.terminal_prefs.right_click_paste === false,
    "disjoint clipboard preference patches did not both survive",
    backend.settings.terminal_prefs,
  );

  backend.dropNextDeliveryFor("B");
  from = backend.calls.length;
  await pageA.uncheck("#term-hide-mouse-while-typing");
  const hideMouse = await backend.waitForCall("A", "patch_terminal_prefs", from);
  assertEqual(
    hideMouse.args,
    { patch: { kind: "hide_mouse_while_typing", value: false } },
    "hide-mouse-while-typing sent a stale terminal record",
  );

  from = backend.calls.length;
  await pageB.$eval("#term-right-click-paste", (field) => {
    field.checked = true;
    field.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await backend.waitForCall("B", "patch_terminal_prefs", from);
  assert(
    backend.settings.terminal_prefs.hide_mouse_while_typing === false
      && backend.settings.terminal_prefs.right_click_paste === true,
    "hide-pointer and clipboard patches did not preserve one another",
    backend.settings.terminal_prefs,
  );
});

await test("vault.sessionLimit is saved, broadcast, and restored through the canonical snapshot", async () => {
  for (const limit of [50, 100, 200, 0]) {
    const from = backend.calls.length;
    await pageA.evaluate(limit => setVaultSessionLimit(limit), limit);
    await backend.waitForCall("A", "set_vault_session_limit", from);
    await pageB.waitForFunction(limit => vaultSessionLimit === limit, limit);
    assertEqual(backend.settings["vault.sessionLimit"], limit, "the vault session limit was not saved");
  }
  await pageA.evaluate(() => { vaultSessionLimit = null; });
  await pageA.evaluate(() => refreshSettingsSnapshot());
  assertEqual(await pageA.evaluate(() => vaultSessionLimit), 0, "all sessions did not survive a snapshot reload");
});

await test("crash watchdog overlays round-trip and reach another window", async () => {
  await openSettings(pageA, "general");
  await openSettings(pageB, "general");
  await backend.externalPatch({ crash: {} }, ["crash"]);
  await pageA.waitForFunction(() => document.getElementById("crash-watchdog").checked);
  const from = backend.calls.length;
  await pageA.locator("#crash-watchdog").uncheck();
  const off = await backend.waitForCall("A", "set_crash_watchdog", from);
  assertEqual(off.args, { patch: { watchdog: false } }, "watchdog toggle replaced another field");
  await pageB.waitForFunction(() => !document.getElementById("crash-watchdog").checked);
  assert(await pageB.locator("#crash-first-ms").isDisabled(), "disabled watchdog still edits thresholds");
  await pageA.locator("#crash-watchdog").check();
  await pageB.waitForFunction(() => document.getElementById("crash-watchdog").checked);
  await blurField(pageA, "#crash-first-ms", 3000);
  await pageB.waitForFunction(() => document.getElementById("crash-first-ms").value === "3000");
  await blurField(pageB, "#crash-second-ms", 8000);
  await pageA.waitForFunction(() => document.getElementById("crash-second-ms").value === "8000");
  assertEqual(backend.settings.crash, { watchdog: true, first_ms: 3000, second_ms: 8000 }, "crash overlay did not persist field patches");
  await backend.externalPatch({ crash: {} }, ["crash"]);
});

await test("the artifact retention days round-trip through the automations pane and follow the ledger at zero", async () => {
  await openSettings(pageA, "automations");
  await openSettings(pageB, "automations");
  await backend.externalPatch({ artifacts_retention_days: 7 }, ["artifacts_retention_days"]);
  await pageA.waitForFunction(() => document.getElementById("artifacts-retention-days")?.value === "7");
  await pageB.waitForFunction(() => document.getElementById("artifacts-retention-days")?.value === "7");
  const bounds = await pageA.$eval("#artifacts-retention-days", (field) => ({
    min: field.min, max: field.max, step: field.step,
  }));
  assertEqual(bounds, { min: "0", max: "365", step: "1" }, "the field's bounds are not the backend's spec");
  let from = backend.calls.length;
  await blurField(pageA, "#artifacts-retention-days", 14);
  const changed = await backend.waitForCall("A", "set_artifacts_retention_days", from);
  assertEqual(changed.args, { days: 14 }, "the retention days did not reach the backend as sent");
  // The other window hearing 14 is the proof the store kept it — read the
  // store only after that, not in the gap between the call and its commit.
  await pageB.waitForFunction(() => document.getElementById("artifacts-retention-days")?.value === "14");
  assertEqual(backend.settings.artifacts_retention_days, 14, "the backend did not keep the days");
  // Zero means "the ledger's own days", and the hint says so.
  from = backend.calls.length;
  await blurField(pageA, "#artifacts-retention-days", 0);
  await backend.waitForCall("A", "set_artifacts_retention_days", from);
  await pageA.waitForFunction(() => document.getElementById("artifacts-retention-days")?.value === "0");
  const hint = await pageA.$eval("#artifacts-retention-days-hint", (node) => node.textContent);
  assert(hint.trim() !== "", "the retention hint went blank", hint);
  // Past the cap: clamped by the backend, and the field shows the clamp.
  from = backend.calls.length;
  await blurField(pageA, "#artifacts-retention-days", 9999);
  await backend.waitForCall("A", "set_artifacts_retention_days", from);
  await pageA.waitForFunction(() => document.getElementById("artifacts-retention-days")?.value === "365");
});

await test("stale windows preserve disjoint auto-save field patches", async () => {
  await openSettings(pageA, "editing");
  await openSettings(pageB, "editing");
  await backend.externalPatch({
    editing_prefs: {
      editor_auto_save: false,
      editor_auto_save_delay_ms: 1000,
      editor_word_wrap: true,
      diff_word_wrap: false,
    },
  }, ["editing_prefs"]);
  await pageA.waitForFunction(() => !document.getElementById("editing-auto-save")?.checked);
  await pageB.waitForFunction(() => document.getElementById("editing-auto-save-delay")?.value === "1000");

  backend.dropNextDeliveryFor("B");
  let from = backend.calls.length;
  await pageA.check("#editing-auto-save");
  const enabled = await backend.waitForCall("A", "patch_editing_prefs", from);
  assertEqual(
    enabled.args,
    { patch: { kind: "editor_auto_save", value: true } },
    "auto-save sent its stale sibling",
  );

  from = backend.calls.length;
  await blurField(pageB, "#editing-auto-save-delay", 2250);
  const delay = await backend.waitForCall("B", "patch_editing_prefs", from);
  assertEqual(
    delay.args,
    { patch: { kind: "editor_auto_save_delay_ms", value: 2250 } },
    "auto-save delay sent its stale sibling",
  );
  assertEqual(
    backend.settings.editing_prefs,
    {
      editor_auto_save: true,
      editor_auto_save_delay_ms: 2250,
      editor_word_wrap: true,
      diff_word_wrap: false,
    },
    "disjoint editing patches did not both survive",
  );
});

await test("a panel gesture sends only the side that moved", async () => {
  backend.dropNextDeliveryFor("B");
  let from = backend.calls.length;
  await pageA.focus("#grip-sidebar");
  await pageA.keyboard.press("ArrowRight");
  const sidebar = await backend.waitForCall("A", "set_panel_width", from);
  assertEqual(
    sidebar.args,
    { side: "sidebar", width: 296 },
    "panel gesture sent the stale width of the untouched column",
  );
  from = backend.calls.length;
  await pageB.focus("#grip-aside");
  await pageB.keyboard.press("ArrowLeft");
  const aside = await backend.waitForCall("B", "set_panel_width", from);
  assertEqual(
    aside.args,
    { side: "aside", width: 366 },
    "aside gesture sent the stale width of the untouched column",
  );
  assertEqual(
    backend.settings.panel_widths,
    { sidebar: 296, aside: 366 },
    "stale panel gestures did not preserve both columns",
  );
});

await test("floating workspace settings persist and reach their two triggers", async () => {
  await openSettings(pageA, "floating-workspace");
  // The Rust default, painted: enabled, the untouched `~`, floating button.
  assert(await pageA.isChecked("#floating-enabled"), "the enable switch did not start on");
  assertEqual(
    await pageA.$eval("#floating-cwd", (field) => field.value),
    "~",
    "an untouched directory did not read as the literal ~",
  );
  assertEqual(
    await pageA.$$eval("#floating-trigger-choice .segment-btn", (buttons) =>
      buttons.map((button) => ({
        location: button.dataset.location,
        pressed: button.getAttribute("aria-pressed"),
      }))),
    [
      { location: "floating-button", pressed: "true" },
      { location: "status-bar", pressed: "false" },
    ],
    "the trigger choice did not start on Orca's floating-button default",
  );
  // Exactly one door stands, and it is the default one.
  assertEqual(
    await pageA.evaluate(() => ({
      button: el("float-toggle").hidden,
      square: el("toggle-term").hidden,
      aside: el("activity-term-tab").hidden,
    })),
    { button: false, square: true, aside: false },
    "the default doors are not button-standing, square-down, aside-standing",
  );

  // Moving the trigger is one field patch, and the two doors swap.
  let from = backend.calls.length;
  await pageA.click('#floating-trigger-choice [data-location="status-bar"]');
  let saved = await backend.waitForCall("A", "patch_floating_workspace", from);
  assertEqual(
    saved.args,
    { patch: { kind: "trigger_location", value: "status-bar" } },
    "the trigger move did not travel as one field patch",
  );
  assertEqual(
    await pageA.evaluate(() => ({
      button: el("float-toggle").hidden,
      square: el("toggle-term").hidden,
    })),
    { button: true, square: false },
    "moving the trigger did not swap the two doors",
  );

  // The native picker's answer travels through the same canonical field patch.
  backend.pickedPaths = ["/tmp/floating-seat"];
  from = backend.calls.length;
  await pageA.click("#floating-cwd-browse");
  await backend.waitForCall("A", "choose_paths", from);
  saved = await backend.waitForCall("A", "patch_floating_workspace", from);
  assertEqual(
    saved.args,
    { patch: { kind: "cwd", value: "/tmp/floating-seat" } },
    "the picked directory did not travel as a cwd patch",
  );
  assertEqual(
    backend.settings.floating_workspace,
    { enabled: true, cwd: "/tmp/floating-seat", trigger_location: "status-bar" },
    "the backend did not persist the picked seat beside the moved trigger",
  );
  assertEqual(
    await pageA.$eval("#floating-cwd", (field) => field.value),
    "/tmp/floating-seat",
    "the field does not show the chosen seat",
  );

  // Disabling takes down every door, and the canonical snapshot restores it.
  from = backend.calls.length;
  await pageA.uncheck("#floating-enabled");
  saved = await backend.waitForCall("A", "patch_floating_workspace", from);
  assertEqual(
    saved.args,
    { patch: { kind: "enabled", value: false } },
    "the enable switch did not travel as one field patch",
  );
  assertEqual(
    await pageA.evaluate(() => ({
      button: el("float-toggle").hidden,
      square: el("toggle-term").hidden,
      aside: el("activity-term-tab").hidden,
    })),
    { button: true, square: true, aside: true },
    "disabling did not take down every door",
  );
  await reopenSettings(pageA, "A", "floating-workspace");
  assert(
    !(await pageA.isChecked("#floating-enabled")),
    "the disable did not restore from the canonical snapshot",
  );

  // The defaults go back for the suites that follow this one.
  await backend.externalPatch(
    { floating_workspace: { enabled: true, cwd: "~", trigger_location: "floating-button" } },
    ["floating_workspace"],
  );
});

await test("available Orca status-bar indicators persist as item patches", async () => {
  await openSettings(pageA, "appearance");
  // A saved choice stands. A fresh install now boots with Orca's full
  // default set (status-bar-defaults.ts — every item ON), so the four-item
  // document an upgrade inherits has to be handed over explicitly; the
  // Newer gauges must then arrive unticked instead of being
  // re-defaulted on.
  const stored = ["claude", "codex", "resource-usage", "ports"];
  await backend.externalPatch({ status_bar_items: stored }, ["status_bar_items"]);
  const controls = ["claude", "codex", "antigravity", "kimi", "grok", "opencode-go", "resource-usage", "ports"];
  assertEqual(
    await pageA.$$eval("[data-status-bar-item]", (fields) =>
      fields.map((field) => ({ item: field.dataset.statusBarItem, checked: field.checked }))),
    controls.map((item) => ({ item, checked: stored.includes(item) })),
    "Appearance did not expose exactly the live status-bar consumers",
  );

  const from = backend.calls.length;
  await pageA.uncheck("#status-bar-resource-usage");
  const saved = await backend.waitForCall("A", "set_status_bar_item", from);
  assertEqual(
    saved.args,
    { item: "resource-usage", enabled: false },
    "status visibility sent sibling state instead of one item",
  );
  assert(
    !backend.settings.status_bar_items.includes("resource-usage"),
    "backend did not persist the hidden resource indicator",
  );
  await reopenSettings(pageA, "A", "appearance");
  assert(
    !(await pageA.isChecked("#status-bar-resource-usage")),
    "status visibility did not restore from the canonical snapshot",
  );

  const restoreFrom = backend.calls.length;
  await pageA.check("#status-bar-resource-usage");
  await backend.waitForCall("A", "set_status_bar_item", restoreFrom);
});

await test("usage percentages default to Used and persist Orca's Remaining choice", async () => {
  await openSettings(pageA, "appearance");
  assertEqual(
    await pageA.$$eval("#usage-percentage-display .segment-btn", (buttons) =>
      buttons.map((button) => ({
        value: button.dataset.value,
        pressed: button.getAttribute("aria-pressed"),
      }))),
    [
      { value: "used", pressed: "true" },
      { value: "remaining", pressed: "false" },
    ],
    "Appearance did not start on Orca's Used default",
  );

  let from = backend.calls.length;
  await pageA.click("#usage-percentage-remaining");
  let saved = await backend.waitForCall("A", "set_usage_percentage_display", from);
  assertEqual(saved.args, { display: "remaining" }, "Remaining sent sibling settings");
  assertEqual(
    backend.settings.usage_percentage_display,
    "remaining",
    "backend did not persist Remaining",
  );

  await reopenSettings(pageA, "A", "appearance");
  assertEqual(
    await pageA.getAttribute("#usage-percentage-remaining", "aria-pressed"),
    "true",
    "Remaining did not restore from the canonical snapshot",
  );

  from = backend.calls.length;
  await pageA.click("#usage-percentage-used");
  saved = await backend.waitForCall("A", "set_usage_percentage_display", from);
  assertEqual(saved.args, { display: "used" }, "Used sent sibling settings");
});

await test("the usage footer defaults to Detailed and persists Orca's Compact mode", async () => {
  await pageA.evaluate(() => setSettingsOpen(false));
  await renderSettled(pageA);
  await pageA.click("#sb-claude");
  await renderSettled(pageA);
  assertEqual(
    await pageA.$$eval("#usage-mode .segment-btn", (buttons) =>
      buttons.map((button) => ({
        value: button.dataset.value,
        pressed: button.getAttribute("aria-pressed"),
      }))),
    [
      { value: "verbose", pressed: "true" },
      { value: "compact", pressed: "false" },
    ],
    "usage footer did not start on Orca's Detailed default",
  );

  let from = backend.calls.length;
  await pageA.click("#usage-mode-compact");
  let saved = await backend.waitForCall("A", "set_status_bar_usage_mode", from);
  assertEqual(saved.args, { mode: "compact" }, "Compact sent sibling settings");
  assertEqual(backend.settings.status_bar_usage_mode, "compact", "backend did not persist Compact");

  await pageA.click("#sb-claude");
  await reopenSettings(pageA, "A", "appearance");
  await pageA.evaluate(() => setSettingsOpen(false));
  await renderSettled(pageA);
  await pageA.click("#sb-claude");
  await renderSettled(pageA);
  assertEqual(
    await pageA.getAttribute("#usage-mode-compact", "aria-pressed"),
    "true",
    "Compact did not restore from the canonical snapshot",
  );

  from = backend.calls.length;
  await pageA.evaluate(() => setStatusBarUsageMode("verbose"));
  saved = await backend.waitForCall("A", "set_status_bar_usage_mode", from);
  assertEqual(saved.args, { mode: "verbose" }, "Detailed sent sibling settings");
  await pageA.evaluate(() => {
    if (!document.getElementById("usage-pop").hidden) setUsageOpen(false);
  });
});

await test("stale windows preserve disjoint status-bar item patches", async () => {
  await openSettings(pageA, "appearance");
  await openSettings(pageB, "appearance");
  backend.dropNextDeliveryFor("B");

  let from = backend.calls.length;
  await pageA.uncheck("#status-bar-claude");
  const claude = await backend.waitForCall("A", "set_status_bar_item", from);
  assertEqual(claude.args, { item: "claude", enabled: false }, "Claude sent sibling state");

  from = backend.calls.length;
  await pageB.uncheck("#status-bar-ports");
  const ports = await backend.waitForCall("B", "set_status_bar_item", from);
  assertEqual(ports.args, { item: "ports", enabled: false }, "Ports sent sibling state");
  assertEqual(
    backend.settings.status_bar_items,
    ["codex", "resource-usage"],
    "stale status-bar writes lost one of the disjoint changes",
  );

  await reopenSettings(pageB, "B", "appearance");
  assert(
    !(await pageB.isChecked("#status-bar-claude"))
      && !(await pageB.isChecked("#status-bar-ports")),
    "canonical status visibility did not survive a settings reopen",
  );

  // Back to the fresh-install document the suite booted with — six items,
  // the serialized Rust default.
  await backend.externalPatch({
    status_bar_items: ["claude", "codex", "antigravity", "kimi", "grok", "opencode-go", "resource-usage", "ports"],
  }, ["status_bar_items"]);
});

await test("browser default zoom is Rust-derived, persisted, and restored", async () => {
  await openSettings(pageA, "browser");
  const choices = await pageA.$$eval("#browser-default-zoom option", (options) =>
    options.map((option) => ({ value: option.value, label: option.textContent })));
  assertEqual(
    [choices.length, choices[0], choices[6], choices.at(-1)],
    [17, { value: "-3", label: "58%" }, { value: "0", label: "100%" }, { value: "5", label: "249%" }],
    "browser zoom choices drifted from the Rust-owned Orca spec",
  );

  const from = backend.calls.length;
  await pageA.selectOption("#browser-default-zoom", "1.5");
  const saved = await backend.waitForCall("A", "set_browser_default_zoom", from);
  assertEqual(saved.args, { zoomLevel: 1.5 }, "browser zoom sent sibling state");
  assert(
    backend.settings.browser.default_zoom_level === 1.5,
    "browser zoom did not persist its canonical level",
  );

  await reopenSettings(pageA, "A", "browser");
  assert(
    await pageA.$eval("#browser-default-zoom", (field) => field.value) === "1.5",
    "browser zoom did not restore from the canonical snapshot",
  );
});

await test("browser link routing keeps Orca defaults and patches both choices independently", async () => {
  await openSettings(pageA, "browser");
  await openSettings(pageB, "browser");
  assert(
    !(await pageA.isChecked("#browser-open-links-in-app"))
      && !(await pageA.isChecked("#browser-link-routing-modifier-inverts")),
    "browser link routing did not start at Orca's system-browser defaults",
  );

  assert(
    await pageA.isChecked("#browser-terminal-link-actions"),
    "terminal link actions did not start at Orca's popover-on default",
  );

  backend.dropNextDeliveryFor("B");
  let from = backend.calls.length;
  await pageA.check("#browser-open-links-in-app");
  const destination = await backend.waitForCall("A", "patch_browser_link_routing", from);
  assertEqual(
    destination.args,
    { patch: { kind: "open_links_in_app", value: true } },
    "browser link destination sent sibling state",
  );
  // 실측 BrowserLinkRoutingSetting: 스위치가 prompted를 함께 적는다 — 토글에
  // 손을 댄 것 자체가 첫-클릭 질문의 답이다.
  const prompted = await backend.waitForCall(
    "A", "patch_browser_link_routing", destination.at + 1,
  );
  assertEqual(
    prompted.args,
    { patch: { kind: "open_links_in_app_prompted", value: true } },
    "the link routing switch stopped recording the one-time answer",
  );

  from = backend.calls.length;
  await pageB.check("#browser-link-routing-modifier-inverts");
  const modifier = await backend.waitForCall("B", "patch_browser_link_routing", from);
  assertEqual(
    modifier.args,
    { patch: { kind: "open_links_in_app_modifier_inverts", value: true } },
    "browser link modifier sent sibling state",
  );
  from = backend.calls.length;
  await pageA.uncheck("#browser-terminal-link-actions");
  const popover = await backend.waitForCall("A", "patch_browser_link_routing", from);
  assertEqual(
    popover.args,
    { patch: { kind: "terminal_link_action_popover", value: false } },
    "terminal link actions sent sibling state",
  );
  assert(
    backend.settings.browser.open_links_in_app === true
      && backend.settings.browser.open_links_in_app_modifier_inverts === true
      && backend.settings.browser.open_links_in_app_prompted === true
      && backend.settings.browser.terminal_link_action_popover === false,
    "stale browser link patches replaced a sibling choice",
    backend.settings.browser,
  );

  await reopenSettings(pageB, "B", "browser");
  assert(
    await pageB.isChecked("#browser-open-links-in-app")
      && await pageB.isChecked("#browser-link-routing-modifier-inverts")
      && !(await pageB.isChecked("#browser-terminal-link-actions")),
    "canonical browser link routing did not survive a settings reopen",
  );
  assert(
    (await pageB.textContent("#browser-link-routing-description")).includes("http(s)"),
    "link routing description stopped naming its exact scheme boundary",
  );
});

await test("per-site user agents are one row patch: set by host, remove by host, a stale window keeps its sibling's row, and the row says 다음 판부터", async () => {
  await openSettings(pageA, "browser");
  await openSettings(pageB, "browser");
  assert(
    await pageA.$eval("#browser-user-agent-empty", (node) => !node.hidden),
    "the per-site user-agent table did not start empty",
  );
  // The words, in whichever language an earlier case left the window in:
  // the row reaches the NEXT pane, never a running one.
  const nextPaneWords = {
    ko: "다음 판부터",
    en: "from the next tab on",
    ja: "次のタブから",
    zh: "下一个标签页起",
    es: "siguiente pestaña",
  };
  const hintLang = await pageA.evaluate(() => document.documentElement.lang);
  assert(
    (await pageA.textContent("#browser-user-agents-hint")).includes(nextPaneWords[hintLang] ?? "다음 판부터"),
    `the hint stopped saying a row reaches the next pane (${hintLang})`,
  );

  backend.dropNextDeliveryFor("B");
  let from = backend.calls.length;
  await pageA.fill("#browser-user-agent-host", " Confluence.Example.COM ");
  await pageA.fill("#browser-user-agent-agent", " Mozilla/5.0 (X11) Chrome/1 ");
  await pageA.click("#browser-user-agent-submit");
  const set = await backend.waitForCall("A", "patch_browser_user_agents", from);
  assertEqual(
    set.args,
    { patch: { kind: "set", value: { host: "Confluence.Example.COM", agent: "Mozilla/5.0 (X11) Chrome/1" } } },
    "the add did not send one row patch",
  );
  await waitForSettingsIdle(pageA);
  assertEqual(
    backend.settings.browser.user_agents,
    [{ host: "confluence.example.com", agent: "Mozilla/5.0 (X11) Chrome/1" }],
    "the row did not persist canonical",
  );
  const rowsOf = (page) => page.$$eval("#browser-user-agent-list .browser-ua-row", (rows) =>
    rows.map((row) => ({
      host: row.querySelector(".browser-ua-host")?.textContent ?? "",
      agent: row.querySelector(".browser-ua-agent")?.textContent ?? "",
      key: row.dataset.host,
    })));
  assertEqual(
    await rowsOf(pageA),
    [{ host: "confluence.example.com", agent: "Mozilla/5.0 (X11) Chrome/1", key: "confluence.example.com" }],
    "A did not paint the canonical row",
  );
  assert(
    await pageA.$eval("#browser-user-agent-host", (node) => node.value === "")
      && await pageA.$eval("#browser-user-agent-agent", (node) => node.value === ""),
    "the add form did not clear after a saved row",
  );
  assert(
    await pageA.$eval("#browser-user-agent-empty", (node) => node.hidden),
    "the empty notice stayed up beside a row",
  );

  // B is stale (its delivery was dropped) and adds a second host: a ROW
  // patch keeps A's row where a table replace would have erased it.
  from = backend.calls.length;
  await pageB.fill("#browser-user-agent-host", "x.test");
  await pageB.fill("#browser-user-agent-agent", "UA-x");
  await pageB.press("#browser-user-agent-agent", "Enter");
  await backend.waitForCall("B", "patch_browser_user_agents", from);
  await waitForSettingsIdle(pageB);
  assertEqual(
    backend.settings.browser.user_agents.map((row) => row.host),
    ["confluence.example.com", "x.test"],
    "a stale window's row patch erased a sibling's row",
  );

  // The same host rewrites its row in place — never a second row.
  from = backend.calls.length;
  await pageA.fill("#browser-user-agent-host", "CONFLUENCE.example.com");
  await pageA.fill("#browser-user-agent-agent", "UA-2");
  await pageA.click("#browser-user-agent-submit");
  await backend.waitForCall("A", "patch_browser_user_agents", from);
  await waitForSettingsIdle(pageA);
  assertEqual(
    backend.settings.browser.user_agents,
    [{ host: "confluence.example.com", agent: "UA-2" }, { host: "x.test", agent: "UA-x" }],
    "a rewrite made a second row",
  );

  // The delete hand sends `remove` by host and drops only that row.
  from = backend.calls.length;
  await pageA.click(
    '#browser-user-agent-list .browser-ua-row[data-host="confluence.example.com"] .browser-ua-del',
  );
  const removed = await backend.waitForCall("A", "patch_browser_user_agents", from);
  assertEqual(
    removed.args,
    { patch: { kind: "remove", value: "confluence.example.com" } },
    "delete did not send a remove patch",
  );
  await waitForSettingsIdle(pageA);
  assertEqual(
    backend.settings.browser.user_agents,
    [{ host: "x.test", agent: "UA-x" }],
    "remove dropped the wrong row",
  );

  // A refused row (a URL, not a host) rolls back visibly, says why, and
  // leaves the words in the fields to fix.
  from = backend.calls.length;
  await pageA.fill("#browser-user-agent-host", "https://bad.test/path");
  await pageA.fill("#browser-user-agent-agent", "UA-bad");
  await pageA.click("#browser-user-agent-submit");
  await backend.waitForCall("A", "patch_browser_user_agents", from);
  await waitForSettingsIdle(pageA);
  await pageA.waitForFunction(
    () => (document.getElementById("browser-user-agent-status")?.textContent ?? "") !== "",
    undefined,
    { timeout: UI_TIMEOUT },
  );
  assert(
    (await pageA.textContent("#browser-user-agent-status")).includes("호스트"),
    "the refusal did not say why",
  );
  assertEqual(
    (await rowsOf(pageA)).map((row) => row.key),
    ["x.test"],
    "a refused row stayed painted",
  );
  assert(
    await pageA.$eval("#browser-user-agent-host", (node) => node.value === "https://bad.test/path"),
    "a refused row lost the words to fix",
  );
  await withoutWorktreePolling([pageA, pageB], async () => {
    // Deliver a canonical repaint only AFTER editing the refused row. A's
    // snapshot response waits; B's event waits until we broadcast it again.
    // Reuse the stateful backend's revision and delivery controls so this
    // schedule does not depend on how busy the browser or Node happens to be.
    backend.dropNextDeliveryFor("B");
    const releaseRepaint = backend.holdNextSnapshot("A");
    const repaintFrom = backend.calls.length;
    const draftAgent = "UA-alone";
    const rowWrites = backend.count("A", "patch_browser_user_agents");
    let repaint;
    try {
      repaint = await backend.externalPatch({ browser: backend.settings.browser });
      await backend.waitForCall("A", "settings_snapshot", repaintFrom);

      // Half a row never leaves the window, even across the delayed repaint.
      await pageA.fill("#browser-user-agent-host", "");
      await pageA.fill("#browser-user-agent-agent", draftAgent);
    } finally {
      releaseRepaint();
    }
    const deliveryFrom = backend.calls.length;
    await backend.broadcast("A", ["browser"]);
    await backend.waitForCall("B", "settings_snapshot", deliveryFrom);
    for (const page of [pageA, pageB]) {
      await page.waitForFunction(
        (revision) => settingsRevision === revision,
        repaint.revision,
        { timeout: UI_TIMEOUT },
      );
    }
    assertEqual(
      await pageA.locator("#browser-user-agent-host").inputValue(),
      "",
      "the delayed canonical repaint restored the refused host over the draft",
    );
    assertEqual(
      await pageA.locator("#browser-user-agent-agent").inputValue(),
      draftAgent,
      "the delayed canonical repaint replaced the edited agent",
    );
    // Both canonical reads have applied. Count the gesture from this boundary,
    // so a sibling's delayed read cannot masquerade as a submitted row.
    const previousStatus = await pageA.textContent("#browser-user-agent-status");
    from = backend.calls.length;
    await pageA.click("#browser-user-agent-submit");
    await pageA.waitForFunction(
      (previous) => document.getElementById("browser-user-agent-host")?.value === ""
        && (document.getElementById("browser-user-agent-status")?.textContent ?? "") !== ""
        && document.getElementById("browser-user-agent-status")?.textContent !== previous,
      previousStatus,
      { timeout: UI_TIMEOUT },
    );
    assert(
      backend.calls.length === from
        && backend.count("A", "patch_browser_user_agents") === rowWrites
        && (await pageA.textContent("#browser-user-agent-status")).length > 0,
      "a row missing its host was sent, or refused in silence",
      backend.calls.slice(repaintFrom),
    );
  });

  // The canonical table survives a reopen on the stale window, and the
  // hands are named without a native title.
  await reopenSettings(pageB, "B", "browser");
  assertEqual(
    (await rowsOf(pageB)).map((row) => row.key),
    ["x.test"],
    "canonical rows did not survive a settings reopen",
  );
  assert(
    await pageB.$eval("#browser-user-agent-list .browser-ua-del", (node) =>
      (node.getAttribute("aria-label") ?? "").includes("x.test") && !node.hasAttribute("title")),
    "the delete hand lost its name or grew a native title",
  );
  assert(
    await pageB.$eval("#browser-user-agent-list .browser-ua-agent", (node) =>
      node.dataset.tip === "UA-x" && !node.hasAttribute("title")),
    "the agent cell lost its full-text tip or grew a native title",
  );
  from = backend.calls.length;
  await pageB.click('#browser-user-agent-list .browser-ua-row[data-host="x.test"] .browser-ua-del');
  await backend.waitForCall("B", "patch_browser_user_agents", from);
  await waitForSettingsIdle(pageB);
  assertEqual(backend.settings.browser.user_agents, [], "the table did not empty");
});

await test("notification and browser controls remain item patches across stale windows", async () => {
  await openSettings(pageA, "notifications");
  await openSettings(pageB, "notifications");
  backend.dropNextDeliveryFor("B");
  let from = backend.calls.length;
  await pageA.uncheck("#notify-agent-attention");
  const attention = await backend.waitForCall("A", "set_notification_preference", from);
  assertEqual(attention.args, { kind: "agent_attention", on: false }, "notification sent sibling state");
  from = backend.calls.length;
  await pageB.uncheck("#notify-agent-completion");
  const completion = await backend.waitForCall("B", "set_notification_preference", from);
  assertEqual(completion.args, { kind: "agent_completion", on: false }, "notification sent sibling state");
  assertEqual(
    backend.settings.notifications,
    { enabled: true, agent_attention: false, agent_completion: false },
    "stale notification patches lost one field",
  );
  // 마스터가 꺼지면 벨 전체가 침묵하고, 종류별 스위치는 만질 수 없게
  // 잠긴다 — 백엔드 사다리(ring_now)와 같은 우선순위를 UI가 말한다.
  from = backend.calls.length;
  await pageA.uncheck("#notify-enabled");
  const master = await backend.waitForCall("A", "set_notification_preference", from);
  assertEqual(master.args, { kind: "enabled", on: false }, "master sent sibling state");
  assert(
    (await pageA.isDisabled("#notify-agent-attention"))
      && (await pageA.isDisabled("#notify-agent-completion")),
    "kind switches stayed live under a dead master",
  );
  from = backend.calls.length;
  await pageA.check("#notify-enabled");
  await backend.waitForCall("A", "set_notification_preference", from);
  assert(
    !(await pageA.isDisabled("#notify-agent-attention")),
    "kind switches did not wake with the master",
  );

  await openSettings(pageA, "browser");
  await openSettings(pageB, "browser");
  backend.dropNextDeliveryFor("B");
  from = backend.calls.length;
  await pageA.fill("#browser-home-page", "https://example.com/home");
  await pageA.$eval("#browser-home-page", (field) => field.dispatchEvent(new Event("change")));
  const home = await backend.waitForCall("A", "set_browser_home_page", from);
  assertEqual(home.args, { homePage: "https://example.com/home" }, "browser home sent sibling state");
  from = backend.calls.length;
  await pageB.selectOption("#browser-search-engine", "kagi");
  const search = await backend.waitForCall("B", "set_browser_search_engine", from);
  assertEqual(search.args, { searchEngine: "kagi" }, "browser search sent sibling state");
  assert(
    backend.settings.browser.home_page === "https://example.com/home"
      && backend.settings.browser.search_engine === "kagi",
    "stale browser patches lost one field",
    backend.settings.browser,
  );
});

await test("browser default zoom remains an item patch in a stale window", async () => {
  await reopenSettings(pageA, "A", "browser");
  await reopenSettings(pageB, "B", "browser");
  backend.dropNextDeliveryFor("B");

  let from = backend.calls.length;
  await pageA.selectOption("#browser-search-engine", "duckduckgo");
  await backend.waitForCall("A", "set_browser_search_engine", from);

  from = backend.calls.length;
  await pageB.selectOption("#browser-default-zoom", "-1.5");
  const zoom = await backend.waitForCall("B", "set_browser_default_zoom", from);
  assertEqual(zoom.args, { zoomLevel: -1.5 }, "browser zoom sent sibling state");
  assert(
    backend.settings.browser.search_engine === "duckduckgo"
      && backend.settings.browser.default_zoom_level === -1.5,
    "stale browser zoom patch overwrote a disjoint browser field",
    backend.settings.browser,
  );
});

await test("an older same-key rejection cannot report over a newer browser value", async () => {
  await openSettings(pageA, "browser");
  const oldHome = "https://old.example.invalid/";
  const newHome = "https://new.example.com/";
  const staleError = "old browser home rejected after the newer value saved";
  const releaseOld = backend.deferRejectionOnce("set_browser_home_page", staleError);
  const oldFrom = backend.calls.length;

  // Use the same shared mutation seam and the browser field's exact onError.
  // Returning its promise makes the deliberately inverted completion order
  // deterministic instead of depending on a timer.
  const oldRequest = pageA.evaluate(({ homePage, message }) => {
    browserPrefs = { ...browserPrefs, home_page: homePage };
    paintBrowserPrefs();
    return commitSetting("browser.home_page", "set_browser_home_page", { homePage }, {
      onError: (error) => {
        document.getElementById("browser-preferences-status").textContent = String(error);
      },
    }).then((answer) => ({ answer, message }));
  }, { homePage: oldHome, message: staleError });
  await backend.waitForCall("A", "set_browser_home_page", oldFrom);

  const newerFrom = backend.calls.length;
  await pageA.fill("#browser-home-page", newHome);
  await pageA.$eval("#browser-home-page", (field) =>
    field.dispatchEvent(new Event("change")));
  await backend.waitForCall("A", "set_browser_home_page", newerFrom);
  await pageA.waitForFunction((wanted) =>
    document.getElementById("browser-home-page")?.value === wanted, newHome);
  assert(backend.settings.browser.home_page === newHome, "newer browser value did not commit");

  // The visible value is optimistic, and the backend records the invocation
  // before its answer reaches the document. Prove the newer mutation settled
  // while the old one is held before testing the deferred recovery read.
  await pageA.waitForFunction(() =>
    settingsMutationsInFlight === 1 && settingsRefreshDeferred);

  const snapshots = backend.count("A", "settings_snapshot");
  const deferredFrom = backend.calls.length;
  releaseOld();
  await oldRequest;
  await backend.waitForCall("A", "settings_snapshot", deferredFrom);
  await renderSettled(pageA);
  const visible = await pageA.evaluate((message) => ({
    home: document.getElementById("browser-home-page")?.value,
    fieldError: document.getElementById("browser-preferences-status")?.textContent,
    staleToasts: [...document.querySelectorAll(".toasts .toast")]
      .filter((note) => note.textContent.includes(message)).length,
  }), staleError);
  assertEqual(visible, { home: newHome, fieldError: "", staleToasts: 0 },
    "stale same-key failure repainted or reported over the newer value");
  assert(
    backend.count("A", "settings_snapshot") === snapshots + 1,
    "overlapping same-key writes did not collapse to one deferred canonical refetch",
  );
});

await test("an old setter response cannot overwrite a newer cross-window revision", async () => {
  await openSettings(pageA, "appearance");
  await openSettings(pageB, "appearance");
  backend.holdNext("set_theme");
  try {
    const first = backend.calls.length;
    await pageA.selectOption("#app-theme", "light");
    await backend.waitForCall("A", "set_theme", first);
    await pageB.waitForFunction(
      () => document.getElementById("app-theme")?.value === "light",
      undefined,
      { timeout: UI_TIMEOUT },
    );
    const oldRevision = backend.settings.revision;

    const second = backend.calls.length;
    await pageB.selectOption("#app-theme", "dark");
    await backend.waitForCall("B", "set_theme", second);
    await pageA.waitForFunction(() => settingsRefreshDeferred, undefined, { timeout: UI_TIMEOUT });
    assert(
      (await controlValue(pageA, "theme")) === "light",
      "a generic refresh crossed the older optimistic theme write",
    );
    assert(backend.settings.revision === oldRevision + 1, "newer mutation did not advance revision");

  } finally {
    backend.release("set_theme");
  }
  await pageA.waitForFunction(
    () => document.getElementById("app-theme")?.value === "dark",
    undefined,
    { timeout: UI_TIMEOUT },
  );
  await pageB.waitForFunction(
    () => document.getElementById("app-theme")?.value === "dark",
    undefined,
    { timeout: UI_TIMEOUT },
  );
  assertEqual(
    [await controlValue(pageA, "theme"), await controlValue(pageB, "theme"), backend.settings.theme],
    ["dark", "dark", "dark"],
    "stale response overwrote the newer revision",
  );
});

await test("task visibility and integration lifecycle use different commands", async () => {
  await openSettings(pageB, "integrations");
  await backend.waitForCall("B", "jira_status", backend.calls.length - 30);
  await pageB.waitForSelector('.integration-site[data-site-id="jira-a"]', { timeout: UI_TIMEOUT });

  const beforeGitHub = backend.calls.length;
  await pageB.click("#integration-github-recheck");
  const refreshed = await backend.waitForCall("B", "github_status", beforeGitHub);
  assert(refreshed.args.force === true, "GitHub recheck did not refresh the CLI path");
  assert(
    !backend.calls.slice(beforeGitHub).some((call) =>
      call.command === "set_task_source_visibility"),
    "rechecking GitHub changed task visibility",
  );

  const beforeTest = backend.calls.length;
  await pageB.click('.integration-site[data-site-id="jira-a"] .integration-jira-test');
  const tested = await backend.waitForCall("B", "jira_test_connection", beforeTest);
  assert(backend.jiraId(tested.args) === "jira-a", "Jira test targeted the wrong site", tested);
  assert(
    !backend.calls.slice(beforeTest).some((call) =>
      call.command === "set_task_source_visibility"),
    "testing a connection changed task visibility",
  );

  await pageB.evaluate(() => showSettingsPane("tasks"));
  const beforeVisibility = backend.calls.length;
  await pageB.check("#show-source-github");
  await backend.waitForCall("B", "set_task_source_visibility", beforeVisibility);
  assert(
    !backend.calls.slice(beforeVisibility).some((call) =>
      ["jira_test_connection", "jira_select_site", "jira_disconnect"].includes(call.command)),
    "a visibility switch performed a connection lifecycle action",
  );
});

await test("Second Brain detects, sets up, registers, and seeds its recurring workflow", async () => {
  backend.secondBrain = {
    saved_path: "",
    vaults: [{ name: "Knowledge", path: "/Users/fixture/Knowledge", open: true }],
    vault: null,
    obsidian_installed: false,
    created: [],
    quick_commands_added: 0,
    skill_installs: [],
    guides: [
      { agent: "claude", label: "Claude", path: "/Users/fixture/.claude/CLAUDE.md",
        state: "missing", detail: "" },
      { agent: "codex", label: "Codex", path: "/Users/fixture/.codex/AGENTS.md",
        state: "missing", detail: "" },
    ],
    weekly_review_enabled: false,
    weekly_review: null,
  };
  delete backend.settings.second_brain_vault;
  const beforeCommands = backend.quickCommands.length;
  const openedAt = backend.calls.length;
  await openSettings(pageB, "integrations");
  await backend.waitForCall("B", "second_brain_status", openedAt);
  await pageB.waitForFunction(() =>
    document.getElementById("second-brain-path")?.value === "/Users/fixture/Knowledge"
      && document.getElementById("second-brain-raw")?.textContent === "4"
      && document.getElementById("second-brain-wiki")?.textContent === "7");
  assert(await pageB.locator("#second-brain-optional").isVisible(),
    "the optional Obsidian explanation was hidden on a machine without the app");
  assert(await pageB.locator("#second-brain-open").isHidden(),
    "Open in Obsidian stood without Obsidian installed");
  // 볼트 밖 판이 볼트를 아는가. 세팅 전에는 아무도 모르고, 다시 연결할 것도 없다.
  const unlinked = await pageB.evaluate(() => ({
    line: document.getElementById("second-brain-guides").textContent,
    unlinkedWord: t("settings.secondBrain.notLinked", "연결 안 됨"),
    relinkDisabled: document.getElementById("second-brain-link").disabled,
  }));
  assertEqual(
    [unlinked.line, unlinked.relinkDisabled],
    [unlinked.unlinkedWord, true],
    "the global-link line claimed a connection before the vault was ever saved",
  );

  const setupAt = backend.calls.length;
  await pageB.click("#second-brain-setup");
  const setup = await backend.waitForCall("B", "second_brain_setup", setupAt);
  assertEqual(setup.args, { path: "/Users/fixture/Knowledge" },
    "setup targeted a path other than the selected vault");
  const registered = await backend.waitForCall("B", "open_project", setupAt);
  assertEqual(registered.args, { path: "/Users/fixture/Knowledge" },
    "setup did not register and open the vault through the project door");
  assertEqual(
    [backend.settings.second_brain_vault, backend.quickCommands.length - beforeCommands],
    ["/Users/fixture/Knowledge", 3],
    "setup did not persist the vault and add exactly three quick commands",
  );
  assertEqual(
    backend.quickCommands.slice(-3).map((row) => row.label),
    ["raw 취합", "위키에 질문", "주간 리뷰"],
    "setup seeded the wrong quick-command set",
  );
  // 그리고 세팅은 볼트 밖에서도 보이게 만든다: 감지된 두 에이전트의 전역
  // 지시문이 볼트를 가리킨다고 카드가 말한다. 세팅 단추는 답을 받은 뒤에도
  // 프로젝트를 열고 나서야 다시 그리므로, 그 칠까지 기다린다.
  await pageB.waitForFunction(() =>
    document.getElementById("second-brain-link")?.disabled === false);
  const linked = await pageB.evaluate(() => ({
    line: document.getElementById("second-brain-guides").textContent,
    expected: t("settings.secondBrain.linked", "{{agents}}에 연결됨",
      { agents: "Claude·Codex" }),
    relinkDisabled: document.getElementById("second-brain-link").disabled,
    troubleHidden: document.getElementById("second-brain-link-error").hidden,
  }));
  assertEqual(
    [linked.line, linked.relinkDisabled, linked.troubleHidden],
    [linked.expected, false, true],
    "the card did not say which agents now look at the vault",
  );

  // 「다시 연결」은 물어보지 않고 저장된 볼트를 다시 싣고, 실패한 파일이 있으면
  // 어느 파일인지 문장으로 말한다 — 경로 없이는 사람이 고칠 수 없다.
  // 세팅은 볼트를 프로젝트로 열면서 시트를 닫으므로, 카드로 돌아와서 누른다.
  await openSettings(pageB, "integrations");
  await pageB.waitForSelector("#second-brain-link:not([disabled])", { state: "visible" });
  backend.secondBrainLinkFailure = "codex";
  const relinkAt = backend.calls.length;
  await pageB.click("#second-brain-link");
  await backend.waitForCall("B", "second_brain_link", relinkAt);
  await pageB.waitForFunction(() =>
    document.getElementById("second-brain-link-error")?.hidden === false);
  const relinked = await pageB.evaluate(() => ({
    line: document.getElementById("second-brain-guides").textContent,
    expected: t("settings.secondBrain.linkStale", "{{agents}}에만 연결됨 · 다시 연결 필요",
      { agents: "Claude" }),
    trouble: document.getElementById("second-brain-link-error").textContent,
  }));
  assertEqual(
    [relinked.line, relinked.trouble.includes("/Users/fixture/.codex/AGENTS.md")],
    [relinked.expected, true],
    "a failed global link did not name the file it could not write",
  );
  backend.secondBrainLinkFailure = null;

  // 주간 리뷰를 zo cron 턴으로 — 스위치는 저장된 볼트가 있어야 살고, 켜면
  // 백엔드가 `zo cron ensure`로 볼트의 등록부에 기록을 앉힌 뒤 설정을 쓰며,
  // 카드는 상태를 다시 물어 영수증 줄에 스케줄러의 이름과 스케줄을 적는다.
  // 끄면 기록이 걷히고 줄이 사라진다.
  const switchOn = backend.calls.length;
  assert(
    await pageB.evaluate(() => document.getElementById("second-brain-weekly-review")?.disabled === false),
    "the weekly-review switch is dead although a vault is saved",
  );
  await pageB.click("#second-brain-weekly-review");
  const turnedOn = await backend.waitForCall("B", "set_second_brain_weekly_review", switchOn);
  assertEqual(turnedOn.args, { enabled: true }, "the switch did not ask to register the cron");
  await backend.waitForCall("B", "second_brain_status", switchOn);
  await pageB.waitForFunction(() =>
    document.getElementById("second-brain-weekly-review-receipt")?.hidden === false
      && document.getElementById("second-brain-weekly-review-receipt").textContent.includes("session_idle"));
  const receipt = await pageB.evaluate(() => ({
    line: document.getElementById("second-brain-weekly-review-receipt").textContent,
    expected: t(
      "settings.secondBrain.reviewRegistered",
      "{{scheduler}} 스케줄러에 등록됨 · {{schedule}} (UTC) · 다음 실행 {{when}}",
      { scheduler: "session_idle", schedule: "0 21 * * 6", when: new Date(1_800_000_000 * 1000).toLocaleString() },
    ),
    checked: document.getElementById("second-brain-weekly-review").checked,
  }));
  assertEqual(
    [receipt.line, receipt.checked, backend.settings.second_brain_weekly_review],
    [receipt.expected, true, true],
    "the receipt line does not name the scheduler and the schedule the registry answered",
  );
  const switchOff = backend.calls.length;
  await pageB.click("#second-brain-weekly-review");
  const turnedOff = await backend.waitForCall("B", "set_second_brain_weekly_review", switchOff);
  assertEqual(turnedOff.args, { enabled: false }, "the switch did not ask to remove the cron");
  await pageB.waitForFunction(() =>
    document.getElementById("second-brain-weekly-review-receipt")?.hidden === true
      && document.getElementById("second-brain-weekly-review").checked === false);
  assertEqual(backend.settings.second_brain_weekly_review, false, "switching off did not clear the setting");
  // A refusal (no zo, a zo too old for `cron`, a refused schedule) rolls the
  // switch back to what the document says: a switch reading "on" while no
  // record stands would be the card promising a turn zo never opens.
  backend.rejectOnce("set_second_brain_weekly_review");
  const refusedAt = backend.calls.length;
  await pageB.click("#second-brain-weekly-review");
  await backend.waitForCall("B", "set_second_brain_weekly_review", refusedAt);
  await backend.waitForCall("B", "settings_snapshot", refusedAt);
  await pageB.waitForFunction(() =>
    document.getElementById("second-brain-weekly-review").checked === false
      && document.getElementById("second-brain-weekly-review-receipt")?.hidden === true);
  assertEqual(backend.settings.second_brain_weekly_review, false, "a refused switch changed the setting");

  backend.automations = [];
  await pageB.evaluate(() => setAutoOpen(true));
  await pageB.waitForSelector('[data-auto-template="raw-ingest"]', { state: "visible" });
  await pageB.click('[data-auto-template="raw-ingest"]');
  const draft = await pageB.evaluate(() => ({
    workspace: document.getElementById("auto-workspace")?.value,
    prompt: document.getElementById("auto-prompt")?.value,
    expectedPrompt: t(
      "auto.templateRawPrompt",
      "raw/에 있지만 wiki/에 아직 반영되지 않은 항목을 모두 취합하고 wiki/log.md에 기록해 줘.",
    ),
    cadence: document.getElementById("auto-cadence")?.value,
    time: document.getElementById("auto-time")?.value,
    evidence: document.getElementById("auto-evidence")?.value,
  }));
  assert(
    draft.workspace === "/Users/fixture/Knowledge"
      && draft.prompt === draft.expectedPrompt
      && draft.cadence === "daily"
      && draft.time === "19:00"
      && draft.evidence === "prompt",
    "raw ingestion template did not target the vault with the evening prompt policy",
    draft,
  );
  await pageB.evaluate(() => {
    closeAutoForm();
    setAutoOpen(false);
  });
});

await test("concurrent GitHub status consumers share one canonical request", async () => {
  const from = backend.calls.length;
  await pageB.evaluate(async () => {
    githubStatusLoaded = false;
    await Promise.all([
      refreshGithubIntegration(),
      refreshGithubIntegration(),
      ensureGithubStatus(),
    ]);
  });
  // Only these three concurrent consumers are measured; they must collapse
  // to a single invocation.
  const calls = backend.calls.slice(from).filter((call) =>
    call.window_id === "B" && call.command === "github_status");
  assert(
    calls.length === 1,
    "one GitHub refresh gesture spawned parallel status authorities",
    calls,
  );
});

await test("GitHub stale account actions and lost acknowledgements recover canonical state", async () => {
  backend.github = githubFixture();
  await openSettings(pageA, "integrations");
  await openSettings(pageB, "integrations");
  await pageA.waitForSelector('.integration-site[data-account-id="gh-b"]', {
    timeout: UI_TIMEOUT,
  });
  await pageB.waitForSelector('.integration-site[data-account-id="gh-b"]', {
    timeout: UI_TIMEOUT,
  });

  assert(
    await pageB.isDisabled('.integration-site[data-account-id="gh-b"] .integration-github-test'),
    "an inactive GitHub account exposed a live-test action",
  );
  assert(
    await pageB.isDisabled('.integration-site[data-account-id="gh-env"] .integration-github-select')
      && await pageB.isDisabled(
        '.integration-site[data-account-id="gh-env"] .integration-github-disconnect',
      ),
    "an environment credential exposed a mutation action",
  );

  let from = backend.calls.length;
  await pageB.click('.integration-site[data-account-id="gh-a"] .integration-github-test');
  const tested = await backend.waitForCall("B", "github_test_connection", from);
  assert(backend.githubId(tested.args) === "gh-a", "GitHub test targeted the wrong account");

  from = backend.calls.length;
  await pageA.click('.integration-site[data-account-id="gh-b"] .integration-github-select');
  await backend.waitForCall("A", "github_select_account", from);
  await pageA.waitForFunction(() =>
    document.querySelector('[data-account-id="gh-b"] .integration-github-select')
      ?.getAttribute("aria-pressed") === "true");
  assert(
    await pageB.getAttribute(
      '.integration-site[data-account-id="gh-b"] .integration-github-select',
      "aria-pressed",
    ) === "false",
    "the second window was not stale before its mutation",
  );

  backend.loseAcknowledgementOnce("github_select_account");
  from = backend.calls.length;
  await pageB.click('.integration-site[data-account-id="gh-b"] .integration-github-select');
  await backend.waitForCall("B", "github_select_account", from);
  await backend.waitForCall("B", "github_status", from);
  await pageB.waitForFunction(() =>
    document.querySelector('[data-account-id="gh-b"] .integration-github-select')
      ?.getAttribute("aria-pressed") === "true"
      && !document.getElementById("integration-github-error")?.hidden
      && document.getElementById("integration-github-error")?.textContent
        .includes("lost acknowledgement")
      && !document.getElementById("integration-github-recheck")?.disabled,
  undefined, { timeout: UI_TIMEOUT });
  assert(backend.github.active_account_id === "gh-b", "lost ack commit was not canonical");

  from = backend.calls.length;
  await pageB.click('.integration-site[data-account-id="gh-a"] .integration-github-disconnect');
  await backend.waitForCall("B", "github_disconnect", from);
  await pageB.waitForSelector('.integration-site[data-account-id="gh-a"]', {
    state: "detached",
    timeout: UI_TIMEOUT,
  });
  assert(
    !backend.github.accounts.some((account) => account.id === "gh-a"),
    "disconnected GitHub account remains canonical",
  );
});

await test("GitHub login is an explicit plain-terminal intent with no renderer credential", async () => {
  backend.github = githubFixture();
  await openSettings(pageB, "integrations");
  const refreshedFrom = backend.calls.length;
  await pageB.click("#integration-github-recheck");
  await backend.waitForCall("B", "github_status", refreshedFrom);
  await pageB.waitForSelector('.integration-site[data-account-id="gh-a"]', {
    timeout: UI_TIMEOUT,
  });
  const from = backend.calls.length;
  await pageB.click("#integration-github-login");
  await backend.waitForCall("B", "github_login_intent", from);
  const opened = await backend.waitForCall("B", "open_term_tab", from);
  const typed = await backend.waitForCall("B", "term_text", from);

  assertEqual(opened.args, { rows: 24, cols: 96, plain: true },
    "GitHub login did not open the plain terminal");
  assertEqual(typed.args, {
    term: backend.nextTerm,
    text: "gh auth login --hostname github.com --web\r",
  }, "GitHub login intent was not executed verbatim in the terminal");
  assert(
    !JSON.stringify(backend.github).match(/gh[pousr]_[A-Za-z0-9]+|token/i),
    "a credential entered the GitHub renderer snapshot",
  );
});

/* GitLab is recognised from the CLI alone (1-g56a) — no token field, no connect
 * form. Each of the three standings has to say which one it is and offer the
 * single thing that ends it, and 다시 확인 has to actually go and ask again:
 * a card repainting a cached answer would satisfy every other facet here. */
await test("GitLab is read off the glab CLI and each standing offers its one way out", async () => {
  await openSettings(pageB, "integrations");
  await pageB.waitForSelector("#integration-gitlab-status", { timeout: UI_TIMEOUT });

  const enter = async (gitlab) => {
    backend.gitlab = gitlab;
    const from = backend.calls.length;
    await pageB.click("#integration-gitlab-recheck");
    const asked = await backend.waitForCall("B", "gitlab_status", from);
    const wanted = gitlab.installed
      ? (gitlab.authenticated ? "connected" : "disconnected")
      : "unavailable";
    await pageB.waitForFunction(
      (state) => document.getElementById("integration-gitlab-status")?.dataset.state === state,
      wanted,
      { timeout: UI_TIMEOUT },
    );
    return { asked, from };
  };

  /* Words are compared against the catalog in force rather than against Korean
   * written out here: an earlier test leaves whatever language it needed
   * behind, and a hardcoded sentence would make this pass or fail on that. */
  const read = () => pageB.evaluate(() => {
    const box = document.getElementById("integration-gitlab-remedy");
    const copy = box.querySelector(".integration-remedy-copy");
    const hosts = document.getElementById("integration-gitlab-hosts");
    return {
      remedyStands: box.hidden === false,
      head: box.querySelector(".integration-remedy-head")?.textContent ?? null,
      installCta: box.querySelector(".integration-gitlab-install")?.textContent ?? null,
      loginCta: box.querySelector(".integration-gitlab-login")?.textContent ?? null,
      command: box.querySelector(".integration-remedy-command")?.textContent ?? null,
      copyCarries: copy?.dataset.tip ?? null,
      hosts: hosts.hidden ? null : hosts.textContent,
      says: {
        install: t("settings.gitlab.install", "MR·이슈·파이프라인을 쓰려면 GitLab CLI를 설치하세요."),
        installCta: t("settings.gitlab.installCta", "GitLab CLI 설치"),
        loginCta: t("settings.gitlab.loginCta", "GitLab 로그인"),
        auth: t(
          "settings.gitlab.auth",
          "GitLab CLI가 설치되어 있지만 인증되지 않았습니다. 로그인을 누르면 앱 터미널에서 glab이 안내합니다.",
        ),
      },
    };
  });

  const missing = await enter({ installed: false, authenticated: false, hosts: [] });
  assert(missing.asked.args.force === true, "GitLab recheck did not re-read the CLI path");
  const withoutCli = await read();
  assert(
    withoutCli.remedyStands
      && withoutCli.head === withoutCli.says.install
      && withoutCli.installCta === withoutCli.says.installCta
      && withoutCli.hosts === null,
    "a machine without glab was not offered the install page",
    withoutCli,
  );
  const opened = backend.calls.length;
  await pageB.click(".integration-gitlab-install");
  const sent = await backend.waitForCall("B", "open_url", opened);
  assertEqual(
    sent.args,
    { url: "https://gitlab.com/gitlab-org/cli#installation" },
    "the install button did not go out the window's external-link door",
  );

  await enter({ installed: true, authenticated: false, hosts: [] });
  const signedOut = await read();
  assert(
    signedOut.remedyStands
      && signedOut.head === signedOut.says.auth
      && signedOut.command === "glab auth login"
      && signedOut.copyCarries === "glab auth login"
      && signedOut.loginCta === signedOut.says.loginCta
      && signedOut.installCta === null,
    "an installed-but-signed-out glab did not show the login door and the command it will type",
    signedOut,
  );

  // The door does what the GitHub card's does: a plain in-app terminal with
  // the command already typed — the credential stays with glab.
  const doorFrom = backend.calls.length;
  await pageB.click(".integration-gitlab-login");
  const doorTerm = await backend.waitForCall("B", "open_term_tab", doorFrom);
  const doorTyped = await backend.waitForCall("B", "term_text", doorFrom);
  assertEqual(doorTerm.args, { rows: 24, cols: 96, plain: true },
    "GitLab login did not open the plain terminal");
  assertEqual(doorTyped.args, {
    term: backend.nextTerm,
    text: "glab auth login\r",
  }, "GitLab login did not type glab's own sentence");
  await openSettings(pageB, "integrations");

  await enter({
    installed: true,
    authenticated: true,
    hosts: ["gitlab.com", "gitlab.acme.test:8443"],
  });
  const connected = await read();
  assert(
    !connected.remedyStands
      && connected.hosts?.includes("gitlab.com, gitlab.acme.test:8443") === true,
    "a connected GitLab did not name the hosts the login covers",
    connected,
  );

  const asked = backend.calls.filter((call) =>
    call.window_id === "B" && call.command === "gitlab_status").length;
  assert(asked >= 3, "다시 확인 repainted a cached answer instead of asking again", asked);
});

await test("Jira reports the credential protection it actually provides", async () => {
  const previousLocale = backend.settings.locale;
  await backend.externalPatch({ locale: "en" }, ["locale"]);
  await renderSettled(pageB);
  await openSettings(pageB, "integrations");
  const storage = pageB.locator("#integration-jira-storage");
  await storage.waitFor({ state: "visible", timeout: UI_TIMEOUT });
  const painted = new Map();
  for (const protection of ["native", "plaintext", "unavailable"]) {
    backend.jira.credential_protection = protection;
    const from = backend.calls.length;
    await pageB.click("#integration-jira-refresh");
    await backend.waitForCall("B", "jira_status", from);
    await renderSettled(pageB);
    painted.set(protection, (await storage.textContent())?.trim() ?? "");
  }
  assert(
    painted.get("native") === "OS credential store"
      && painted.get("plaintext") === "Legacy plaintext awaiting secure migration (not used)"
      && painted.get("unavailable") === "OS credential store unavailable",
    "Jira credential protection states were not painted distinctly",
    Object.fromEntries(painted),
  );
  await backend.externalPatch({ locale: previousLocale }, ["locale"]);
  await renderSettled(pageB);
});

await test("Jira site selection and disconnect return canonical lifecycle state", async () => {
  await pageB.evaluate(() => setTaskOpen(true));
  await pageB.waitForSelector("#jira-site-picker", { timeout: UI_TIMEOUT });
  const selectedFrom = backend.calls.length;
  await pageB.selectOption("#jira-site-picker", "jira-b");
  await backend.waitForCall("B", "jira_select_site", selectedFrom);
  assert(backend.jira.active_site_id === "jira-b", "task picker did not select the active site");

  await pageB.evaluate(() => {
    setTaskOpen(false);
    setSettingsOpen(true);
    showSettingsPane("integrations");
  });
  await pageB.waitForSelector('.integration-site[data-site-id="jira-b"]', { timeout: UI_TIMEOUT });
  const disconnectedFrom = backend.calls.length;
  await pageB.click('.integration-site[data-site-id="jira-b"] .integration-jira-disconnect');
  await backend.waitForCall("B", "jira_disconnect", disconnectedFrom);
  assert(!backend.jira.sites.some((site) => site.id === "jira-b"), "disconnected site remains canonical");
  assert(backend.jira.active_site_id === "jira-a", "disconnect did not choose the remaining site");
});

await test("Jira lifecycle lost acknowledgements refetch canonical state and release controls", async () => {
  await pageB.click("#integration-jira-add");
  await pageB.fill("#jira-site-url", "https://gamma.atlassian.net");
  await pageB.fill("#jira-identity", "nari@gamma.example");
  await pageB.fill("#jira-token", "test-token");
  backend.loseAcknowledgementOnce("jira_connect");
  let from = backend.calls.length;
  await pageB.click("#jira-connect-submit");
  await backend.waitForCall("B", "jira_connect", from);
  await backend.waitForCall("B", "jira_status", from);
  const connected = backend.jira.sites.find((site) =>
    site.site_url === "https://gamma.atlassian.net");
  assert(connected, "a committed connect was absent from canonical status");
  await pageB.waitForSelector(`.integration-site[data-site-id="${connected.id}"]`, {
    timeout: UI_TIMEOUT,
  });
  await pageB.waitForFunction(
    () => !document.getElementById("jira-connect-error")?.hidden
      && document.getElementById("jira-connect-error")?.textContent.includes("lost acknowledgement")
      && !document.getElementById("jira-connect-submit")?.disabled
      && !document.getElementById("integration-jira-refresh")?.disabled,
    undefined,
    { timeout: UI_TIMEOUT },
  );
  await pageB.click("#jira-connect-cancel");

  from = backend.calls.length;
  await pageB.evaluate(() => setTaskOpen(true));
  await backend.waitForCall("B", "jira_status", from);
  await pageB.waitForSelector("#jira-site-picker", { timeout: UI_TIMEOUT });
  backend.loseAcknowledgementOnce("jira_select_site");
  from = backend.calls.length;
  await pageB.selectOption("#jira-site-picker", "jira-a");
  await backend.waitForCall("B", "jira_select_site", from);
  await backend.waitForCall("B", "jira_status", from);
  await pageB.waitForFunction(
    () => document.getElementById("jira-site-picker")?.value === "jira-a"
      && !document.getElementById("jira-error")?.hidden
      && document.getElementById("jira-error")?.textContent.includes("lost acknowledgement")
      && !document.getElementById("jira-site-picker")?.disabled,
    undefined,
    { timeout: UI_TIMEOUT },
  );
  assert(backend.jira.selected_site_id === "jira-a", "lost select acknowledgement rolled back canonical selection");

  from = backend.calls.length;
  await pageB.evaluate(() => {
    setTaskOpen(false);
    setSettingsOpen(true);
    showSettingsPane("integrations");
  });
  await backend.waitForCall("B", "jira_status", from);
  await pageB.waitForSelector(`.integration-site[data-site-id="${connected.id}"]`, {
    timeout: UI_TIMEOUT,
  });
  backend.loseAcknowledgementOnce("jira_disconnect");
  from = backend.calls.length;
  await pageB.click(
    `.integration-site[data-site-id="${connected.id}"] .integration-jira-disconnect`,
  );
  await backend.waitForCall("B", "jira_disconnect", from);
  await backend.waitForCall("B", "jira_status", from);
  await pageB.waitForFunction(
    (siteId) => !document.querySelector(`.integration-site[data-site-id="${siteId}"]`)
      && !document.getElementById("integration-jira-error")?.hidden
      && !document.getElementById("integration-jira-refresh")?.disabled,
    connected.id,
    { timeout: UI_TIMEOUT },
  );
  assert(
    !backend.jira.sites.some((site) => site.id === connected.id),
    "lost disconnect acknowledgement restored a canonically removed site",
  );
});

await test("Claude rows distinguish organization types and badge only identical UUID pairs", async () => {
  await setQualityLocale(pageA, "en");
  const result = await pageA.evaluate(() => {
    const base = { email: "same@example.test", signed_in: true, organization_name: "Organization", account_uuid: "person" };
    accountReport = { can_add: true, accounts: [
      { ...base, id: "team", organization_uuid: "work", organization_type: "claude_team" },
      { ...base, id: "max", organization_uuid: "personal", organization_type: "claude_max" },
      { ...base, id: "duplicate", organization_uuid: "personal", organization_type: "claude_max" },
      { ...base, id: "other", account_uuid: "other-person", organization_uuid: "personal", organization_type: "claude_pro" },
      { ...base, id: "unknown", account_uuid: null, organization_uuid: null, organization_type: "claude_free" },
    ] };
    paintClaudeAccounts();
    return [...document.querySelectorAll("#account-list .account-row")].slice(1).map((row) => ({
      under: row.querySelector(".agent-row-cmd").textContent,
      duplicate: !!row.querySelector(".account-duplicate"),
    }));
  });
  assert(result[0].under.includes("Team") && result[1].under.includes("Max")
    && result[3].under.includes("Pro") && result[4].under.includes("Free"), JSON.stringify(result));
  assert(same(result.map((row) => row.duplicate), [false, true, true, false, false]), JSON.stringify(result));
});

await test("Claude pending identity keeps its old label and offers both explicit resolutions", async () => {
  for (const choice of ["add", "apply"]) {
    const original = { id: "team", email: "same@example.test", signed_in: true, organization_uuid: "work", organization_name: "Work", organization_type: "claude_team" };
    const pending = { email: "same@example.test", account_uuid: "person", organization_uuid: "personal", organization_name: "Personal", organization_type: "claude_max" };
    backend.claudeAccounts = { can_add: true, accounts: [{ ...original, pending }] };
    await pageA.evaluate(async () => { await refreshClaudeAccounts(); });
    const result = await pageA.evaluate(() => {
      const row = document.querySelectorAll("#account-list .account-row")[1];
      return { message: row.querySelector(".agent-row-cmd").textContent,
        actions: row.querySelectorAll(".account-identity-choice").length,
        disabled: row.querySelector(".account-pick").disabled };
    });
    assert(result.message.includes("Work") && result.message.includes("Personal") && result.message.includes("→"), JSON.stringify(result));
    assert(result.actions === 2 && result.disabled, JSON.stringify(result));
    const before = backend.count("A", "resolve_claude_account_identity");
    await pageA.evaluate((choice) => document.querySelector(`#account-list [data-choice="${choice}"]`).click(), choice);
    await pageA.waitForFunction(() => !accountReport.accounts.some((row) => row.pending));
    assert(backend.count("A", "resolve_claude_account_identity") === before + 1, "choice was not a single IPC action");
    assert(backend.claudeAccounts.accounts.length === (choice === "add" ? 2 : 1), "choice lost or duplicated a row");
    assert(backend.claudeAccounts.accounts[0].organization_uuid === (choice === "add" ? "work" : "personal"), "choice overwrote the wrong identity");
  }
  backend.claudeAccounts = { accounts: [], can_add: true };
  await pageA.evaluate(async () => { await refreshClaudeAccounts(); });
});

await test("platform modifier opens Settings by keyboard and Escape restores the opener", async () => {
  const qualityPage = await context.newPage();
  const id = "quality-keyboard";
  try {
    attachFaultRecorder(qualityPage, id);
    await installTauri(qualityPage, id);
    await bootPage(qualityPage, id);
    const modifier = process.platform === "darwin" ? "Meta" : "Control";
    const wrongModifier = process.platform === "darwin" ? "Control" : "Meta";

    await qualityPage.keyboard.press(`${wrongModifier}+Comma`);
    assert(await qualityPage.locator("#settings-view").isHidden(), "non-platform modifier opened Settings");

    // A real button gesture establishes the same focus origin a person gets;
    // the rest of the flow is keyboard-only. Close once, then prove the
    // platform chord opens from and Escape returns to that control.
    await qualityPage.click("#foot-settings");
    await qualityPage.locator("#settings-view").waitFor({ state: "visible", timeout: UI_TIMEOUT });
    await qualityPage.keyboard.press("Escape");
    await qualityPage.locator("#settings-view").waitFor({ state: "hidden", timeout: UI_TIMEOUT });
    assert(
      await qualityPage.evaluate(() => document.activeElement?.id === "foot-settings"),
      "pointer-opened Settings did not return focus to its opener",
    );

    await qualityPage.keyboard.press(`${modifier}+Comma`);
    await qualityPage.waitForSelector("#settings-view:not([hidden])", { timeout: UI_TIMEOUT });
    const capturedOpener = await qualityPage.evaluate(() => settingsReturnFocus?.id ?? null);
    assertEqual(capturedOpener, "foot-settings", "Settings did not retain its keyboard opener");
    assert(
      await qualityPage.evaluate(() => document.activeElement === document.getElementById("settings-view")),
      "keyboard-opened Settings did not take focus",
    );

    await qualityPage.keyboard.press("Tab");
    assert(
      await qualityPage.evaluate(() => document.activeElement?.id === "settings-back"),
      "Tab did not reach the Settings back button",
    );
    await qualityPage.keyboard.press("Tab");
    assert(
      await qualityPage.evaluate(() => document.activeElement?.id === "settings-search"),
      "Tab did not reach Settings search",
    );
    await qualityPage.keyboard.press("Tab");
    assertEqual(
      await qualityPage.evaluate(() => document.activeElement?.dataset?.pane ?? null),
      "agents",
      "Tab did not enter the Settings section rail",
    );
    await qualityPage.keyboard.press("Enter");
    assert(
      await qualityPage.locator('.settings-pane[data-pane="agents"]').isVisible(),
      "Enter did not activate a Settings rail item",
    );

    // The walk's budget is the rail itself: every section must be reachable
    // before focus runs off the list's end, however many panes stand — a
    // fixed count here broke the day the rail grew a pane.
    const railSteps = await qualityPage.locator(".settings-rail-item").count();
    for (let step = 0; step < railSteps; step += 1) {
      const pane = await qualityPage.evaluate(() => document.activeElement?.dataset?.pane ?? null);
      if (pane === "notifications") break;
      await qualityPage.keyboard.press("Tab");
    }
    assertEqual(
      await qualityPage.evaluate(() => document.activeElement?.dataset?.pane ?? null),
      "notifications",
      "notification settings were not keyboard reachable from the rail",
    );
    await qualityPage.keyboard.press("Enter");
    assert(
      await qualityPage.locator('.settings-pane[data-pane="notifications"]').isVisible(),
      "Enter did not activate the notification pane",
    );

    for (let step = 0; step < 20; step += 1) {
      const activeId = await qualityPage.evaluate(() => document.activeElement?.id ?? null);
      if (activeId === "notify-agent-attention") break;
      await qualityPage.keyboard.press("Tab");
    }
    assert(
      await qualityPage.evaluate(() => document.activeElement?.id === "notify-agent-attention"),
      "a Settings checkbox was not keyboard reachable",
    );
    const attentionBefore = await qualityPage.locator("#notify-agent-attention").isChecked();
    const beforeToggle = backend.calls.length;
    await qualityPage.keyboard.press("Space");
    const toggled = await backend.waitForCall(
      id,
      "set_notification_preference",
      beforeToggle,
    );
    assertEqual(toggled.args.kind, "agent_attention", "Space toggled the wrong preference");
    assertEqual(
      toggled.args.on,
      !attentionBefore,
      "Space did not activate the focused checkbox",
    );
    await qualityPage.keyboard.press("Shift+Tab");
    assert(
      await qualityPage.evaluate(() => document.activeElement?.id !== "notify-agent-attention"),
      "Shift+Tab did not move backward from a Settings control",
    );
    await qualityPage.keyboard.press("Tab");
    assert(
      await qualityPage.evaluate(() => document.activeElement?.id === "notify-agent-attention"),
      "Tab did not return to the Settings control after reverse traversal",
    );

    await qualityPage.keyboard.press("Escape");
    await qualityPage.locator("#settings-view").waitFor({ state: "hidden", timeout: UI_TIMEOUT });
    const restoredFocus = await qualityPage.evaluate(() => ({
      activeId: document.activeElement?.id ?? null,
      activeTag: document.activeElement?.tagName ?? null,
      openerHiddenBy: document.getElementById("foot-settings")?.closest("[hidden]")?.id ?? null,
    }));
    assert(
      restoredFocus.activeId === "foot-settings",
      "Escape did not restore focus to the Settings opener",
      restoredFocus,
    );
    return `${modifier}+Comma`;
  } finally {
    await qualityPage.close();
  }
});

await test("visible Settings panes and connection dialogs have no WCAG A or AA axe violations", async () => {
  const qualityPage = await context.newPage();
  const id = "quality-axe";
  try {
    attachFaultRecorder(qualityPage, id);
    await installTauri(qualityPage, id);
    await bootPage(qualityPage, id);
    await openSettings(qualityPage, "agents");
    const panes = await settingsPaneIds(qualityPage);
    const violations = [];

    for (const theme of SETTINGS_QUALITY_SPEC.themes) {
      await setQualityTheme(qualityPage, theme);
      for (const pane of panes) {
        await qualityPage.evaluate((wanted) => showSettingsPane(wanted), pane);
        if (pane === "terminal") {
          await qualityPage.$eval("#term-color-overrides", (details) => { details.open = true; });
        }
        await settlePaint(qualityPage);
        const found = await axeViolations(qualityPage, AxeBuilder, ".settings-view");
        if (found.length > 0) violations.push({ surface: `settings:${theme}:${pane}`, found });
      }
      await qualityPage.evaluate(() => openJiraConnectDialog());
      await settlePaint(qualityPage);
      const found = await axeViolations(qualityPage, AxeBuilder, "#jira-connect-scrim");
      if (found.length > 0) violations.push({ surface: `jira:${theme}`, found });
      await qualityPage.evaluate(() => closeJiraConnectDialog());
      await qualityPage.evaluate(() => openSshHostDialog());
      await settlePaint(qualityPage);
      const sshFound = await axeViolations(qualityPage, AxeBuilder, "#ssh-host-scrim");
      if (sshFound.length > 0) violations.push({ surface: `ssh:${theme}`, found: sshFound });
      await qualityPage.evaluate(() => closeSshHostDialog());
      await qualityPage.evaluate(() => openRemoteWorkspaceDialog());
      await settlePaint(qualityPage);
      const workspaceFound = await axeViolations(
        qualityPage,
        AxeBuilder,
        "#remote-workspace-scrim",
      );
      if (workspaceFound.length > 0) {
        violations.push({ surface: `remote-workspace:${theme}`, found: workspaceFound });
      }
      await qualityPage.evaluate(() => closeRemoteWorkspaceDialog());
      await qualityPage.evaluate(() => openRemoteServerDialog());
      await settlePaint(qualityPage);
      const serverFound = await axeViolations(qualityPage, AxeBuilder, "#remote-server-scrim");
      if (serverFound.length > 0) {
        violations.push({ surface: `remote-server:${theme}`, found: serverFound });
      }
      await qualityPage.evaluate(() => closeRemoteServerDialog());
      await qualityPage.evaluate(() => {
        terminalThemeImportPreview = {
          themes: [{
            id: "warp:quality",
            name: "Quality Theme",
            source: "warp",
            mode: "dark",
            terminal: { background: "#102030", foreground: "#f0f1f2", red: "#aabbcc" },
            importedAtMs: 1,
            sourceLabel: "quality.yaml",
            unsupportedFeatures: [],
          }],
          skipped: [],
        };
        paintTerminalThemeImportPreview();
        document.getElementById("term-theme-import-scrim").hidden = false;
      });
      await settlePaint(qualityPage);
      const themeImportFound = await axeViolations(
        qualityPage,
        AxeBuilder,
        "#term-theme-import-scrim",
      );
      if (themeImportFound.length > 0) {
        violations.push({ surface: `terminal-theme-import:${theme}`, found: themeImportFound });
      }
      await qualityPage.evaluate(() => closeTerminalThemeImport());
      await qualityPage.evaluate(() => {
        ghosttyImportPreview = {
          found: true,
          configPaths: ["/Users/test/.config/ghostty/config"],
          patch: { fontSize: 16 },
          changes: [{ key: "terminalFontSize", value: "16" }],
          unsupportedKeys: ["selection-word-chars"],
        };
        ghosttyImportBusy = false;
        ghosttyImportApplied = false;
        ghosttyImportApplyError = "";
        paintGhosttyImportPreview();
        document.getElementById("term-ghostty-import-scrim").hidden = false;
      });
      await settlePaint(qualityPage);
      const ghosttyImportFound = await axeViolations(
        qualityPage,
        AxeBuilder,
        "#term-ghostty-import-scrim",
      );
      if (ghosttyImportFound.length > 0) {
        violations.push({ surface: `ghostty-import:${theme}`, found: ghosttyImportFound });
      }
      await qualityPage.evaluate(() => closeGhosttyImport());
    }

    assertEqual(violations, [], "WCAG A/AA violations");
    return `${panes.length} panes × 2 themes + Jira + SSH + Remote Workspace + Remote Server + Warp + Ghostty Import`;
  } finally {
    await qualityPage.close();
  }
});

await test("five locales keep every Settings pane inside a 720 by 480 viewport", async () => {
  const qualityPage = await context.newPage();
  const id = "quality-layout";
  try {
    attachFaultRecorder(qualityPage, id);
    await qualityPage.setViewportSize(SETTINGS_QUALITY_SPEC.viewport);
    await installTauri(qualityPage, id);
    await bootPage(qualityPage, id);
    await openSettings(qualityPage, "agents");
    const panes = await settingsPaneIds(qualityPage);
    const failures = [];

    for (const locale of SETTINGS_QUALITY_SPEC.locales) {
      await setQualityLocale(qualityPage, locale);
      for (const pane of panes) {
        await qualityPage.evaluate((wanted) => showSettingsPane(wanted), pane);
        if (pane === "terminal") {
          await qualityPage.$eval("#term-color-overrides", (details) => { details.open = true; });
        }
        await settlePaint(qualityPage);
        const layout = await inspectSettingsLayout(qualityPage);
        if (
          layout.overflowing.length > 0
          || layout.unnamed.length > 0
          || layout.outOfBounds.length > 0
        ) failures.push({ locale, pane, ...layout });
      }
    }

    assertEqual(failures, [], "localized Settings layout/accessibility bounds");
    return `${SETTINGS_QUALITY_SPEC.locales.length} locales × ${panes.length} panes`;
  } finally {
    await qualityPage.close();
  }
});

await test("maximal Settings fixture stays within p95 budgets and creates no long tasks", async () => {
  const qualityPage = await context.newPage();
  const id = "quality-performance";
  try {
    attachFaultRecorder(qualityPage, id);
    await installTauri(qualityPage, id);
    await bootPage(qualityPage, id);
    const panes = await settingsPaneIds(qualityPage);
    const fixture = await maximalSettingsFixture(qualityPage, backend.snapshot());
    const performance = await measureSettingsPerformance(qualityPage, fixture, panes);
    const failures = performanceFailures(performance);
    await writeQualityReport({
      version: 1,
      platform: process.platform,
      viewport: SETTINGS_QUALITY_SPEC.viewport,
      generated_at: new Date().toISOString(),
      performance,
    });
    assertEqual(failures.overBudget, [], "Settings p95 performance budget exceeded");
    assertEqual(failures.longTasks, [], "Settings produced browser long tasks");
    return Object.entries(performance.operations)
      .map(([name, result]) => `${name} p95=${result.p95_ms}ms/${result.budget_ms}ms`)
      .join(", ");
  } finally {
    await qualityPage.close();
  }
});

/* 세션 쿠키 구역 — 브라우저를 한 번도 열지 않은 사람도 쿠키 단지에 닿는다.
 *
 * 가져오는 기계는 전부 있었고 닿는 길이 브라우저 도구모음 하나였다. 원본은
 * 같은 구역을 설정의 브라우저 페이지 아래에 둔다(`BrowserPane.tsx:276-290`).
 * 계약 넷: 없으면 빈 자리를 보이고 · 있으면 행을 세우고 · 마지막으로 마신
 * 자리를 한 줄로 말하고 · 이 페이지에 올 때마다 목록을 다시 읽는다. */
await test("the browser page shows the cookie jars and where each one drank", async () => {
  backend.browserProfiles = [];
  await openSettings(pageA, "browser");
  await pageA.waitForFunction(
    () => document.getElementById("browser-profile-empty")?.hidden === false,
    undefined,
    { timeout: UI_TIMEOUT },
  );
  assertEqual(
    await pageA.locator("#browser-profile-list > *").count(),
    0,
    "an empty jar shelf still drew rows",
  );

  // 한 잔도 안 마신 프로필과, 어디에서 마셨는지 적힌 프로필.
  backend.browserProfiles = [
    { id: "1".repeat(32), name: "일하는 계정" },
    {
      id: "2".repeat(32),
      name: "사는 계정",
      source: { family: "chrome", label: "Google Chrome", profile: "Profile 1" },
    },
  ];
  const readAgain = backend.count("A", "browser_profiles");
  await pageA.evaluate(() => showSettingsPane("appearance"));
  await pageA.evaluate(() => showSettingsPane("browser"));
  await pageA.waitForFunction(
    () => document.querySelectorAll("#browser-profile-list > *").length === 2,
    undefined,
    { timeout: UI_TIMEOUT },
  );
  assert(
    backend.count("A", "browser_profiles") > readAgain,
    "arriving at the browser page did not re-read the profiles",
  );
  assert(
    await pageA.locator("#browser-profile-empty").isHidden(),
    "the empty state stayed up beside two rows",
  );

  const rows = await pageA.locator("#browser-profile-list > *").evaluateAll((held) =>
    held.map((row) => ({
      name: row.querySelector(".browser-profile-name")?.textContent ?? "",
      note: row.querySelector(".browser-profile-note")?.textContent ?? "",
      act: row.querySelector(".browser-profile-import")?.textContent ?? "",
    }))
  );
  assertEqual(rows.map((row) => row.name), ["일하는 계정", "사는 계정"], "the jars drifted");
  // 앞선 시험이 이 창을 어느 말로 두고 갔든 서야 하는 것들이라, 말이 아니라
  // 실체로 묻는다: 안 마신 잔은 소스를 말하지 않고, 두 단추는 서로 다르게
  // 읽힌다(가져오기 ↔ 다시 가져오기).
  assert(
    !rows[0].note.includes("Google Chrome"),
    `a jar that never drank claimed a source: ${JSON.stringify(rows[0])}`,
  );
  assert(
    rows[1].note.includes("Google Chrome (Profile 1)"),
    `the source line lost the browser it names: ${JSON.stringify(rows[1])}`,
  );
  assert(
    rows[0].act.length > 0 && rows[1].act.length > 0 && rows[0].act !== rows[1].act,
    `importing and re-importing read the same: ${JSON.stringify(rows.map((row) => row.act))}`,
  );

  // 새 프로필은 이 페이지에서 바로 만들어지고, 만들자마자 선다.
  await pageA.click("#browser-profile-new");
  await pageA.fill(".profile-pop input", "세 번째");
  await pageA.press(".profile-pop input", "Enter");
  await pageA.waitForFunction(
    () => document.querySelectorAll("#browser-profile-list > *").length === 3,
    undefined,
    { timeout: UI_TIMEOUT },
  );
  const made = backend.browserProfiles.at(-1);
  assertEqual(made.name, "세 번째", "the new jar took a different name than it was given");

  backend.browserProfiles = [];
  return `${rows.length} jars, then ${backend.count("A", "create_browser_profile")} made here`;
});

/* 카드의 세 가지 새 계약(원본 BrowserProfileRow + 우리 쪽 이득): 카드를 눌러
 * "새 탭 기본"을 고르면 활성 배지가 서고 다시 누르면 내장 기본으로 돌아가며,
 * 스테이징이 앉은 프로필은 "적용 대기" 칩을 들고, 삭제는 확인을 거쳐 그 단지를
 * 목록에서 지운다. */
await test("a profile card picks the default, badges pending imports, and deletes on confirm", async () => {
  backend.defaultProfileId = null;
  backend.browserProfiles = [
    { id: "1".repeat(32), name: "일하는 계정" },
    {
      id: "2".repeat(32),
      name: "사는 계정",
      staged: true,
      source: { family: "chrome", label: "Google Chrome", profile: "Profile 1" },
    },
  ];
  await openSettings(pageA, "browser");
  await pageA.waitForFunction(
    () => document.querySelectorAll("#browser-profile-list > *").length === 2,
    undefined,
    { timeout: UI_TIMEOUT },
  );

  // 스테이징에 쿠키가 앉은 프로필만 "적용 대기" 칩을 든다.
  assertEqual(
    await pageA.locator("#browser-profile-list > *").nth(1).locator(".browser-profile-chip").count(),
    1,
    "a profile with staged cookies showed no pending chip",
  );
  assertEqual(
    await pageA.locator("#browser-profile-list > *").nth(0).locator(".browser-profile-chip").count(),
    0,
    "a profile with nothing staged still claimed pending",
  );

  // 카드를 누르면 그 프로필이 새 탭의 기본이 된다 — 시스템 손에 적히고,
  // "활성" 배지가 선다.
  await gestureAndWait(pageA, "A", "set_browser_default_profile", () =>
    pageA.locator("#browser-profile-list > *").nth(0).locator(".browser-profile-identity").click());
  await pageA.waitForFunction(
    () => document.querySelector("#browser-profile-list > *.is-active") !== null,
    undefined,
    { timeout: UI_TIMEOUT },
  );
  assertEqual(backend.defaultProfileId, "1".repeat(32), "the card click did not set the default");
  assertEqual(
    await pageA.locator("#browser-profile-list > *.is-active .browser-profile-badge").count(),
    1,
    "the active card wore no badge",
  );

  // 다시 누르면 내장 기본 저장소로 돌아간다(우리에겐 이 토글이 그 자리로 가는 유일한 문).
  await gestureAndWait(pageA, "A", "set_browser_default_profile", () =>
    pageA.locator("#browser-profile-list > *").nth(0).locator(".browser-profile-identity").click());
  await pageA.waitForFunction(
    () => document.querySelector("#browser-profile-list > *.is-active") === null,
    undefined,
    { timeout: UI_TIMEOUT },
  );
  assertEqual(backend.defaultProfileId, null, "clicking the active card did not clear the default");

  // 삭제는 확인을 묻고, 예를 눌러야 그 단지가 목록에서 사라진다.
  await pageA.locator("#browser-profile-list > *").nth(1).locator(".browser-profile-del").click();
  await pageA.waitForFunction(
    () => {
      const scrim = document.getElementById("ask-scrim");
      return scrim && !scrim.hidden;
    },
    undefined,
    { timeout: UI_TIMEOUT },
  );
  await gestureAndWait(pageA, "A", "delete_browser_profile", () => pageA.click("#ask-yes"));
  await pageA.waitForFunction(
    () => document.querySelectorAll("#browser-profile-list > *").length === 1,
    undefined,
    { timeout: UI_TIMEOUT },
  );
  assertEqual(backend.browserProfiles.length, 1, "the deleted jar is still on file");
  assertEqual(backend.browserProfiles[0].id, "1".repeat(32), "delete took the wrong jar");

  backend.browserProfiles = [];
  backend.defaultProfileId = null;
  return "default toggles, pending chip, delete on confirm";
});

/* 파일에서 가져오기(원본 "From File…"): 감지된 브라우저가 없어도 카드의
 * 가져오기 메뉴엔 파일 항목이 서고, 고르면 시스템 손이 파일을 읽어 요약을
 * 세우며 카드 소줄이 그 자리를 적는다. */
await test("a profile card imports cookies from a file", async () => {
  backend.defaultProfileId = null;
  backend.cancelFileImport = false;
  backend.browserProfiles = [{ id: "1".repeat(32), name: "일하는 계정" }];
  await openSettings(pageA, "browser");
  await pageA.waitForFunction(
    () => document.querySelectorAll("#browser-profile-list > *").length === 1,
    undefined,
    { timeout: UI_TIMEOUT },
  );

  // 카드의 가져오기 → 소스 브라우저가 없으니 메뉴엔 "파일에서 가져오기…"만 선다.
  await pageA.locator("#browser-profile-list .browser-profile-import").first().click();
  const fileLabel = await pageA.evaluate(() => t("browser.importFromFile", "파일에서 가져오기…"));
  await pageA.waitForFunction(
    (label) =>
      [...document.querySelectorAll("#sidebar-menu .sidebar-menu-item")].some((one) =>
        one.textContent.includes(label),
      ),
    fileLabel,
    { timeout: UI_TIMEOUT },
  );
  await gestureAndWait(pageA, "A", "import_cookie_file", () =>
    pageA.evaluate((label) => {
      [...document.querySelectorAll("#sidebar-menu .sidebar-menu-item")]
        .find((one) => one.textContent.includes(label))
        ?.click();
    }, fileLabel));

  // 요약 카드가 실제로 뜨고, 가져온 수를 말한다.
  await pageA.waitForFunction(
    () => {
      const card = document.getElementById("cookie-import-pop");
      if (!card || card.hidden) return false;
      const box = card.getBoundingClientRect();
      return box.width > 0 && box.height > 0;
    },
    undefined,
    { timeout: UI_TIMEOUT },
  );
  const said = await pageA.locator("#cookie-import-pop p").allTextContents();
  assert(
    said.some((line) => line.includes("9")),
    `the file-import summary did not name the count: ${JSON.stringify(said)}`,
  );

  // 카드 소줄이 파일 자리("cookies.txt")로 갱신된다.
  await pageA.waitForFunction(
    () =>
      document
        .querySelector("#browser-profile-list .browser-profile-note")
        ?.textContent?.includes("cookies.txt"),
    undefined,
    { timeout: UI_TIMEOUT },
  );

  backend.browserProfiles = [];
  return "file import summary + source line";
});

await test("Blank starts a deliberately plain terminal", async () => {
  await openSettings(pageB, "agents");
  if (!same(backend.settings.default_agent, { kind: "blank" })) {
    await gestureAndWait(pageB, "B", "set_default_agent", () =>
      pageB.click('.agent-pill[data-agent="blank"]'));
  }
  await pageB.evaluate(() => setSettingsOpen(false));
  const from = backend.calls.length;
  await pageB.evaluate(() => openTermTab({ placement: "tab" }));
  const opened = await backend.waitForCall("B", "open_term_tab", from);
  assert(opened.args.plain === true, "Blank fell through to the configured terminal command", opened);
});

await test("설정 문법: 칸은 제 값만큼 · 남는 폭은 설명이 · 실패는 우리 말로 접어서 · 머리는 네 낱말", async () => {
  /* The pane the grammar is written on, read at a width where the old pane's
     complaint is visible: a 2000px window gives this column 1720px, and every
     control in it used to be 1598px wide. */
  await pageA.setViewportSize({ width: 2000, height: 1200 });
  try {
    await openSettings(pageA, "api-routers");
    backend.keychain.delete(TYPESAFE_SERVICE);
    delete backend.zoSettings.smart;
    backend.zoSettings.providers = [];
    await pageA.evaluate(() => refreshApiRouters());
    await renderSettled(pageA);
    const said = (key, fallback, vars) =>
      pageA.evaluate(([one, words, values]) => t(one, words, values), [key, fallback, vars]);
    const box = (selector) => pageA.locator(selector).boundingBox();

    // ---- 1. a field is as wide as its value, and it names a rung ----------
    const rungs = await pageA.evaluate(() => {
      const root = getComputedStyle(document.documentElement);
      return ["sm", "md", "lg", "full"].map((rung) => root.getPropertyValue(`--field-${rung}`).trim());
    });
    assertEqual(rungs, ["18ch", "26ch", "34ch", "100%"], "the field rungs are not four scale tokens");
    const pane = await box('.settings-pane[data-pane="api-routers"]');
    const key = await box("#router-key-input");
    assert(pane.width > 1200, "the pane was not read at a width where this matters", pane.width);
    assert(
      key.width < pane.width / 4,
      "the API key box still fills the column instead of holding a key",
      { key: Math.round(key.width), pane: Math.round(pane.width) },
    );
    // Every field on the pane picks a rung, and no field carries a width in px.
    const widths = await pageA.evaluate(() => [...document
      .querySelectorAll('.settings-pane[data-pane="api-routers"] .settings-field')]
      .map((field) => ({
        rung: field.dataset.field ?? null,
        inline: field.getAttribute("style") ?? "",
      })));
    assert(widths.length > 0, "the pane has no fields to read");
    assertEqual(
      widths.filter((field) => field.rung === null || field.inline.includes("px")),
      [],
      "a field on this pane sets its own width instead of naming a rung",
    );

    // ---- 2. the leftover width belongs to the words ----------------------
    const label = await box('label[for="router-base-url-input"] .settings-label');
    const url = await box("#router-base-url-input");
    assert(
      label.y < url.y + url.height && url.y < label.y + label.height,
      "the label and its control are not on one line at this width",
      { label, url },
    );
    assert(label.x + label.width <= url.x + 1, "the words run under the control", { label, url });
    const rowEnd = await pageA.evaluate(() => {
      const field = document.querySelector('label[for="router-base-url-input"]');
      const style = getComputedStyle(field);
      return field.getBoundingClientRect().right - parseFloat(style.paddingRight);
    });
    assert(
      Math.abs(url.x + url.width - rowEnd) < 1.5,
      "the control does not take the end of the row",
      { control: url.x + url.width, rowEnd },
    );
    const copy = await pageA.evaluate(() => {
      const first = document.querySelector('.settings-pane[data-pane="api-routers"] .settings-field-copy');
      return { width: first.getBoundingClientRect().width, max: getComputedStyle(first).maxWidth };
    });
    assert(copy.max.endsWith("px") && parseFloat(copy.max) > 0, "the words carry no measure", copy);
    assert(copy.width <= parseFloat(copy.max) + 1, "the words outran their measure", copy);

    // And a column too narrow for two things stacks them, as it always did.
    // The COLUMN is narrowed rather than the window: that is the claim — the
    // rail takes 280px before the pane sees any, so the pane's own width is
    // what the row can be asked about.
    const narrow = await pageA.evaluate(() => {
      const pane = document.querySelector('.settings-pane[data-pane="api-routers"]');
      pane.style.maxWidth = "560px";
      return pane.getBoundingClientRect().width;
    });
    await renderSettled(pageA);
    assert(narrow < 620, "the narrow reading was not taken below the gate", narrow);
    const stackedLabel = await box('label[for="router-base-url-input"] .settings-label');
    const stackedUrl = await box("#router-base-url-input");
    assert(
      stackedUrl.y >= stackedLabel.y + stackedLabel.height - 1,
      "a narrow column did not stack the field",
      { stackedLabel, stackedUrl },
    );
    await pageA.evaluate(() => {
      document.querySelector('.settings-pane[data-pane="api-routers"]').style.maxWidth = "";
    });
    await renderSettled(pageA);

    // ---- 3. a refusal says ours first and folds the server's away --------
    const body = '서버 오류: HTTP 401 Unauthorized — 无效的令牌 (request id: 20260918103344512345678901234)';
    backend.routerProbeRefusal = body;
    try {
      await pageA.fill("#router-base-url-input", origin);
      await pageA.fill("#router-name-input", "refused");
      const testedAt = backend.calls.length;
      await pageA.click("#router-test-btn");
      await backend.waitForCall("A", "test_router_connection", testedAt);
      await pageA.waitForSelector("#router-test-status .settings-status-said", { timeout: UI_TIMEOUT });
      assertEqual(
        await statusSaid(pageA, "router-test-status"),
        await said("settings.apiRouters.testRefused", "연결하지 못했습니다 — 기본 URL과 API 키를 확인하세요."),
        "the refusal did not lead with our own sentence",
      );
      const fold = pageA.locator("#router-test-status .settings-status-raw");
      assert(await fold.count() === 1, "the server's words are not behind a fold");
      assert(
        !(await pageA.locator("#router-test-status .settings-status-body").isVisible()),
        "the server's words are open before anybody asked for them",
      );
      assertEqual(
        await pageA.locator("#router-test-status .settings-status-body").textContent(),
        body,
        "the fold does not hold what the server actually said",
      );
      assertEqual(
        await pageA.locator("#router-test-status .settings-status-id").textContent(),
        await said("settings.status.requestId", "요청 id — {{id}}", { id: "20260918103344512345678901234" }),
        "the request id was left inside the prose",
      );
      // Ours is one line at this measure; the server's used to be two.
      const line = await pageA.locator("#router-test-status .settings-status-said").boundingBox();
      const leading = await pageA.evaluate(() =>
        parseFloat(getComputedStyle(document.querySelector("#router-test-status .settings-status-said")).lineHeight));
      assertEqual(Math.round(line.height / leading), 1, "our sentence does not fit one line");
    } finally {
      delete backend.routerProbeRefusal;
    }

    // ---- 4. the card head says where it stands, in the one table's words --
    assertEqual(
      await pageA.locator("#router-card-state").textContent(),
      await standingWord(pageA, "failed"),
      "a card that could not connect does not say so in its head",
    );
    const table = await pageA.evaluate(() => SETTINGS_STANDINGS.map((row) => row.id));
    assertEqual(table, ["connected", "keySaved", "unchecked", "failed"], "the standings are not one four-row table");
    assertEqual(
      await pageA.locator("#typesafe-key-state").textContent(),
      await standingWord(pageA, "unchecked"),
      "the TypeSafe card head does not read from the same table",
    );

    // ---- 5. five seats, one row each, the paragraph folded ---------------
    const seats = await pageA.evaluate(() => [...document.querySelectorAll("[data-jev-row]")]
      .filter((row) => !row.hidden)
      .map((row) => ({
        order: Number(row.style.order),
        seat: row.querySelector("[data-jev-seat]")?.dataset.jevSeat ?? null,
        name: row.querySelector(".settings-label")?.textContent.trim() ?? "",
        summary: row.querySelector(".settings-row-desc")?.textContent.trim() ?? "",
        folded: row.querySelector("details.settings-fold")?.open === false,
        paragraph: row.querySelector("details.settings-fold p")?.textContent.trim().length ?? 0,
      })));
    assertEqual(
      seats.map((row) => row.seat),
      JEV_SEATS.map((seat) => seat.id),
      "the card's rows are not the use table's rows, in its order",
    );
    assertEqual(seats.map((row) => row.order), JEV_SEATS.map((_, at) => at), "a row does not take the table's place");
    assertEqual(
      seats.filter((row) => row.name === "" || row.summary === "" || !row.folded || row.paragraph < 80),
      [],
      "a seat is not [name · mode · one line] with its paragraph folded away",
    );
    assertEqual(
      await pageA.locator("#jev-uses-count").textContent(),
      String(JEV_SEATS.length),
      "the card does not count the seats the table named",
    );

    // ---- 6. one thing to finish, and it is the only filled button --------
    for (const [card, finish] of [["router-card", "router-save-btn"], ["typesafe-card", "typesafe-save-btn"]]) {
      const bottom = await pageA.evaluate((id) => {
        const box = document.getElementById(id);
        const last = box.lastElementChild;
        const filled = [...box.querySelectorAll(".btn--primary")].map((one) => one.id);
        return {
          actions: last.className,
          buttonsOutside: [...box.querySelectorAll(".btn")]
            .filter((one) => one.closest(".settings-action-row") !== last).map((one) => one.id),
          filled,
        };
      }, card);
      assert(bottom.actions.includes("settings-action-row"), `${card} does not end in its actions`, bottom);
      assertEqual(bottom.buttonsOutside, [], `${card} leaves a button loose above its footer`);
      assertEqual(bottom.filled, [finish], `${card} does not fill exactly the one thing to finish`);
    }
    return `key ${Math.round(key.width)}px in a ${Math.round(pane.width)}px column`;
  } finally {
    await pageA.setViewportSize({ width: 1280, height: 860 });
    await renderSettled(pageA);
  }
});

await test("the stateful backend refused every command the fixture did not define", async () => {
  assertEqual(backend.unknown, [], "unknown IPC was treated as success");
});

await test("both documents raised no page errors", async () => {
  assertEqual(faults, [], "browser page errors");
});

await browser.close();
files.close();

let failed = 0;
for (const result of results) {
  if (!result.pass) failed += 1;
  const detail = result.detail ? `  — ${result.detail}` : "";
  console.log(`${result.pass ? "PASS" : "FAIL"}  ${result.name}${detail}`);
}
console.log(`\n${results.length - failed}/${results.length} passed`);
process.exit(failed ? 1 : 0);
