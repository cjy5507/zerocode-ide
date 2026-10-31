/* Settings, driven through a stateful backend.
 *
 * This suite is intentionally separate from window.mjs. The broad window
 * suite is a renderer fixture; this one is a persistence/concurrency fixture:
 * one Node-owned settings store, two real documents, strict IPC, revisions,
 * failed writes, a lost acknowledgement, and cross-window events.
 *
 *   node ui/tests/settings.mjs
 */

import { endRun } from "./end-run.mjs";
import { PLATFORM_OVERRIDE, reportedPlatformScript, TEST_PLATFORM } from "./test-platform.mjs";
import { createRequire } from "node:module";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import {
  SETTINGS_QUALITY_SPEC,
  axeViolations,
  contrastTable,
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
const SUMMON_PROFILES = JSON.parse(await readFile(resolve(UI, "..", "crates", "zerocode-core", "src", "summon-profiles.json"), "utf8"));
/* What this harness's lineup offers each agent (`summon_lineup` picks): the
 * shipped table's models, one newly arrived, and for zo — which runs any
 * provider's model — a connection's. */
const FIXTURE_EFFORTS = ["low", "medium", "high", "xhigh", "max"];
const FIXTURE_LINEUP = {
  codex: [
    ...["low", "mid", "high"].map((difficulty) => ({
      id: SUMMON_PROFILES.codex[difficulty].model, fresh: false, provider: "openai", connection: false,
      efforts: FIXTURE_EFFORTS, effortsKnown: true, fits: [difficulty],
    })),
    { id: "gpt-fixture-new", fresh: true, provider: "openai", connection: false, efforts: ["low", "medium", "high"], effortsKnown: true, fits: ["low"] },
  ],
  claude: [
    { id: SUMMON_PROFILES.claude.low.model, fresh: false, provider: "claude", connection: false, efforts: FIXTURE_EFFORTS, effortsKnown: true, fits: ["low", "mid", "high"] },
  ],
  zo: [
    { id: SUMMON_PROFILES.claude.low.model, fresh: false, provider: "claude", connection: false, efforts: FIXTURE_EFFORTS, effortsKnown: true, fits: ["low", "mid", "high"] },
    { id: "fixture-router/model-x", fresh: false, provider: "fixture-router", connection: true, efforts: ["medium", "high", "max"], effortsKnown: false, fits: ["low"] },
  ],
};
/* The rows each agent runs when nobody wrote one: the shipped table, and for
 * zo — which the table has no row for — the lineup's model. Each names a
 * model only: no effort is shipped, and an effort nobody wrote stays off the
 * command line for the agent's CLI to default (t-74181). */
const FIXTURE_SHIPPED = {
  ...SUMMON_PROFILES,
  zo: Object.fromEntries(["low", "mid", "high"].map((difficulty) =>
    [difficulty, { model: SUMMON_PROFILES.claude.low.model }])),
};
const TERMINAL_THEME_CATALOG = Object.freeze(JSON.parse(
  await readFile(
    resolve(UI, "..", "crates", "zerocode-shell", "src", "terminal_themes.json"),
    "utf8",
  ),
));
/* The keychain item the TypeSafe key lives in — `typesafe_settings.rs`
 * (`SERVICE_KEYCHAIN_SERVICE_PREFIX` + `TYPESAFE_API_KEY_ENV`). */
const TYPESAFE_SERVICE = "dev.zerocode.key.TYPESAFE_API_KEY";
/* The judgment card's current line and its state word (t-75762 v2): the
   model the settings select, and what the server it runs on answered. */
const JUDGMENT_NOW = '[data-model-current="judgment"] .model-line-name';
const JUDGMENT_STATE = '[data-model-current="judgment"] .model-state';
/* A saved connection's row on the connections page — the rows the settings'
   providers make, not a login, a managed model or Jev (t-75762 v2). */
const SAVED_CONNECTION_ROWS = '#model-connection-groups .model-line[data-connection-id]:not([data-connection-id^="managed:"]):not([data-connection-id="jev"])';
/* A zo model list as `zo models --json` answers it (t-75762 덧붙임 D):
   three providers' login models with an older generation among them, and
   each source's report. Every id is the fixture's own; the screen reads
   whatever zo lists, so no real name is needed to prove it. */
const ZO_LIST_FIXTURE = Object.freeze({
  models: [
    { id: "fixture-claude-old", provider: "claude", displayName: "Fixture Claude Old", band: "superseded", supersededBy: "fixture-claude-top" },
    { id: "fixture-claude-top", provider: "claude", displayName: "Fixture Claude Top", band: "top", supersededBy: null },
    { id: "fixture-claude-second", provider: "claude", displayName: "Fixture Claude Second", band: "second", supersededBy: null },
    { id: "fixture-openai-top", provider: "openai", displayName: "Fixture OpenAI Top", band: "top", supersededBy: null },
    { id: "fixture-openai-old", provider: "openai", displayName: "Fixture OpenAI Old", band: "superseded", supersededBy: "fixture-openai-top" },
    { id: "fixture-google-flash", provider: "google", displayName: "Fixture Google Flash", band: null, supersededBy: null },
  ],
  reports: [
    { provider: "anthropic", source: "anthropic-api", ok: true, count: 3, age: "5m old" },
    { provider: "openai", source: "chatgpt-backend", ok: true, count: 2, age: "12m old" },
    { provider: "google", source: "antigravity-registry", ok: true, count: 1, age: "2h old" },
    { provider: "xai", source: "xai-api", ok: false, count: 0, diagnosis: { status: null, kind: null, skipped: true }, age: "12m old" },
  ],
  aliases: [{ alias: "fixture-claude", canonical: "fixture-claude-top", provider: "anthropic" }],
});
/* The value seat's table — the very file the product reads
 * (`zerocode_core::type_value`) — and the window's key store's prefix
 * (`zerocode_harness::SERVICE_KEYCHAIN_SERVICE_PREFIX`, held here by the Rust
 * contract `the_settings_harness_mirrors_the_key_stores_prefix`). */
const TYPE_VALUE_TABLE = Object.freeze(JSON.parse(
  await readFile(
    resolve(UI, "..", "crates", "zerocode-core", "fixtures", "type-value", "models.json"),
    "utf8",
  ),
));
const TYPE_VALUE_KEY_PREFIX = "dev.zerocode.key.";
/* Mirrors `type_value_keys.rs`: the Computer Use pane offers the key the
 * chosen row reads, when its road is one the walk's writer takes
 * (`value::endpoint_of` — no client spoken for, Code Assist not built), kept
 * in the item the writer reads (`value::key_service`), with every row of the
 * table that reads that item down a road it takes. */
const typeValueService = (row) => row.keychainService
  ?? (row.credentialKey ? `${TYPE_VALUE_KEY_PREFIX}${row.credentialKey}` : null);
const typeValueTaken = (row) => Boolean(row.credentialKey) && !row.clientFingerprint
  && (row.road === "anthropic" || (row.road === "openai-compat" && Boolean(row.baseUrl)));
function typeValueSlots() {
  const chosen = TYPE_VALUE_TABLE.rows.find((row) => row.id === TYPE_VALUE_TABLE.chosen);
  if (!chosen || !typeValueTaken(chosen)) return [];
  const service = typeValueService(chosen);
  return [{
    name: chosen.credentialKey,
    service,
    chosen: chosen.id,
    rows: TYPE_VALUE_TABLE.rows.filter((row) => typeValueTaken(row) && typeValueService(row) === service),
  }];
}
/* Mirrors `type_value_keys::card` (t-10372): the login roads `auto` tries, in
 * order, each with the row the table names for it — the words are the core's
 * `GeneratorRoad`, held to the markup's radios by the Rust contract
 * `the_generator_road_rides_the_settings_document_and_the_card_says_whose_login`. */
const GENERATOR_ROADS = Object.freeze(["auto", "claude_login", "codex_login", "api_key", "off"]);
/* The two words `AgentPaneFirstView` takes (t-82038), held to the markup's
 * radios by the Rust contract `the_agent_pane_first_view_is_one_remembered_choice`. */
const AGENT_PANE_FIRST_VIEWS = Object.freeze(["conversation", "terminal"]);
const GENERATOR_LOGINS = Object.freeze([
  { road: "claude_login", row: TYPE_VALUE_TABLE.claudeLogin },
  { road: "codex_login", row: TYPE_VALUE_TABLE.codexLogin },
]);
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
   settings key a person may write by hand, the modes it offers — a labeled
   seat offers all four, a seat nothing labels offers no `auto` — and the mode
   it stands at while Jev is switched on and nobody wrote a word for it
   (`JevUse::recommended`) — `off` for the two seats stopped on their own
   evidence (t-6342) — and, for a seat split off another, the setting whose
   word it reads while it has none of its own (`JevUse::follows`, t-6877).
   `typesafe_settings.rs` holds this list against the table. */
const JEV_SEATS = Object.freeze([
  Object.freeze({ id: "routing", setting: "decisionShadow", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "recall", setting: "rerankShadow", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "skills", setting: "skillSearch", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "skill_suggestion", setting: "skillSuggestion", modes: "off shadow on auto", recommended: "auto", follows: "skillSearch" }),
  Object.freeze({ id: "browser", setting: "browserAction", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "desktop", setting: "desktopAction", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "emulator", setting: "emulatorAction", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "cover", setting: "jevCover", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "stall", setting: "stallCause", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "placement", setting: "workerPlacement", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "summon", setting: "summonChoice", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "summon_difficulty", setting: "summonDifficulty", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "summon_model", setting: "summonModel", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "model_tiers", setting: "jevModelTiers", modes: "off shadow on", recommended: "shadow" }),
  Object.freeze({ id: "effort", setting: "stepEffort", modes: "off shadow on auto", recommended: "off" }),
  Object.freeze({ id: "step_effort", setting: "zoStepEffort", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "compaction", setting: "jevCompaction", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "agent_tool", setting: "agentTool", modes: "off shadow on", recommended: "on" }),
  Object.freeze({ id: "project_rules", setting: "projectRules", modes: "off shadow on", recommended: "on" }),
  Object.freeze({ id: "custom", setting: "customDecisions", modes: "off shadow on", recommended: "on" }),
  Object.freeze({ id: "browser_read", setting: "jevBrowserRead", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "notify", setting: "jevNotify", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "mention_rerank", setting: "jevMentionRerank", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "branching", setting: "jevBranching", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "judgment_cache", setting: "jevJudgmentCache", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "mail_triage", setting: "jevMailTriage", modes: "off shadow", recommended: "shadow" }),
  Object.freeze({ id: "challenger", setting: "jevChallenger", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "patch_review", setting: "jevPatchReview", modes: "off shadow on auto", recommended: "off" }),
  Object.freeze({ id: "claim", setting: "jevClaimCheck", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "vault_pairs", setting: "jevVaultPairs", modes: "off shadow on", recommended: "shadow" }),
  Object.freeze({ id: "file_pick", setting: "jevFilePick", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "command_guard", setting: "jevCommandGuard", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "tool_text_guard", setting: "jevToolTextGuard", modes: "off shadow on auto", recommended: "auto" }),
  Object.freeze({ id: "reflex_decide", setting: "jevReflexDecide", modes: "off shadow on auto", recommended: "auto" }),
]);
/* The door's object under `smart` and the one word its folder list may hold
   that is not a folder (`zerocode_core::jev::door::JEV_SETTINGS_KEY`,
   `EVERY_WORKSPACE`) — what a press of the one switch writes
   (`door::switch_on`). `typesafe_settings.rs` holds these to the core's. */
const JEV_SETTINGS_KEY = "jev";
const JEV_EVERY_WORKSPACE = "*";
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
   and what each one does. Only the probing word calls a probe; the routing
   seat is asked under every word but `off` (t-6346) — `typesafe_settings.rs`
   holds this fixture to the table and to zo's own source. */
const CLASSIFIER_SETTING = "autoClassifier";
const CLASSIFIER_MODES = Object.freeze([
  Object.freeze({ mode: "off", runs: false, markers: false, probes: false, reaches: false }),
  Object.freeze({ mode: "deterministic", runs: true, markers: false, probes: false, reaches: true }),
  Object.freeze({ mode: "assisted", runs: true, markers: true, probes: false, reaches: true }),
  Object.freeze({ mode: "probed", runs: true, markers: false, probes: true, reaches: true }),
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
  "status_bar_items_off": [],
  "usage_percentage_display": "used",
  "status_bar_usage_mode": "verbose",
  "usage_analytics_off": [],
  "source_control_view_mode": "list",
  "show_titlebar_app_name": true,
  "show_menu_bar_icon": true,
  "minimize_to_tray_on_close": false,
  "compact_worktree_cards": false,
  "second_brain_weekly_review": false,
  "second_brain_raw_capture": false,
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
  "sidebar_view": { "group_by": "repo", "sort_by": "default", "project_order": "default", "layout": "team" },
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
  "artifacts_auto_open_beside": true,
  "diff_side_by_side": true,
  "conversation_timeline_view": false,
  "agent_pane_first_view": "conversation",
  "window_material": { "terminal_opacity": 1.0, "blur": false },
  "default_agent": { "kind": "auto" },
  "agent_teams_mode": "panes",
  "claude_autoswitch_mode": "ask",
  "harness": {
    "gate": { "mode": "notify", "task_usd": null, "day_usd": null },
    "launches": { "concurrent": 4, "per_hour": 300, "per_day": 2000 },
    "alerts": { "landing": true }
  },
  "worktree_prefs": { "branch_prefix": "git-username" },
  "notifications": { "enabled": true, "agent_attention": true, "agent_completion": "long" },
  "computer_awake_mode": "off",
  "computer_confirm_payment": true,
  "computer_confirm_transfer": true,
  "computer_confirm_delete": true,
  "computer_live_reflex": false,
  "computer_share_required": false,
  "computer_input_policy": "background",
  "computer_generator_road": "auto",
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
  "set_notification_preference", "set_finish_notification_mode", "set_browser_home_page", "set_browser_search_engine",
  "patch_browser_link_routing", "patch_browser_user_agents", "set_browser_restore_tabs", "set_browser_default_zoom", "set_browser_open_tabs",
  "set_terminal_opacity",
  "set_window_blur", "set_agent_teams_mode", "set_claude_autoswitch_mode", "set_harness_settings", "set_default_agent",
  "set_shortcut_visibility", "set_task_source_visibility", "set_keybinding",
  "set_diff_side_by_side", "set_conversation_timeline_view", "set_agent_pane_first_view", "set_confirm_close_pinned",
  "set_computer_live_reflex", "set_computer_generator_road", "set_computer_input_policy", "set_computer_share_required",
  "set_skip_close_terminal_with_running_process_confirm", "set_ctrl_tab_order_mode",
  "patch_workspace_creation_prefs",
  "patch_floating_workspace",
  "patch_open_in_applications",
  "set_skip_delete_worktree_confirm", "set_skip_delete_automation_confirm",
  "set_vault_session_limit",
  "set_artifacts_retention_days",
  "set_artifacts_auto_open_beside",
  "set_second_brain_weekly_review",
  "set_second_brain_raw_capture",
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
    this.nextWitness = 40;
    // A wait or a logout that answers with a table read EARLIER than the one
    // the rows hold now — a completion that arrives late (t-7170 R2b).
    // Consumed once.
    this.staleWaitAnswer = null;
    this.terminalSessions = [];
    this.routerPresets = [...ROUTER_PRESETS_CATALOG];
    this.zoSettings = { providers: [] };
    // What `zo models --json` lists besides the settings' routers: the
    // login models, the sources' reports and the aliases (t-75762) — a
    // test swaps them. Routers zo reads from its own environment ride
    // `zoEnvRouters`; the old judgment setting's keys ride `legacyKeys`.
    this.zoList = clone(ZO_LIST_FIXTURE);
    this.zoEnvRouters = [];
    this.legacyKeys = new Map();
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
      // 주간 리뷰 — 스위치는 설정, 영수증은 창 자동화 목록이 그 일을 든 모습.
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
    // Live reflex (t-10221): the helper reads the kernel, no check kept, and
    // a check made now passes unless a refusal is set.
    this.liveReflexHelperReads = true;
    this.liveReflexCheck = null;
    this.liveReflexRefusal = null;
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
        // The person's own switch, as Rust's `switch_status_bar_item` keeps it:
        // on goes into the list and out of the off record, off the other way.
        const enabled = new Set(this.settings.status_bar_items);
        const off = new Set(this.settings.status_bar_items_off ?? []);
        if (args.enabled === true) {
          enabled.add(args.item);
          off.delete(args.item);
        } else {
          enabled.delete(args.item);
          off.add(args.item);
        }
        this.settings.status_bar_items = available.filter((item) => enabled.has(item));
        this.settings.status_bar_items_off = [...off].sort();
        keys = ["status_bar_items", "status_bar_items_off"];
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
      case "set_artifacts_auto_open_beside":
        this.settings.artifacts_auto_open_beside = args.on === true;
        keys = ["artifacts_auto_open_beside"];
        break;
      case "set_conversation_timeline_view":
        this.settings.conversation_timeline_view = args.on === true;
        keys = ["conversation_timeline_view"];
        break;
      case "set_agent_pane_first_view":
        // serde's refusal of a word `AgentPaneFirstView` does not name.
        if (!AGENT_PANE_FIRST_VIEWS.includes(args.view)) throw `unknown variant \`${args.view}\``;
        this.settings.agent_pane_first_view = args.view;
        keys = ["agent_pane_first_view"];
        break;
      case "set_computer_live_reflex":
        this.settings.computer_live_reflex = args.on === true;
        keys = ["computer_live_reflex"];
        break;
      case "set_computer_share_required":
        this.settings.computer_share_required = args.on === true;
        keys = ["computer_share_required"];
        break;
      case "set_computer_input_policy":
        if (!["background", "shared-desktop"].includes(args.policy)) throw "invalid input policy";
        this.settings.computer_input_policy = args.policy;
        keys = ["computer_input_policy"];
        break;
      case "set_computer_generator_road":
        // serde's refusal of a word `GeneratorRoad` does not name.
        if (!GENERATOR_ROADS.includes(args.road)) throw `unknown variant \`${args.road}\``;
        this.settings.computer_generator_road = args.road;
        keys = ["computer_generator_road"];
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
        // The backend writes the window's job first (t-77377) and the
        // setting only then; the fixture answers what the store would hold:
        // a job that has not run yet, due the coming Sunday at 06:00.
        const enabled = args.enabled === true;
        this.settings.second_brain_weekly_review = enabled;
        this.secondBrain = {
          ...this.secondBrain,
          weekly_review_enabled: enabled,
          weekly_review: enabled
            ? { registered: true, enabled: true, next_run_at: 29_891_880, last_run_at: null }
            : null,
        };
        keys = ["second_brain_weekly_review"];
        break;
      }
      case "set_second_brain_raw_capture": {
        // The backend refuses to turn the feed on without a saved vault and
        // writes the one switch otherwise; the card's vault is saved by the
        // time the section presses it.
        this.settings.second_brain_raw_capture = args.enabled === true;
        keys = ["second_brain_raw_capture"];
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
      case "set_finish_notification_mode":
        this.settings.notifications = {
          ...this.settings.notifications,
          agent_completion: args.mode,
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
      case "set_claude_autoswitch_mode":
        if (!["off", "ask", "auto"].includes(args.mode)) throw new Error(`${args.mode}는 이 창이 아는 자동 전환 낱말이 아닙니다 (off|ask|auto)`);
        this.settings.claude_autoswitch_mode = args.mode;
        keys = ["claude_autoswitch_mode"];
        break;
      case "set_harness_settings": {
        // The Rust contract (`HarnessSettings::parse`): a launch ceiling is a whole
        // number of launches or blank — anything else refuses the save, so a typo
        // never silently lifts a ceiling — and a budget that is not a positive
        // amount under the typo guard is none, field by field.
        const asked = args.harness ?? {};
        const launches = { ...this.settings.harness.launches };
        for (const key of ["concurrent", "per_hour", "per_day"]) {
          if (!(key in (asked.launches ?? {}))) continue;
          const value = asked.launches[key];
          if (value !== null && !(Number.isInteger(value) && value >= 0)) {
            throw new Error("실행 한도는 0 이상의 정수나 빈칸(한도 없음)이어야 합니다");
          }
          launches[key] = value;
        }
        const budget = (value) => (Number.isFinite(value) && value > 0 && value <= 100_000 ? value : null);
        this.settings.harness = {
          gate: {
            mode: ["off", "notify", "stop"].includes(asked.gate?.mode) ? asked.gate.mode : "notify",
            task_usd: budget(asked.gate?.task_usd),
            day_usd: budget(asked.gate?.day_usd),
          },
          launches,
          // The Rust contract: only a plain `false` turns the ledger's alerts off.
          alerts: { landing: asked.alerts?.landing !== false },
        };
        keys = ["harness"];
        break;
      }
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
      case "report_terminal_colors": return null;
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
      case "worktree_landing_stamp": return "";
      // The release lane's two files, read once at boot and on the usage
      // gauge's period (t-3005). This fixture's machine has no lane: two
      // nulls and no notice, which is what a fresh install answers.
      case "release_status": return { status: null, installed: null, notice: null };
      // What a restart left for the window it started (t-67936): this
      // fixture's window was started fresh, so nothing.
      case "boot_after_restart": return null;
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
      // The tree's scoped question after an agent's write ends (t-31715): this fixture's
      // workspace has no agent writing in it, so nothing changed.
      case "scm_numstat": return [];
      // The file tree's head reads the branch's standing (t-24298); this fixture's
      // workspace tracks no upstream, so the answer is the real backend's for that.
      case "upstream_status": return { upstream: null, ahead: 0, behind: 0, behind_commits_are_patch_equivalent: null };
      case "list_dir": return [];
      case "stage_layouts": return null;
      case "pane_layouts": return [];
      // 재시작 때 대화가 돌던 다른 작업 공간(t-14036) — 이 픽스처에는 없다.
      case "standing_pane_worktrees": return [];
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
        return this.grokUsage ? clone(this.grokUsage) : { usage: null, fetching: false };
      case "opencode_usage":
        return { usage: null, fetching: false };
      // 계정마다 제 게이지와 전환 표의 판정(t-7538) — 계정이 없는 이 픽스처에서는
      // 「혼자」이고 아무것도 나가지 않는다. 모드는 설정 문서의 것.
      case "claude_account_usage":
        return {
          accounts: [],
          plan: {
            mode: this.settings.claude_autoswitch_mode,
            active: null,
            decision: { kind: "stay", why: "alone" },
            landing: null,
            fitness: [],
            next: null,
            walled: [],
            last_switch_ms: null,
            cooldown_until_ms: null,
            failed_recently: [],
            token: "settings-fixture",
            now_ms: 0,
          },
          sent: 0,
          fetching: false,
        };
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
      // 런마다 코디 자리(t-45938: 역할 칩) — 런이 없는 창은 실제 백엔드가 빈
      // 목록으로 답한다(`cmd::board::coordinator_seats`).
      case "coordinator_seats": return [];
      case "board_columns": return [];
      // 그래프 보드가 묻는 스냅샷 — 열 없음·주의 0·전체 0은 실제 백엔드가 카드
      // 없는 창에 답하는 그대로(`zerocode_core::board::snapshot`).
      case "board_snapshot": return { columns: [], attention_count: 0, total_count: 0 };
      case "pane_subagents": return [];
      // 받은 편지함 — 기다리는 것이 없는 창이 실제 백엔드에서 받는 답(t-26595, t-67483).
      case "board_inbox": return { items: [], counts: { all: 0, question: 0, handoff: 0, blocked: 0, notice: 0, waiting: 0 } };
      case "clear_finish_mark": return false;
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
      case "claude_autoswitch_mode": return this.settings.claude_autoswitch_mode;
      // What the card says of the window's counting: what is running, what the
      // last hour and day let through, which provider rests, what the day cost.
      // What a person set is the document's, and comes with its snapshot.
      case "harness_status":
        return {
          counters: {
            active: 1, last_hour: 3, last_day: 12,
            resting: [{ provider: "claude", until_ms: Date.now() + 11 * 60_000 }],
          },
          day_spent_usd: 4.2,
        };
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
      // The integration queue's switch for the active project (t-45315): off until a person
      // turns it on, and on only where this machine runs the queue (macOS for now).
      case "integration_queue_setting": return clone(this.integrationQueue ?? { enabled: false, supported: true });
      case "set_integration_queue_enabled": {
        const held = this.integrationQueue ?? { enabled: false, supported: true };
        if (args.enabled === true && !held.supported) throw "the integration queue can be turned on only on macOS for now";
        this.integrationQueue = { ...held, enabled: args.enabled === true };
        return clone(this.integrationQueue);
      }
      case "worktree_prefs": return { branch_prefix: "none", custom_prefix: null };
      // Which releases the catalog superseded, asked once as every window boots (t-46882): this
      // fixture's lineup supersedes none, so it answers with the real command's empty map.
      case "superseded_models": return {};
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
      case "cli_login_list": {
        // What the read found as it began: a read held in flight answers the
        // table as it stood then, however the table moved while it was out —
        // and only that one read is held, so a later read answers first
        // (t-7170 R2b).
        const answer = clone(this.cliLogins ?? { rows: [] });
        const held = this.holds.get(command);
        if (held && !held.taken) {
          held.taken = true;
          held.arrive();
          await held.promise;
        }
        return answer;
      }
      // The three doors that MOVE a login answer with the table re-read, the
      // way the backend does: the row the test asked about flips and the
      // rest stand.
      case "cli_login_start":
      case "cli_login_wait": {
        // A wait that ran out says so the way the backend does — the ceiling
        // reached with the file unchanged (t-7170 R2).
        if (this.rejectNext.delete(command)) throw new Error("로그인이 완료되지 않았습니다");
        // A held wait stands where the backend's would: the file has not
        // moved yet, and the walk is out until the test lets it land.
        const holding = this.holds.get(command);
        if (holding) {
          holding.arrive();
          await holding.promise;
        }
        const held = this.cliLogins.rows.find((row) => row.agent === args.agent);
        if (held) held.signed_in = args.signedIn ?? true;
        if (this.staleWaitAnswer !== null) {
          const stale = this.staleWaitAnswer;
          this.staleWaitAnswer = null;
          return stale;
        }
        return clone(this.cliLogins);
      }
      // The witness taken BEFORE a CLI runs (t-7170 R2): the backend keeps the
      // snapshot and hands the window a number to carry into the wait.
      case "cli_login_witness": {
        if (this.rejectNext.delete(command)) throw new Error("refused: cli_login_witness");
        return ++this.nextWitness;
      }
      case "cli_login_witness_drop": return null;
      // The launch door the CLI login roads open an agent's own pane through:
      // a shell id, the way `open_term_tab` answers.
      case "launch_agent_tab": {
        if (this.rejectNext.delete(command)) throw new Error("refused: launch_agent_tab");
        return ++this.nextTerm;
      }
      case "cli_login_logout": {
        const held = this.cliLogins.rows.find((row) => row.agent === args.agent);
        if (held) held.signed_in = false;
        if (this.staleWaitAnswer !== null) {
          const stale = this.staleWaitAnswer;
          this.staleWaitAnswer = null;
          return stale;
        }
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
          tcc_rows: clone(this.computerUseTccRows ?? []),
        };
      case "computer_use_tcc_row_action": {
        // `tccutil reset` takes the row away: it reads denied, with no
        // buttons, until the app is added again (t-6058).
        const at = (this.computerUseTccRows ?? []).findIndex((row) =>
          row.id === args.id && row.bundle_id === args.bundleId);
        if (at < 0 || !this.computerUseTccRows[at].actions.includes(args.action)) {
          throw new Error(`the row offers no ${args.action}`);
        }
        if (args.action === "reset") {
          this.computerUseTccRows[at] = { ...this.computerUseTccRows[at], grant: "denied", pin: undefined, actions: [] };
        }
        return {
          platform: this.computerUsePlatform ?? "darwin",
          helper_app_path: "/Applications/ZeroCode Computer Use.app",
          helper_unavailable_reason: null,
          permissions: clone(this.computerUsePermissions),
          tcc_rows: clone(this.computerUseTccRows),
        };
      }
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
      // Mirrors `reflex::settings_answer`: the handshake with `liveReflex`
      // as the door reads it (this platform and helper, the person's
      // setting) and the check kept for this helper, or none.
      case "computer_use_capabilities":
        return this.computerUseCapabilities();
      case "computer_live_reflex_check":
        this.liveReflexCheck = {
          ok: this.liveReflexRefusal === null,
          at_ms: Date.now(),
          helper: { version: "1.0.0", planVersion: 1 },
          reason: this.liveReflexRefusal,
        };
        return this.computerUseCapabilities();
      case "list_automations": return clone(this.automations);
      // 세컨드 브레인 카드의 「켜기」(t-77381)가 여는 두 길. 실제 백엔드처럼 목록을 답하고,
      // 카드가 다시 물을 볼트의 매일 취합도 그 자동화를 따라 고친다.
      case "enable_automation": {
        const row = this.automations.find((held) => held.id === args.id);
        if (row) row.enabled = args.enabled;
        const ingest = this.secondBrain.daily_ingest;
        if (ingest?.automation_id === args.id) {
          this.secondBrain.daily_ingest = { ...ingest, enabled: args.enabled, overdue_since: null };
        }
        return clone(this.automations);
      }
      case "save_automation": {
        const automation = args.automation;
        this.automations = [
          ...this.automations.filter((held) => held.id !== automation.id),
          automation,
        ];
        if (automation.workspace === this.secondBrain.saved_path
          && String(automation.prompt).includes("raw/")) {
          this.secondBrain.daily_ingest = {
            automation_id: automation.id,
            name: automation.name,
            enabled: automation.enabled,
            last_run_at: null,
            overdue_since: null,
          };
        }
        return clone(this.automations);
      }
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
      // 빌린 기기 한 줄의 부팅 질문(t-6336) — 이 하니스의 창에는 에이전트의
      // 판이 없으니 빌린 기기도 없다.
      case "emulator_loans": return { count: 0, lastUsedMs: null };
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
      case "model_connections": return this.modelConnections();
      // The window's record of each connection's last check (t-76500,
      // model_connection_checks.rs). A scene keeps one in `connectionMemory`;
      // without it the read fails, the way a window that cannot read its
      // record cannot keep the six-hour spacing and checks nothing quietly.
      case "remembered_model_connection_checks": return this.connectionMemoryRead();
      case "remember_model_connection_check": return this.connectionMemoryKeep(args.answer, "check");
      case "remember_model_judgment_check": return this.connectionMemoryKeep(args.answer, "judgment");
      // `model_connection_checks::remember_applied` (t-76598): the identity
      // read back off the settings, never one the page sends.
      case "remember_judgment_applied": {
        if (!this.connectionMemory) throw new Error("model_connection_checks");
        const applied = this.connectionMemory.applied ??= {};
        const identity = this.typesafeSettings().providerIdentity;
        // The identity applied last is the record's own word (t-76759).
        const last = this.connectionMemory.lastApplied ?? Object.entries(applied).sort((a, b) => b[1] - a[1])[0]?.[0];
        if (last !== identity) applied[identity] = this.connectionMemory.nowMs;
        this.connectionMemory.lastApplied = identity;
        return this.connectionMemoryRead();
      }
      case "zo_model_list":
        if (args?.fresh) this.zoListHeld = null;
        return this.zoListRead();
      case "select_coding_model": {
        // Mirrors `model_connections::apply_coding_model`, checked against
        // the same cached list the screen read.
        const listed = this.zoListRead();
        const model = String(args.model ?? "");
        const zoLists = listed.models.some((row) => row.id === model) || listed.customProviders.some((row) => row.models.includes(model));
        if (!zoLists) throw "provider_model";
        if ((this.zoSettings.model ?? null) !== (args.expectedModel ?? null)) throw "settings_changed";
        const smart = this.zoSettings.smart ??= {};
        const [name, ...rest] = model.split("/");
        const wanted = rest.join("/");
        const connection = rest.length ? this.modelConnections().connections.find((row) => row.provider.name.toLowerCase() === name.toLowerCase()
          && (row.provider.models ?? []).some((one) => (typeof one === "string" ? one : one.id).toLowerCase() === wanted.toLowerCase())) : null;
        smart.previousCodingSelection = this.zoSettings.model ?? null;
        if (connection) smart.codingConnection = { id: connection.id, generation: connection.generation, model: wanted, protocol: "openai_json" };
        else delete smart.codingConnection;
        this.zoSettings.model = model;
        return this.modelConnections();
      }
      case "adopt_legacy_judgment_key": {
        // Mirrors `model_connections::adopt_legacy_judgment_key`: the key the
        // old setting kept moves under a window router variable of the row's.
        const row = this.modelConnections().connections.find((one) => one.id === args.id);
        if (!row || row.generation !== args.expectedGeneration) throw "settings_changed";
        const key = this.legacyKeys.get(args.id);
        if (!key) throw "no_key";
        const provider = this.zoSettings.providers.find((one) => (one.connection?.id ?? `fixture-${one.name}`) === args.id);
        provider.auth_env = `ZEROCODE_ROUTER_${provider.name.toUpperCase().replace(/[^A-Z0-9]+/g, "_")}_KEY`;
        this.keychain.set(`dev.zerocode.router.${provider.auth_env}`, key);
        this.legacyKeys.delete(args.id);
        this.zoListHeld = null;
        return this.modelConnections();
      }
      case "sync_model_connections": return clone(this.connectionReloadStatus ?? { queued: 1, restartRequired: 0, unknown: 0 });
      case "check_model_connection": {
        if (args.expectedProviderIdentity !== this.typesafeSettings().providerIdentity) throw "settings_changed";
        const checked = this.modelConnections().connections.find((row) => row.id === args.reference.id);
        // The door refuses a keyless request before anything is sent.
        const keyless = checked && checked.provider.requires_auth && !checked.keyAvailable ? { answered: false, failure: "no_key" } : null;
        const answer = await (this.connectionCheckWait ?? this.connectionCheck ?? keyless ?? { answered: true, failure: null });
        return { preflight: { ...answer, candidateIdentity: JSON.stringify(args.reference), activeProviderIdentity: args.expectedProviderIdentity, model: args.reference.model, elapsedMs: 1 }, legacyKeyPresent: false };
      }
      case "select_model_connection": {
        if (this.connectionApplyError) throw this.connectionApplyError;
        // Another window wrote first and this write did not land: the answer
        // still comes back, and only reading the settings again shows it.
        if (this.connectionApplyLost) return this.modelConnections();
        const smart = this.zoSettings.smart ??= {};
        const row = this.modelConnections().connections.find(row => row.id === args.reference.id);
        if (!row || row.generation !== args.reference.generation) throw "settings_changed";
        if (args.role === "judgment") {
          if (args.expectedProviderIdentity !== this.typesafeSettings().providerIdentity) throw "settings_changed";
          smart.previousJudgmentSelection ??= { judgmentProvider: smart.judgmentProvider ?? null };
          // `remember_formats`: the format of the connection judgment left and
          // of the one it is on now, kept by id (t-76211).
          const left = smart.judgmentConnection;
          smart.judgmentFormats = { ...(smart.judgmentFormats ?? {}), ...(left ? { [left.id]: left.protocol } : {}), [args.reference.id]: args.reference.protocol };
          smart.judgmentConnection = clone(args.reference);
          smart.judgmentProvider = { kind: "remote", protocol: args.reference.protocol, baseUrl: row.provider.base_url, model: args.reference.model, requiresAuth: row.provider.requires_auth };
          // Another window edits the connection's address right after this
          // write: the same id and model, the next generation.
          if (this.connectionApplyMoved) {
            const moved = this.zoSettings.providers.find((provider) => provider.connection?.id === args.reference.id);
            moved.base_url = "https://judge-moved.example.test/v1";
            moved.connection.generation += 1;
            smart.judgmentConnection = { ...smart.judgmentConnection, generation: moved.connection.generation };
            smart.judgmentProvider.baseUrl = moved.base_url;
          }
        } else {
          smart.previousCodingSelection ??= this.zoSettings.model ?? null;
          smart.codingConnection = clone(args.reference);
          this.zoSettings.model = `${row.provider.name}/${args.reference.model}`;
        }
        return this.modelConnections();
      }
      case "clear_model_connection": {
        const smart = this.zoSettings.smart ??= {};
        if (args.role === "judgment") {
          if (smart.previousJudgmentSelection?.judgmentProvider) smart.judgmentProvider = smart.previousJudgmentSelection.judgmentProvider;
          else delete smart.judgmentProvider;
          delete smart.judgmentConnection; delete smart.previousJudgmentSelection;
        } else {
          if (smart.previousCodingSelection) this.zoSettings.model = smart.previousCodingSelection;
          else delete this.zoSettings.model;
          delete smart.codingConnection; delete smart.previousCodingSelection;
        }
        return this.modelConnections();
      }
      case "test_router_connection": {
        /* `api_routers::test_router_endpoint` hands a refusal back as its
           own reading of the request (`ProbeFailure`, t-77116): a kind, the
           status, and a sentence — its own prefix, the status, and whatever
           the server put in the body, which is the shape the pane has to
           keep off its first line. */
        if (this.routerProbeRefusal) throw clone(this.routerProbeRefusal);
        if (this.routerProbeHold) await this.routerProbeHold;
        const answered = this.routerProbeAnswers?.[args.baseUrl];
        if (answered) return answered.map((id) => ({ id, context_length: null }));
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
          requires_auth: this.keychain.has(service) || !["localhost", "127.0.0.1", "[::1]"].includes(new URL(args.baseUrl).hostname),
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
        // Mirrors the command's `forget_zo_models`: the next read asks zo.
        this.zoListHeld = null;
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
        // Mirrors `remove_api_router`: a row named with the id and generation
        // the list showed goes only while it is still that row, half of that
        // pair is refused, and the key is found on the row itself and deleted
        // only when no other row still reads it.
        if ((args.sourceId == null) !== (args.expectedGeneration == null)) throw "settings_changed";
        if (args.sourceId != null) {
          const held = this.modelConnections().connections.find((row) => row.id === args.sourceId);
          if (!held || held.generation !== args.expectedGeneration || held.provider.name !== args.name) throw "settings_changed";
        }
        const own = this.zoSettings.providers.find((p) => p.name === args.name)?.auth_env;
        this.zoSettings.providers = this.zoSettings.providers.filter((p) => p.name !== args.name);
        this.zoListHeld = null;
        if (own?.startsWith("ZEROCODE_ROUTER_")
          && !this.zoSettings.providers.some((p) => p.auth_env === own)) {
          this.keychain.delete(`dev.zerocode.router.${own}`);
        }
        return clone(this.zoSettings.providers);
      }
      // Mirrors `typesafe_settings.rs`: the key is kept trimmed under the one
      // item every zo reads and never sent back; the switch is
      // `smart.jev.enabled`, written as the core's press writes it.
      case "typesafe_settings": {
        // A read the scene broke right after an apply (t-76598 r2 ①).
        if (this.settingsReadBroken) throw "status_unavailable";
        return clone(await (this.judgmentSettingsWait ?? this.typesafeSettings()));
      }
      case "managed_judgment_status": return clone(await (this.judgmentStatusWait ?? this.managedJudgmentSettings()));
      case "judgment_latest_request": {
        if (this.judgmentUsageError) throw this.judgmentUsageError;
        return clone(await (this.judgmentUsageWait ?? this.judgmentUsage ?? {
          providerIdentity: this.typesafeSettings().providerIdentity, observedAtMs: Date.now(), scope: "machine", coverage: "none",
          state: "unknown", reason: "unattributed", unattributedNewer: false, lastRequest: null,
        }));
      }
      case "managed_judgment_inspect_source": {
        if (this.managedSourceError) throw this.managedSourceError;
        const snapshot = this.managedJudgmentSettings();
        const model = {
          ...clone(snapshot.models[0]), id: "fixture-imported", label: "Imported Native Model", installed: false,
          local: { supported: true, reason: null }, runtime: { state: "missing", downloadBytes: 2048, reason: null },
        };
        this.managedSourceInspection = clone(this.managedSourceResult ?? {
          inspectionId: "fixture-inspection", instanceId: snapshot.instanceId, catalogFingerprint: snapshot.catalogFingerprint,
          model, local: model.local, existingServer: null, readyForInstall: true, reason: null,
          defaultDestinationDirectory: "/models/managed", allowsDestinationSelection: true,
          sourceKind: args.source.kind, downloadBytes: args.source.kind === "hugging_face" ? 1024 : 0,
          copyBytes: args.source.kind === "local_directory" ? 1024 : 0, runtimeDownloadBytes: 2048,
        });
        return clone(await (this.managedSourceWait ?? this.managedSourceInspection));
      }
      case "managed_judgment_install_source": {
        if (this.managedSourceInstallWait) return clone(await this.managedSourceInstallWait);
        const held = this.managedModelState;
        const inspection = this.managedSourceInspection;
        if (!inspection || args.inspectionId !== inspection.inspectionId || args.instanceId !== held.instanceId || args.expectedCatalogFingerprint !== held.catalogFingerprint) throw "source_changed";
        const model = clone(inspection.model);
        held.models.push(model);
        held.catalogFingerprint = `${held.catalogFingerprint}-source`;
        held.revision += 1;
        model.installed = !this.managedPendingKinds?.has("download");
        model.installedDirectory = args.destinationDirectory ? `${args.destinationDirectory}/owned-fixture-model` : "/models/managed/owned-fixture-model";
        return this.managedOperation("download", model.id);
      }
      case "managed_judgment_register_catalog": {
        const held = this.managedModelState;
        if (args.instanceId !== held.instanceId || args.expectedCatalogFingerprint !== held.catalogFingerprint) throw "catalog_changed";
        let catalog;
        try { catalog = JSON.parse(args.catalogJson); } catch { throw "manifest_invalid"; }
        if (catalog.version !== 1 || !Array.isArray(catalog.models)) throw "manifest_invalid";
        if (catalog.models.some((model) => held.models.some((current) => current.id === model.id))) throw "manifest_invalid";
        held.models.push(...catalog.models.map((model) => ({ ...clone(model), installed: false, origin: "registered" })));
        held.catalogFingerprint = `${held.catalogFingerprint}-added`;
        held.revision += 1;
        return this.managedJudgmentSettings();
      }
      case "managed_judgment_prepare_model_runtime": {
        const model = this.managedModelState?.models.find((entry) => entry.id === args.modelId && entry.revision === args.revision);
        if (!model?.runtime) throw "unknown_model";
        model.runtime.state = "ready";
        return this.managedOperation("runtime", model.id);
      }
      case "managed_judgment_prepare_runtime": {
        this.managedJudgmentSettings();
        this.managedModelState.runtime.state = "ready";
        return this.managedOperation("runtime");
      }
      case "managed_judgment_install": {
        this.managedJudgmentSettings();
        const model = this.managedModelState.models.find((row) => row.id === args.modelId && row.revision === args.revision);
        if (!model) throw "model_changed";
        if (!this.managedPendingKinds?.has("download")) model.installed = true;
        return this.managedOperation(args.sourceDirectory ? "import" : "download", model.id);
      }
      case "managed_judgment_cancel": {
        if (this.managedModelState?.operation?.id !== args.operationId) throw "operation_changed";
        this.managedModelState.operation.state = "cancelled";
        this.managedModelState.revision += 1;
        return this.managedJudgmentSettings();
      }
      case "managed_judgment_start": {
        this.managedJudgmentSettings();
        const model = this.managedModelState.models.find((row) => row.id === args.modelId && row.revision === args.revision);
        if (!model?.installed || (model.runtime ?? this.managedModelState.runtime).state !== "ready") throw "not_installed";
        this.managedModelState.server = {
          id: "fixture-server", state: "ready", modelId: model.id, checked: false, error: null,
          provider: { kind: "local", protocol: "system_one", baseUrl: "http://127.0.0.1:32101", model: model.id, requiresAuth: false },
        };
        return this.managedOperation("start", model.id);
      }
      case "managed_judgment_switch_model": {
        const held = this.managedModelState;
        if (this.managedSwitchOtherWindow) {
          this.zoSettings.smart = { ...(this.zoSettings.smart ?? {}), [JEV_MODEL_SETTING]: "jev-other-window" };
          delete this.zoSettings.smart.judgmentProvider;
        }
        if (args.expectedProviderIdentity !== this.typesafeSettings().providerIdentity) throw "settings_changed";
        if (args.instanceId !== held.instanceId || args.serverId !== held.server.id) throw "server_changed";
        const model = held.models.find((entry) => entry.id === args.modelId && entry.revision === args.revision);
        if (!model?.installed || (model.runtime ?? held.runtime).state !== "ready") throw "not_installed";
        const previousModelId = held.server.modelId;
        this.managedOperation("switch", model.id);
        if (this.managedSwitchFailure) {
          held.operation.state = "failed";
          held.operation.error = "native_protocol_failed";
          held.modelSwitch = { previousModelId, targetModelId: model.id, state: "restored", error: "native_protocol_failed", restoreError: null };
          return this.managedJudgmentSettings();
        }
        held.server = { id: `fixture-server-${model.id}`, state: "ready", modelId: model.id, checked: true, error: null,
          provider: { kind: "local", protocol: "system_one", baseUrl: "http://127.0.0.1:32102", model: model.id, requiresAuth: false } };
        this.zoSettings.smart = { ...(this.zoSettings.smart ?? {}), judgmentProvider: clone(held.server.provider) };
        held.configuredModelId = model.id;
        held.appliedModelId = model.id;
        held.modelSwitch = { previousModelId, targetModelId: model.id, state: "applied", error: null, restoreError: null };
        return this.managedJudgmentSettings();
      }
      case "managed_judgment_check": {
        if (this.managedModelState?.server.id !== args.serverId) throw "server_changed";
        this.managedModelState.server.checked = !this.managedCheckFailure;
        const snapshot = this.managedOperation("check", this.managedModelState.server.modelId);
        if (this.managedCheckFailure) {
          this.managedModelState.operation.state = "failed";
          this.managedModelState.operation.error = "invalid_response";
          return this.managedJudgmentSettings();
        }
        if (this.managedChangeOnCheck) this.zoSettings.smart = { ...(this.zoSettings.smart ?? {}), [JEV_MODEL_SETTING]: "jev-other-window" };
        return snapshot;
      }
      case "managed_judgment_apply": {
        if (args.expectedProviderIdentity !== this.typesafeSettings().providerIdentity) throw "settings_changed";
        const held = this.managedModelState;
        if (!held?.server.checked || held.server.id !== args.serverId || held.server.state !== "ready") throw "server_changed";
        this.zoSettings.smart = { ...(this.zoSettings.smart ?? {}), judgmentProvider: clone(held.server.provider) };
        held.configuredModelId = held.server.modelId;
        held.appliedModelId = held.server.modelId;
        held.revision += 1;
        const answer = this.managedJudgmentSettings();
        // What a scene does right after the write lands: the settings can no
        // longer be read, or another window names another provider.
        if (this.managedApplyThen === "unreadable") this.settingsReadBroken = true;
        if (this.managedApplyThen === "overwritten") this.zoSettings.smart = { ...this.zoSettings.smart, judgmentProvider: { kind: "jev", protocol: "system_one", model: "jev-other-window" } };
        // Or names the same model id on another local server, or asked as a
        // general model (t-76873 P2 ①).
        if (this.managedApplyThen === "anotherServer") this.zoSettings.smart = { ...this.zoSettings.smart, judgmentProvider: { ...clone(held.server.provider), baseUrl: "http://127.0.0.1:32107" } };
        if (this.managedApplyThen === "anotherProtocol") this.zoSettings.smart = { ...this.zoSettings.smart, judgmentProvider: { ...clone(held.server.provider), protocol: "openai_json" } };
        return answer;
      }
      case "managed_judgment_stop": {
        // A refusal a test puts in front of the next stop — the settings
        // moved first (t-59744 ⑥) — consumed once.
        if (this.managedStopRefusal) {
          const refusal = this.managedStopRefusal;
          delete this.managedStopRefusal;
          throw refusal;
        }
        if (this.managedModelState?.server.id !== args.serverId) throw "server_changed";
        this.managedModelState.server.state = "stopped";
        this.managedModelState.appliedModelId = null;
        this.managedModelState.revision += 1;
        return this.managedJudgmentSettings();
      }
      case "managed_judgment_remove": {
        const held = this.managedModelState;
        if (held.configuredModelId === args.modelId || (held.server.modelId === args.modelId && held.server.state !== "stopped")) throw "model_in_use";
        const model = held.models.find((row) => row.id === args.modelId && row.revision === args.revision);
        if (!model) throw "model_changed";
        model.installed = false;
        held.revision += 1;
        return this.managedJudgmentSettings();
      }
      case "save_typesafe_key": {
        const key = String(args.key ?? "").trim();
        if (!key) throw { kind: "failed", message: "the TypeSafe key is empty" };
        if (this.routerKeychainUnavailable) {
          throw { kind: "keychain-unavailable", message: "no keychain for keys" };
        }
        this.keychain.set(this.judgmentService(args.provider ?? this.typesafeSettings().provider), key);
        return this.typesafeSettings();
      }
      case "remove_typesafe_key":
        this.keychain.delete(this.judgmentService(this.typesafeSettings().provider));
        return this.typesafeSettings();
      case "set_jev_enabled": {
        // Mirrors `zerocode_core::jev::door::switch_on` / `switch_off`: on is
        // the switch, every folder and no seat's own word; off is the switch
        // and nothing else. A `jev` of another shape is refused.
        if (this.typesafeSetFailure) {
          throw { kind: "failed", message: "fixture refused the switch" };
        }
        const smart = { ...(this.zoSettings.smart ?? {}) };
        const jev = smart[JEV_SETTINGS_KEY] ?? {};
        if (jev === null || typeof jev !== "object" || Array.isArray(jev)) {
          throw { kind: "failed", message: "smart.jev is not an object" };
        }
        if (args.on) {
          smart[JEV_SETTINGS_KEY] = { ...jev, enabled: true, workspaces: [JEV_EVERY_WORKSPACE] };
          for (const seat of JEV_SEATS) delete smart[seat.setting];
        } else {
          smart[JEV_SETTINGS_KEY] = { ...jev, enabled: false };
        }
        this.zoSettings.smart = smart;
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
      case "set_summon_profile": {
        // Mirrors `typesafe_settings::set_summon_profile` over this harness's
        // lineup: a model it does not offer, or an effort the row's model does
        // not take, is refused by name and nothing is written; both fields
        // empty clear the cell, and the last cell cleared the table.
        const { agent, difficulty } = args;
        const model = args.model ?? null;
        const effort = args.effort ?? null;
        const picks = FIXTURE_LINEUP[agent] ?? [];
        const kept = this.zoSettings.smart?.summonProfiles?.[agent]?.[difficulty] ?? {};
        if (model && model !== kept.model && !picks.some((pick) => pick.id === model)) {
          throw { kind: "summon-row", message: `${model} is not in today's lineup`, refusal: { kind: "notListed", model } };
        }
        const rides = model ?? SUMMON_PROFILES[agent]?.[difficulty]?.model;
        const takes = picks.find((pick) => pick.id === rides)?.efforts;
        if (effort && takes && !takes.includes(effort) && !(effort === kept.effort && model === (kept.model ?? null))) {
          throw { kind: "summon-row", message: `${rides} takes ${takes.join(", ")}`, refusal: { kind: "effortOutside", effort, model: rides, takes } };
        }
        const table = clone(this.zoSettings.smart?.summonProfiles ?? {});
        const rows = table[agent] ?? {};
        if (model || effort) rows[difficulty] = { ...(model ? { model } : {}), ...(effort ? { effort } : {}) };
        else delete rows[difficulty];
        if (Object.keys(rows).length) table[agent] = rows;
        else delete table[agent];
        const smart = { ...(this.zoSettings.smart ?? {}) };
        if (Object.keys(table).length) smart.summonProfiles = table;
        else delete smart.summonProfiles;
        this.zoSettings.smart = smart;
        return this.typesafeSettings();
      }
      case "refresh_summon_lineup":
        return this.typesafeSettings();
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
      case "list_custom_decisions":
        this.customDecisions ??= { decisions: [], trash: [], folders: [], everywhere: false };
        return clone(this.customDecisions);
      case "check_custom_decision":
        return { refusals: [], version: JSON.stringify(args.draft), questions: { [args.draft.id ?? "draft"]: args.draft.question }, line: args.draft.name };
      case "save_custom_decision": {
        this.customDecisions ??= { decisions: [], trash: [], folders: [], everywhere: false };
        const store = this.customDecisions;
        const kept = store.decisions.find((one) => one.draft.id === args.draft.id);
        const version = JSON.stringify(args.draft);
        if (kept) {
          Object.assign(kept, { draft: clone(args.draft), version });
        } else {
          const recording = JEV_SEATS.find((seat) => seat.setting === "customDecisions").modes.split(" ")[1];
          const id = `cd-${String(store.decisions.length + store.trash.length + 1).padStart(8, "0")}`;
          store.decisions.push({ mode: recording, version, createdAt: "", updatedAt: "", draft: { ...clone(args.draft), id } });
        }
        return clone(store);
      }
      case "set_custom_decision_mode": {
        const store = this.customDecisions;
        store.decisions.find((one) => one.draft.id === args.id).mode = args.mode;
        return clone(store);
      }
      case "delete_custom_decision": {
        const store = this.customDecisions;
        const at = store.decisions.findIndex((one) => one.draft.id === args.id);
        const [gone] = store.decisions.splice(at, 1);
        store.trash.push({ id: args.id, name: gone.draft.name, deletedAt: "", kept: gone });
        return clone(store);
      }
      case "restore_custom_decision": {
        const store = this.customDecisions;
        const trashed = store.trash.find((one) => one.id === args.id);
        store.trash = store.trash.filter((one) => one !== trashed);
        store.decisions.push(trashed.kept);
        return clone(store);
      }
      case "set_judgment_provider": {
        if (args.expectedProviderIdentity && args.expectedProviderIdentity !== this.typesafeSettings().providerIdentity) throw "settings_changed";
        if (this.typesafeSetFailure) throw { kind: "failed", message: "fixture refused the model" };
        const provider = clone(args.provider);
        if (!provider.model?.trim() || !["system_one", "openai_json"].includes(provider.protocol)) {
          throw { kind: "failed", message: "invalid decision provider" };
        }
        const endpoint = new URL(provider.baseUrl);
        if (provider.kind === "local" && !["localhost", "127.0.0.1", "[::1]"].includes(endpoint.hostname)) {
          throw { kind: "failed", message: "provider_not_loopback" };
        }
        if (provider.kind === "remote" && endpoint.protocol !== "https:") {
          throw { kind: "failed", message: "provider_https_required" };
        }
        const smart = { ...(this.zoSettings.smart ?? {}) };
        if (provider.kind === "jev") {
          const pin = jevModelPin(provider.model);
          if (!pin) throw { kind: "failed", message: "not a model" };
          if (pin === JEV_MODEL_ALIAS) delete smart[JEV_MODEL_SETTING];
          else smart[JEV_MODEL_SETTING] = pin;
        }
        if (args.key?.trim()) {
          if (!provider.requiresAuth) throw { kind: "failed", message: "provider_auth_not_required" };
          this.keychain.set(this.judgmentService(provider), args.key.trim());
        }
        smart.judgmentProvider = provider;
        this.zoSettings.smart = smart;
        return this.typesafeSettings();
      }
      case "judgment_models": {
        if (this.judgmentModelsError) throw this.judgmentModelsError;
        return clone(await (this.judgmentModelsWait ?? this.judgmentModels ?? [{ id: "fixture-decision-a" }, { id: "fixture-decision-b" }]));
      }
      case "preflight_judgment_provider": {
        if (this.judgmentPreflightError) throw this.judgmentPreflightError;
        if (args.expectedProviderIdentity !== this.typesafeSettings().providerIdentity) throw "settings_changed";
        if (args.provider.kind === "jev" && !jevModelPin(args.provider.model)) throw "provider_model";
        const check = await (this.judgmentPreflightWait ?? this.judgmentPreflight ?? {
          candidateIdentity: `fixture-provider:${JSON.stringify(args.provider)}`,
          activeProviderIdentity: args.expectedProviderIdentity,
          answered: true, model: args.provider.model, failure: null, elapsedMs: 1,
        });
        if (args.expectedProviderIdentity !== this.typesafeSettings().providerIdentity) throw "settings_changed";
        return clone(check);
      }
      case "check_typesafe_key": {
        if (args.providerIdentity && args.providerIdentity !== this.typesafeSettings().providerIdentity) {
          throw "settings_changed";
        }
        return clone(await (this.typesafeCheckWait ?? this.typesafeCheck));
      }
      // Mirrors `type_value_keys.rs`: a name no walk reads is refused before
      // the key is looked at, an empty key is refused, a machine with no
      // keychain refuses by kind, and the key is kept trimmed in the item the
      // walk's writer reads and never sent back.
      case "type_value_keys": return this.typeValueKeys();
      case "save_type_value_key": {
        const slot = this.typeValueSlot(args.credentialKey);
        const key = String(args.key ?? "").trim();
        if (!key) throw { kind: "failed", message: "the key is empty" };
        if (this.routerKeychainUnavailable) {
          throw { kind: "keychain-unavailable", message: "no keychain for keys" };
        }
        this.keychain.set(slot.service, key);
        return this.typeValueKeys();
      }
      case "remove_type_value_key":
        this.keychain.delete(this.typeValueSlot(args.credentialKey).service);
        return this.typeValueKeys();
      case "jev_summary":
        if (this.jevSummary === null) throw new Error("unknown argument 'jev'");
        return clone(this.jevSummary);
      // The boot timeline's marks (t-20078, boot_timeline.rs): the window
      // writes the phase down and answers nothing.
      case "boot_phase": return null;
      // The computer-use band asks what is shared at boot (t-73906). Nothing
      // is shared in these scenes, so the band stays hidden.
      case "computer_share_state": return { target: null, label: null, takenBack: false };
      // The plain-text secrets line asks once the settings are drawn
      // (t-76698). These scenes' zo writes no secret as plain text, so the
      // line stays hidden; the line's own suite is settings-secrets.mjs.
      case "settings_secret_literals": return null;
      default:
        this.unknown.push({ window_id: windowId, command, args: clone(args) });
        throw new Error(`unknown command: ${command}`);
    }
  }

  computerUseCapabilities() {
    const platform = this.computerUsePlatform ?? "darwin";
    return {
      platform,
      provider: "zerocode-computer-use-macos",
      protocolVersion: 1,
      liveReflex: {
        supported: platform === "darwin" && this.liveReflexHelperReads,
        enabled: this.settings.computer_live_reflex === true,
      },
      check: clone(this.liveReflexCheck),
    };
  }

  typeValueSlot(name) {
    const slot = typeValueSlots().find((one) => one.name === name);
    if (!slot) throw { kind: "failed", message: "not a key a walk reads" };
    return slot;
  }

  typeValueKeys() {
    return {
      road: this.settings.computer_generator_road,
      logins: GENERATOR_LOGINS.map(({ road, row }) => {
        const found = TYPE_VALUE_TABLE.rows.find((one) => one.id === row);
        return {
          road,
          model: found.model,
          installed: !(this.generatorMissingClis ?? []).includes(road),
          account: this.generatorAccounts?.[road] ?? null,
        };
      }),
      last: this.generatorLast ?? null,
      keysKeptHere: !this.routerKeychainUnavailable,
      keys: typeValueSlots().map((slot) => ({
        credentialKey: slot.name,
        rows: slot.rows.map((row) => ({ id: row.id, model: row.model })),
        chosen: slot.chosen,
        keySaved: !this.routerKeychainUnavailable && Boolean(this.keychain.get(slot.service)?.trim()),
      })),
    };
  }

  judgmentService(provider) {
    return provider.kind === "jev" ? TYPESAFE_SERVICE : `fixture-decision-key:${provider.protocol}:${provider.baseUrl}`;
  }

  managedJudgmentSettings() {
    this.managedModelState ??= {
      instanceId: "fixture-instance", revision: 1, catalogVersion: 1, catalogFingerprint: "fixture-catalog",
      models: [{ id: "fixture-decision", label: "Fixture Decision", engine: "fixture", protocol: "system_one", revision: "fixture-revision", license: "Apache-2.0", source: "https://models.example.com/decision", downloadBytes: 1024, installed: false, compatibility: "supported", compatibilityReason: null, quality: "experimental" }],
      runtime: { state: "missing", pythonRequirement: "fixture", downloadBytes: 2048, reason: null },
      operation: null,
      server: { id: null, state: "stopped", modelId: null, provider: null, checked: false, error: null },
      providerIdentity: this.typesafeSettings().providerIdentity, configuredModelId: null, appliedModelId: null,
    };
    if (this.managedModelState.providerIdentity !== this.typesafeSettings().providerIdentity) {
      this.managedModelState.providerIdentity = this.typesafeSettings().providerIdentity;
      this.managedModelState.revision += 1;
    }
    return clone(this.managedModelState);
  }

  managedOperation(kind, modelId = null) {
    const held = this.managedModelState;
    held.revision += 1;
    held.operation = { id: `operation-${held.revision}`, kind, modelId, state: this.managedPendingKinds?.has(kind) ? "running" : "done", receivedBytes: 0, totalBytes: 1024, error: null };
    return this.managedJudgmentSettings();
  }

  async emitManaged(snapshot) {
    for (const [, page] of this.pages) {
      if (page.isClosed() || page.url() === "about:blank") continue;
      await page.evaluate((payload) => window.__SETTINGS_TEST_EMIT__("managed-judgment:changed", payload), snapshot);
    }
  }

  modelConnections() {
    const smart = this.zoSettings.smart ?? {};
    const coding = smart.codingConnection ?? null, judgment = smart.judgmentConnection ?? null;
    return clone({
      connections: this.zoSettings.providers.map(provider => {
        const id = provider.connection?.id ?? `fixture-${provider.name}`;
        return { id, generation: provider.connection?.generation ?? 0, provider,
          auth: provider.connection?.auth ?? { header: "Authorization", scheme: "Bearer" },
          keyAvailable: !provider.requires_auth || this.keychain.has(`dev.zerocode.router.${provider.auth_env}`),
          legacyKey: provider.requires_auth && !this.keychain.has(`dev.zerocode.router.${provider.auth_env}`) && this.legacyKeys.has(id),
          uses: [["coding",coding],["judgment",judgment]].filter(([, ref]) => ref?.id === id).map(([role]) => role),
          judgmentFormat: smart.judgmentFormats?.[id] ?? (judgment?.id === id ? judgment.protocol : null),
        };
      }), coding, judgment, codingModel: this.zoSettings.model ?? null,
      activeProviderIdentity: this.typesafeSettings().providerIdentity, legacyKeyPresent: false,
    });
  }

  /* `model_connection_checks::read` against the scene's clock. */
  connectionMemoryRead() {
    if (!this.connectionMemory) throw new Error("model_connection_checks");
    return clone({ nowMs: this.connectionMemory.nowMs, everyMs: 6 * 60 * 60 * 1000, connections: this.connectionMemory.connections, applied: this.connectionMemory.applied ?? {} });
  }

  /* `remember_check` / `remember_judgment`: state words only, a record of
     another generation starts over, a press forgets the models' judgment
     checks, and a connection no longer saved loses its record. */
  connectionMemoryKeep(answer, kind) {
    if (!this.connectionMemory) throw new Error("model_connection_checks");
    // A record the window refuses to write (`model_connection_checks_full`).
    if (this.connectionMemoryRefusal) throw new Error(this.connectionMemoryRefusal);
    const allowed = kind === "check" ? ["id", "generation", "state", "count", "forgetJudgments"] : ["id", "generation", "model", "protocol", "state"];
    const words = ["ok", "usable", "nokey", "unreadable", "badkey", "down", "schema", "failed"];
    if (Object.keys(answer).some((key) => !allowed.includes(key)) || !words.includes(answer.state)) throw new Error("model_connection_check");
    const memory = this.connectionMemory;
    const now = memory.nowMs;
    let record = memory.connections[answer.id];
    if (!record || record.generation !== answer.generation) record = memory.connections[answer.id] = { generation: answer.generation };
    if (kind === "check") {
      record.state = answer.state;
      if (answer.state === "ok" && answer.count != null) record.count = answer.count; else delete record.count;
      record.checkedMs = now;
      if (answer.forgetJudgments) delete record.judgments;
    } else {
      record.judgments = (record.judgments ?? []).filter((kept) => !(kept.model === answer.model && kept.protocol === answer.protocol));
      record.judgments.push({ model: answer.model, protocol: answer.protocol, state: answer.state, checkedMs: now });
    }
    const live = new Set(this.zoSettings.providers.map((provider) => provider.connection?.id ?? `fixture-${provider.name}`));
    for (const id of Object.keys(memory.connections)) if (!live.has(id)) delete memory.connections[id];
    return this.connectionMemoryRead();
  }

  /* The list as `slash_catalog::zo_models_catalog` hands it out: with
     `zoListCaching` on, the read held until it is forgotten, the way the
     window's cache holds it; off, every read sees the settings at once. */
  zoListRead() {
    if (!this.zoListCaching) return this.zoModelList();
    this.zoListHeld ??= this.zoModelList();
    return clone(this.zoListHeld);
  }

  /* `zo models --json` as the window reads it (`zo_model_list`): zo spells
     every router model `<name>/<model>`, the settings' routers first. */
  zoModelList() {
    return clone({
      models: this.zoList.models,
      reports: this.zoList.reports,
      aliases: this.zoList.aliases,
      // Where a zo pane reads its Claude login (`slash_catalog::zo_model_list`).
      ...(this.zoList.paneCredentialStore ? { paneCredentialStore: this.zoList.paneCredentialStore } : {}),
      customProviders: [
        ...this.zoSettings.providers.map((provider) => ({
          name: provider.name,
          models: (provider.models ?? []).map((model) => `${provider.name}/${typeof model === "string" ? model : model.id}`),
          usable: true,
          credentialEnvVars: provider.auth_env ? [provider.auth_env] : [],
        })),
        ...this.zoEnvRouters,
      ],
    });
  }

  typesafeSettings() {
    const smart = this.zoSettings.smart ?? {};
    const jev = smart[JEV_SETTINGS_KEY];
    const door = jev !== null && typeof jev === "object" && !Array.isArray(jev) ? jev : null;
    /* `door::switched_on`: the switch written, and true. */
    const switchedOn = door?.enabled === true;
    /* Every seat reads its own row's words and anything else is off; a seat
       split off another reads the word written for that other while it has
       none of its own (`JevUse::word_in`); a seat nobody wrote a word for
       stands at its recommendation while the switch is on and is off
       otherwise: `JevUse::mode_in`. */
    const wordOf = (seat) => smart[seat.setting] !== undefined || seat.follows === undefined
      ? smart[seat.setting]
      : smart[seat.follows];
    const mode = (seat, given) => {
      const offered = seat.modes.split(" ");
      if (given === undefined) return switchedOn ? seat.recommended : offered[0];
      const word = String(given ?? "").trim().toLowerCase();
      return offered.includes(word) ? word : offered[0];
    };
    const switches = JEV_SEATS.map((seat) => ({
      id: seat.id,
      setting: seat.setting,
      mode: mode(seat, wordOf(seat)),
      modes: clone(jevSeatModes(seat)),
      written: wordOf(seat) !== undefined,
    }));
    /* `JevSwitch::of`: the door lets a request through (unset is on, anything
       but `true` is off), some seat asks, and some folder is consented. */
    const enabled = jev === undefined || (door !== null && (door.enabled === undefined || door.enabled === true));
    const roots = Array.isArray(door?.workspaces)
      ? door.workspaces.filter((root) => typeof root === "string" && root.trim())
      : [];
    const everywhere = roots.includes(JEV_EVERY_WORKSPACE);
    const folders = roots.filter((root) => root !== JEV_EVERY_WORKSPACE).length;
    const asks = switches.some((row) => row.modes.find((choice) => choice.mode === row.mode)?.asks);
    const pin = jevModelPin(smart[JEV_MODEL_SETTING]);
    const jevProvider = { kind: "jev", protocol: "system_one", baseUrl: "https://api.typesafe.ai", model: pin ?? JEV_MODEL_ALIAS, requiresAuth: true };
    const provider = smart.judgmentProvider && smart.judgmentProvider.kind !== "jev"
      ? clone(smart.judgmentProvider) : jevProvider;
    return {
      keysKeptHere: !this.routerKeychainUnavailable,
      keySaved: provider.requiresAuth && Boolean(this.keychain.get(this.judgmentService(provider))?.trim()),
      provider,
      providerIdentity: `fixture-provider:${JSON.stringify(provider)}`,
      providerConnection: smart.judgmentConnection ?? null,
      // `TypeSafeSettings::provider_connection_name` (t-76598): the bound
      // connection's own name.
      providerConnectionName: smart.judgmentConnection
        ? this.zoSettings.providers.find((row) => (row.connection?.id ?? `fixture-${row.name}`) === smart.judgmentConnection.id)?.name ?? null
        : null,
      jevProvider,
      jev: { on: enabled && asks && (everywhere || folders > 0), everywhere, folders },
      // The rows a person wrote, and what each difficulty runs (t-14437),
      // as `summon_difficulty::lineup::rows` answers them (t-74181): each
      // written field, the model the shipped table's where none is written,
      // and no effort where none is written — the agent's CLI runs its own
      // default, so the command line names the model alone.
      summonProfiles: clone(smart.summonProfiles ?? {}),
      summonRows: Object.fromEntries(Object.entries(FIXTURE_SHIPPED).map(([agent, levels]) => [
        agent,
        Object.entries(levels).map(([difficulty, auto]) => {
          const written = smart.summonProfiles?.[agent]?.[difficulty] ?? {};
          const model = written.model ?? auto.model;
          const effort = written.effort ?? null;
          const effortWords = !effort ? [] : agent === "codex"
            ? ["-c", `model_reasoning_effort=${effort}`]
            : ["--effort", effort];
          return {
            difficulty, model, effort, from: written.model ? "person" : "table", candidates: [],
            effortRule: effort ? "person" : "cli-default",
            autoModel: auto.model, autoEffort: null,
            writtenModel: written.model ?? null, writtenEffort: written.effort ?? null,
            standing: { kind: FIXTURE_LINEUP[agent]?.some((pick) => pick.id === model) ? "listed" : "missing" },
            launch: ["--model", model, ...effortWords],
          };
        }),
      ])),
      summonLineup: clone(FIXTURE_LINEUP),
      summonAgents: {
        codex: { installed: true, auth: "authorized", evidence: null, anyProvider: false },
        claude: { installed: true, auth: "unauthorized", evidence: "no identity file", anyProvider: false },
        zo: { installed: true, auth: "unknown", evidence: null, anyProvider: true },
      },
      summonSources: {
        readAtMs: 1,
        reports: [
          { provider: "openai", source: "chatgpt-backend", ok: true, count: 4, age: "6m old", skipped: false, failure: null },
          { provider: "xai", source: "xai-api", ok: false, count: 0, age: null, skipped: true, failure: null },
        ],
        connections: [
          { name: "fixture-router", usable: true, models: ["fixture-router/model-x"] },
          { name: "fixture-keyless", usable: false, models: ["fixture-keyless/model-y"] },
        ],
      },
      model: {
        setting: JEV_MODEL_SETTING,
        model: pin ?? JEV_MODEL_ALIAS,
        pinned: pin !== null,
        alias: JEV_MODEL_ALIAS,
      },
      switches,
      classifier: {
        setting: CLASSIFIER_SETTING,
        mode: classifierMode(this.zoSettings.smart?.[CLASSIFIER_SETTING]).mode,
        probes: classifierMode(this.zoSettings.smart?.[CLASSIFIER_SETTING]).probes,
        reaches: classifierMode(this.zoSettings.smart?.[CLASSIFIER_SETTING]).reaches,
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

const addReportedPlatform = (page, platform) => page.addInitScript(reportedPlatformScript, platform);

const pageA = await context.newPage();
const pageB = await context.newPage();
// `ZO_TEST_PLATFORM` names another platform: both windows report it before shell.js reads it (test-platform.mjs).
if (PLATFORM_OVERRIDE) {
  await addReportedPlatform(pageA, PLATFORM_OVERRIDE);
  await addReportedPlatform(pageB, PLATFORM_OVERRIDE);
}
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
  if (process.env.SETTINGS_FILTER && !new RegExp(process.env.SETTINGS_FILTER).test(name)) return;
  if (process.env.SETTINGS_TRACE) process.stderr.write(`settings START ${name}\n`);
  try {
    const detail = await run();
    pass(name, detail ?? "");
  } catch (error) {
    fail(name, error?.stack ?? error);
  } finally {
    if (process.env.SETTINGS_TRACE) {
      const result = results.at(-1);
      process.stderr.write(`settings END ${result.pass ? "PASS" : "FAIL"} ${name}\n`);
      if (!result.pass) process.stderr.write(`${result.detail}\n`);
    }
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

/* The model pane's 고급 fold holds the old judgment editor, the model
   library and the diagnosis line (t-75762 v2): opened with it. */
async function openJudgmentSettings(page) {
  const pane = await page.locator("#typesafe-card").evaluate((card) => card.closest(".settings-pane").dataset.pane);
  await openSettings(page, pane);
  for (const fold of ["#model-advanced", "#judgment-connection-settings"]) {
    const advanced = page.locator(fold);
    if (await advanced.count()) await advanced.evaluate((element) => { element.open = true; });
  }
}

async function withJudgmentStatus(run) {
  const previous = backend.zoSettings.smart === undefined ? undefined : clone(backend.zoSettings.smart);
  const previousManaged = backend.managedModelState === undefined ? undefined : clone(backend.managedModelState);
  delete backend.managedModelState;
  const provider = { kind: "local", protocol: "system_one", baseUrl: "http://127.0.0.1:32104", model: "fixture-decision", requiresAuth: false };
  backend.zoSettings.smart = { judgmentProvider: provider, [JEV_SETTINGS_KEY]: { enabled: false, workspaces: [JEV_EVERY_WORKSPACE] } };
  backend.managedJudgmentSettings();
  Object.assign(backend.managedModelState, {
    configuredModelId: provider.model, appliedModelId: provider.model,
    runtime: { state: "ready", downloadBytes: 0, reason: null },
    server: { id: "fixture-active-server", state: "ready", modelId: provider.model, provider, checked: true, error: null },
  });
  backend.managedModelState.models[0].installed = true;
  const page = await context.newPage();
  await installTauri(page, "judgment-status");
  attachFaultRecorder(page, "judgment-status");
  try {
    await bootPage(page, "judgment-status");
    await openSettings(page, "judgment-models");
    // The model library and the diagnosis line wait in the 고급 fold (t-75762 v2).
    await page.locator("#model-advanced").evaluate((fold) => { fold.open = true; });
    await page.waitForFunction(() => managedJudgmentController.snapshot?.server.id === "fixture-active-server");
    return await run(page, provider);
  } finally {
    delete backend.judgmentSettingsWait;
    delete backend.judgmentStatusWait;
    delete backend.judgmentUsageWait;
    delete backend.judgmentUsage;
    delete backend.judgmentUsageError;
    delete backend.managedSourceWait;
    delete backend.managedSourceInstallWait;
    delete backend.managedSourceResult;
    delete backend.managedSourceError;
    delete backend.managedSourceInspection;
    await page.close();
    backend.zoSettings.smart = previous;
    backend.managedModelState = previousManaged;
  }
}

function nativeSourceInspection(snapshot, { inspectionId = "fixture-native-inspection", label = "Native Decision Lite" } = {}) {
  const local = { supported: false, reason: "engine_unverified", downloadOnly: true, inputs: ["text", "json"], unsupportedInputs: ["images", "videos"] };
  const model = {
    ...clone(snapshot.models[0]), id: "fixture-native", label, engine: "transformers_system_one", nativeFormat: "d3-code-readout-v1",
    revision: "native-revision", source: "https://huggingface.co/acme/native-decision/tree/native-revision", installed: false,
    local, runtime: { state: "unverified", downloadBytes: 4096, reason: "engine_unverified" }, compatibility: "unsupported", compatibilityReason: "engine_unverified",
  };
  return {
    inspectionId, instanceId: snapshot.instanceId, catalogFingerprint: snapshot.catalogFingerprint, model, local, existingServer: null,
    readyForInstall: false, readyForDownload: true, reason: "engine_unverified", defaultDestinationDirectory: "/models/managed",
    allowsDestinationSelection: true, sourceKind: "hugging_face", downloadBytes: 1024, copyBytes: 0, runtimeDownloadBytes: 4096,
  };
}

async function openSettings(page, pane = null) {
  await page.evaluate((wanted) => {
    setSettingsOpen(true);
    if (wanted) showSettingsPane(wanted);
  }, pane);
  await renderSettled(page);
}

async function connectionScenePictures(page, name) {
  const folder = process.env.CONNECTION_SCENE_SHOTS;
  if (!folder) return;
  await mkdir(folder, { recursive: true });
  for (const [theme, width] of [["dark", 320], ["dark", 768], ["dark", 1440], ["light", 768]]) {
    await setQualityTheme(page, theme);
    await page.setViewportSize({ width, height: 1069 });
    await page.evaluate(() => { const pane = document.querySelector('.settings-pane:not([hidden])'); pane?.parentElement.scrollTo({ top: 0, behavior: "instant" }); });
    await settlePaint(page);
    const geometry = await page.locator('.settings-pane:not([hidden])').evaluate(pane => ({ width: pane.clientWidth, scroll: pane.scrollWidth }));
    assert(geometry.scroll <= geometry.width + 1, `${name}/${theme}/${width} overflows`, geometry);
    await page.screenshot({ path: resolve(folder, `${name}-${theme}-${width}.png`), animations: "disabled" });
  }
  await page.setViewportSize({ width: 1280, height: 900 });
  await setQualityTheme(page, "dark");
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

// A no-IPC assertion belongs to its gesture, not to the window's own clocks:
// the repository's ambient stamp beat, and the one update knock a window sends
// a minute after its boot (`armUpdateBootCheck`) — the suite reaches the
// user-agent gesture 60–70 s in, so that knock landed inside it on a fast run,
// on main too (t-55370, reports/t-55370/settings-race). Park both and restore
// each document even when the assertion fails; a boot knock still owed is sent
// once on the way out. Settings broadcasts, reads and writes keep their real
// stateful backend throughout.
async function withoutAmbientReads(pages, run) {
  const setRead = (page, value) => page.evaluate((next) => {
    const previous = projectsRead;
    projectsRead = next;
    worktreePoll.sync();
    return previous;
  }, value);
  const parkBootKnock = (page) => page.evaluate(() => {
    clearTimeout(updateBootTimer);
    return window.__SETTINGS_TEST_WINDOW__;
  });
  const bootKnocked = (id) => backend.calls.some((call) =>
    call.window_id === id && call.command === "update_check" && call.args.knock === "clock");
  const held = [];
  try {
    for (const page of pages) {
      const previous = await setRead(page, false);
      const id = await parkBootKnock(page);
      held.push({ page, previous, owed: !bootKnocked(id) });
    }
    return await run();
  } finally {
    for (const { page, previous, owed } of held) {
      await setRead(page, previous);
      if (owed) await page.evaluate(() => void askUpdateCheck(UPDATE_KNOCK.clock));
    }
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
    claude_autoswitch: () => document.getElementById("account-autoswitch")?.value,
    harness_mode: () => document.getElementById("harness-gate-mode")?.value,
    setup_script_launch_mode: () => document.querySelector(
      "[data-setup-launch-mode][aria-pressed='true']",
    )?.dataset.setupLaunchMode,
    terminal_shortcut_policy: () => document.getElementById("terminal-shortcut-policy")?.value,
    agent_pane_first_view: () => document.querySelector("#agent-pane-first-view input:checked")?.value,
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
    case "claude_autoswitch": return page.selectOption("#account-autoswitch", value);
    case "harness_mode": return page.selectOption("#harness-gate-mode", value);
    case "setup_script_launch_mode": return page.click(
      `[data-setup-launch-mode="${value}"]`,
    );
    case "terminal_shortcut_policy": return page.selectOption(
      "#terminal-shortcut-policy",
      value,
    );
    case "agent_pane_first_view": return page.check(`#agent-pane-first-view input[value="${value}"]`);
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

// t-6058: each TCC row under a Computer Use permission — both bundles' rows,
// the judged one first — says one of four grants in the catalog's words for
// every language, and only a stale row offers buttons: its reset (which the
// row then reads back as denied) and its System Settings pane.
await test("Computer Use's TCC rows say one of four grants in five languages and only a stale row offers a reset", async () => {
  const helperRow = {
    bundle_id: "dev.zerocode.app.computer-use",
    name: "ZeroCode Computer Use",
    path: "/Applications/ZeroCode.app/Contents/Resources/ZeroCode Computer Use.app",
  };
  const appRow = { bundle_id: "dev.zerocode.app", name: "ZeroCode", path: "/Applications/ZeroCode.app" };
  backend.computerUseTccRows = [
    { id: "accessibility", ...helperRow, judged: true, grant: "granted", pin: "signature", actions: [] },
    { id: "accessibility", ...appRow, judged: false, grant: "stale", pin: "cdhash", actions: ["reset", "open-settings"] },
    { id: "screenshots", ...appRow, judged: true, grant: "unreadable", unreadable: "no-full-disk-access", actions: [] },
    { id: "screenshots", ...helperRow, judged: false, grant: "denied", actions: [] },
  ];
  const previousLocale = backend.settings.locale;
  try {
    const from = backend.calls.length;
    await openSettings(pageA, "computer-use");
    await pageA.click("#computer-use-refresh");
    await backend.waitForCall("A", "computer_use_permission_status", from);
    await pageA.waitForFunction(
      () => document.querySelectorAll(".computer-use-tcc-row").length === 4,
      undefined,
      { timeout: UI_TIMEOUT },
    );
    const painted = () => pageA.evaluate(() => [...document.querySelectorAll(".computer-use-tcc-row")].map((row) => ({
      permission: row.closest("[data-permission]")?.dataset.permission,
      bundle: row.dataset.bundle,
      grant: row.dataset.grant,
      judged: row.dataset.judged,
      marked: row.querySelector(".computer-use-tcc-judged")?.textContent ?? null,
      words: row.querySelector(".computer-use-tcc-grant")?.textContent,
      buttons: [...row.querySelectorAll("[data-tcc-action]")].map((button) => [button.dataset.tccAction, button.textContent]),
    })));
    // The sentence each row should say in the locale in force, read straight
    // from the catalogs (Korean is the call site's own words).
    const expected = (code) => pageA.evaluate((code) => {
      const say = (entry, vars) => {
        const text = CATALOG[code]?.[entry.key] || entry.word;
        return text.replace(/\{\{(\w+)\}\}/g, (whole, name) => (name in (vars ?? {}) ? vars[name] : whole));
      };
      const why = say(COMPUTER_TCC_WORDS.unreadable["no-full-disk-access"]);
      return {
        granted: say(COMPUTER_TCC_WORDS.grant.granted),
        stale: say(COMPUTER_TCC_WORDS.grant.stale),
        unreadable: say(COMPUTER_TCC_WORDS.grant.unreadable, { why }),
        denied: say(COMPUTER_TCC_WORDS.grant.denied),
        reset: say(COMPUTER_TCC_WORDS.action.reset),
        open: say(COMPUTER_TCC_WORDS.action["open-settings"]),
        judged: say({ key: "computerUse.tccJudged", word: "판정 행" }),
      };
    }, code);
    const korean = await expected("ko");
    try {
      for (const code of ["ko", "en", "ja", "zh", "es"]) {
        await backend.externalPatch({ locale: code }, ["locale"]);
        await renderSettled(pageA);
        const words = await expected(code);
        if (code !== "ko") {
          for (const [word, text] of Object.entries(words)) {
            assert(text !== korean[word], `${code} has no translation for the ${word} words`, text);
          }
        }
        assertEqual(await painted(), [
          { permission: "accessibility", bundle: "dev.zerocode.app.computer-use", grant: "granted", judged: "true", marked: words.judged, words: words.granted, buttons: [] },
          { permission: "accessibility", bundle: "dev.zerocode.app", grant: "stale", judged: "false", marked: null, words: words.stale, buttons: [["reset", words.reset], ["open-settings", words.open]] },
          { permission: "screenshots", bundle: "dev.zerocode.app", grant: "unreadable", judged: "true", marked: words.judged, words: words.unreadable, buttons: [] },
          { permission: "screenshots", bundle: "dev.zerocode.app.computer-use", grant: "denied", judged: "false", marked: null, words: words.denied, buttons: [] },
        ], `the TCC rows did not say the table's words in ${code}`);
      }
    } finally {
      await backend.externalPatch({ locale: previousLocale }, ["locale"]);
      await renderSettled(pageA);
    }
    // t-9719: the judged mark draws its line and corners from the palette's
    // rule and pill — the same pixels the two literals drew, so it looks the same.
    const judgedDress = await pageA.evaluate(() => {
      const mark = getComputedStyle(document.querySelector(".computer-use-tcc-judged"));
      const root = getComputedStyle(document.documentElement);
      return {
        line: [mark.borderTopWidth, root.getPropertyValue("--rule-width").trim()],
        corner: [mark.borderTopLeftRadius, root.getPropertyValue("--radius-pill").trim()],
      };
    });
    assertEqual(judgedDress, { line: ["1px", "1px"], corner: ["999px", "999px"] },
      "the judged mark's line or corners moved");

    // The stale row's pane, then its reset: each asks the backend for that row
    // alone, and the reset's answer is painted back — the row now denied.
    let askedAt = backend.calls.length;
    await pageA.click('.computer-use-tcc-row[data-grant="stale"] [data-tcc-action="open-settings"]');
    const opened = await backend.waitForCall("A", "computer_use_tcc_row_action", askedAt);
    assertEqual(opened.args, { id: "accessibility", bundleId: "dev.zerocode.app", action: "open-settings" }, "the pane button named another row");
    await pageA.waitForFunction(
      () => document.querySelector('.computer-use-tcc-row[data-grant="stale"] [data-tcc-action="reset"]')?.disabled === false,
      undefined,
      { timeout: UI_TIMEOUT },
    );
    askedAt = backend.calls.length;
    await pageA.click('.computer-use-tcc-row[data-grant="stale"] [data-tcc-action="reset"]');
    const reset = await backend.waitForCall("A", "computer_use_tcc_row_action", askedAt);
    assertEqual(reset.args, { id: "accessibility", bundleId: "dev.zerocode.app", action: "reset" }, "the reset named another row");
    await pageA.waitForFunction(
      () => document.querySelector('#computer-use-accessibility-tcc [data-bundle="dev.zerocode.app"]')?.dataset.grant === "denied",
      undefined,
      { timeout: UI_TIMEOUT },
    );
    assertEqual(await pageA.locator("[data-tcc-action]").count(), 0, "a row that is not stale offered a button");
  } finally {
    // Pass or fail, the next Computer Use tests open the pane fresh and
    // expect it to read — a failure here must not become theirs.
    backend.computerUseTccRows = undefined;
    await pageA.evaluate(() => setSettingsOpen(false));
  }
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

// The live reflex switch (t-10221): the card says three things — whether
// this platform and helper read the reflex kernel, whether both permissions
// are granted (the permission report's own judgment), and whether a check
// made with this helper passed — and the switch turns on only when all three
// stand. Otherwise it stays off and unpressable, saying why; once on it is
// never turned off behind the person's back (a revoked permission says "on,
// cannot run now"), and turning it off always works.
await test("Computer Use는 기본 배경 입력을 유지하고 공유 입력은 사람의 설정으로만 허용한다", async () => {
  const original = backend.settings.computer_input_policy;
  const peer = await context.newPage();
  const peerId = "computer-policy-peer";
  try {
    if (PLATFORM_OVERRIDE) await addReportedPlatform(peer, PLATFORM_OVERRIDE);
    await installTauri(peer, peerId);
    attachFaultRecorder(peer, peerId);
    await bootPage(peer, peerId);
    backend.settings.computer_input_policy = "background";
    await backend.broadcast(null, ["computer_input_policy"]);
    await openSettings(pageA, "computer-use");
    await openSettings(peer, "computer-use");
    const first = pageA.locator("#computer-shared-desktop");
    const second = peer.locator("#computer-shared-desktop");
    assert(await pageA.locator("#computer-input-policy-card").isVisible());
    assert(!(await first.isChecked()), "shared desktop input is on by default");
    assert(!(await second.isChecked()), "the other window starts with shared input");
    assertEqual(await first.getAttribute("aria-describedby"), "computer-input-policy-about");
    const before = backend.calls.length;
    await first.check();
    const enabled = await backend.waitForCall("A", "set_computer_input_policy", before);
    assertEqual(enabled.args, { policy: "shared-desktop" });
    await peer.waitForFunction(() => document.getElementById("computer-shared-desktop").checked, undefined, { timeout: UI_TIMEOUT });
    const restore = backend.calls.length;
    await second.uncheck();
    const disabled = await backend.waitForCall(peerId, "set_computer_input_policy", restore);
    assertEqual(disabled.args, { policy: "background" });
    await pageA.waitForFunction(() => !document.getElementById("computer-shared-desktop").checked, undefined, { timeout: UI_TIMEOUT });
    assertEqual(backend.settings.computer_input_policy, "background");
  } finally {
    await peer.close();
    backend.settings.computer_input_policy = original;
    await backend.broadcast(null, ["computer_input_policy"]);
    await pageA.evaluate(() => setSettingsOpen(false));
  }
});

await test("Computer Use 공유 의무는 기본 꺼짐이며 저장과 다른 창의 변경을 따른다", async () => {
  assertEqual(backend.settings.computer_share_required, false);
  const peer = await context.newPage();
  const peerId = "share-required-peer";
  try {
    if (PLATFORM_OVERRIDE) await addReportedPlatform(peer, PLATFORM_OVERRIDE);
    await installTauri(peer, peerId);
    attachFaultRecorder(peer, peerId);
    await bootPage(peer, peerId);
    await openSettings(pageA, "computer-use");
    await openSettings(peer, "computer-use");
    const first = pageA.locator("#computer-share-required");
    const second = peer.locator("#computer-share-required");
    assertEqual(await first.count(), 1, "share requirement setting is missing");
    assert(!(await first.isChecked()));
    assert(!(await second.isChecked()));
    assertEqual(await first.getAttribute("aria-describedby"), "computer-share-required-about");
    const before = backend.calls.length;
    await first.check();
    assertEqual((await backend.waitForCall("A", "set_computer_share_required", before)).args, { on: true });
    await peer.waitForFunction(() => document.getElementById("computer-share-required").checked);
    const restore = backend.calls.length;
    await second.uncheck();
    assertEqual((await backend.waitForCall(peerId, "set_computer_share_required", restore)).args, { on: false });
    await pageA.waitForFunction(() => !document.getElementById("computer-share-required").checked);
  } finally {
    await peer.close();
    backend.settings.computer_share_required = false;
    await backend.broadcast(null, ["computer_share_required"]);
    await pageA.evaluate(() => setSettingsOpen(false));
  }
});

const LIVE_REFLEX_WORDS = Object.freeze({
  supported: { key: "computerUse.liveReflexSupported", word: "이 Mac과 헬퍼가 반사 커널을 읽습니다" },
  helperOld: { key: "computerUse.liveReflexHelperOld", word: "이 헬퍼는 반사 커널을 읽지 않습니다 — Computer Use를 다시 시작하세요" },
  platform: { key: "computerUse.unsupported", word: "지원되지 않는 플랫폼" },
  granted: { key: "computerUse.granted", word: "허용됨" },
  notGranted: { key: "computerUse.notEnabled", word: "허용 안 됨" },
  checkNone: { key: "computerUse.liveReflexCheckNone", word: "이 헬퍼로 한 점검이 없습니다" },
  checkPassed: { key: "computerUse.liveReflexCheckPassed", word: "통과 · {{time}}" },
  checkFailed: { key: "computerUse.liveReflexCheckFailed", word: "통과 못 함 · {{time}} — {{reason}}" },
  needs: { key: "computerUse.liveReflexNeeds", word: "아직 켤 수 없습니다 — {{why}}" },
  onBlocked: { key: "computerUse.liveReflexOnBlocked", word: "켜짐 · 지금은 못 돎 — {{why}}" },
  on: { key: "computerUse.liveReflexOn", word: "켜짐 — 에이전트가 반사 계획을 시작할 때만 헬퍼가 화면을 보고 누릅니다." },
  whySupport: { key: "computerUse.liveReflexWhySupport", word: "이 플랫폼이나 헬퍼가 반사 층을 지원하지 않습니다" },
  whyPermissions: { key: "computerUse.liveReflexWhyPermissions", word: "접근성과 스크린샷 권한이 모두 허용되어야 합니다" },
  whyCheck: { key: "computerUse.liveReflexWhyCheck", word: "이 헬퍼로 한 점검이 통과해야 합니다" },
});

await test("Computer Use의 실시간 반사 층 스위치는 지원·권한·점검이 모두 설 때만 켜지고, 끄기는 늘 된다", async () => {
  const words = (one, vars = {}) => pageA.evaluate(
    ([key, word, values]) => t(key, word, values),
    [LIVE_REFLEX_WORDS[one].key, LIVE_REFLEX_WORDS[one].word, vars],
  );
  const clock = (ms) => pageA.evaluate((at) => knowledgeClock(at, Date.now()), ms);
  const grant = (status) => {
    backend.computerUsePermissions = backend.computerUsePermissions.map((permission) => ({ ...permission, status }));
  };
  const card = pageA.locator("#computer-live-reflex-card");
  const toggle = pageA.locator("#computer-live-reflex");
  const line = (name) => pageA.locator(`#computer-live-reflex-${name}`);
  const refresh = async () => {
    const at = backend.calls.length;
    await pageA.click("#computer-use-refresh");
    await backend.waitForCall("A", "computer_use_capabilities", at);
    await renderSettled(pageA);
  };
  const says = async (name, expected) => {
    await pageA.waitForFunction(
      ([id, text]) => document.getElementById(id)?.textContent === text,
      [`computer-live-reflex-${name}`, expected],
      { timeout: UI_TIMEOUT },
    ).catch(async () => {
      throw new Error(`#computer-live-reflex-${name} said ${JSON.stringify(await line(name).textContent())}, not ${JSON.stringify(expected)}`);
    });
  };
  const permissionsSaid = async (one) => `${await pageA.evaluate(() => t("computerUse.accessibility", "접근성"))} ${await words(one)} · ${await pageA.evaluate(() => t("computerUse.screenshots", "스크린샷"))} ${await words(one)}`;
  const pressDoesNothing = async () => {
    const writes = backend.count("A", "set_computer_live_reflex");
    // Playwright will not press an `aria-disabled` control on its own; a
    // person can, so the press is forced and must change nothing.
    await toggle.click({ force: true });
    await renderSettled(pageA);
    assert(!(await toggle.isChecked()), "an unpressable switch turned on");
    assertEqual(backend.count("A", "set_computer_live_reflex"), writes, "an unpressable switch wrote the setting");
  };
  const shots = process.env.LIVE_REFLEX_SHOTS;
  const shoot = (name) => shots ? card.screenshot({ path: resolve(shots, `live-reflex-${name}.png`), animations: "disabled" }) : null;

  try {
    // Nothing granted, nothing checked: off, unpressable, and saying why —
    // the tip is what the switch is described by.
    await openSettings(pageA, "computer-use");
    await refresh();
    assert(await card.isVisible(), "the Computer Use pane has no live reflex card");
    assert(!(await toggle.isChecked()), "live reflex is on by default");
    assertEqual(await toggle.getAttribute("role"), "switch");
    assertEqual(await toggle.getAttribute("aria-describedby"), "computer-live-reflex-why");
    await says("support", await words("supported"));
    await says("permissions", await permissionsSaid("notGranted"));
    await says("check-line", await words("checkNone"));
    await says("why", await words("needs", { why: await words("whyPermissions") }));
    assertEqual(await toggle.getAttribute("aria-disabled"), "true", "an ungranted Mac offered the switch");
    await pressDoesNothing();
    await shoot("1-needs-permissions");

    // Granted, no check: the next thing it needs is the check.
    grant("granted");
    await refresh();
    await says("permissions", await permissionsSaid("granted"));
    await says("why", await words("needs", { why: await words("whyCheck") }));
    assertEqual(await toggle.getAttribute("aria-disabled"), "true", "an unchecked helper offered the switch");

    // A check that fails says why, with its time, and the switch stays shut.
    backend.liveReflexRefusal = "the operator is stopped (hotkey)";
    let checkedAt = backend.calls.length;
    await pageA.click("#computer-live-reflex-check");
    await backend.waitForCall("A", "computer_live_reflex_check", checkedAt);
    await says("check-line", await words("checkFailed", { time: await clock(backend.liveReflexCheck.at_ms), reason: backend.liveReflexRefusal }));
    assertEqual(await toggle.getAttribute("aria-disabled"), "true", "a failed check offered the switch");

    // A check that passes opens it; pressing it writes the setting once.
    backend.liveReflexRefusal = null;
    checkedAt = backend.calls.length;
    await pageA.click("#computer-live-reflex-check");
    await backend.waitForCall("A", "computer_live_reflex_check", checkedAt);
    await says("check-line", await words("checkPassed", { time: await clock(backend.liveReflexCheck.at_ms) }));
    await pageA.waitForFunction(() => document.getElementById("computer-live-reflex")?.getAttribute("aria-disabled") === "false", null, { timeout: UI_TIMEOUT });
    assertEqual(await line("why").textContent(), "", "a switch that may turn on still said why not");
    await shoot("2-ready");
    const onAt = backend.calls.length;
    await toggle.click();
    const on = await backend.waitForCall("A", "set_computer_live_reflex", onAt);
    assertEqual(on.args, { on: true }, "the switch did not write the setting on");
    assertEqual(backend.settings.computer_live_reflex, true);
    await says("why", await words("on"));
    assert(await toggle.isChecked(), "the switch fell back after it was written");

    // Permissions taken away: still on — never turned off behind the
    // person's back — saying it cannot run now, and still pressable to off.
    grant("not-granted");
    const quietAt = backend.calls.length;
    await refresh();
    await says("why", await words("onBlocked", { why: await words("whyPermissions") }));
    assert(await toggle.isChecked(), "a revoked permission turned the switch off");
    assertEqual(await toggle.getAttribute("aria-disabled"), "false", "a switch that is on could not be turned off");
    assertEqual(backend.calls.slice(quietAt).filter((call) => call.command === "set_computer_live_reflex").length, 0, "the page wrote the setting on its own");
    await shoot("3-on-cannot-run");
    const offAt = backend.calls.length;
    await toggle.click();
    assertEqual((await backend.waitForCall("A", "set_computer_live_reflex", offAt)).args, { on: false });
    assertEqual(backend.settings.computer_live_reflex, false);
    assertEqual(await toggle.getAttribute("aria-disabled"), "true", "an off switch with a revoked permission was offered");

    // A helper that does not read the kernel, then a platform that runs no
    // reflex: the support line says which, and the switch stays shut.
    grant("granted");
    backend.liveReflexHelperReads = false;
    await refresh();
    await says("support", await words("helperOld"));
    await says("why", await words("needs", { why: await words("whySupport") }));
    assertEqual(await toggle.getAttribute("aria-disabled"), "true");
    backend.computerUsePlatform = "windows";
    backend.liveReflexHelperReads = true;
    await refresh();
    await says("support", await words("platform"));
    await says("why", await words("needs", { why: await words("whySupport") }));
    assertEqual(await toggle.getAttribute("aria-disabled"), "true", "Windows offered live reflex");
    await pressDoesNothing();

    // Settings lands the switch in the pane it lives on.
    const landing = await pageA.evaluate(() => ({
      pane: PANE_OF["computer-live-reflex"] ?? null,
      strays: Object.entries(PANE_OF).flatMap(([id, pane]) => {
        const lives = document.getElementById(id)?.closest(".settings-pane")?.dataset.pane;
        return lives === undefined || lives === pane ? [] : [{ id, pane, lives }];
      }),
    }));
    assertEqual(landing, { pane: "computer-use", strays: [] }, "settings does not land on the live reflex switch");
  } finally {
    backend.computerUsePlatform = "darwin";
    backend.liveReflexHelperReads = true;
    backend.liveReflexCheck = null;
    backend.liveReflexRefusal = null;
    backend.settings.computer_live_reflex = false;
    grant("not-granted");
    await pageA.evaluate(() => setSettingsOpen(false));
  }
  return "switch shut ×4 · check fail/pass · on · on-blocked · off";
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

// What an agent's pane opens on (t-82038, 「나는 터미널이 편한데」): two
// radios in the terminal pane, the conversation for a person who never chose,
// the core's two words on the wire, the value on screen before the answer and
// the canonical document after a reopen; a refused write is put back by the
// refetch the shared writer makes.
await test("Agent Pane First View offers 대화 and 터미널 and survives a canonical reopen", async () => {
  await openSettings(pageA, "terminal");
  const offered = await pageA.locator("#agent-pane-first-view input[type=radio]").evaluateAll((radios) =>
    radios.map((radio) => ({ value: radio.value, checked: radio.checked })));
  assertEqual(
    offered,
    [{ value: "conversation", checked: true }, { value: "terminal", checked: false }],
    "Agent Pane First View did not open on the conversation",
  );
  const call = await gestureAndWait(
    pageA,
    "A",
    "set_agent_pane_first_view",
    () => pageA.check('#agent-pane-first-view input[value="terminal"]'),
  );
  assertEqual(call.args, { view: "terminal" }, "the terminal choice sent a different wire value");
  await reopenSettings(pageA, "A", "terminal");
  assertEqual(
    await controlValue(pageA, "agent_pane_first_view"),
    "terminal",
    "Agent Pane First View did not survive a canonical refetch",
  );
  assertEqual(await pageA.evaluate(() => agentPaneFirstView), "terminal", "the next pane would not open on its terminal");
  // A refused write never stands: the radio and the next pane's side go back
  // to what the document says, through the refetch the shared writer makes.
  backend.rejectOnce("set_agent_pane_first_view");
  const refusedAt = backend.calls.length;
  await pageA.check('#agent-pane-first-view input[value="conversation"]');
  await backend.waitForCall("A", "set_agent_pane_first_view", refusedAt);
  await backend.waitForCall("A", "settings_snapshot", refusedAt);
  await pageA.waitForFunction(() =>
    document.querySelector("#agent-pane-first-view input:checked")?.value === "terminal", null, { timeout: UI_TIMEOUT });
  await waitForSettingsIdle(pageA);
  assertEqual(backend.settings.agent_pane_first_view, "terminal", "a refused write changed the setting");
  assertEqual(await pageA.evaluate(() => agentPaneFirstView), "terminal", "a refused write decides the next pane");
  // Back to everyone's default, so the persistence matrix makes its own write.
  await gestureAndWait(
    pageA,
    "A",
    "set_agent_pane_first_view",
    () => pageA.check('#agent-pane-first-view input[value="conversation"]'),
  );
  assertEqual(backend.settings.agent_pane_first_view, "conversation", "the conversation choice was not stored");
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
  // t-59962: the card says whose login this is — the one zo and the window
  // share — and that a terminal's agy may be signed in as someone else.
  const loginWords = await pageA.evaluate(() => ({
    about: document.querySelector('[data-i18n="settings.googleAccount.about"]')?.textContent ?? "",
    note: document.getElementById("google-account-note")?.textContent ?? "",
    differs: t("settings.googleAccount.agyDiffers", "터미널의 agy는 자기 로그인을 따로 가지므로 다른 계정일 수 있습니다."),
    shared: t("settings.googleAccount.about", "zo와 창이 함께 쓰는 Google 로그인(credentials.json)입니다."),
    gauge: t("usage.antigravityTip", "zo와 창이 함께 쓰는 Google 로그인(credentials.json)으로 읽습니다 — 터미널의 agy는 다른 계정일 수 있습니다."),
  }));
  assertEqual(loginWords.about.trim(), loginWords.shared, "the Google card does not say whose login it shows");
  assert(loginWords.note.includes(loginWords.differs), "the Google card does not say agy may use another login");
  assert(loginWords.gauge.includes("credentials.json"), "the Antigravity gauge tip does not name its login");
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
  // under a fresh login saying 「Google 로그인 필요」.
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

await test("만료된 Grok 로그인 행은 「한 번 실행」 단추를 세우고, 누르면 CLI를 판에 띄워 갱신을 맡긴다", async () => {
  // 게이지가 「로그인 만료 — CLI가 다음 실행에서 갱신한다」고 말할 때(Grok의
  // `delegated-refresh-required`, usage_grok.rs), 계정 화면의 그 행은 로그인을
  // 다시 시키지 않고 그 CLI를 한 번 띄운다 — 창은 로그인도 갱신도 스스로 하지
  // 않는다(설계 그대로). 여기 행은 셸로 여는 모양이라 판의 길이 픽스처의 문
  // 둘(`open_term_tab`·`term_text`)로 보인다; 에이전트 판으로 여는 같은 손은
  // 아래 세 시험이 본다(t-7170 R1/R2).
  const row = {
    agent: "grok",
    name: "Grok",
    program: "grok",
    homepage_url: "https://example.com",
    installed: true,
    signed_in: true,
    account: "person@example.com",
    known: true,
    proof: "file",
    opens: "shell",
    home: "~/.grok",
    road: "verb",
    run_command: "GROK_HOME=/Users/dev/.grok grok",
    logout_road: "verb",
  };
  backend.cliLogins = { rows: [row] };
  await pageA.evaluate(() => {
    usageProvider("grok").write({
      provider: "grok",
      session: null,
      weekly: null,
      fable_weekly: null,
      updated_at: Date.now(),
      error: "Grok 로그인이 만료되었습니다 — 이 컴퓨터에서 grok을 한 번 실행하세요",
      status: "error",
      failure_kind: "delegated-refresh-required",
      account: null,
    }, false);
    setSettingsOpen(false);
  });
  const openedAt = backend.calls.length;
  await openSettings(pageA, "provider-accounts");
  await backend.waitForCall("A", "cli_login_list", openedAt);
  await renderSettled(pageA);
  const rows = pageA.locator("#cli-login-list .account-row");
  assertEqual(await rows.count(), 1, "the card did not paint the one row");
  const words = await pageA.evaluate(() => ({
    runOnce: t("usage.runOnce", "{{program}} 한 번 실행", { program: "grok" }),
    expired: t(
      "settings.cliLogins.expired",
      "로그인 만료 — {{program}}을(를) 한 번 실행하면 CLI가 세션을 갱신합니다",
      { program: "grok" },
    ),
    relogin: t("settings.accounts.relogin", "다시 로그인"),
    logout: t("settings.accounts.logout", "로그아웃"),
  }));
  assertEqual(
    await rows.nth(0).locator(".agent-row-cmd").textContent(),
    words.expired,
    "the expired row did not say what to run",
  );
  assertEqual(
    await rows.nth(0).locator("button").allTextContents(),
    [words.runOnce, words.relogin, words.logout],
    "the expired row's verbs",
  );

  // The press: the bare program in a shell of this window, the backend
  // watching the credential file, and the gauge re-read past its floor once
  // the CLI has renewed — never the login verb.
  const pressing = backend.calls.length;
  await rows.nth(0).locator("button").first().click();
  await backend.waitForCall("A", "open_term_tab", pressing);
  const typed = await backend.waitForCall("A", "term_text", pressing);
  assertEqual(
    typed.args.text,
    "GROK_HOME=/Users/dev/.grok grok\r",
    "the window typed something other than the bare program",
  );
  assertEqual(
    typed.args.term,
    backend.nextTerm,
    "the line went to a shell the run did not just open",
  );
  const waited = await backend.waitForCall("A", "cli_login_wait", pressing);
  assertEqual(waited.args.signedIn, true, "the watch was not for the renewed login");
  // The re-read is the FORCED ask: an unforced one may ride the same window
  // (the bar's own asking), and would be the previous reading held.
  let reread = await backend.waitForCall("A", "grok_usage", pressing);
  while (reread.args?.force !== true) {
    reread = await backend.waitForCall("A", "grok_usage", reread.at + 1);
  }
  await renderSettled(pageA);
  assertEqual(
    backend.calls.slice(pressing).filter((call) => call.command === "cli_login_start").length,
    0,
    "the window ran the login verb instead of the program",
  );
  backend.cliLogins = { rows: [] };
  await pageA.evaluate(() => {
    usageProvider("grok").write(null, false);
    setSettingsOpen(false);
  });
  return "expired → run once → bare program typed → watched → re-read";
});

/* ---- t-7170 R1/R2: the run-once road on the table's own shape ------------
 *
 * The test above opens the CLI through a plain shell (opens: "shell"); the
 * real Grok row opens it through the launch door (opens: "agent"), and that
 * road REUSED a pane the agent already held — a press that focused a running
 * program and waited for a renewal nobody triggered (astra m-7239 R1) — and
 * then typed the bare line at a pane whose program had LEFT, on the pane
 * table's `idle`, a word about the past that proves nothing about the line
 * or the foreground at the instant of the write (R1b). And the witness was
 * taken when the wait began, after the run: a CLI that renewed on start,
 * faster than the window's second call, left a watch waiting for a change
 * that had already happened (R2). */
const expiredGrokRow = () => ({
  agent: "grok",
  name: "Grok",
  program: "grok",
  homepage_url: "https://example.com",
  installed: true,
  signed_in: true,
  account: "person@example.com",
  known: true,
  proof: "file",
  opens: "agent",
  home: "~/.grok",
  road: "verb",
  run_command: "GROK_HOME=/Users/dev/.grok grok",
  logout_road: "verb",
});

const expiredGrokGauge = () => ({
  provider: "grok",
  session: null,
  weekly: null,
  fable_weekly: null,
  updated_at: Date.now(),
  error: "Grok 로그인이 만료되었습니다 — 이 컴퓨터에서 grok을 한 번 실행하세요",
  status: "error",
  failure_kind: "delegated-refresh-required",
  account: null,
});

/* The gauge says expired again — every walk ends in a forced re-read, and
 * this fixture's answer to that read (no figure) takes the run button with
 * it; a second press needs the gauge standing where the first found it. */
async function standExpiredGrokGauge() {
  await pageA.evaluate((expired) => {
    usageProvider("grok").write(expired, false);
    paintCliLogins();
  }, expiredGrokGauge());
  await renderSettled(pageA);
}

/* The accounts pane standing on one expired Grok row, its 「한 번 실행」 first. */
async function openExpiredGrokCard(row) {
  backend.cliLogins = { rows: [row] };
  await pageA.evaluate((expired) => {
    usageProvider("grok").write(expired, false);
    setSettingsOpen(false);
  }, expiredGrokGauge());
  const openedAt = backend.calls.length;
  await openSettings(pageA, "provider-accounts");
  await backend.waitForCall("A", "cli_login_list", openedAt);
  await renderSettled(pageA);
  const rows = pageA.locator("#cli-login-list .account-row");
  assertEqual(await rows.count(), 1, "the card did not paint the one row");
  const runOnce = await pageA.evaluate(
    (program) => t("usage.runOnce", "{{program}} 한 번 실행", { program }),
    row.program,
  );
  assertEqual(
    await rows.nth(0).locator("button").first().textContent(),
    runOnce,
    "the expired row's first verb is not the run",
  );
  return rows;
}

/* Leave the card and the panes behind: the next test starts with no Grok pane. */
async function leaveExpiredGrokCard() {
  backend.cliLogins = { rows: [] };
  await pageA.evaluate((agent) => {
    for (const [term, held] of [...paneAgents]) {
      if (held === agent) {
        paneAgents.delete(term);
        hookStates.delete(term);
      }
    }
    usageProvider("grok").write(null, false);
    setSettingsOpen(false);
  }, "grok");
}

/* One press of the run, with the gauge saying expired as the press finds it. */
async function pressRunOnce(rows) {
  await standExpiredGrokGauge();
  const pressing = backend.calls.length;
  await rows.nth(0).locator("button").first().click();
  return pressing;
}

/* The forced re-read a finished walk asks for — the one with `force`, past
 * the bar's own unforced asking that may ride the same window. */
async function forcedGrokReread(from) {
  let reread = await backend.waitForCall("A", "grok_usage", from);
  while (reread.args?.force !== true) {
    reread = await backend.waitForCall("A", "grok_usage", reread.at + 1);
  }
  return reread;
}

await test("a_run_once_never_types_at_a_pane_it_did_not_open: 「한 번 실행」은 언제나 그 실행을 위해 창이 여는 새 판에서 — 프로그램이 남은 판에도, 떠난 판(셸 프롬프트)에도 아무것도 치지 않는다", async () => {
  const row = expiredGrokRow();
  // A pane the window holds for this agent: the person's.
  const held = await pageA.evaluate(async (agent) => {
    const term = await openTermTab({ placement: "tab" });
    paneAgents.set(term, agent);
    return term;
  }, row.agent);
  const rows = await openExpiredGrokCard(row);
  // Every door that puts bytes into a pane this window already holds.
  const intoAPane = (from) => backend.calls
    .slice(from)
    .filter((call) => ["term_text", "term_key", "term_paste", "send_prompt"].includes(call.command))
    .map((call) => `${call.command}@${call.args.term}`);

  // 1. The program is still in the pane — no word from it yet, a turn under
  //    way, a turn done at its composer: the run is a NEW pane through the
  //    launch door, bare, and nothing is typed anywhere.
  for (const word of [null, "working", "done"]) {
    await pageA.evaluate(([term, said]) => {
      if (said === null) hookStates.delete(term);
      else hookStates.set(term, said);
    }, [held, word]);
    await openSettings(pageA, "provider-accounts");
    const pressing = await pressRunOnce(rows);
    await backend.waitForCall("A", "cli_login_wait", pressing);
    await forcedGrokReread(pressing);
    await renderSettled(pageA);
    assertEqual(intoAPane(pressing), [], `the run typed at a pane (${word ?? "no word yet"})`);
    const launched = backend.calls
      .slice(pressing)
      .find((call) => call.command === "launch_agent_tab");
    assert(launched, `no new pane opened for the run (${word ?? "no word yet"})`);
    assertEqual(launched.args.agent, row.agent, "the launch door was asked for another agent");
    assertEqual(launched.args.prompt, "", "a run is bare — the door was handed words");
  }

  // 2. The program has LEFT the pane: `idle`, a shell at its prompt as the
  //    pane table last saw it. That is a word about the past (astra R1b) — a
  //    person may have typed half a command there since, or started another
  //    program in front of it — so the run is still a NEW pane, and the left
  //    pane is typed at by nobody.
  await pageA.evaluate((term) => hookStates.set(term, "idle"), held);
  await openSettings(pageA, "provider-accounts");
  const leaving = await pressRunOnce(rows);
  await backend.waitForCall("A", "cli_login_wait", leaving);
  await forcedGrokReread(leaving);
  await renderSettled(pageA);
  assertEqual(intoAPane(leaving), [], "the run typed at a pane the program had left");
  const fresh = backend.calls
    .slice(leaving)
    .find((call) => call.command === "launch_agent_tab");
  assert(fresh, "no new pane opened for the run beside a pane the program had left");
  assertEqual(fresh.args.prompt, "", "a run is bare — the door was handed words");

  // 3. A second press while the first walk is still out runs nothing: the
  //    row is busy, and two runs racing for one credential file is the
  //    mistake the guard exists to prevent.
  await openSettings(pageA, "provider-accounts");
  const reached = backend.holdNext("cli_login_wait");
  const holding = await pressRunOnce(rows);
  await reached;
  await renderSettled(pageA);
  assert(
    await rows.nth(0).locator("button").first().isDisabled(),
    "the row's run was not disabled while its walk was out",
  );
  await rows.nth(0).locator("button").first().click({ force: true });
  await renderSettled(pageA);
  const runsAfter = () => backend.calls
    .slice(holding)
    .filter((call) => ["term_text", "launch_agent_tab", "send_prompt"].includes(call.command))
    .length;
  assertEqual(runsAfter(), 1, "a second press ran the program again");
  // Closing the panel and coming back finds the same walk out: the row is
  // still busy, and nothing runs twice for the re-entry either.
  await pageA.evaluate(() => setSettingsOpen(false));
  await openSettings(pageA, "provider-accounts");
  await renderSettled(pageA);
  assert(
    await rows.nth(0).locator("button").first().isDisabled(),
    "re-entering the pane forgot the walk that is still out",
  );
  await rows.nth(0).locator("button").first().click({ force: true });
  await renderSettled(pageA);
  assertEqual(runsAfter(), 1, "a press after re-entry ran the program again");
  backend.release("cli_login_wait");
  await backend.waitForCall("A", "cli_login_list", holding);
  await forcedGrokReread(holding);
  await renderSettled(pageA);
  await leaveExpiredGrokCard();
  return "program in the pane (3 words) → new pane, nothing typed; program left → new pane, nothing typed; busy → one run; re-entry → still one";
});

await test("a_finished_wait_paints_no_row_of_its_own: 기다림의 답은 행을 쓰지 않는다 — 걷기가 끝나면 표를 다시 읽고, 늦게 온 옛 답이 새 행을 덮지 않는다", async () => {
  const row = expiredGrokRow();
  const rows = await openExpiredGrokCard(row);
  // The wait is held; while it is out, the table moves on (a home switched,
  // a row renamed — whatever the backend re-reads next), and the wait then
  // answers with the table as it stood when the wait BEGAN.
  const reached = backend.holdNext("cli_login_wait");
  backend.staleWaitAnswer = { rows: [{ ...row, name: "Grok (stale)", signed_in: true }] };
  const pressing = await pressRunOnce(rows);
  await reached;
  backend.cliLogins = { rows: [{ ...row, name: "Grok (fresh)", signed_in: true }] };
  backend.release("cli_login_wait");
  const waited = await backend.waitForCall("A", "cli_login_wait", pressing);
  await forcedGrokReread(pressing);
  await renderSettled(pageA);
  assertEqual(
    await rows.nth(0).locator(".agent-row-name").textContent(),
    "Grok (fresh)",
    "the row painted the wait's own answer instead of the table re-read after it",
  );
  const listed = await backend.waitForCall("A", "cli_login_list", waited.at + 1);
  assert(listed.at > waited.at, "the table was not re-read after the wait came back");
  await leaveExpiredGrokCard();
  return "wait answers stale → table re-read → row paints the fresh table";
});

await test("a_finished_logout_paints_no_row_of_its_own: 로그아웃의 답도 행을 쓰지 않는다 — 길이 끝나면 표를 다시 읽는다", async () => {
  const row = expiredGrokRow();
  const rows = await openExpiredGrokCard(row);
  // The verb signs the row out; its answer is the table as an earlier read
  // saw it, and the table the backend holds now says signed out.
  backend.staleWaitAnswer = { rows: [{ ...row, name: "Grok (stale)", signed_in: true }] };
  const logout = await pageA.evaluate(() => t("settings.accounts.logout", "로그아웃"));
  const pressing = backend.calls.length;
  await rows.nth(0).locator("button", { hasText: logout }).click();
  await pageA.locator("#ask-yes").click();
  await backend.waitForCall("A", "cli_login_logout", pressing);
  await forcedGrokReread(pressing);
  await renderSettled(pageA);
  assertEqual(
    await rows.nth(0).locator(".agent-row-name").textContent(),
    "Grok",
    "the row painted the logout's own answer instead of the table re-read after it",
  );
  assertEqual(
    await rows.nth(0).locator("button", { hasText: logout }).count(),
    0,
    "a signed-out row still offers a logout",
  );
  await leaveExpiredGrokCard();
  return "logout answers stale → table re-read → row paints signed out";
});

await test("a_late_read_never_paints_over_a_newer_one: 표 읽기는 가장 나중에 부른 것만 그린다 — 먼저 나가 늦게 온 옛 표가 뒤에 읽은 새 표를 덮지 않는다", async () => {
  const row = expiredGrokRow();
  const rows = await openExpiredGrokCard(row);
  const shown = () => rows.nth(0).locator(".agent-row-name").textContent();
  // A read of the table goes out — the read the pane's arrival makes — and
  // is held in flight, answering the table as it stood when it began.
  backend.cliLogins = { rows: [{ ...row, name: "Grok (stale)" }] };
  const reached = backend.holdNext("cli_login_list");
  await pageA.evaluate(() => {
    window.__cliLoginFirstRead = refreshCliLogins();
  });
  await reached;
  // The table moves on, and a walk's end reads it again; that read answers
  // first.
  backend.cliLogins = { rows: [{ ...row, name: "Grok (fresh)" }] };
  const pressing = await pressRunOnce(rows);
  const waited = await backend.waitForCall("A", "cli_login_wait", pressing);
  await backend.waitForCall("A", "cli_login_list", waited.at + 1);
  await forcedGrokReread(pressing);
  await renderSettled(pageA);
  assertEqual(await shown(), "Grok (fresh)", "the walk's own re-read did not paint");
  // And now the read asked earlier lands.
  backend.release("cli_login_list");
  await pageA.evaluate(() => window.__cliLoginFirstRead);
  await renderSettled(pageA);
  assertEqual(
    await shown(),
    "Grok (fresh)",
    "a read asked earlier and answered later painted over the newer table",
  );
  await pageA.evaluate(() => {
    delete window.__cliLoginFirstRead;
  });
  await leaveExpiredGrokCard();
  return "held read (stale) → walk's re-read (fresh) paints → stale lands → nothing painted";
});

await test("a_refresh_that_lands_before_the_watch_is_still_seen: 자격 증명 파일의 baseline은 실행 전에 찍고, 기다림이 그 표를 든다", async () => {
  const rows = await openExpiredGrokCard(expiredGrokRow());
  const ticket = backend.nextWitness + 1;
  const pressing = await pressRunOnce(rows);
  // The order IS the fix: the witness before the run, the wait after it.
  const witnessed = await backend.waitForCall("A", "cli_login_witness", pressing);
  assertEqual(witnessed.args.agent, "grok", "the witness was taken of another row");
  const launched = await backend.waitForCall("A", "launch_agent_tab", pressing);
  const waited = await backend.waitForCall("A", "cli_login_wait", pressing);
  assert(
    witnessed.at < launched.at && launched.at < waited.at,
    `the witness (${witnessed.at}), the run (${launched.at}) and the wait (${waited.at}) were not in that order`,
  );
  assertEqual(waited.args.witness, ticket, "the wait did not carry the witness the window took");
  assertEqual(waited.args.signedIn, true, "the watch was not for the renewed login");
  await forcedGrokReread(pressing);
  await renderSettled(pageA);
  await leaveExpiredGrokCard();
  return "witness → run → wait(witness)";
});

await test("a_refused_witness_runs_nothing: 백엔드가 witness를 잡아 주지 않으면 실행도 없다 — 아무도 지켜보지 않는 실행이 이 순서가 막는 구멍", async () => {
  const rows = await openExpiredGrokCard(expiredGrokRow());
  backend.rejectOnce("cli_login_witness");
  const pressing = await pressRunOnce(rows);
  await backend.waitForCall("A", "cli_login_witness", pressing);
  await backend.waitForCall("A", "cli_login_list", pressing);
  await forcedGrokReread(pressing);
  await renderSettled(pageA);
  const ran = backend.calls
    .slice(pressing)
    .filter((call) => ["launch_agent_tab", "term_text", "cli_login_wait"].includes(call.command));
  assertEqual(ran, [], "a witness nobody took was followed by a run or a wait");
  assertEqual(
    await rows.nth(0).locator("button").first().isDisabled(),
    false,
    "the row stayed busy after the witness was refused",
  );
  await leaveExpiredGrokCard();
  return "witness refused → no run, no wait, row free";
});

await test("a_refused_launch_lets_the_witness_go: 판이 열리지 않으면 witness를 놓고, 기다림은 시작하지 않으며, 행은 풀리고 다시 누를 수 있다", async () => {
  const rows = await openExpiredGrokCard(expiredGrokRow());
  backend.rejectOnce("launch_agent_tab");
  const ticket = backend.nextWitness + 1;
  const pressing = await pressRunOnce(rows);
  await backend.waitForCall("A", "launch_agent_tab", pressing);
  const dropped = await backend.waitForCall("A", "cli_login_witness_drop", pressing);
  assertEqual(dropped.args.witness, ticket, "the witness let go was not the one taken");
  await backend.waitForCall("A", "cli_login_list", pressing);
  await forcedGrokReread(pressing);
  await renderSettled(pageA);
  assertEqual(
    backend.calls.slice(pressing).filter((call) => call.command === "cli_login_wait").length,
    0,
    "a wait began for a run that never started",
  );
  assertEqual(
    await rows.nth(0).locator("button").first().isDisabled(),
    false,
    "the row stayed busy after the launch was refused",
  );
  // And the next press is a whole new attempt: a new witness, a new run.
  const again = await pressRunOnce(rows);
  const retaken = await backend.waitForCall("A", "cli_login_witness", again);
  assert(retaken.at >= again, "the retry took no witness of its own");
  await backend.waitForCall("A", "launch_agent_tab", again);
  await backend.waitForCall("A", "cli_login_wait", again);
  await forcedGrokReread(again);
  await renderSettled(pageA);
  await leaveExpiredGrokCard();
  return "launch refused → witness dropped, no wait, row free → retry runs";
});

await test("an_arrival_is_not_a_figure: 파일이 바뀌어 기다림이 돌아와도 강제 재조회가 여전히 만료를 말하면 행은 만료를 계속 말하고 「한 번 실행」을 다시 세운다", async () => {
  const rows = await openExpiredGrokCard(expiredGrokRow());
  // The forced re-read answers what the gauge really saw: still expired.
  backend.grokUsage = { usage: expiredGrokGauge(), fetching: false };
  const pressing = await pressRunOnce(rows);
  await backend.waitForCall("A", "cli_login_wait", pressing);
  await forcedGrokReread(pressing);
  await renderSettled(pageA);
  const words = await pageA.evaluate(() => ({
    runOnce: t("usage.runOnce", "{{program}} 한 번 실행", { program: "grok" }),
    expired: t(
      "settings.cliLogins.expired",
      "로그인 만료 — {{program}}을(를) 한 번 실행하면 CLI가 세션을 갱신합니다",
      { program: "grok" },
    ),
  }));
  assertEqual(
    await rows.nth(0).locator(".agent-row-cmd").textContent(),
    words.expired,
    "the row called the session renewed on the file's word alone",
  );
  assertEqual(
    await rows.nth(0).locator("button").first().textContent(),
    words.runOnce,
    "the row put its run away while the gauge still says expired",
  );
  assertEqual(
    await rows.nth(0).locator("button").first().isDisabled(),
    false,
    "the row stayed busy",
  );
  backend.grokUsage = null;
  await leaveExpiredGrokCard();
  return "arrival → forced read still expired → row still says so";
});

await test("a_timed_out_watch_still_rereads_the_gauge: 기다림이 시간을 다 써도 게이지를 한 번은 강제로 다시 읽고, 행은 바쁨에서 풀린다", async () => {
  const rows = await openExpiredGrokCard(expiredGrokRow());
  backend.rejectOnce("cli_login_wait");
  const pressing = await pressRunOnce(rows);
  await backend.waitForCall("A", "cli_login_wait", pressing);
  // The re-read is the FORCED ask, on the failure road too: the CLI may have
  // renewed without the file changing in a way the watch counts, and the
  // gauge is the only reader that knows.
  await forcedGrokReread(pressing);
  await backend.waitForCall("A", "cli_login_list", pressing);
  await renderSettled(pageA);
  assertEqual(
    await rows.nth(0).locator("button").first().isDisabled(),
    false,
    "the row stayed busy after its wait ran out",
  );
  await leaveExpiredGrokCard();
  return "wait ran out → gauge forced once → row free";
});

await test("라우터 추가→연결 시험→모델 셋 켬→settings.json providers 한 항목", async () => {
  await openSettings(pageA, "api-routers");
  await pageA.evaluate(() => modelConnectionsController.addManual());

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

  // The saved connection is a row of the connections page (t-75762 v2).
  const connectedRows = pageA.locator(SAVED_CONNECTION_ROWS);
  assertEqual(await connectedRows.count(), 1, "connected router appears in list");
  assertEqual(await connectedRows.locator(".model-line-name").textContent(), "OpenRouter", "connected router name matches");
  const cmdText = await connectedRows.locator(".model-line-way").first().textContent();
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
    (rows) => document.querySelectorAll(rows).length === 2,
    SAVED_CONNECTION_ROWS,
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

await test("라우터 모델 표의 머리칸 하나로 전체 선택·전체 해제", async () => {
  await openSettings(pageA, "api-routers");
  await pageA.evaluate(() => modelConnectionsController.addManual());
  await pageA.selectOption("#router-preset-select", "custom");
  await pageA.fill("#router-name-input", "pick-all");
  await pageA.fill("#router-base-url-input", origin);
  await pageA.click("#router-test-btn");
  await pageA.waitForSelector("#router-models-section:not([hidden])", { timeout: UI_TIMEOUT });
  const all = pageA.locator("#router-models-all");
  const rows = pageA.locator(".router-model-check");
  assertEqual(await rows.count(), 4, "stub returned 4 models");
  // The header's one box: what it shows, what it would do next, how many rows are on.
  const state = () => pageA.evaluate(() => {
    const box = document.getElementById("router-models-all");
    return {
      checked: box?.checked ?? null,
      indeterminate: box?.indeterminate ?? null,
      label: box?.getAttribute("aria-label") ?? null,
      on: document.querySelectorAll(".router-model-check:checked").length,
    };
  });
  assertEqual(await state(), { checked: true, indeterminate: false, label: "전체 해제", on: 4 },
    "after the probe every model is on and the header box says so");
  await all.click();
  assertEqual(await state(), { checked: false, indeterminate: false, label: "전체 선택", on: 0 },
    "one click on the header box clears every model");
  assert(await pageA.locator("#router-save-btn").isDisabled(), "with no model chosen there is nothing to save");
  await all.click();
  assertEqual(await state(), { checked: true, indeterminate: false, label: "전체 해제", on: 4 },
    "one click on the header box selects every model");
  assert(!await pageA.locator("#router-save-btn").isDisabled(), "every model chosen can be saved");
  await rows.nth(1).uncheck();
  assertEqual(await state(), { checked: false, indeterminate: true, label: "전체 선택", on: 3 },
    "a partial pick shows on the header box as indeterminate");
  await all.click();
  assertEqual(await state(), { checked: true, indeterminate: false, label: "전체 해제", on: 4 },
    "a click on a partial pick selects the rest");
  await pageA.fill("#router-key-input", "changed-credential");
  assertEqual(await state(), { checked: false, indeterminate: false, label: "전체 선택", on: 0 },
    "an invalidated probe clears the header box with the rows");
  return "header box: all → none → all → partial (indeterminate) → all; cleared with the rows";
});

await test("라우터 연결 변경은 이전 모델 목록과 늦게 도착한 응답을 무효화한다", async () => {
  await openSettings(pageA, "api-routers");
  await pageA.evaluate(() => modelConnectionsController.addManual());
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
  await pageA.evaluate(() => modelConnectionsController.addManual());
  await pageA.selectOption("#router-preset-select", "custom");
  await pageA.fill("#router-name-input", "saved-draft");
  await pageA.fill("#router-base-url-input", origin);
  await pageA.click("#router-test-btn");
  await pageA.waitForSelector("#router-models-section:not([hidden])", { timeout: UI_TIMEOUT });
  for (const change of ["name", "model"]) {
    const held = backend.holdBeforeNext("save_api_router");
    const saving = pageA.evaluate(() => saveApiRouterEntry());
    // Awaited below. A failed wait before that must not leave this rejection
    // unhandled: Node would end the whole suite before its summary.
    saving.catch(() => {});
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
  await pageA.evaluate(() => modelConnectionsController.addManual());
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

  // 지우기 on the renamed preset row names the row the list showed — its name,
  // id and generation, never an id the pane derived; the key it read is gone
  // and every other row keeps its own.
  const renamed = backend.zoSettings.providers.find((p) => p.name === "OpenRouter 회사");
  const listed = backend.modelConnections().connections.find((row) => row.provider.name === "OpenRouter 회사");
  const droppedAt = backend.calls.length;
  const rows = pageA.locator(SAVED_CONNECTION_ROWS);
  const index = (await rows.locator(".model-line-name").allTextContents()).indexOf("OpenRouter 회사");
  await pageA.evaluate(() => modelConnectionsController.show("catalog"));
  await rows.nth(index).locator(".account-drop").click();
  await rows.nth(index).locator(".connection-delete-note").getByRole("button", { name: "삭제", exact: true }).click();
  const removal = await backend.waitForCall("A", "remove_api_router", droppedAt);
  assertEqual(removal.args, { name: "OpenRouter 회사", sourceId: listed.id, expectedGeneration: listed.generation }, "the removal did not name the row the list showed");
  await pageA.waitForFunction(
    (rows) => document.querySelectorAll(rows).length === 3,
    SAVED_CONNECTION_ROWS,
    { timeout: UI_TIMEOUT },
  );
  assertEqual(backend.keychain.has(`dev.zerocode.router.${renamed.auth_env}`), false, "the renamed row's key outlived it");
  assertEqual(keyOf("OpenRouter"), "sk-or-mine", "removing one OpenRouter row took the other's key");
  return envs.join(", ");
});

await test("키체인이 없는 컴퓨터: 판이 키체인을 약속하지 않고 키 칸을 내주지 않으며, 키 없는 저장은 된다", async () => {
  await openSettings(pageA, "api-routers");
  await pageA.evaluate(() => modelConnectionsController.addManual());
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
  await pageA.selectOption("#connection-auth", "new");
  assert(!(await pageA.locator("#router-key-input").isDisabled()), "a machine with a keychain was left without the key field");
  assert(await pageA.locator("#router-keychain-hint").isVisible(), "a machine with a keychain lost its hint");
  assert(!(await pageA.locator("#router-no-keychain-hint").isVisible()), "a machine with a keychain still said it had none");
  return reason;
});

await test("판단 모델 전용 화면은 HF 주소만 받아 기본 폴더에 설치하고 확인 뒤 사용한다", async () => {
  const previous = backend.zoSettings.smart === undefined ? undefined : clone(backend.zoSettings.smart);
  const previousManaged = backend.managedModelState === undefined ? undefined : clone(backend.managedModelState);
  delete backend.managedModelState;
  const modelPage = await context.newPage();
  await installTauri(modelPage, "source-basic");
  attachFaultRecorder(modelPage, "source-basic");
  try {
    await bootPage(modelPage, "source-basic");
    assertEqual(await modelPage.locator('.settings-pane[data-pane="judgment-models"]').count(), 1, "model setup has no dedicated pane");
    await openSettings(modelPage, "judgment-models");
    await modelPage.waitForFunction(() => managedJudgmentController.snapshot?.catalogFingerprint);
    // Getting a model file is a 고급 item now (t-75762 v2): closed until asked for.
    assert(await modelPage.locator("#judgment-source-repository").isHidden(), "model files are offered before 고급 is opened");
    await modelPage.locator("#model-advanced > summary").click();
    assert(await modelPage.locator("#judgment-source-repository").isVisible(), "the basic view has no HF address input");
    assert(await modelPage.locator("#judgment-endpoint").isHidden(), "the basic view requires a server address");
    assert(await modelPage.locator("#typesafe-key-input").isHidden(), "the basic local view requires an API key");
    assert(!await modelPage.locator("#judgment-connection-settings").evaluate((element) => element.open), "advanced server settings start open");
    assertEqual(await modelPage.locator(".judgment-model-source .btn--primary").count(), 1, "basic setup has competing primary actions");
    const before = clone(backend.typesafeSettings().provider);
    const inspectedAt = backend.calls.length;
    await modelPage.fill("#judgment-source-repository", "https://huggingface.co/acme/decision-model");
    await modelPage.click("#judgment-source-inspect");
    const inspected = await backend.waitForCall("source-basic", "managed_judgment_inspect_source", inspectedAt);
    assertEqual(inspected.args.source, { kind: "hugging_face", repository: "https://huggingface.co/acme/decision-model" }, "the user must supply a manifest or hashes");
    await modelPage.waitForFunction(() => judgmentModelSourceController.inspection?.readyForInstall && !judgmentModelSourceController.busy);
    assert(await modelPage.locator("#judgment-source-use").evaluate((element) => element.classList.contains("btn--primary")), "the inspected model has no primary use action");
    assertEqual(backend.typesafeSettings().provider, before, "inspection changed the active model");
    assert(!backend.calls.slice(inspectedAt).some((entry) => ["managed_judgment_install_source", "managed_judgment_start", "managed_judgment_apply"].includes(entry.command)), "inspection downloaded or executed a model");
    const usingAt = backend.calls.length;
    await modelPage.click("#judgment-source-use");
    const installed = await backend.waitForCall("source-basic", "managed_judgment_install_source", usingAt);
    assertEqual(installed.args, { inspectionId: "fixture-inspection", instanceId: "fixture-instance", expectedCatalogFingerprint: "fixture-catalog" }, "the default install is not bound to the inspected source");
    await backend.waitForCall("source-basic", "managed_judgment_apply", usingAt);
    await modelPage.waitForFunction(() => !managedJudgmentController.activating);
    const commands = backend.calls.slice(usingAt).filter((entry) => entry.window_id === "source-basic").map((entry) => entry.command);
    const flow = ["managed_judgment_install_source", "managed_judgment_prepare_model_runtime", "managed_judgment_start", "managed_judgment_check", "managed_judgment_apply"];
    for (let index = 1; index < flow.length; index += 1) {
      assert(commands.indexOf(flow[index - 1]) >= 0 && commands.indexOf(flow[index]) > commands.indexOf(flow[index - 1]), "the one-click setup steps are out of order", commands);
    }
    assertEqual(backend.typesafeSettings().provider.model, "fixture-imported");
  } finally {
    await modelPage.close();
    backend.zoSettings.smart = previous;
    backend.managedModelState = previousManaged;
    delete backend.managedSourceInspection;
  }
  return "dedicated pane · no port/key/JSON requirement · explicit install · automatic runtime/check/apply";
});

await test("내 모델과 저장 폴더는 선택기로 고르고 설치 취소는 현재 모델을 유지한다", async () => {
  const previous = backend.zoSettings.smart === undefined ? undefined : clone(backend.zoSettings.smart);
  const previousManaged = backend.managedModelState === undefined ? undefined : clone(backend.managedModelState);
  const previousPaths = backend.pickedPaths;
  delete backend.managedModelState;
  backend.managedPendingKinds = new Set(["download"]);
  const modelPage = await context.newPage();
  await installTauri(modelPage, "source-local");
  attachFaultRecorder(modelPage, "source-local");
  try {
    await bootPage(modelPage, "source-local");
    assertEqual(await modelPage.locator("#judgment-source-local").count(), 1, "there is no local-folder entry point");
    await openSettings(modelPage, "judgment-models");
    await modelPage.waitForFunction(() => managedJudgmentController.snapshot?.catalogFingerprint);
    await modelPage.locator("#model-advanced > summary").click();
    backend.pickedPaths = ["/models/source-folder"];
    await modelPage.click("#judgment-source-local");
    await modelPage.waitForFunction(() => judgmentModelSourceController.directory === "/models/source-folder" && !judgmentModelSourceController.busy);
    backend.pickedPaths = ["/models/chosen-destination"];
    await modelPage.click("#judgment-source-change-destination");
    await modelPage.waitForFunction(() => judgmentModelSourceController.destination === "/models/chosen-destination" && !judgmentModelSourceController.busy);
    const inspectedAt = backend.calls.length;
    await modelPage.click("#judgment-source-inspect");
    const inspected = await backend.waitForCall("source-local", "managed_judgment_inspect_source", inspectedAt);
    assertEqual(inspected.args.source, { kind: "local_directory", directory: "/models/source-folder" });
    await modelPage.waitForFunction(() => judgmentModelSourceController.inspection?.readyForInstall && !judgmentModelSourceController.busy);
    const before = clone(backend.typesafeSettings().provider);
    const usingAt = backend.calls.length;
    await modelPage.click("#judgment-source-use");
    const installed = await backend.waitForCall("source-local", "managed_judgment_install_source", usingAt);
    assertEqual(installed.args.destinationDirectory, "/models/chosen-destination");
    assert(!Object.hasOwn(installed.args, "directory") && !Object.hasOwn(installed.args, "source"), "install accepts a client replacement for the inspected source");
    await modelPage.waitForFunction(() => managedJudgmentController.snapshot?.operation?.state === "running");
    await modelPage.click("#managed-judgment-cancel");
    await modelPage.waitForFunction(() => !managedJudgmentController.activating);
    assertEqual(backend.typesafeSettings().provider, before, "cancelled import changed the active model");
    assert(!backend.calls.slice(usingAt).some((entry) => ["managed_judgment_start", "managed_judgment_apply"].includes(entry.command)), "cancelled installation reached execution or application");
  } finally {
    await modelPage.close();
    backend.zoSettings.smart = previous;
    backend.managedModelState = previousManaged;
    backend.pickedPaths = previousPaths;
    delete backend.managedPendingKinds;
    delete backend.managedSourceInspection;
  }
  return "native folder pickers · explicit destination · immutable inspected source · cancellation preserves active provider";
});

await test("판단 설정은 적용된 모델과 아직 연결하지 않은 선택을 분리한다", async () => {
  await openJudgmentSettings(pageA);
  await pageA.evaluate(() => refreshApiRouters());
  const applied = clone(backend.typesafeSettings().provider);
  try {
    assert(await pageA.locator(JUDGMENT_NOW).isVisible(), "the active model has no separate summary");
    assert((await pageA.locator(JUDGMENT_NOW).textContent()).includes(applied.model), "the summary does not name the applied model");
    await pageA.selectOption("#judgment-provider-kind", "remote");
    await pageA.fill("#judgment-endpoint", "");
    assertEqual(await pageA.locator("#judgment-setup-hint").getAttribute("data-step"), "connection", "the next step does not ask for a server");
    assert((await pageA.locator("#judgment-setup-hint").textContent()).trim(), "the next step has no instructions");
    assert((await pageA.locator(JUDGMENT_NOW).textContent()).includes(applied.model), "a draft replaced the active-model summary");
    assertEqual(backend.typesafeSettings().provider, applied, "choosing a setup path applied it");
  } finally {
    await pageA.evaluate(() => { judgmentProviderController.draft = null; return refreshApiRouters(); });
  }
  return "applied model separate from draft · next connection step named";
});

await test("판단 모델 연결 동작은 기능별 진단보다 먼저 보인다", async () => {
  await openJudgmentSettings(pageA);
  const order = await pageA.evaluate(() => {
    const editor = document.getElementById("judgment-provider-editor");
    const actions = document.querySelector(".settings-judgment-actions");
    const diagnostics = document.querySelector("#typesafe-card [data-jev-standings]");
    return {
      keyInsideConnection: editor.contains(document.getElementById("typesafe-key-input")),
      actionsBeforeDiagnostics: Boolean(actions.compareDocumentPosition(diagnostics) & Node.DOCUMENT_POSITION_FOLLOWING),
      modelInsideConnection: editor.contains(document.getElementById("typesafe-model-input")),
    };
  });
  assertEqual(order, { keyInsideConnection: true, actionsBeforeDiagnostics: true, modelInsideConnection: true }, "connection controls are scattered around diagnostics");
  return "credentials and model together · apply before diagnostics";
});

await test("판단 기능별 진단은 기본으로 접고 사용자가 펼친다", async () => {
  await openJudgmentSettings(pageA);
  const details = pageA.locator("#judgment-feature-details");
  assertEqual(await details.count(), 1, "the feature diagnostics have no fold");
  assert(!(await details.evaluate((element) => element.open)), "diagnostics are expanded before setup");
  await details.locator("summary").click();
  assert(await details.evaluate((element) => element.open), "the diagnostics fold did not open");
  assertEqual(await details.locator("[data-jev-standings]").count(), 1, "the existing feature diagnostics were removed");
  await details.locator("summary").click();
  return "diagnostic details retained and opt-in";
});

await test("TypeSafe 키는 키체인에만 가고, Jev는 스위치 하나로 켜고 끈다 — 카드에 고를 모드는 없다", async () => {
  await openJudgmentSettings(pageA);
  backend.keychain.delete(TYPESAFE_SERVICE);
  delete backend.zoSettings.smart;
  backend.zoSettings.providers = [{ name: "OpenRouter", base_url: origin, models: [], requires_auth: false, auth_env: "ZEROCODE_ROUTER_OPENROUTER_KEY" }];
  await pageA.evaluate(() => refreshApiRouters());
  await renderSettled(pageA);
  const said = (key, fallback, vars) => pageA.evaluate(([one, words, values]) => t(one, words, values), [key, fallback, vars]);
  const status = () => statusSaid(pageA, "typesafe-status");
  const toggle = pageA.locator("#jev-enabled");
  const partial = pageA.locator("#typesafe-card [data-jev-partial]");
  // The door's object as the press left it, keys in one order however the
  // press happened to write them.
  const doorObject = () => Object.fromEntries(Object.entries(backend.zoSettings.smart[JEV_SETTINGS_KEY]).sort());
  const statusIs = async (words) => pageA.waitForFunction(
    (expected) => document.querySelector("#typesafe-status .settings-status-said")?.textContent === expected,
    words, { timeout: UI_TIMEOUT },
  );
  // The switch answers beside itself, at the judgment card's head (t-75762 v2).
  const switchSaid = async (words) => pageA.waitForFunction(
    (expected) => document.querySelector("#model-judgment-status .settings-status-said")?.textContent === expected,
    words, { timeout: UI_TIMEOUT },
  );

  // Nothing saved: the badge says so, and nothing but typing a key is offered.
  assertEqual(
    await pageA.locator("#typesafe-key-state").textContent(),
    await standingWord(pageA, "unchecked"),
    "a card with nothing checked does not wear the table's word for it",
  );
  for (const id of ["#typesafe-check-btn", "#typesafe-remove-btn", "#typesafe-save-btn"]) {
    assert(await pageA.locator(id).isDisabled(), `${id} was offered with no key saved`);
  }
  assert(await toggle.isVisible(), "the card has no switch");
  assert(!(await toggle.isChecked()), "a machine that never met Jev reads as using it");
  assert(await partial.isHidden(), "a switch that is off says where it is used");
  // The library's filter stands over the library's rows (t-69998): the
  // fixture's one catalog model is not received and no server answers with
  // it, so this computer holds no model and has nothing to filter.
  await pageA.waitForFunction(() => managedJudgmentController.snapshot);
  assertEqual(await pageA.evaluate(() => ({
    catalog: managedJudgmentController.snapshot.models.map((model) => [model.id, model.installed]),
    rows: document.querySelectorAll("#managed-judgment-models > .managed-judgment-model").length,
  })), { catalog: [["fixture-decision", false]], rows: 0 }, "a catalog model this computer does not hold stands as a library row");
  // A closed fold keeps its body's boxes (content-visibility) while nobody
  // sees them, so what is folded away is counted as out of sight.
  const selects = await pageA.locator("#typesafe-card select").evaluateAll((all) => ({
    ids: all.map((select) => select.id),
    visible: all.filter((select) => !select.closest("details:not([open])") && select.getClientRects().length > 0).map((select) => select.id),
  }));
  assertEqual(selects, {
    // Each saved connection's judgment format (t-76028 F3) is a protocol
    // choice too, folded in 「고급 — 판단 응답 형식」.
    ids: ["judgment-format-fixture-OpenRouter", "managed-judgment-filter", "judgment-provider-kind", "judgment-protocol", "judgment-listed-model"],
    visible: ["managed-judgment-filter", "judgment-provider-kind"],
  }, "the provider and candidate filter are visible; protocol and inactive choices stay folded");
  // The worker rows and the classifier moved to the orchestration pane
  // (t-75762 v2, decision 4), each on a card of its own since the person
  // passed the before/after draft (2026-10-27) — nothing to unfold.
  for (const card of ["#worker-models-card", "#route-classifier-card"]) {
    assertEqual(await pageA.locator(card).evaluate((one) => one.closest(".settings-pane").dataset.pane), "orchestration");
    assertEqual(await pageA.locator(card).evaluate((one) => Boolean(one.closest("details"))), false, `${card} is folded`);
  }

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
    !(await pageA.locator("#typesafe-card").innerHTML()).includes("apikey_fixture"),
    "the key came back to the page",
  );
  assertEqual(await status(), await said("settings.typesafe.saved", "키를 키체인에 저장했습니다. zo가 다음에 필요할 때 읽습니다."));

  // The check is zo's answer: a model and its latency, or the failure named.
  await pageA.click("#typesafe-check-btn");
  await pageA.waitForFunction(() => document.querySelector("#typesafe-status .settings-status-said")?.textContent.includes("jev-1.13.0"), null, { timeout: UI_TIMEOUT });
  assertEqual(await status(), await said("settings.typesafe.answered", "응답했습니다 — {{model}}, {{ms}} ms", { model: "jev-1.13.0", ms: 612 }));
  backend.typesafeCheck = { answered: false, failure: "unauthorized", elapsedMs: 515 };
  await pageA.click("#typesafe-check-btn");
  await statusIs(await said("settings.typesafe.unauthorized", "키가 거절되었습니다 — 키를 다시 확인하세요."));
  backend.typesafeCheck = { answered: true, model: "jev-1.13.0", elapsedMs: 612 };

  // One press on: the switch, every folder, every seat at its recommendation —
  // and nothing else of zo's settings moves.
  const routers = clone(backend.zoSettings.providers);
  const onAt = backend.calls.length;
  await toggle.click();
  assertEqual((await backend.waitForCall("A", "set_jev_enabled", onAt)).args.on, true);
  await switchSaid(await said("settings.typesafe.turnedOn", "켰습니다. 모든 폴더에서 기능마다 권장 설정으로 판단을 요청합니다."));
  assertEqual(doorObject(), { enabled: true, workspaces: [JEV_EVERY_WORKSPACE] });
  assertEqual(backend.zoSettings.providers, routers, "the switch moved the router rows");
  await pageA.evaluate(() => refreshApiRouters());
  assert(await toggle.isChecked(), "a reopened card lost the switch");
  assert(await partial.isHidden(), "every folder read as some");

  // A file set up before the switch: seat words by hand and two folders. Jev
  // is in use — the switch says so — in two folders only, and the line under
  // it offers the rest in one press, which is the switch's own.
  backend.zoSettings.smart = {
    [JEV_SEATS[0].setting]: "shadow",
    [JEV_SEATS[1].setting]: "on",
    [JEV_SETTINGS_KEY]: { workspaces: ["/work/a", "/work/b"], labelDrafts: true },
  };
  await pageA.evaluate(() => refreshApiRouters());
  assert(await toggle.isChecked(), "a file that sends read as off");
  assert(await partial.isVisible(), "consent for some folders did not say so");
  assertEqual(
    (await partial.locator("[data-jev-partial-said]").textContent()).trim(),
    await said("settings.typesafe.partial", "일부 폴더({{count}}개)에서만 사용 중입니다.", { count: "2" }),
  );
  const everyAt = backend.calls.length;
  await partial.locator("[data-jev-everywhere]").click();
  assertEqual((await backend.waitForCall("A", "set_jev_enabled", everyAt)).args.on, true);
  await pageA.waitForFunction(() => document.querySelector("#typesafe-card [data-jev-partial]")?.hidden === true, null, { timeout: UI_TIMEOUT });
  assertEqual(doorObject(), { enabled: true, labelDrafts: true, workspaces: [JEV_EVERY_WORKSPACE] });
  for (const seat of JEV_SEATS) {
    assert(!(seat.setting in backend.zoSettings.smart), `${seat.id} kept its own word`);
  }

  // A refused press keeps the card as it stood and says why.
  backend.typesafeSetFailure = true;
  try {
    const refusedAt = backend.calls.length;
    await toggle.click();
    await backend.waitForCall("A", "set_jev_enabled", refusedAt);
    await pageA.waitForFunction(() => document.getElementById("jev-enabled")?.checked === true, null, { timeout: UI_TIMEOUT });
    assertEqual(backend.zoSettings.smart[JEV_SETTINGS_KEY].enabled, true, "a refused press mutated the backend");
  } finally {
    delete backend.typesafeSetFailure;
  }

  // One press off: the switch alone moves, and nothing is sent.
  const offAt = backend.calls.length;
  await toggle.click();
  assertEqual((await backend.waitForCall("A", "set_jev_enabled", offAt)).args.on, false);
  await switchSaid(await said("settings.typesafe.turnedOffAll", "껐습니다. 아무것도 보내지 않습니다."));
  assertEqual(doorObject(), { enabled: false, labelDrafts: true, workspaces: [JEV_EVERY_WORKSPACE] });
  assert(!(await toggle.isChecked()), "the switch stayed on");

  // The model pin (`smart.jevModel`), under 고급: unpinned, the field is empty
  // with the alias behind it; a version pins it where the door reads it; an
  // empty field unpins — the key leaves the file — and a pin the door would
  // not read is refused and writes nothing.
  await openSettings(pageA, "orchestration");
  // Each cell is a model and an effort picked from what the agent's lineup
  // offers today (t-14437); both start at 「자동」, what runs there now.
  const cellOf = (agent, difficulty, field) =>
    pageA.locator(`#summon-profiles-table select[data-agent="${agent}"][data-difficulty="${difficulty}"][data-field="${field}"]`);
  const lowModel = cellOf("codex", "low", "model");
  const lowEffort = cellOf("codex", "low", "effort");
  await lowModel.waitFor({ timeout: UI_TIMEOUT });
  assertEqual(await lowModel.inputValue(), "", "an automatic cell showed a written model");
  assertEqual(await lowModel.locator("option").first().textContent(),
    await said("settings.workerModels.auto", "자동 · {{value}}", { value: SUMMON_PROFILES.codex.low.model }));
  const offered = (select) => select.locator("option:not([disabled])").evaluateAll((all) => all.map((option) => option.value).filter(Boolean));
  const fits = (pick) => pick.fits.includes("low");
  assertEqual(await offered(lowModel),
    [...FIXTURE_LINEUP.codex.filter(fits), ...FIXTURE_LINEUP.codex.filter((pick) => !fits(pick))].map((pick) => pick.id),
    "the cell offers the lineup — the models for its difficulty first — and nothing else");
  assertEqual(await offered(lowEffort), FIXTURE_LINEUP.codex.find((pick) => pick.id === SUMMON_PROFILES.codex.low.model).efforts,
    "the effort offers what the row's model takes");
  // A connection's models are the zo row's to pick from; a keyless one is
  // listed and offers nothing.
  const zoModels = await cellOf("zo", "high", "model").locator("optgroup").evaluateAll((groups) =>
    groups.map((group) => [group.label, [...group.querySelectorAll("option")].map((option) => [option.value, option.disabled])]));
  assertEqual(zoModels.slice(-2), [
    [await said("settings.workerModels.connection", "연결 · {{name}}", { name: "fixture-router" }), [["fixture-router/model-x", false]]],
    [await said("settings.workerModels.connectionKeyless", "연결 · {{name}} — 키 필요", { name: "fixture-keyless" }), [["fixture-keyless/model-y", true]]],
  ]);
  assertEqual(await cellOf("codex", "high", "model").locator("optgroup").evaluateAll((groups) =>
    groups.some((group) => group.querySelector('option[value^="fixture-router/"]'))), false, "a one-provider agent offered a connection");
  // The agent's standing beside its name, as the readiness table saw it.
  assertEqual(await pageA.locator('#summon-profiles-table tr[data-agent="claude"] .worker-models-agent-state').textContent(),
    await said("settings.agents.loginNeeded", "로그인 필요"));

  // A pick is saved at once — one cell, the effort left open — and the words
  // the next summons runs are the backend's own: the model alone, its CLI
  // running its own default effort, which the screen calls 「자동」
  // (t-74181).
  const pickAt = backend.calls.length;
  await lowModel.selectOption("gpt-fixture-new");
  const picked = await backend.waitForCall("A", "set_summon_profile", pickAt);
  assertEqual(picked.args, { agent: "codex", difficulty: "low", model: "gpt-fixture-new", effort: null });
  assertEqual(backend.zoSettings.smart.summonProfiles, { codex: { low: { model: "gpt-fixture-new" } } }, "more than the cell was written");
  await pageA.waitForFunction(
    (expected) => document.querySelector("#orch-models-status .settings-status-said")?.textContent === expected,
    await said("settings.workerModels.saved", "저장했습니다 — 다음 {{agent}} {{difficulty}} 소환부터 {{model}} · {{effort}}", {
      agent: await pageA.evaluate(() => agentName("codex")), difficulty: await said("settings.workerModels.low", "쉬움"), model: "gpt-fixture-new",
      effort: await said("settings.workerModels.autoPlain", "자동"),
    }), { timeout: UI_TIMEOUT },
  );
  assertEqual(await pageA.locator("#worker-models-next-words").textContent(), "--model gpt-fixture-new");
  assertEqual(await lowModel.inputValue(), "gpt-fixture-new", "the cell did not repaint from the answer");
  assertEqual(await pageA.locator("#worker-models-auto").textContent(),
    await said("settings.workerModels.someWritten", "{{total}}칸 중 {{count}}칸 직접 정함", { total: 9, count: 1 }));
  // An effort the model does not take is refused by name; nothing is written.
  const refusedAt = backend.calls.length;
  await pageA.evaluate(() => saveWorkerCell("codex", "low", "gpt-fixture-new", "max"));
  await backend.waitForCall("A", "set_summon_profile", refusedAt);
  await pageA.waitForFunction(
    (expected) => document.querySelector("#orch-models-status .settings-status-said")?.textContent === expected,
    await said("settings.workerModels.effortRefused", "{{model}}은(는) {{takes}}만 받아 {{effort}}(으)로 저장하지 않았습니다.", {
      model: "gpt-fixture-new", takes: FIXTURE_LINEUP.codex.find((pick) => pick.id === "gpt-fixture-new").efforts.join(", "), effort: "max",
    }), { timeout: UI_TIMEOUT },
  );
  assertEqual(backend.zoSettings.smart.summonProfiles, { codex: { low: { model: "gpt-fixture-new" } } }, "a refused cell was written");
  // 「자동으로」 clears the cell back to the lineup, and the table with it.
  const resetAt = backend.calls.length;
  await pageA.locator('#summon-profiles-table .worker-models-reset[data-agent="codex"][data-difficulty="low"]').click();
  assertEqual((await backend.waitForCall("A", "set_summon_profile", resetAt)).args, { agent: "codex", difficulty: "low", model: null, effort: null });
  await pageA.waitForFunction(() => !typesafeBusy);
  assertEqual(backend.zoSettings.smart.summonProfiles, undefined, "a cleared cell left a table behind");
  assertEqual(await lowModel.inputValue(), "");
  // 「목록 새로고침」 asks for a fresh read and paints its answer.
  const refreshAt = backend.calls.length;
  await pageA.click("#worker-models-refresh");
  await backend.waitForCall("A", "refresh_summon_lineup", refreshAt);
  await pageA.waitForFunction(
    (expected) => document.querySelector("#orch-models-status .settings-status-said")?.textContent === expected,
    await said("settings.workerModels.refreshed", "목록을 새로 받았습니다."), { timeout: UI_TIMEOUT },
  );
  await openJudgmentSettings(pageA);
  const modelInput = pageA.locator("#typesafe-model-input");
  assertEqual(await modelInput.inputValue(), JEV_MODEL_ALIAS, "the active alias was not shown");
  assertEqual(await modelInput.getAttribute("placeholder"), JEV_MODEL_ALIAS);
  const pinAt = backend.calls.length;
  await modelInput.fill("jev-1.13.0");
  await modelInput.dispatchEvent("change");
  assert(!backend.calls.slice(pinAt).some((call) => call.command === "set_judgment_provider"), "editing applied a provider");
  await pageA.click("#judgment-save-provider");
  assertEqual((await backend.waitForCall("A", "set_judgment_provider", pinAt)).args.provider.model, "jev-1.13.0");
  await statusIs(await said("settings.judgment.checkedAndApplied", "응답을 확인한 모델을 적용했습니다. 연결 확인은 작업 정확도 검증과 별개입니다."));
  assertEqual(backend.zoSettings.smart[JEV_MODEL_SETTING], "jev-1.13.0");
  await pageA.evaluate(() => refreshApiRouters());
  assertEqual(await modelInput.inputValue(), "jev-1.13.0", "a reopened card lost the pin");
  const slipAt = backend.calls.length;
  await modelInput.fill("jev 1.13");
  await modelInput.dispatchEvent("change");
  await pageA.click("#judgment-save-provider");
  await backend.waitForCall("A", "preflight_judgment_provider", slipAt);
  await pageA.waitForFunction(() => !typesafeBusy);
  assert(!backend.calls.slice(slipAt).some((call) => call.command === "set_judgment_provider"), "an invalid pin passed preflight and reached apply");
  assertEqual(backend.zoSettings.smart[JEV_MODEL_SETTING], "jev-1.13.0", "a refused pin was written");
  const unpinAt = backend.calls.length;
  await modelInput.fill("");
  await modelInput.dispatchEvent("change");
  await pageA.click("#judgment-save-provider");
  await backend.waitForCall("A", "set_judgment_provider", unpinAt);
  await statusIs(await said("settings.judgment.checkedAndApplied", "응답을 확인한 모델을 적용했습니다. 연결 확인은 작업 정확도 검증과 별개입니다."));
  assert(!(JEV_MODEL_SETTING in backend.zoSettings.smart), "an unpin left the key in the file");

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
  return `one switch · ${selects.total} selects · ${selects.visible} visible`;
});

await test("현재 판단 상태는 실행·요청 허용·응답 점검·실제 사용을 구분한다", async () => withJudgmentStatus(async (page, provider) => {
  assert(await page.locator("#model-judgment").isVisible(), "the applied model has no primary status area");
  const label = backend.managedJudgmentSettings().models.find((model) => model.id === provider.model).label;
  assert((await page.locator(JUDGMENT_NOW).textContent()).includes(label), "the card does not name the applied model");
  assert(await page.locator("#model-judgment .model-off").isVisible(), "a running service read as judgment requests allowed");
  assert(!(await page.locator("#jev-enabled").isChecked()), "the switch at the card's head reads on");
  assertEqual(await page.locator(JUDGMENT_STATE).getAttribute("data-model-state"), "running", "a matching checked server was not read as running");
  assertEqual(await page.locator("#judgment-current-usage").getAttribute("data-state"), "unknown", "a native check was presented as task use");
  assert(!/\d/.test(await page.locator("#judgment-current-usage").textContent()), "usage without evidence invented a count or timestamp");
  assertEqual(await page.locator('.settings-pane[data-pane="judgment-models"] > header #typesafe-key-state').count(), 0, "the key/check badge still looks like the whole page's state");
  const reopenedAt = backend.calls.length;
  await page.evaluate(() => { setSettingsOpen(false); setSettingsOpen(true); });
  await backend.waitForCall("judgment-status", "managed_judgment_status", reopenedAt);
  await page.waitForFunction(() => !document.getElementById("judgment-status-refresh").disabled);
  assertEqual(backend.calls.slice(reopenedAt).filter((call) => call.window_id === "judgment-status" && call.command === "typesafe_settings").length, 1, "reopening duplicated the settings read");
  assertEqual(backend.calls.slice(reopenedAt).filter((call) => call.window_id === "judgment-status" && call.command === "managed_judgment_status").length, 1, "reopening did not share one current server read");
  const order = await page.evaluate(() => Boolean(document.getElementById("model-judgment").compareDocumentPosition(document.getElementById("model-advanced")) & Node.DOCUMENT_POSITION_FOLLOWING));
  assert(order, "model changes precede current status");
  await page.locator("#model-advanced").evaluate((fold) => { fold.open = true; });
  const before = backend.calls.length;
  await page.click("#judgment-source-jev");
  assert((await page.locator("#judgment-provider-draft").textContent()).includes("Jev"));
  assertEqual(await page.locator("#judgment-provider-draft").getAttribute("data-state"), "unapplied");
  assert(await page.locator("#typesafe-key-state").isHidden(), "the active provider's key/check badge was attributed to a draft provider");
  assert((await page.locator(JUDGMENT_NOW).textContent()).includes(label), "a Jev draft replaced the applied identity");
  assert(!backend.calls.slice(before).some((call) => ["set_judgment_provider", "managed_judgment_start", "managed_judgment_check"].includes(call.command)), "opening a change performed a model action");
  await page.click("#typesafe-check-btn");
  await page.waitForFunction(() => !typesafeBusy);
  assertEqual(await page.locator("#judgment-current-usage").getAttribute("data-state"), "unknown", "a draft probe became actual usage");
  assertEqual(await page.locator(JUDGMENT_STATE).getAttribute("data-model-state"), "running", "a draft probe replaced the current model's state");
  return "running with requests disabled · matching check · usage unknown · explicit Jev draft";
}));

await test("현재 모델과 다른 서버·점검 기록은 현재 상태로 표시하지 않는다", async () => withJudgmentStatus(async (page, provider) => {
  const snapshot = backend.managedJudgmentSettings();
  snapshot.revision += 1;
  snapshot.server.provider = { ...provider, model: "other-decision" };
  snapshot.server.modelId = "other-decision";
  snapshot.appliedModelId = "other-decision";
  await backend.emitManaged(snapshot);
  assert(await page.locator(JUDGMENT_STATE).getAttribute("data-model-state") !== "running", "another running model was attributed to the active provider");
  snapshot.revision += 1;
  snapshot.providerIdentity = "other-provider-identity";
  snapshot.server.provider = provider;
  snapshot.server.modelId = provider.model;
  snapshot.appliedModelId = provider.model;
  await backend.emitManaged(snapshot);
  assertEqual(await page.locator(JUDGMENT_STATE).getAttribute("data-model-state"), "unknown", "a stale settings identity supplied current server status");
  assertEqual(await page.locator("#judgment-current-usage").getAttribute("data-state"), "unknown");
  backend.judgmentSettingsWait = Promise.resolve({ ...backend.typesafeSettings(), provider: null, providerIdentity: null });
  await page.locator("#model-advanced").evaluate((fold) => { fold.open = true; });
  await page.click("#judgment-status-refresh");
  await page.waitForFunction(() => !document.getElementById("judgment-status-refresh").disabled);
  assert(await page.locator("#model-judgment .model-off").isVisible(), "missing provider metadata erased known request permission");
  assert(await page.locator("#jev-enabled").isEnabled(), "missing provider metadata disabled the existing consent switch");
  assert(await page.locator(JUDGMENT_STATE).getAttribute("data-model-state") !== "running");
  return "server provider, model and canonical identity must agree";
}));

await test("현재 상태 새로고침은 초안을 보존하고 늦은 응답을 버린다", async () => withJudgmentStatus(async (page, provider) => {
  await page.locator("#model-advanced").evaluate((fold) => { fold.open = true; });
  await page.click("#judgment-source-jev");
  await page.fill("#typesafe-model-input", "unapplied-cloud-model");
  const before = backend.calls.length;
  let finishSettings;
  let finishStatus;
  let finishUsage;
  const oldSettings = backend.typesafeSettings();
  const oldStatus = backend.managedJudgmentSettings();
  backend.judgmentSettingsWait = new Promise((resolve) => { finishSettings = resolve; });
  backend.judgmentStatusWait = new Promise((resolve) => { finishStatus = resolve; });
  backend.judgmentUsageWait = new Promise((resolve) => { finishUsage = resolve; });
  await page.click("#judgment-status-refresh");
  await backend.waitForCall("judgment-status", "typesafe_settings", before);
  await backend.waitForCall("judgment-status", "managed_judgment_status", before);
  await backend.waitForCall("judgment-status", "judgment_latest_request", before);
  await page.evaluate(() => { void refreshTypeSafe(); void managedJudgmentController.refresh(); });
  assertEqual(backend.calls.slice(before).filter((call) => call.window_id === "judgment-status" && call.command === "typesafe_settings").length, 1, "refresh duplicated the canonical settings request");
  assertEqual(backend.calls.slice(before).filter((call) => call.window_id === "judgment-status" && call.command === "judgment_latest_request").length, 1, "refresh duplicated the actual-use read");
  const next = clone(oldSettings);
  next.provider.model = "new-current-decision";
  next.providerIdentity = "new-current-identity";
  await page.evaluate((state) => paintTypeSafe(state), next);
  finishSettings(oldSettings);
  finishStatus(oldStatus);
  finishUsage({
    providerIdentity: oldSettings.providerIdentity, observedAtMs: Date.now(), scope: "machine", coverage: "window_requests",
    state: "observed", reason: null, unattributedNewer: false,
    lastRequest: { feature: "stall", atMs: Date.now() - 60_000, outcome: "answered", elapsedMs: 12, applied: true },
  });
  delete backend.judgmentSettingsWait;
  delete backend.judgmentStatusWait;
  delete backend.judgmentUsageWait;
  await page.waitForFunction(() => !document.getElementById("judgment-status-refresh").disabled);
  assert((await page.locator(JUDGMENT_NOW).textContent()).includes("new-current-decision"), "a late read restored the old identity");
  assertEqual(await page.inputValue("#typesafe-model-input"), "unapplied-cloud-model", "refresh discarded an unsaved draft");
  assertEqual(await page.locator("#judgment-provider-draft").getAttribute("data-state"), "unapplied");
  assert(await page.locator(JUDGMENT_STATE).getAttribute("data-model-state") !== "running");
  assertEqual(await page.locator("#judgment-current-usage").getAttribute("data-state"), "unknown", "a late usage read was assigned to a new model");
  await page.evaluate(() => setSettingsOpen(false));
  const hiddenAt = backend.calls.length;
  await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));
  await renderSettled(page);
  assert(!backend.calls.slice(hiddenAt).some((call) => call.window_id === "judgment-status" && ["typesafe_settings", "managed_judgment_status", "judgment_latest_request"].includes(call.command)), "a hidden settings page polls current status");
  await page.evaluate(() => setSettingsOpen(true));
  await page.waitForFunction(() => !document.getElementById("judgment-status-refresh").disabled);
  await page.fill("#settings-search", "no-such-judgment-setting-fixture");
  assert(await page.locator("#model-judgment").isHidden());
  const filteredAt = backend.calls.length;
  await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));
  await renderSettled(page);
  assert(!backend.calls.slice(filteredAt).some((call) => call.window_id === "judgment-status" && ["typesafe_settings", "managed_judgment_status", "judgment_latest_request"].includes(call.command)), "a settings pane hidden by search reads current status");
  assertEqual(backend.typesafeSettings().provider, provider, "status refresh changed saved settings");
  return "one settings/status read · late identity discarded · unsaved draft kept · no hidden refresh";
}));

await test("실제 요청의 성공·실패·적용 결과는 응답 점검과 서버 실행을 바꾸지 않는다", async () => withJudgmentStatus(async (page) => {
  const atMs = Date.parse("2026-10-01T10:20:30Z");
  backend.judgmentUsage = {
    providerIdentity: backend.typesafeSettings().providerIdentity, observedAtMs: Date.now(), scope: "machine", coverage: "window_requests",
    state: "observed", reason: null, unattributedNewer: false,
    lastRequest: { feature: "stall", atMs, outcome: "answered", elapsedMs: 37, applied: true },
  };
  const words = (key, fallback) => page.evaluate(([name, source]) => t(name, source), [key, fallback]);
  await page.locator("#model-advanced").evaluate((fold) => { fold.open = true; });
  await page.click("#judgment-status-refresh");
  await page.waitForFunction(() => !document.getElementById("judgment-status-refresh").disabled);
  assertEqual(await page.locator("#judgment-current-usage").getAttribute("data-state"), "observed", "a real request remained permanently unknown");
  const feature = await page.evaluate(() => { const name = jevFeature("stall").name; return t(name.key, name.source); });
  const when = await page.evaluate((at) => new Intl.DateTimeFormat(document.documentElement.lang || "ko", { dateStyle: "short", timeStyle: "medium" }).format(new Date(at)), atMs);
  const used = await page.locator("#judgment-current-usage").textContent();
  assert(used.includes(feature) && used.includes(when), "the real feature and source timestamp were not displayed", used);
  assert(used.includes(await words("settings.judgment.usageApplied", "결과 적용됨")), "an explicitly applied result was not distinguished");
  backend.judgmentUsage.lastRequest = { feature: "stall", atMs, outcome: "http_503", elapsedMs: 0, applied: false };
  backend.judgmentUsage.unattributedNewer = true;
  await page.click("#judgment-status-refresh");
  await page.waitForFunction(() => !document.getElementById("judgment-status-refresh").disabled);
  const failed = await page.locator("#judgment-current-usage").textContent();
  assert(failed.includes(await words("settings.judgment.usageFailed", "요청 실패")), "HTTP failure was hidden");
  assert(failed.includes(await words("settings.judgment.usageNotApplied", "결과 미적용")), "a failed request was presented as an applied result");
  assert(failed.includes(await words("settings.judgment.usageNewerUnknown", "더 새 기록에는 요청 당시 모델 정보가 없어 결과를 확인할 수 없습니다.")), "a newer unattributed request was hidden");
  assert(failed.includes("0 ms"), "the source's measured zero latency was replaced with an invented value");
  assertEqual(await page.locator(JUDGMENT_STATE).getAttribute("data-model-state"), "running", "a request failure was presented as a stopped server");
  return "real source time and feature · explicit application · HTTP failure is not server shutdown · newer history gap disclosed";
}));

await test("로컬 판단 서버가 거절한 이유는 카드에 그대로 보이고 HTTP 상태로 바뀌지 않는다", async () => withJudgmentStatus(async (page) => {
  const library = await readFile(resolve(UI, "..", "crates", "judgment-provider", "src", "lib.rs"), "utf8");
  const tokens = library.slice(library.indexOf("pub const fn token(self)"));
  const refusals = new Map([...tokens.slice(0, tokens.indexOf("\n    }")).matchAll(/Self::(\w+) => "(\w+)"/g)].map(([, variant, word]) => [variant, word]));
  assert(refusals.has("DeadlineBudget") && refusals.has("SettingsChanged"), "judgment-provider's refusal words were not read", [...refusals]);
  const words = (key, fallback) => page.evaluate(([name, said]) => t(name, said), [key, fallback]);
  const failed = await words("settings.judgment.usageFailed", "요청 실패");
  const discarded = await words("settings.judgment.usageDiscarded", "설정 변경으로 응답 미사용");
  await page.locator("#model-advanced").evaluate((fold) => { fold.open = true; });
  const unsaid = [];
  for (const outcome of refusals.values()) {
    backend.judgmentUsage = {
      providerIdentity: backend.typesafeSettings().providerIdentity, observedAtMs: Date.now(), scope: "machine", coverage: "window_requests",
      state: "observed", reason: null, unattributedNewer: false,
      lastRequest: { feature: "stall", atMs: Date.parse("2026-10-01T10:20:30Z"), outcome, elapsedMs: 41, applied: false },
    };
    await page.click("#judgment-status-refresh");
    await page.waitForFunction(() => !document.getElementById("judgment-status-refresh").disabled);
    const used = await page.locator("#judgment-current-usage").textContent();
    const reason = await page.evaluate((token) => { const row = jevTokenRow(token); return row ? t(row.key, row.word) : null; }, outcome);
    const said = outcome === refusals.get("SettingsChanged") ? used.includes(discarded) : reason !== null && used.includes(`${failed} · ${reason}`);
    if (!said || used.includes("(HTTP")) unsaid.push({ outcome, used });
  }
  assertEqual(unsaid, [], "a local server's refusal reached the card without its reason, or as an HTTP status");
  return `${refusals.size} refusal words read from judgment-provider · each shown with its reason · none as an HTTP status`;
}));

await test("실제 사용은 최근 미관측·출처 부족·다른 제공자·지원 없는 명령을 구분한다", async () => withJudgmentStatus(async (page) => {
  backend.judgmentUsage = {
    providerIdentity: backend.typesafeSettings().providerIdentity, observedAtMs: Date.now(), scope: "machine", coverage: "window_requests",
    state: "none_observed", reason: null, unattributedNewer: false, lastRequest: null,
  };
  await page.locator("#model-advanced").evaluate((fold) => { fold.open = true; });
  await page.click("#judgment-status-refresh");
  await page.waitForFunction(() => !document.getElementById("judgment-status-refresh").disabled);
  assertEqual(await page.locator("#judgment-current-usage").getAttribute("data-state"), "none_observed");
  assert(!/\d/.test(await page.locator("#judgment-current-usage").textContent()), "bounded non-observation invented a total or last-used time");
  backend.judgmentUsage.state = "unknown";
  backend.judgmentUsage.reason = "unattributed";
  await page.click("#judgment-status-refresh");
  await page.waitForFunction(() => !document.getElementById("judgment-status-refresh").disabled);
  assertEqual(await page.locator("#judgment-current-usage").getAttribute("data-state"), "unknown");
  backend.judgmentUsage = {
    ...backend.judgmentUsage, providerIdentity: "another-provider", state: "observed", reason: null,
    lastRequest: { feature: "stall", atMs: Date.now() - 60_000, outcome: "answered", elapsedMs: 31, applied: true },
  };
  await page.click("#judgment-status-refresh");
  await page.waitForFunction(() => !document.getElementById("judgment-status-refresh").disabled);
  assertEqual(await page.locator("#judgment-current-usage").getAttribute("data-state"), "unknown", "old provider history became current use");
  backend.judgmentUsageError = "command_not_supported";
  await page.click("#judgment-status-refresh");
  await page.waitForFunction(() => !document.getElementById("judgment-status-refresh").disabled);
  assertEqual(await page.locator("#judgment-current-usage").getAttribute("data-state"), "unknown", "unsupported usage reporting fabricated activity");
  assertEqual(await page.locator(JUDGMENT_STATE).getAttribute("data-model-state"), "running");
  return "recent evidence only · unknown legacy provenance · different provider discarded · old backend tolerated";
}));

await test("실제 요청에 없는 시각과 적용 여부는 만들어 내지 않는다", async () => withJudgmentStatus(async (page) => {
  backend.judgmentUsage = {
    providerIdentity: backend.typesafeSettings().providerIdentity, observedAtMs: Date.now(), scope: "machine", coverage: "window_requests",
    state: "observed", reason: null, unattributedNewer: false,
    lastRequest: { feature: "stall", atMs: null, outcome: "answered", elapsedMs: null, applied: null },
  };
  await page.locator("#model-advanced").evaluate((fold) => { fold.open = true; });
  await page.click("#judgment-status-refresh");
  await page.waitForFunction(() => !document.getElementById("judgment-status-refresh").disabled);
  assertEqual(await page.locator("#judgment-current-usage").getAttribute("data-state"), "observed");
  const used = await page.locator("#judgment-current-usage").textContent();
  const missing = await page.evaluate(() => [t("settings.judgment.usageTimeUnknown", "시각 정보 없음"), t("settings.judgment.usageAppliedUnknown", "결과 적용 여부 모름")]);
  assert(missing.every((words) => used.includes(words)), "missing source facts were inferred", used);
  assert(!/\d/.test(used), "a missing timestamp or latency became an invented number", used);
  return "known real request · unknown source timestamp/latency/application kept unknown";
}));

await test("가져오기 실패는 해당 초안 옆에 한 번만 표시한다", async () => withJudgmentStatus(async (page, provider) => {
  await page.evaluate(() => {
    judgmentModelSourceController.kind = "local_directory";
    judgmentModelSourceController.directory = "/models/unsupported";
    judgmentModelSourceController.invalidate();
  });
  const snapshot = backend.managedJudgmentSettings();
  backend.managedSourceError = "not_native_decision_model";
  const current = await page.locator("#model-judgment").textContent();
  await page.click("#judgment-source-inspect");
  await page.waitForFunction(() => !judgmentModelSourceController.busy);
  snapshot.revision += 1;
  snapshot.operation = { id: "unsupported-inspection", kind: "inspect", state: "failed", error: "not_native_decision_model", receivedBytes: 0, totalBytes: 0 };
  await backend.emitManaged(snapshot);
  const error = await page.evaluate(() => managedJudgmentController.failure("not_native_decision_model"));
  assertEqual(await page.locator("#judgment-source-status").textContent(), error);
  assertEqual(await page.locator("#managed-judgment-status").textContent(), "", "the import warning leaked into model management");
  assertEqual(await page.locator("#model-judgment").textContent(), current, "an unsupported import changed active status");
  assertEqual(backend.typesafeSettings().provider, provider);
  await page.click("#judgment-source-hub");
  assertEqual(await page.locator("#judgment-source-status").textContent(), "", "an import warning followed a different draft");
  return "one local input warning · current provider preserved · warning cleared for another source";
}));

await test("주소를 붙여 넣으면 정보만 확인하고 실행 검증 전 모델은 누른 뒤에 파일만 받는다", async () => withJudgmentStatus(async (page, provider) => {
  backend.managedSourceResult = nativeSourceInspection(backend.managedJudgmentSettings());
  const address = "https://huggingface.co/acme/native-decision";
  const current = await page.locator("#model-judgment").textContent();
  const words = await page.evaluate(() => ({
    absent: t("settings.models.badgeNotInstalled", "아직 받지 않음"),
    present: t("settings.models.badgeInstalled", "파일 준비됨"),
    readiness: t("settings.models.badgeDownloadOnly", "파일만 받기 · 실행 검증 전"),
    action: t("settings.models.downloadOnly", "파일만 받기"),
    kept: t("settings.models.keptOnly", "모델 파일을 받아 검증했습니다. 이 컴퓨터의 직접 실행은 아직 검증되지 않아 현재 모델은 그대로입니다."),
    library: t("settings.managed.filesReady", "파일 준비됨 · 이 컴퓨터의 직접 실행은 아직 검증되지 않았습니다. 현재 모델은 바뀌지 않습니다."),
  }));
  const pastedAt = backend.calls.length;
  await page.evaluate((text) => {
    const input = document.getElementById("judgment-source-repository");
    input.focus();
    input.value = text;
    input.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertFromPaste" }));
  }, address);
  const inspected = await backend.waitForCall("judgment-status", "managed_judgment_inspect_source", pastedAt);
  assertEqual(inspected.args.source, { kind: "hugging_face", repository: address }, "a pasted address was not inspected");
  await page.waitForFunction(() => judgmentModelSourceController.inspection?.readyForDownload && !judgmentModelSourceController.inspecting);
  assert(!backend.calls.slice(pastedAt).some((entry) => entry.command !== "managed_judgment_inspect_source" && entry.command.startsWith("managed_judgment_") && entry.command !== "managed_judgment_status"),
    "pasting an address did more than read its metadata", backend.calls.slice(pastedAt).map((entry) => entry.command));
  assert(await page.locator("#judgment-source-result").isVisible(), "the inspected address has no result row");
  assertEqual(await page.locator("#judgment-source-model-name").textContent(), "Native Decision Lite");
  assertEqual(await page.locator("#judgment-source-badge-installed").textContent(), words.absent);
  assertEqual(await page.locator("#judgment-source-badge-readiness").textContent(), words.readiness);
  assert(await page.locator("#judgment-source-badge-current").isHidden(), "a model not in use was marked current");
  assertEqual(await page.locator("#judgment-source-use").textContent(), words.action, "an unverified runtime offered to run the model");
  assert(await page.locator("#judgment-source-use").isEnabled(), "the download-only action is a dead end");
  assertEqual(await page.locator(".judgment-model-source .btn--primary:visible").count(), 1, "the result row has competing primary actions");
  const usingAt = backend.calls.length;
  await page.click("#judgment-source-use");
  const installed = await backend.waitForCall("judgment-status", "managed_judgment_install_source", usingAt);
  assertEqual(installed.args, { inspectionId: "fixture-native-inspection", instanceId: "fixture-instance", expectedCatalogFingerprint: "fixture-catalog" });
  await page.waitForFunction((text) => document.getElementById("judgment-source-status").textContent === text, words.kept);
  const steps = await page.locator("#judgment-source-steps li").evaluateAll((items) => items.map((item) => [item.dataset.step, item.dataset.state]));
  assertEqual(steps, [["inspect", "done"], ["download", "done"], ["verify", "done"], ["keep", "done"]], "the download-only steps are not named");
  const commands = backend.calls.slice(usingAt).filter((entry) => entry.window_id === "judgment-status").map((entry) => entry.command);
  assert(!commands.some((command) => ["managed_judgment_prepare_model_runtime", "managed_judgment_prepare_runtime", "managed_judgment_start", "managed_judgment_switch_model", "managed_judgment_check", "managed_judgment_apply"].includes(command)),
    "a download-only model reached runtime preparation, execution or application", commands);
  assertEqual(backend.typesafeSettings().provider, provider, "keeping files changed the active model");
  assertEqual(await page.locator("#model-judgment").textContent(), current, "keeping files changed the status card");
  assertEqual(await page.locator("#judgment-source-badge-installed").textContent(), words.present);
  assert(await page.locator("#judgment-source-badge-current").isHidden(), "kept files looked applied");
  assert(await page.locator("#judgment-source-use").isHidden(), "kept files still offered an action");
  assertEqual(await page.evaluate(() => managedJudgmentController.rows.get("fixture-native")?.state.textContent), words.library, "the library row hid that the runtime is not verified");
  return "paste reads metadata only · one explicit download-only action · named steps · no runtime/start/check/apply · active model kept";
}));

await test("늦게 끝난 모델 확인은 새 주소의 결과와 진행 중 표시를 덮어쓰지 않는다", async () => withJudgmentStatus(async (page, provider) => {
  const snapshot = backend.managedJudgmentSettings();
  let finishStale;
  let finishFresh;
  backend.managedSourceWait = new Promise((resolve) => { finishStale = resolve; });
  const staleAt = backend.calls.length;
  await page.fill("#judgment-source-repository", "acme/stale-model");
  await page.press("#judgment-source-repository", "Enter");
  await backend.waitForCall("judgment-status", "managed_judgment_inspect_source", staleAt);
  await page.waitForFunction(() => judgmentModelSourceController.inspecting);
  backend.managedSourceResult = nativeSourceInspection(snapshot, { inspectionId: "fresh-inspection", label: "Fresh Native Model" });
  backend.managedSourceWait = new Promise((resolve) => { finishFresh = resolve; });
  const freshAt = backend.calls.length;
  await page.fill("#judgment-source-repository", "acme/fresh-model");
  await page.press("#judgment-source-repository", "Enter");
  finishStale(nativeSourceInspection(snapshot, { inspectionId: "stale-inspection", label: "Stale Native Model" }));
  const fresh = await backend.waitForCall("judgment-status", "managed_judgment_inspect_source", freshAt);
  assertEqual(fresh.args.source, { kind: "hugging_face", repository: "acme/fresh-model" });
  assert(await page.evaluate(() => judgmentModelSourceController.inspecting), "the stale inspection ended the newer one's progress");
  assert(await page.evaluate(() => judgmentModelSourceController.inspection === null), "a stale inspection became the selected result");
  assert((await page.locator("#judgment-source-model-name").textContent()) !== "Stale Native Model", "a stale inspection was drawn");
  finishFresh(backend.managedSourceResult);
  await page.waitForFunction(() => judgmentModelSourceController.inspection?.inspectionId === "fresh-inspection" && !judgmentModelSourceController.inspecting);
  assertEqual(await page.locator("#judgment-source-model-name").textContent(), "Fresh Native Model");
  assertEqual(await page.locator("#judgment-source-repository").inputValue(), "acme/fresh-model", "a stale inspection rewrote the address");
  assert(await page.locator("#judgment-source-repository").isEnabled(), "the address stayed locked after the newer inspection");
  assertEqual(backend.typesafeSettings().provider, provider);
  return "address editable while inspecting · stale result and stale busy ending ignored · newest result shown";
}));

await test("받는 중 취소하면 현재 모델을 지키고 같은 결과 줄에서 다시 시도한다", async () => withJudgmentStatus(async (page, provider) => {
  backend.managedSourceResult = nativeSourceInspection(backend.managedJudgmentSettings());
  backend.managedPendingKinds = new Set(["download"]);
  try {
    const words = await page.evaluate(() => ({
      cancelled: managedJudgmentController.failure("cancelled"),
      kept: t("settings.models.keptOnly", "모델 파일을 받아 검증했습니다. 이 컴퓨터의 직접 실행은 아직 검증되지 않아 현재 모델은 그대로입니다."),
    }));
    await page.fill("#judgment-source-repository", "acme/native-decision");
    await page.press("#judgment-source-repository", "Enter");
    await page.waitForFunction(() => judgmentModelSourceController.inspection?.readyForDownload && !judgmentModelSourceController.inspecting);
    const usingAt = backend.calls.length;
    await page.click("#judgment-source-use");
    await backend.waitForCall("judgment-status", "managed_judgment_install_source", usingAt);
    await page.waitForFunction(() => managedJudgmentController.snapshot?.operation?.state === "running");
    const active = await page.locator('#judgment-source-steps li[data-state="active"]').evaluateAll((items) => items.map((item) => item.dataset.step));
    assertEqual(active, ["download"], "the running step is not shown next to the row");
    assert(await page.locator("#judgment-source-progress").isVisible(), "the row shows no download progress");
    assert(await page.locator("#judgment-source-cancel").isVisible(), "the row offers no cancellation");
    await page.click("#judgment-source-cancel");
    await backend.waitForCall("judgment-status", "managed_judgment_cancel", usingAt);
    await page.waitForFunction((text) => document.getElementById("judgment-source-status").textContent === text, words.cancelled);
    const failed = await page.locator('#judgment-source-steps li[data-state="failed"]').evaluateAll((items) => items.map((item) => item.dataset.step));
    assertEqual(failed, ["download"], "the cancelled step is not marked");
    assertEqual(backend.typesafeSettings().provider, provider, "cancelling changed the active model");
    assert(await page.locator("#judgment-source-retry").isVisible(), "a cancelled download left no retry");
    backend.managedPendingKinds.delete("download");
    backend.managedSourceResult = nativeSourceInspection(backend.managedJudgmentSettings(), { inspectionId: "retry-inspection" });
    const retryAt = backend.calls.length;
    await page.click("#judgment-source-retry");
    const reinspected = await backend.waitForCall("judgment-status", "managed_judgment_inspect_source", retryAt);
    assertEqual(reinspected.args.source, { kind: "hugging_face", repository: "acme/native-decision" });
    assert(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(reinspected.args.requestId ?? ""), "the retried inspection has no request correlation");
    const retried = await backend.waitForCall("judgment-status", "managed_judgment_install_source", retryAt);
    assertEqual(retried.args.inspectionId, "retry-inspection", "retry reused a spent inspection");
    await page.waitForFunction((text) => document.getElementById("judgment-source-status").textContent === text, words.kept);
    assertEqual(backend.typesafeSettings().provider, provider);
    assert(!backend.calls.slice(usingAt).some((entry) => ["managed_judgment_start", "managed_judgment_apply"].includes(entry.command)), "a download-only retry reached execution");
  } finally {
    delete backend.managedPendingKinds;
  }
  return "step progress beside the row · cancel keeps the active model · retry re-inspects then keeps files";
}));

await test("지원하지 않는 주소는 결과 줄에서 이유와 다음 행동을 보여 준다", async () => withJudgmentStatus(async (page, provider) => {
  backend.managedSourceError = "source_invalid";
  const reason = await page.evaluate(() => managedJudgmentController.failure("source_invalid"));
  const at = backend.calls.length;
  await page.fill("#judgment-source-repository", "https://example.com/models/decision");
  await page.press("#judgment-source-repository", "Enter");
  await backend.waitForCall("judgment-status", "managed_judgment_inspect_source", at);
  await page.waitForFunction((text) => document.getElementById("judgment-source-status").textContent === text, reason);
  assert(await page.locator("#judgment-source-result").isVisible(), "an unsupported address left no result row");
  assertEqual(await page.locator("#judgment-source-result").getAttribute("data-state"), "unsupported");
  assert(await page.locator("#judgment-source-use").isHidden(), "an unsupported address kept a dead action");
  assert(await page.locator("#judgment-source-retry").isVisible(), "an unsupported address offers no retry");
  assert(await page.locator("#judgment-source-folder-instead").isVisible(), "an unsupported address offers no other way in");
  assertEqual(backend.typesafeSettings().provider, provider);
  delete backend.managedSourceError;
  await page.click("#judgment-source-folder-instead");
  await page.waitForFunction(() => judgmentModelSourceController.kind === "local_directory" && !judgmentModelSourceController.busy);
  assert(await page.locator("#judgment-source-result").isHidden(), "the unsupported address followed the folder choice");
  return "reason in the result row · retry and folder route · active model kept";
}));

await test("실행 검증 전 카탈로그 후보는 주소 확인으로 같은 결과 줄에 불러온다", async () => withJudgmentStatus(async (page) => {
  const native = nativeSourceInspection(backend.managedJudgmentSettings()).model;
  backend.managedModelState.models.push({ ...native, origin: "bundled" });
  backend.managedModelState.revision += 1;
  backend.managedSourceResult = nativeSourceInspection(backend.managedJudgmentSettings());
  await backend.emitManaged(backend.managedJudgmentSettings());
  assertEqual(await page.locator("#managed-judgment-candidate-list").count(), 1, "the catalog models not received have no list of their own");
  await page.waitForFunction(() => managedJudgmentController.candidates.has("fixture-native"));
  assert(!(await page.evaluate(() => managedJudgmentController.rows.has("fixture-native"))), "a model whose files were never received stood as a library row");
  const candidate = page.locator('#managed-judgment-candidate-list [data-model-id="fixture-native"]');
  const button = candidate.locator('button[data-model-action="inspect-source"]');
  assert(!(await button.evaluate((element) => element.hidden || element.disabled)), "the candidate has no way to its download-only action");
  assert(await candidate.locator('button[data-model-action="install"]').evaluate((element) => element.hidden), "a download-only candidate offers a download that cannot run");
  const at = backend.calls.length;
  await button.evaluate((element) => element.click());
  const inspected = await backend.waitForCall("judgment-status", "managed_judgment_inspect_source", at);
  assertEqual(inspected.args.source, { kind: "hugging_face", repository: native.source });
  assertEqual(await page.locator("#judgment-source-repository").inputValue(), native.source);
  await page.waitForFunction(() => judgmentModelSourceController.inspection?.readyForDownload && !judgmentModelSourceController.inspecting);
  assert(!backend.calls.slice(at).some((entry) => ["managed_judgment_install", "managed_judgment_install_source"].includes(entry.command)), "the candidate downloaded before the person chose to");
  return "candidate, not a row · address check only · routes into the same inspected row · no download before the explicit action";
}));

await test("주소를 고쳐도 다른 요청이 시작한 모델 확인을 취소하지 않는다", async () => withJudgmentStatus(async (page, provider) => {
  let finish;
  backend.managedSourceResult = nativeSourceInspection(backend.managedJudgmentSettings());
  backend.managedSourceWait = new Promise((resolve) => { finish = resolve; });
  await page.evaluate(() => {
    const original = window.__SETTINGS_INVOKE__;
    window.__SOURCE_CANCEL_IDS__ = [];
    window.__SETTINGS_INVOKE__ = (payload) => {
      if (payload.command === "managed_judgment_cancel") window.__SOURCE_CANCEL_IDS__.push(payload.args.operationId);
      return original(payload);
    };
  });
  try {
    const from = backend.calls.length;
    await page.fill("#judgment-source-repository", "acme/own-source");
    await page.press("#judgment-source-repository", "Enter");
    await backend.waitForCall("judgment-status", "managed_judgment_inspect_source", from);
    const held = backend.managedModelState;
    held.revision += 1;
    held.operation = { id: "foreign-inspection", kind: "inspect", state: "running", sourceRequestId: "00000000-0000-4000-8000-000000000002", receivedBytes: 0, totalBytes: 0 };
    await backend.emitManaged(clone(held));
    await page.fill("#judgment-source-repository", "acme/new-source");
    assertEqual(await page.evaluate(() => window.__SOURCE_CANCEL_IDS__), [], "editing the address cancelled another caller's inspection");
    assertEqual(backend.typesafeSettings().provider, provider);
  } finally { finish(backend.managedSourceResult); }
  return "foreign inspection is not cancelled · current model is kept";
}));

await test("확인 취소는 자기 요청의 시작 통지가 늦어도 그 작업에만 한 번 보낸다", async () => withJudgmentStatus(async (page, provider) => {
  let finish;
  backend.managedSourceResult = nativeSourceInspection(backend.managedJudgmentSettings());
  backend.managedSourceWait = new Promise((resolve) => { finish = resolve; });
  try {
    const from = backend.calls.length;
    await page.fill("#judgment-source-repository", "acme/own-source");
    await page.press("#judgment-source-repository", "Enter");
    const request = await backend.waitForCall("judgment-status", "managed_judgment_inspect_source", from);
    assert(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(request.args.requestId ?? ""), "inspection has no request correlation");
    await page.click("#judgment-source-cancel");
    assert(!backend.calls.slice(from).some((entry) => entry.command === "managed_judgment_cancel"), "cancel guessed an operation before its own report");
    const held = backend.managedModelState;
    held.revision += 1;
    held.operation = { id: "owned-inspection", kind: "inspect", state: "running", sourceRequestId: request.args.requestId, receivedBytes: 0, totalBytes: 0 };
    const announcement = clone(held);
    await backend.emitManaged(announcement);
    const cancelled = await backend.waitForCall("judgment-status", "managed_judgment_cancel", from);
    assertEqual(cancelled.args, { operationId: "owned-inspection" });
    await backend.emitManaged(announcement);
    assertEqual(backend.calls.slice(from).filter((entry) => entry.command === "managed_judgment_cancel").length, 1);
    assertEqual(backend.typesafeSettings().provider, provider);
  } finally { finish(backend.managedSourceResult); }
  return "request correlation · late owned start is cancelled once · active model kept";
}));

await test("파일 받기 시작 응답 전에는 다른 작업의 진행과 취소를 가져오지 않는다", async () => withJudgmentStatus(async (page, provider) => {
  let refuse;
  backend.managedSourceResult = nativeSourceInspection(backend.managedJudgmentSettings());
  backend.managedSourceInstallWait = new Promise((resolve, reject) => { refuse = reject; });
  await page.evaluate(() => {
    const original = window.__SETTINGS_INVOKE__;
    window.__SOURCE_CANCEL_IDS__ = [];
    window.__SETTINGS_INVOKE__ = (payload) => {
      if (payload.command === "managed_judgment_cancel") window.__SOURCE_CANCEL_IDS__.push(payload.args.operationId);
      return original(payload);
    };
  });
  try {
    await page.fill("#judgment-source-repository", "acme/native-decision");
    await page.press("#judgment-source-repository", "Enter");
    await page.waitForFunction(() => judgmentModelSourceController.inspection?.readyForDownload && !judgmentModelSourceController.inspecting);
    const from = backend.calls.length;
    await page.click("#judgment-source-use");
    await backend.waitForCall("judgment-status", "managed_judgment_install_source", from);
    const held = backend.managedModelState;
    held.revision += 1;
    held.operation = { id: "foreign-download", kind: "download", modelId: "fixture-native", state: "running", receivedBytes: 512, totalBytes: 1024 };
    await backend.emitManaged(clone(held));
    await page.evaluate(() => judgmentModelSourceController.cancel());
    assertEqual(await page.evaluate(() => window.__SOURCE_CANCEL_IDS__), [], "pending install adopted another operation's cancel target");
    assertEqual(await page.locator("#judgment-source-progress").getAttribute("value"), null, "foreign byte progress was shown as ours");
    assertEqual(backend.typesafeSettings().provider, provider);
  } finally { refuse("operation_busy"); }
  return "no operation authority before start reply · no foreign progress or cancellation";
}));

await test("선택된 모델이 멈췄으면 다시 실행하고 이미 받은 파일은 재설치하지 않는다", async () => withJudgmentStatus(async (page) => {
  const held = backend.managedModelState;
  held.server = { id: null, state: "stopped", modelId: null, provider: null, checked: false, error: null };
  held.operation = null;
  held.revision += 1;
  const model = clone(held.models[0]);
  model.installed = true;
  model.local = { supported: true, downloadOnly: false, reason: null };
  model.runtime = { state: "ready", downloadBytes: 0, reason: null };
  backend.managedSourceResult = {
    inspectionId: "installed-inspection", instanceId: held.instanceId, catalogFingerprint: held.catalogFingerprint,
    model, local: model.local, readyForInstall: true, readyForDownload: true, reason: null,
    sourceKind: "hugging_face", downloadBytes: 0, runtimeDownloadBytes: 0, allowsDestinationSelection: false,
  };
  await backend.emitManaged(clone(held));
  await page.fill("#judgment-source-repository", "acme/installed-model");
  await page.press("#judgment-source-repository", "Enter");
  await page.waitForFunction(() => judgmentModelSourceController.inspection?.inspectionId === "installed-inspection" && !judgmentModelSourceController.inspecting);
  assert(await page.locator("#judgment-source-badge-current").isVisible());
  assert(await page.locator("#judgment-source-use").isVisible(), "selection hid the action to start the stopped server");
  assert(await page.locator("#judgment-source-use").isEnabled());
  const label = await page.locator("#judgment-source-badge-current").textContent();
  assert(!["In use", "현재 사용 중", "使用中", "正在使用", "En uso"].includes(label), "selected-but-stopped was labelled in use");
  const from = backend.calls.length;
  await page.click("#judgment-source-use");
  await backend.waitForCall("judgment-status", "managed_judgment_apply", from);
  const commands = backend.calls.slice(from).map((entry) => entry.command);
  assert(!commands.some((command) => ["managed_judgment_install_source", "managed_judgment_install", "managed_judgment_prepare_runtime", "managed_judgment_prepare_model_runtime"].includes(command)), "already installed files/runtime were installed again", commands);
  assertEqual(commands.filter((command) => command === "managed_judgment_start").length, 1);
  assertEqual(commands.filter((command) => command === "managed_judgment_check").length, 1);
  return "selection is not running · restart from the same row · no repeated installation";
}));

await test("판단 상태는 다섯 언어·두 테마·좁은 폭에서 읽을 수 있다", async () => withJudgmentStatus(async (page) => {
  backend.judgmentUsage = {
    providerIdentity: backend.typesafeSettings().providerIdentity, observedAtMs: Date.now(), scope: "machine", coverage: "window_requests",
    state: "observed", reason: null, unattributedNewer: true,
    lastRequest: { feature: "stall", atMs: Date.parse("2026-10-01T10:20:30Z"), outcome: "http_503", elapsedMs: 0, applied: null },
  };
  await page.locator("#model-advanced").evaluate((fold) => { fold.open = true; });
  await page.click("#judgment-status-refresh");
  await page.waitForFunction(() => !document.getElementById("judgment-status-refresh").disabled);
  assertEqual(await page.locator("#judgment-current-usage").getAttribute("data-state"), "observed");
  for (const locale of SETTINGS_QUALITY_SPEC.locales) {
    await setQualityLocale(page, locale);
    await page.evaluate(() => judgmentProviderController.controls());
    for (const theme of SETTINGS_QUALITY_SPEC.themes) {
      await setQualityTheme(page, theme);
      assertEqual(await axeViolations(page, AxeBuilder, "#model-judgment"), [], `${locale}/${theme} current judgment accessibility`);
      const usageWidth = await page.locator("#judgment-current-usage").evaluate((usage) => ({ row: usage.getBoundingClientRect().width, item: usage.closest(".model-advanced-item").getBoundingClientRect().width }));
      assert(Math.abs(usageWidth.row - usageWidth.item) <= 1, `${locale}/${theme} request history should use the item's full width`, usageWidth);
      if (process.env.JUDGMENT_STATUS_SHOTS && locale === "ko") {
        const status = page.locator("#model-judgment");
        await status.scrollIntoViewIfNeeded();
        await status.screenshot({ path: resolve(process.env.JUDGMENT_STATUS_SHOTS, `judgment-status-${theme}.png`), animations: "disabled" });
      }
    }
    await page.setViewportSize({ width: 980, height: 1069 });
    const size = await page.locator("#model-judgment").evaluate((status) => ({ client: status.clientWidth, scroll: status.scrollWidth }));
    assert(size.client > 0 && size.scroll <= size.client + 1, `${locale} current judgment status overflows`, size);
    const untranslated = await page.locator("#model-judgment").evaluate((status) => [...status.querySelectorAll("[data-i18n]")].filter((node) => !node.textContent.trim()).map((node) => node.dataset.i18n));
    assertEqual(untranslated, [], `${locale} empty judgment status label`);
  }
  await page.locator("#judgment-status-refresh").focus();
  const before = backend.calls.length;
  await page.keyboard.press("Enter");
  await backend.waitForCall("judgment-status", "typesafe_settings", before);
  await page.waitForFunction(() => !document.getElementById("judgment-status-refresh").disabled);
  return "five languages · light/dark · narrow layout · keyboard refresh";
}));

await test("판단 모델은 명시적으로 적용하고 로컬 연결은 Jev 키를 쓰지 않는다", async () => {
  await openJudgmentSettings(pageA);
  const previous = backend.zoSettings.smart === undefined ? undefined : clone(backend.zoSettings.smart);
  const previousCheck = clone(backend.typesafeCheck);
  backend.zoSettings.smart = { [JEV_MODEL_SETTING]: "jev-held", [JEV_SETTINGS_KEY]: { enabled: false, workspaces: ["/work/acme"] } };
  backend.keychain.set(TYPESAFE_SERVICE, "cloud-fixture");
  try {
    await pageA.evaluate(() => refreshApiRouters());
    await pageA.selectOption("#judgment-provider-kind", "local");
    await pageA.click("#judgment-local-manual");
    await pageA.fill("#judgment-endpoint", "http://127.0.0.1:1234");
    await pageA.fill("#typesafe-model-input", "acme/new-decision-model");
    assert(await pageA.locator("#typesafe-check-btn").isEnabled(), "a complete draft cannot be checked without replacing the active provider");
    assert(await pageA.locator("#typesafe-key-input").isHidden(), "an anonymous local server asked for a key");
    assertEqual(backend.typesafeSettings().provider.kind, "jev", "typing silently changed the active provider");
    const listingAt = backend.calls.length;
    await pageA.click("#judgment-refresh-models");
    const listing = await backend.waitForCall("A", "judgment_models", listingAt);
    assertEqual(listing.args.provider.model, "acme/new-decision-model");
    await pageA.waitForFunction(() => document.querySelectorAll("#judgment-model-options option").length === 2);
    assertEqual(await pageA.inputValue("#typesafe-model-input"), "acme/new-decision-model", "refresh replaced the selected ID");
    const appliedAt = backend.calls.length;
    await pageA.click("#judgment-save-provider");
    await backend.waitForCall("A", "set_judgment_provider", appliedAt);
    await pageA.waitForFunction(() => !document.getElementById("typesafe-check-btn").disabled);
    const provider = backend.typesafeSettings().provider;
    assertEqual(provider, { kind: "local", protocol: "system_one", baseUrl: "http://127.0.0.1:1234", model: "acme/new-decision-model", requiresAuth: false });
    assertEqual(backend.zoSettings.smart[JEV_MODEL_SETTING], "jev-held");
    assertEqual(backend.zoSettings.smart[JEV_SETTINGS_KEY], { enabled: false, workspaces: ["/work/acme"] });
    assertEqual(backend.keychain.get(TYPESAFE_SERVICE), "cloud-fixture");
    if (process.env.JUDGMENT_SHOTS) {
      for (const theme of ["light", "dark"]) {
        await pageA.evaluate((selected) => { document.documentElement.dataset.theme = selected; }, theme);
        await renderSettled(pageA);
        await pageA.locator("#typesafe-card").screenshot({ path: resolve(process.env.JUDGMENT_SHOTS, `judgment-local-${theme}.png`), animations: "disabled" });
      }
      await pageA.evaluate(() => { delete document.documentElement.dataset.theme; });
    }
    const identity = backend.typesafeSettings().providerIdentity;
    backend.typesafeCheck = { answered: true, model: `${identity}:actual-decision-model`, elapsedMs: 42 };
    const checkedAt = backend.calls.length;
    await pageA.click("#typesafe-check-btn");
    assertEqual((await backend.waitForCall("A", "check_typesafe_key", checkedAt)).args.providerIdentity, identity);
    await pageA.waitForFunction(() => document.querySelector("#typesafe-status .settings-status-said")?.textContent.includes("actual-decision-model"));
    assert(!(await statusSaid(pageA, "typesafe-status")).includes(identity), "a machine identity was displayed as the model");
    let finishCheck;
    backend.typesafeCheckWait = new Promise((resolve) => { finishCheck = resolve; });
    const delayedAt = backend.calls.length;
    await pageA.click("#typesafe-check-btn");
    await backend.waitForCall("A", "check_typesafe_key", delayedAt);
    backend.zoSettings.smart.judgmentProvider.model = "another-windows-selection";
    await pageA.evaluate(() => refreshTypeSafe());
    finishCheck({ answered: true, model: `${identity}:old-model`, elapsedMs: 10 });
    delete backend.typesafeCheckWait;
    await pageA.waitForFunction(() => !typesafeBusy);
    assertEqual(await pageA.locator("#typesafe-key-state").textContent(), await standingWord(pageA, "unchecked"), "a late connection check approved a different model");
    assert(!(await statusSaid(pageA, "typesafe-status")).includes("old-model"));
    await pageA.selectOption("#judgment-provider-kind", "jev");
    assertEqual(await pageA.inputValue("#typesafe-model-input"), "jev-held", "returning to Jev lost its pin");
    await pageA.click("#judgment-save-provider");
    await pageA.waitForFunction(() => document.getElementById("judgment-save-provider").disabled);
    assertEqual(backend.typesafeSettings().provider.kind, "jev");
  } finally {
    backend.zoSettings.smart = previous;
    backend.typesafeCheck = previousCheck;
    delete backend.typesafeCheckWait;
    await pageA.evaluate(() => { judgmentProviderController.draft = null; return refreshApiRouters(); });
  }
  return "Jev pin kept · anonymous local check · exact ID kept · no chat adapter";
});

await test("로컬 모델은 서버 주소 대신 다운로드 상태를 보이고 취소와 오래된 진행 이벤트를 구별한다", async () => {
  const previous = backend.zoSettings.smart === undefined ? undefined : clone(backend.zoSettings.smart);
  delete backend.managedModelState;
  backend.managedPendingKinds = new Set(["download"]);
  await openJudgmentSettings(pageA);
  await pageA.evaluate(() => { managedJudgmentController.snapshot = null; judgmentProviderController.localMode = "managed"; });
  try {
    await pageA.selectOption("#judgment-provider-kind", "local");
    await pageA.waitForFunction(() => managedJudgmentController.snapshot?.models.length > 0);
    assert(await pageA.locator("#managed-judgment-panel").isVisible(), "local selection does not offer managed models");
    assert(await pageA.locator("#judgment-endpoint").isHidden(), "local setup still starts with an empty server address");
    const applied = clone(backend.typesafeSettings().provider);
    // A model not received yet is a candidate, folded under the library (t-69998).
    assertEqual(await pageA.locator("#managed-judgment-candidates").count(), 1, "the catalog models not received have no folded line");
    await pageA.locator("#managed-judgment-candidates > summary").click();
    const downloadedAt = backend.calls.length;
    await pageA.click('#managed-judgment-candidate-list [data-model-id="fixture-decision"] [data-model-action="install"]');
    const request = await backend.waitForCall("A", "managed_judgment_install", downloadedAt);
    assertEqual(request.args, { modelId: "fixture-decision", revision: "fixture-revision" }, "download did not bind the catalog revision");
    await pageA.waitForFunction(() => !document.getElementById("managed-judgment-progress").hidden);
    const meterStyle = await pageA.locator("#managed-judgment-progress progress").evaluate((meter) => ({
      appearance: getComputedStyle(meter).appearance,
      height: meter.getBoundingClientRect().height,
      label: document.getElementById(meter.getAttribute("aria-labelledby"))?.textContent.trim(),
    }));
    assertEqual(meterStyle.appearance, "none", "download progress still uses native platform decoration");
    assert(meterStyle.height > 0 && meterStyle.label, "download progress is invisible or has no accessible label");
    const stale = clone(backend.managedModelState);
    backend.managedModelState.revision += 1;
    backend.managedModelState.operation.receivedBytes = 512;
    await backend.emitManaged(backend.managedJudgmentSettings());
    await backend.emitManaged(stale);
    assertEqual(await pageA.locator("#managed-judgment-progress progress").evaluate((meter) => meter.value), 512, "a late event rewound download progress");
    await pageA.click("#managed-judgment-cancel");
    await pageA.waitForFunction(() => document.getElementById("managed-judgment-progress").hidden);
    assertEqual(backend.typesafeSettings().provider, applied, "download or cancellation changed the applied model");
    assert(!backend.managedModelState.models[0].installed, "a cancelled download was shown as installed");
    assert(await pageA.evaluate(() => managedJudgmentController.candidates.has("fixture-decision") && !managedJudgmentController.rows.has("fixture-decision")),
      "a cancelled download left the candidates for the library");
    assert(!backend.calls.slice(downloadedAt).some((entry) => entry.command === "managed_judgment_apply"), "download silently applied a model");
  } finally {
    backend.zoSettings.smart = previous;
    delete backend.managedPendingKinds;
    await pageA.evaluate(() => {
      const fold = document.getElementById("managed-judgment-candidates");
      if (fold) fold.open = false;
      judgmentProviderController.draft = null;
      return refreshApiRouters();
    });
  }
  return "no server field · candidate's revision-bound download · stale event ignored · cancellation is not installation";
});

await test("로컬 모델은 준비와 응답 확인을 마친 같은 서버만 명시적으로 적용한다", async () => {
  const previous = backend.zoSettings.smart === undefined ? undefined : clone(backend.zoSettings.smart);
  delete backend.managedModelState;
  backend.managedJudgmentSettings();
  backend.managedModelState.runtime.state = "ready";
  backend.managedModelState.models[0].installed = true;
  backend.managedCheckFailure = true;
  await openJudgmentSettings(pageA);
  await pageA.evaluate(() => { managedJudgmentController.snapshot = null; judgmentProviderController.localMode = "managed"; });
  try {
    await pageA.selectOption("#judgment-provider-kind", "local");
    await pageA.waitForFunction(() => managedJudgmentController.snapshot?.models[0]?.installed);
    const before = clone(backend.typesafeSettings().provider);
    const failedAt = backend.calls.length;
    await pageA.click('[data-model-action="activate"]');
    await backend.waitForCall("A", "managed_judgment_check", failedAt);
    await pageA.waitForFunction(() => !managedJudgmentController.activating);
    assertEqual(backend.typesafeSettings().provider, before, "a failed response check replaced the provider");
    assert(!backend.calls.slice(failedAt).some((entry) => entry.command === "managed_judgment_apply"), "a failed check reached apply");
    const runningWords = await pageA.evaluate(() => t("settings.managed.runningUnapplied", "실행 중 · 아직 적용하지 않음"));
    assert((await pageA.locator(".managed-judgment-model").textContent()).includes(runningWords), "a running but unverified model was shown as merely installed");
    backend.managedCheckFailure = false;
    backend.managedChangeOnCheck = true;
    const changedAt = backend.calls.length;
    await pageA.click('[data-model-action="activate"]');
    await backend.waitForCall("A", "managed_judgment_apply", changedAt);
    await pageA.waitForFunction(() => !managedJudgmentController.activating);
    assertEqual(backend.typesafeSettings().provider.kind, "jev", "apply overwrote another window's selection");
    assertEqual(backend.typesafeSettings().provider.model, "jev-other-window", "the newer setting was lost");
    backend.managedChangeOnCheck = false;
    await pageA.evaluate(() => managedJudgmentController.refresh());
    const successfulAt = backend.calls.length;
    await pageA.click('[data-model-action="activate"]');
    await backend.waitForCall("A", "managed_judgment_apply", successfulAt);
    await pageA.waitForFunction(() => managedJudgmentController.snapshot?.appliedModelId === "fixture-decision" && !managedJudgmentController.activating);
    assertEqual(backend.typesafeSettings().provider.model, "fixture-decision");
    assert(await pageA.locator('[data-model-action="remove"]').isDisabled(), "the configured model can be deleted");
    await pageA.click('[data-model-action="stop"]');
    await pageA.waitForFunction(() => managedJudgmentController.snapshot?.server.state === "stopped");
    assertEqual(backend.managedModelState.configuredModelId, "fixture-decision", "stopping forgot the configured binding");
    assertEqual(backend.managedModelState.appliedModelId, null, "a stopped server is shown as running and applied");
  } finally {
    backend.zoSettings.smart = previous;
    delete backend.managedCheckFailure;
    delete backend.managedChangeOnCheck;
    await pageA.evaluate(() => { judgmentProviderController.draft = null; return refreshApiRouters(); });
  }
  return "failed check cannot apply · cross-window identity guard · explicit apply · stopped is not running";
});

await test("실행 중인 판단 모델은 원자적 전환 한 번으로 바꾸며 실패나 다른 창의 선택을 덮지 않는다", async () => {
  const previous = backend.zoSettings.smart === undefined ? undefined : clone(backend.zoSettings.smart);
  const previousManaged = backend.managedModelState === undefined ? undefined : clone(backend.managedModelState);
  const provider = { kind: "local", protocol: "system_one", baseUrl: "http://127.0.0.1:32101", model: "fixture-decision", requiresAuth: false };
  backend.zoSettings.smart = { ...(previous ?? {}), judgmentProvider: provider };
  delete backend.managedModelState;
  backend.managedJudgmentSettings();
  const held = backend.managedModelState;
  held.runtime.state = "ready";
  held.models[0].installed = true;
  held.models.push({ ...clone(held.models[0]), id: "fixture-next", label: "Next Decision", revision: "next-revision" });
  held.server = { id: "fixture-original-server", state: "ready", modelId: "fixture-decision", checked: true, error: null, provider: clone(provider) };
  held.configuredModelId = "fixture-decision";
  held.appliedModelId = "fixture-decision";
  const modelPage = await context.newPage();
  await installTauri(modelPage, "model-switch");
  attachFaultRecorder(modelPage, "model-switch");
  try {
    await bootPage(modelPage, "model-switch");
    await openSettings(modelPage, "judgment-models");
    await modelPage.locator("#model-advanced").evaluate((fold) => { fold.open = true; });
    await modelPage.waitForFunction(() => managedJudgmentController.snapshot?.appliedModelId === "fixture-decision");
    const switchingAt = backend.calls.length;
    const identity = backend.typesafeSettings().providerIdentity;
    await modelPage.click('[data-model-id="fixture-next"] [data-model-action="activate"]');
    const switched = await backend.waitForCall("model-switch", "managed_judgment_switch_model", switchingAt);
    assertEqual(switched.args, { modelId: "fixture-next", revision: "next-revision", serverId: "fixture-original-server", expectedProviderIdentity: identity, instanceId: "fixture-instance" });
    await modelPage.waitForFunction(() => !managedJudgmentController.activating && managedJudgmentController.snapshot?.appliedModelId === "fixture-next");
    const calls = backend.calls.slice(switchingAt).filter((entry) => entry.window_id === "model-switch");
    assertEqual(calls.filter((entry) => entry.command === "managed_judgment_switch_model").length, 1, "a single click switched more than once");
    assert(!calls.some((entry) => ["managed_judgment_start", "managed_judgment_stop", "managed_judgment_check", "managed_judgment_apply"].includes(entry.command)), "the UI repeated lifecycle actions owned by the atomic switch");
    const selected = clone(backend.typesafeSettings().provider);
    const server = clone(held.server);
    backend.managedSwitchFailure = true;
    const failingAt = backend.calls.length;
    await modelPage.click('[data-model-id="fixture-decision"] [data-model-action="activate"]');
    await backend.waitForCall("model-switch", "managed_judgment_switch_model", failingAt);
    await modelPage.waitForFunction(() => !managedJudgmentController.activating);
    assertEqual(backend.typesafeSettings().provider, selected, "failed switch replaced the selected provider");
    assertEqual(held.server, server, "the UI stopped or replaced the restored server");
    assertEqual(held.appliedModelId, "fixture-next");
    assert((await modelPage.locator("#managed-judgment-status").textContent()).includes("System One"), "protocol failure has no actionable explanation");
    backend.managedSwitchFailure = false;
    backend.managedSwitchOtherWindow = true;
    const staleAt = backend.calls.length;
    await modelPage.click('[data-model-id="fixture-decision"] [data-model-action="activate"]');
    await backend.waitForCall("model-switch", "managed_judgment_switch_model", staleAt);
    await modelPage.waitForFunction(() => !managedJudgmentController.activating);
    assertEqual(backend.typesafeSettings().provider.model, "jev-other-window", "stale switch replaced another window's choice");
    const errors = await modelPage.evaluate(() => ({
      changed: managedJudgmentController.failure(new Error("operation_changed")),
      timeout: managedJudgmentController.failure(new Error("operation_timeout")),
      generic: managedJudgmentController.failure(new Error("private-error-value")),
      expectedChanged: managedJudgmentController.failure("server_changed"),
      expectedTimeout: managedJudgmentController.failure("runtime_timeout"),
    }));
    assertEqual(errors.changed, errors.expectedChanged, "Error objects lose the changed-state explanation");
    assertEqual(errors.timeout, errors.expectedTimeout, "Error objects lose the timeout explanation");
    assert(!errors.generic.includes("private-error-value"), "unknown error details reached the UI");
    const sourceErrors = await modelPage.evaluate(() => {
      const codes = ["source_invalid", "source_unavailable", "source_auth_required", "source_hash_unavailable", "source_inspection_changed", "source_folder_required", "destination_unavailable", "weights_format_invalid", "manager_instance_changed", "catalog_conflict", "model_manifest_missing"];
      return codes.map((code) => ({ code, message: managedJudgmentController.failure(new Error(code)) }));
    });
    for (const { code, message } of sourceErrors) {
      assert(message && message !== errors.generic, `${code} has no actionable recovery guidance`);
      assert(!message.includes(code), `${code} leaked an internal error token`);
    }
  } finally {
    await modelPage.close();
    backend.zoSettings.smart = previous;
    backend.managedModelState = previousManaged;
    delete backend.managedSwitchFailure;
    delete backend.managedSwitchOtherWindow;
  }
  return "one guarded switch · no duplicate start/check/apply · rollback preserved · newer selection preserved";
});

await test("스스로 멈춘 판단 서버는 중지됨과 그 까닭을 다섯 언어로 모델 카드에 보인다", async () => {
  const previous = backend.zoSettings.smart === undefined ? undefined : clone(backend.zoSettings.smart);
  const previousManaged = backend.managedModelState === undefined ? undefined : clone(backend.managedModelState);
  const provider = { kind: "local", protocol: "system_one", baseUrl: "http://127.0.0.1:32101", model: "fixture-decision", requiresAuth: false };
  backend.zoSettings.smart = { ...(previous ?? {}), judgmentProvider: provider };
  delete backend.managedModelState;
  backend.managedJudgmentSettings();
  const held = backend.managedModelState;
  held.runtime.state = "ready";
  held.models[0].installed = true;
  held.configuredModelId = "fixture-decision";
  const ended = (exit) => ({
    id: "fixture-ended-server", state: "failed", modelId: "fixture-decision", provider: null, checked: false, error: "server_stopped",
    exit: { code: null, signal: null, task: null, stderr: ["fixture: last words"], ...exit },
  });
  held.server = ended({ cause: "runtime_memory_limit", code: 75 });
  const keys = ["settings.managed.stoppedReason", "settings.managed.exit.memory", "settings.managed.exit.timeout", "settings.managed.exit.timeoutTask",
    "settings.managed.exit.failed", "settings.managed.exit.code", "settings.managed.exit.signal"];
  const page = await context.newPage();
  await installTauri(page, "server-exit");
  attachFaultRecorder(page, "server-exit");
  const shown = () => page.locator('[data-model-id="fixture-decision"].managed-judgment-model').textContent();
  try {
    await bootPage(page, "server-exit");
    await openSettings(page, "judgment-models");
    await page.locator("#model-advanced").evaluate((fold) => { fold.open = true; });
    await page.waitForFunction(() => managedJudgmentController.snapshot?.server.state === "failed");
    const missing = await page.evaluate((names) => ["en", "ja", "zh", "es"].flatMap((code) => names
      .filter((key) => typeof CATALOG?.[code]?.[key] !== "string" || !CATALOG[code][key].trim()).map((key) => `${code}:${key}`)), keys);
    assertEqual(missing, [], "a stopped server's reason is untranslated");
    const said = {};
    for (const locale of SETTINGS_QUALITY_SPEC.locales) {
      await setQualityLocale(page, locale);
      await page.evaluate(() => managedJudgmentController.render());
      const words = await page.evaluate(() => ({
        line: t("settings.managed.stoppedReason", "중지됨 · {{reason}}. 실행하고 적용을 눌러 다시 시작하세요.", {
          reason: t("settings.managed.exit.memory", "메모리 한도를 넘어 실행 환경이 스스로 멈췄습니다"),
        }),
        generic: t("settings.managed.configuredStopped", "선택된 모델 · 실행되지 않음. 실행하고 적용을 눌러 다시 연결하세요."),
      }));
      const text = await shown();
      assert(text.includes(words.line), `${locale} hides why the server stopped`, { text, words });
      assert(!text.includes(words.generic), `${locale} shows a stopped server as merely not running`, text);
      assert(!text.includes("runtime_memory_limit") && !text.includes("fixture: last words"), `${locale} leaked a raw token or stderr into the card`, text);
      said[locale] = words.line;
    }
    assertEqual(new Set(Object.values(said)).size, SETTINGS_QUALITY_SPEC.locales.length, "a locale repeats another's reason");
    await setQualityLocale(page, "ko");
    for (const [exit, part, absent] of [
      [{ cause: "runtime_exited", code: 7 }, "종료 코드 7", "신호"],
      [{ cause: "runtime_exited", signal: 9 }, "신호 9", "종료 코드"],
      [{ cause: "runtime_timeout", signal: 9, task: "stall-seat" }, "stall-seat 요청이 제한 시간을 넘겨", "신호"],
      [{ cause: "runtime_timeout", signal: 9 }, "요청이 제한 시간을 넘겨 서버를 멈췄습니다", "stall-seat"],
      [{ cause: "runtime_failed", signal: 9 }, "실행 환경과 주고받지 못해", "제한 시간"],
    ]) {
      held.server = ended(exit);
      held.revision += 1;
      await backend.emitManaged(clone(held));
      await page.waitForFunction((revision) => managedJudgmentController.snapshot?.revision === revision, held.revision);
      const text = await shown();
      assert(text.includes(part) && !text.includes(absent), `the card did not say how the server ended: ${JSON.stringify(exit)}`, text);
    }
  } finally {
    await page.close();
    backend.zoSettings.smart = previous;
    backend.managedModelState = previousManaged;
  }
  return "stopped with its reason · five locales · each cause · no raw token or stderr in the card";
});

await test("다시 띄우기를 기다리는 판단 서버는 다시 띄우는 중과 그 까닭을 다섯 언어로 보이고 중지 단추를 남긴다", async () => {
  const previous = backend.zoSettings.smart === undefined ? undefined : clone(backend.zoSettings.smart);
  const previousManaged = backend.managedModelState === undefined ? undefined : clone(backend.managedModelState);
  const provider = { kind: "local", protocol: "system_one", baseUrl: "http://127.0.0.1:32101", model: "fixture-decision", requiresAuth: false };
  backend.zoSettings.smart = { ...(previous ?? {}), judgmentProvider: provider };
  delete backend.managedModelState;
  backend.managedJudgmentSettings();
  const held = backend.managedModelState;
  held.runtime.state = "ready";
  held.models[0].installed = true;
  held.configuredModelId = "fixture-decision";
  held.server = {
    id: "fixture-waiting-server", state: "starting", modelId: "fixture-decision", provider: clone(provider), checked: false, error: null,
    exit: { cause: "runtime_exited", code: 7, signal: null, task: null, stderr: ["fixture: last words"] },
  };
  const page = await context.newPage();
  await installTauri(page, "server-restart");
  attachFaultRecorder(page, "server-restart");
  const card = page.locator('[data-model-id="fixture-decision"].managed-judgment-model');
  try {
    await bootPage(page, "server-restart");
    await openSettings(page, "judgment-models");
    await page.locator("#model-advanced").evaluate((fold) => { fold.open = true; });
    await page.waitForFunction(() => managedJudgmentController.snapshot?.server.state === "starting");
    const missing = await page.evaluate(() => ["en", "ja", "zh", "es"].filter((code) =>
      typeof CATALOG?.[code]?.["settings.managed.restartingReason"] !== "string" || !CATALOG[code]["settings.managed.restartingReason"].includes("{{reason}}")));
    assertEqual(missing, [], "a waiting restart's line is untranslated");
    const said = {};
    for (const locale of SETTINGS_QUALITY_SPEC.locales) {
      await setQualityLocale(page, locale);
      await page.evaluate(() => managedJudgmentController.render());
      const words = await page.evaluate(() => ({
        line: t("settings.managed.restartingReason", "다시 띄우는 중 · {{reason}}. 중지를 누르면 다시 띄우지 않습니다.", {
          reason: t("settings.managed.exit.code", "실행 환경이 스스로 끝났습니다(종료 코드 {{code}})", { code: 7 }),
        }),
        starting: t("settings.managed.starting", "모델을 시작하는 중 · 아직 적용하지 않음"),
      }));
      const text = await card.textContent();
      assert(text.includes(words.line), `${locale} hides that the server is restarting and why`, { text, words });
      assert(!text.includes(words.starting), `${locale} shows a restart as a first start`, text);
      assert(!text.includes("fixture: last words"), `${locale} leaked stderr into the card`, text);
      said[locale] = words.line;
    }
    assertEqual(new Set(Object.values(said)).size, SETTINGS_QUALITY_SPEC.locales.length, "a locale repeats another's restart line");
    const stop = card.locator('[data-model-action="stop"]');
    assert(await stop.isVisible(), "the stop button is gone while a restart waits");
    await stop.click();
    await page.waitForFunction(() => managedJudgmentController.snapshot?.server.state === "stopped");
    assertEqual(backend.last("server-restart", "managed_judgment_stop")?.args.serverId, "fixture-waiting-server", "the stop did not name the waiting server");
  } finally {
    await page.close();
    backend.zoSettings.smart = previous;
    backend.managedModelState = previousManaged;
  }
  return "restarting with its reason · five locales · stop kept and sent for the waiting server";
});

await test("판단 모델 목록은 실제 지원 경로로 검색하고 서버 모델을 현재 설정 변경 없이 고른다", async () => {
  const previous = backend.zoSettings.smart === undefined ? undefined : clone(backend.zoSettings.smart);
  const previousManaged = backend.managedModelState === undefined ? undefined : clone(backend.managedModelState);
  delete backend.managedModelState;
  backend.managedJudgmentSettings();
  // Both models are this computer's (t-69998): the local one received, the
  // server one's files kept though it cannot run here.
  const local = backend.managedModelState.models[0];
  local.local = { supported: true, reason: null };
  local.existingServer = { supported: true, protocol: "system_one", modelId: local.id, requiresCheck: true };
  local.installed = true;
  backend.managedModelState.models.push({
    ...clone(local), id: "fixture-server-decision", label: "Example Decision Server", engine: "transformers_system_one",
    serverModelId: "acme/decision", compatibility: "unsupported", compatibilityReason: "engine_not_supported",
    local: { supported: false, downloadOnly: true, reason: "engine_not_supported" },
    existingServer: { supported: true, protocol: "system_one", modelId: "acme/decision", requiresCheck: true },
  });
  // Not received, unable to run here and with no files to keep: its server is
  // its one way in.
  backend.managedModelState.models.push({
    ...clone(local), id: "fixture-server-only", label: "Fixture Server Only", engine: "transformers_system_one", installed: false,
    serverModelId: "acme/server-only", compatibility: "unsupported", compatibilityReason: "engine_not_supported",
    local: { supported: false, downloadOnly: false, reason: "engine_not_supported" },
    existingServer: { supported: true, protocol: "system_one", modelId: "acme/server-only", requiresCheck: true },
  });
  await openJudgmentSettings(pageA);
  await pageA.evaluate(() => {
    managedJudgmentController.snapshot = null;
    managedJudgmentController.query = "";
    managedJudgmentController.filter = "all";
    judgmentProviderController.localMode = "managed";
  });
  try {
    await pageA.selectOption("#judgment-provider-kind", "local");
    await pageA.waitForFunction(() => managedJudgmentController.snapshot?.models.length === 3);
    assertEqual(await pageA.locator("#managed-judgment-search").count(), 1, "a multi-model library has no search control");
    const before = clone(backend.typesafeSettings().provider);
    const requestsAt = backend.calls.length;
    await pageA.fill("#managed-judgment-search", "EXAMPLE");
    assert(await pageA.locator('[data-model-id="fixture-decision"].managed-judgment-model').isHidden(), "search did not filter unrelated models");
    const server = pageA.locator('[data-model-id="fixture-server-decision"].managed-judgment-model');
    assert(await server.isVisible(), "search is not case-insensitive");
    assert(await server.locator('[data-model-action="activate"]').isHidden(), "an unsupported local adapter offers to run here");
    assert(await server.locator('[data-model-action="connect"]').isVisible(), "a compatible server model has no connection path");
    assertEqual(await pageA.locator('[data-model-id="fixture-server-only"].managed-judgment-model').count(), 0, "a model not received here stands as a library row");
    await pageA.fill("#managed-judgment-search", "");
    await pageA.locator("#managed-judgment-candidates > summary").click();
    const serverOnly = pageA.locator('#managed-judgment-candidate-list > [data-model-id="fixture-server-only"]');
    for (const action of ["install", "inspect-source", "import"]) {
      assert(await serverOnly.locator(`[data-model-action="${action}"]`).isHidden(), `a candidate that cannot run or be kept here offers ${action}`);
    }
    assert(await serverOnly.locator('[data-model-action="connect"]').isVisible(), "a candidate reached only through its server has no connection path");
    await pageA.fill("#managed-judgment-search", "");
    await pageA.selectOption("#managed-judgment-filter", "local");
    assert(await server.isHidden(), "the local filter includes server-only models");
    await pageA.selectOption("#managed-judgment-filter", "all");
    assert(!backend.calls.slice(requestsAt).some((entry) => entry.command.startsWith("managed_judgment_")), "filtering models performs backend work");
    await pageA.click("#judgment-local-manual");
    await pageA.fill("#judgment-endpoint", "http://127.0.0.1:32102");
    await pageA.click("#judgment-local-managed");
    await server.locator('[data-model-action="connect"]').click();
    assert(await pageA.locator("#judgment-endpoint").isVisible(), "connecting a server model did not open its connection form");
    assertEqual(await pageA.inputValue("#typesafe-model-input"), "acme/decision", "connection did not use the server model ID");
    assertEqual(await pageA.inputValue("#judgment-endpoint"), "http://127.0.0.1:32102", "selecting a model discarded the configured server address");
    assertEqual(backend.typesafeSettings().provider, before, "choosing a library model changed the applied provider");
  } finally {
    backend.zoSettings.smart = previous;
    backend.managedModelState = previousManaged;
    await pageA.evaluate(() => {
      managedJudgmentController.snapshot = null;
      managedJudgmentController.query = "";
      managedJudgmentController.filter = "all";
      judgmentProviderController.draft = null;
      document.getElementById("managed-judgment-candidates").open = false;
      return refreshApiRouters();
    });
  }
  return "search and support filters · no backend polling · no local run for an unsupported adapter · a server-only candidate offers its server alone · exact server ID and address preserved";
});

/* The settings › 판단 모델 picture the person sent on 10-22 (t-69998): the
 * bundled catalog plus the address they had checked, with only Laya received.
 * Written to `MANAGED_JUDGMENT_SHOTS` beside what the library drew, so the
 * same run makes the before and the after of a change. */
async function photographManagedLibrary(folder) {
  const previousManaged = backend.managedModelState === undefined ? undefined : clone(backend.managedModelState);
  const manifests = resolve(UI, "..", "crates", "local-judgment", "manifests");
  const bundled = [];
  for (const name of ["models.json", "native-models.json"]) bundled.push(...JSON.parse(await readFile(resolve(manifests, name), "utf8")).models);
  const entry = (model, installed) => {
    const local = model.engine === "laya";
    return {
      id: model.id, label: model.label, engine: model.engine, protocol: model.protocol, revision: model.revision, license: model.license,
      source: model.source, quality: model.quality, origin: "bundled", serverModelId: model.serverModelId ?? null, nativeFormat: model.nativeFormat ?? null,
      downloadBytes: model.files.reduce((sum, file) => sum + file.bytes, 0),
      weightsBytes: model.weightsBytes ?? model.files.filter((file) => file.path.endsWith(".safetensors")).reduce((sum, file) => sum + file.bytes, 0),
      local: { supported: local, downloadOnly: !local, reason: local ? null : "engine_unverified", inputs: ["text", "json"], unsupportedInputs: ["images", "videos"] },
      existingServer: { supported: true, protocol: model.protocol, modelId: model.serverModelId ?? null, requiresCheck: true },
      runtime: { state: local ? "ready" : "unverified", downloadBytes: 0, reason: local ? null : "engine_unverified" },
      installed, installedDirectory: installed ? `/Users/dev/Library/Application Support/dev.zerocode.app/managed-judgment/models/${model.id}` : null,
      compatibility: local ? "supported" : "unsupported", compatibilityReason: local ? null : "engine_unverified",
    };
  };
  const [laya, d3] = ["laya-multilingual-322m", "d3-lite"].map((id) => bundled.find((model) => model.id === id));
  delete backend.managedModelState;
  backend.managedJudgmentSettings();
  backend.managedModelState.runtime.state = "ready";
  backend.managedModelState.models = [entry(laya, true), entry(d3, false),
    { ...entry(d3, false), id: "native-checked-address", label: d3.serverModelId, revision: "checked-address-revision", origin: "registered", quality: "source_inspected_unverified" }];
  const page = await context.newPage();
  await installTauri(page, "managed-photo");
  attachFaultRecorder(page, "managed-photo");
  try {
    await page.setViewportSize({ width: 1280, height: 2400 });
    await bootPage(page, "managed-photo");
    await openSettings(page, "judgment-models");
    await page.locator("#model-advanced").evaluate((fold) => { fold.open = true; });
    await page.waitForFunction(() => managedJudgmentController.snapshot?.models.length === 3);
    await renderSettled(page);
    const drawn = await page.evaluate(() => ({
      rows: [...document.querySelectorAll("#managed-judgment-models > .managed-judgment-model h4")].map((title) => title.textContent),
      pinnedFolds: [...document.querySelectorAll("#managed-judgment-panel details > summary")]
        .filter((summary) => summary.textContent === t("settings.managed.details", "모델 출처와 고정 버전")).length,
      candidates: [...document.querySelectorAll("#managed-judgment-candidate-list > li .managed-judgment-candidate-name")].map((name) => name.textContent),
    }));
    await mkdir(folder, { recursive: true });
    await writeFile(resolve(folder, "library.json"), `${JSON.stringify(drawn, null, 2)}\n`);
    for (const theme of SETTINGS_QUALITY_SPEC.themes) {
      await setQualityTheme(page, theme);
      await renderSettled(page);
      await page.locator("#managed-judgment-panel").screenshot({ path: resolve(folder, `library-${theme}.png`), animations: "disabled" });
      const fold = page.locator("#managed-judgment-candidates");
      if (!(await fold.isVisible())) continue;
      await fold.evaluate((element) => { element.open = true; });
      await renderSettled(page);
      await page.locator("#managed-judgment-panel").screenshot({ path: resolve(folder, `library-${theme}-unfolded.png`), animations: "disabled" });
      await fold.evaluate((element) => { element.open = false; });
    }
  } finally {
    await page.close();
    backend.managedModelState = previousManaged;
  }
}

await test("받은 판단 모델만 목록 행으로 서고 받지 않은 카탈로그 모델은 「받을 수 있는 모델」 접힘 한 줄로 간다", async () => {
  if (process.env.MANAGED_JUDGMENT_SHOTS) await photographManagedLibrary(process.env.MANAGED_JUDGMENT_SHOTS);
  const previous = backend.zoSettings.smart === undefined ? undefined : clone(backend.zoSettings.smart);
  const previousManaged = backend.managedModelState === undefined ? undefined : clone(backend.managedModelState);
  delete backend.managedModelState;
  backend.managedJudgmentSettings();
  // The person's photo of 10-22 (t-69998): a catalog of three, one of them
  // received. Beside it stand a model this computer can run and a
  // download-only one that a System One server can answer with.
  const held = backend.managedModelState;
  const base = held.models[0];
  held.runtime.state = "ready";
  held.models = [
    { ...clone(base), id: "fixture-laya", label: "Fixture Laya 322M", engine: "laya", revision: "laya-revision", weightsBytes: 643_835_514,
      local: { supported: true, downloadOnly: false, reason: null }, installed: true, installedDirectory: "/models/managed/fixture-laya" },
    { ...clone(base), id: "fixture-local-candidate", label: "Fixture Local Candidate", revision: "local-candidate-revision", weightsBytes: 1_073_741_824,
      local: { supported: true, downloadOnly: false, reason: null } },
    { ...clone(base), id: "fixture-d3", label: "Fixture D3 lite", engine: "transformers_system_one", revision: "d3-revision", serverModelId: "acme/d3",
      source: "https://huggingface.co/acme/d3/tree/d3-revision", weightsBytes: 1_707_072_248, compatibility: "unsupported", compatibilityReason: "engine_unverified",
      local: { supported: false, downloadOnly: true, reason: "engine_unverified" },
      existingServer: { supported: true, protocol: "system_one", modelId: "acme/d3", requiresCheck: true } },
  ];
  const page = await context.newPage();
  await installTauri(page, "managed-library");
  attachFaultRecorder(page, "managed-library");
  try {
    await bootPage(page, "managed-library");
    await openSettings(page, "judgment-models");
    await page.locator("#model-advanced").evaluate((fold) => { fold.open = true; });
    await page.waitForFunction(() => managedJudgmentController.snapshot?.models.length === 3);
    assertEqual(await page.locator("#managed-judgment-candidates").count(), 1, "the catalog models not received have no folded line");
    const words = await page.evaluate(() => ({
      details: t("settings.managed.details", "모델 출처와 고정 버전"),
      source: t("settings.managed.detailsSource", "모델 출처"),
      connected: t("settings.managed.connectedApplied", "기존 판단 서버로 연결됨 · 현재 적용됨"),
      offered: [2, 1].map((count) => t("settings.managed.availableCount", "받을 수 있는 모델 {{count}}개", { count })),
      counted: [1, 2, 3].map((total) => t("settings.managed.libraryCount", "모델 {{shown}}개 표시 · 전체 {{total}}개", { shown: total, total })),
      download: t("settings.managed.download", "다운로드"),
      register: t("settings.managed.import", "기존 파일 등록"),
      inspect: t("settings.managed.inspectSource", "주소로 확인"),
      connect: t("settings.managed.connectServer", "이 모델의 서버 연결"),
      sizes: [1_073_741_824, 1_707_072_248].map((bytes) => bytesLabel(bytes)),
    }));
    const library = () => page.evaluate(() => [...document.querySelectorAll("#managed-judgment-models > .managed-judgment-model")].map((row) => row.dataset.modelId));
    const said = () => page.evaluate(() => ({
      count: document.getElementById("managed-judgment-library-summary").textContent,
      fold: document.getElementById("managed-judgment-candidates").hidden ? null : document.querySelector("#managed-judgment-candidates > summary").textContent.trim(),
    }));
    const fold = page.locator("#managed-judgment-candidates");
    assertEqual(await library(), ["fixture-laya"], "a catalog model not received here stood as a library row");
    assertEqual(await said(), { count: words.counted[0], fold: words.offered[0] }, "the library is not counted by its rows, or the candidates by theirs");
    assert(!(await fold.evaluate((element) => element.open)), "the candidates start unfolded");
    assert(await fold.locator(".settings-notice").isHidden(), "the experimental notice stands while the candidates are folded");
    assert(await page.locator("#managed-judgment-none").isHidden(), "a library holding a model says it holds none");
    // A pinned revision is a fact of received files alone.
    const pinned = await page.evaluate((details) => {
      const panel = document.getElementById("managed-judgment-panel");
      return {
        folds: [...panel.querySelectorAll("details > summary")].filter((summary) => summary.textContent === details)
          .map((summary) => summary.closest("[data-model-id]")?.dataset.modelId ?? null),
        revisions: ["laya-revision", "local-candidate-revision", "d3-revision"].filter((revision) => panel.textContent.includes(revision)),
      };
    }, words.details);
    assertEqual(pinned, { folds: ["fixture-laya"], revisions: ["laya-revision"] }, "a model not received here shows a source and pinned revision");

    await fold.locator("summary").click();
    assert(await fold.evaluate((element) => element.open), "the folded line did not open");
    assert(await fold.locator(".settings-notice").isVisible(), "the unfolded candidates lost the experimental notice");
    const offered = await page.evaluate(() => [...document.querySelectorAll("#managed-judgment-candidate-list > li")].map((item) => ({
      id: item.dataset.modelId,
      name: item.querySelector(".managed-judgment-candidate-name").textContent,
      size: item.querySelector(".managed-judgment-candidate-name ~ span").textContent,
      actions: [...item.querySelectorAll("button[data-model-action]")].filter((button) => !button.hidden).map((button) => [button.dataset.modelAction, button.textContent]),
      folds: item.querySelectorAll("details").length,
    })));
    assertEqual(offered, [
      { id: "fixture-local-candidate", name: "Fixture Local Candidate", size: words.sizes[0], actions: [["install", words.download], ["import", words.register]], folds: 0 },
      { id: "fixture-d3", name: "Fixture D3 lite", size: words.sizes[1], actions: [["inspect-source", words.inspect], ["connect", words.connect]], folds: 0 },
    ], "an unfolded candidate is not one line of its name, its size and the ways to have it");
    assert(await page.locator('[data-model-id="fixture-d3"] .managed-judgment-candidate-reason').isVisible(), "a download-only candidate does not explain its limitation");
    await page.fill("#managed-judgment-search", "fixture-d3");
    assert(await page.locator('#managed-judgment-candidate-list > li[data-model-id="fixture-d3"]').isVisible(), "search cannot find a model not downloaded yet");
    assert(await page.locator('#managed-judgment-candidate-list > li[data-model-id="fixture-local-candidate"]').isHidden(), "candidate search ignored the query");
    assert(await page.locator("#managed-judgment-no-matches").isHidden(), "a matching candidate is reported as no matches");
    await page.fill("#managed-judgment-search", "");
    for (const theme of SETTINGS_QUALITY_SPEC.themes) {
      await setQualityTheme(page, theme);
      await renderSettled(page);
      assertEqual(await axeViolations(page, AxeBuilder, "#managed-judgment-panel"), [], `${theme} unfolded candidates accessibility`);
    }
    await page.setViewportSize(SETTINGS_QUALITY_SPEC.viewport);
    await renderSettled(page);
    const geometry = await page.locator("#managed-judgment-panel").evaluate((panel) => ({ client: panel.clientWidth, scroll: panel.scrollWidth }));
    assert(geometry.client > 0 && geometry.scroll <= geometry.client + 1, "the unfolded candidates overflow the narrowest settings pane", geometry);

    // Received, a candidate joins the library with the revision it was pinned to.
    const installedAt = backend.calls.length;
    await page.click('#managed-judgment-candidate-list [data-model-id="fixture-local-candidate"] [data-model-action="install"]');
    const request = await backend.waitForCall("managed-library", "managed_judgment_install", installedAt);
    assertEqual(request.args, { modelId: "fixture-local-candidate", revision: "local-candidate-revision" }, "the candidate's download did not bind its catalog revision");
    await page.waitForFunction(() => managedJudgmentController.rows.has("fixture-local-candidate") && !managedJudgmentController.busy);
    assertEqual(await library(), ["fixture-laya", "fixture-local-candidate"], "a received candidate did not join the library");
    assertEqual(await said(), { count: words.counted[1], fold: words.offered[1] }, "receiving a candidate did not move it from one count to the other");
    assert(await page.evaluate(() => managedJudgmentController.rows.get("fixture-local-candidate").identity.textContent.includes("local-candidate-revision")),
      "the received model does not show the revision it was pinned to");

    // A server of the person's own answering with the download-only model
    // makes it a row, with no revision it cannot know. The address the person
    // checked for the same model answers with the same server model ID: one
    // connection is one row, and the other entry stays a candidate.
    const checked = { ...clone(held.models.find((model) => model.id === "fixture-d3")), id: "fixture-d3-checked", label: "acme/d3", revision: "d3-checked-revision", origin: "registered" };
    held.models.push(checked);
    held.revision += 1;
    backend.zoSettings.smart = { ...(previous ?? {}), judgmentProvider: { kind: "local", protocol: "system_one", baseUrl: "http://127.0.0.1:32105", model: "acme/d3", requiresAuth: false } };
    await page.evaluate(() => refreshTypeSafe());
    await page.waitForFunction(() => managedJudgmentController.rows.has("fixture-d3") && managedJudgmentController.snapshot?.models.length === 4);
    const answering = () => page.evaluate((connected) => ({
      rows: [...document.querySelectorAll("#managed-judgment-models > .managed-judgment-model")].filter((row) => row.textContent.includes(connected)).map((row) => row.dataset.modelId),
      candidates: [...document.querySelectorAll("#managed-judgment-candidate-list > li")].map((item) => item.dataset.modelId),
    }), words.connected);
    assertEqual(await answering(), { rows: ["fixture-d3"], candidates: ["fixture-d3-checked"] }, "two catalog entries of one server model stand as two connected rows");
    assert(await page.locator('#managed-judgment-candidate-list [data-model-id="fixture-d3-checked"] [data-model-action="connect"]').isDisabled(), "the same connected server is offered as a new connection");
    assertEqual(await library(), ["fixture-laya", "fixture-local-candidate", "fixture-d3"], "the model a connected server answers with is not in the library");
    const served = await page.evaluate(() => {
      const row = managedJudgmentController.rows.get("fixture-d3");
      return { summary: row.summary.textContent, identity: row.identity.textContent, state: row.state.textContent };
    });
    assertEqual(served, { summary: words.source, identity: "acme/d3 · transformers_system_one", state: words.connected },
      "the connected server's model claims a pinned revision it cannot know, or hides that it is connected");
    assertEqual(await said(), { count: words.counted[2], fold: words.offered[1] }, "the connected server model is not counted as one row beside one candidate");
    held.configuredModelId = checked.id;
    held.revision += 1;
    await page.evaluate(() => refreshTypeSafe());
    await page.waitForFunction(() => managedJudgmentController.snapshot?.configuredModelId === "fixture-d3-checked");
    assertEqual(await answering(), { rows: ["fixture-d3-checked"], candidates: ["fixture-d3"] }, "a connection bound to the second entry still selected the first");
    held.configuredModelId = null;
    // Files received for the checked address make it the connection's row,
    // and the bundled entry goes back to the candidates.
    checked.installed = true;
    checked.installedDirectory = "/models/managed/fixture-d3-checked";
    held.revision += 1;
    await page.evaluate(() => refreshTypeSafe());
    await page.waitForFunction(() => managedJudgmentController.snapshot?.models.some((model) => model.id === "fixture-d3-checked" && model.installed));
    assertEqual(await answering(), { rows: ["fixture-d3-checked"], candidates: ["fixture-d3"] }, "the connection's row is not the entry whose files are here");
    assertEqual(await library(), ["fixture-laya", "fixture-local-candidate", "fixture-d3-checked"], "a connected server model stands as more than one row");
  } finally {
    await page.close();
    backend.zoSettings.smart = previous;
    backend.managedModelState = previousManaged;
  }
  return "catalog 3 · received 1 → 1 row + 「2 to add」 folded · revision only on received files · unfolded: name, size, ways · axe 2 themes · download joins the library · connected server model is a row without a revision · two entries of one server model ID → 1 row + 1 candidate, the received one the row";
});

await test("받은 판단 모델도 연결된 서버도 없으면 목록은 빈 상태 한 줄만 보이고 후보는 접혀 있다", async () => {
  const previous = backend.zoSettings.smart === undefined ? undefined : clone(backend.zoSettings.smart);
  const previousManaged = backend.managedModelState === undefined ? undefined : clone(backend.managedModelState);
  delete backend.managedModelState;
  backend.managedJudgmentSettings();
  const page = await context.newPage();
  await installTauri(page, "managed-none");
  attachFaultRecorder(page, "managed-none");
  try {
    await bootPage(page, "managed-none");
    await openSettings(page, "judgment-models");
    await page.locator("#model-advanced").evaluate((fold) => { fold.open = true; });
    await page.waitForFunction(() => managedJudgmentController.snapshot?.models.length === 1);
    assertEqual(await page.locator("#managed-judgment-none").count(), 1, "the library has no line for holding nothing");
    const seen = () => page.evaluate(() => {
      const shown = (node) => Boolean(node && !node.parentElement.closest("details:not([open])") && node.getClientRects().length > 0);
      return {
        rows: document.querySelectorAll("#managed-judgment-models > .managed-judgment-model").length,
        lines: ["managed-judgment-library-tools", "managed-judgment-library-summary", "managed-judgment-no-matches", "managed-judgment-none", "managed-judgment-empty", "managed-judgment-candidates"]
          .filter((id) => shown(document.getElementById(id))),
        notice: shown(document.querySelector("#managed-judgment-candidates .settings-notice")),
        open: document.getElementById("managed-judgment-candidates").open,
        pinned: document.getElementById("managed-judgment-panel").textContent.includes("fixture-revision"),
      };
    });
    assertEqual(await seen(), { rows: 0, lines: ["managed-judgment-library-tools", "managed-judgment-none", "managed-judgment-candidates"], notice: false, open: false, pinned: false },
      "a computer holding no model cannot search the available candidates");
    assertEqual(await page.locator("#managed-judgment-none").textContent(),
      await page.evaluate(() => t("settings.managed.noneReceived", "받은 판단 모델이 없습니다 — 아래에서 불러오세요")), "the empty library does not point at the candidates below");
    // With nothing in the catalog either, the line left is the server one.
    backend.managedModelState.models = [];
    backend.managedModelState.revision += 1;
    await page.evaluate(() => managedJudgmentController.refresh());
    await page.waitForFunction(() => managedJudgmentController.snapshot?.models.length === 0);
    assertEqual(await seen(), { rows: 0, lines: ["managed-judgment-empty"], notice: false, open: false, pinned: false },
      "an empty catalog still points at candidates that are not there");
  } finally {
    await page.close();
    backend.zoSettings.smart = previous;
    backend.managedModelState = previousManaged;
  }
  return "received 0 · server 0 → 0 rows, one line and the candidates folded · no search, count or notice · an empty catalog keeps its server line";
});

await test("로컬 모델 실패는 실제 원인과 다음 행동을 안내하고 적용된 모델을 보존한다", async () => {
  const previousManaged = backend.managedModelState === undefined ? undefined : clone(backend.managedModelState);
  delete backend.managedModelState;
  backend.managedJudgmentSettings();
  await openJudgmentSettings(pageA);
  await pageA.evaluate(() => { managedJudgmentController.snapshot = null; judgmentProviderController.localMode = "managed"; });
  try {
    await pageA.selectOption("#judgment-provider-kind", "local");
    await pageA.waitForFunction(() => managedJudgmentController.snapshot?.models.length > 0);
    const before = clone(backend.typesafeSettings().provider);
    backend.managedModelState.operation = { id: "fixture-space-failure", kind: "download", modelId: "fixture-decision", state: "failed", error: "insufficient_space", receivedBytes: 0, totalBytes: 1024 };
    backend.managedModelState.revision += 1;
    await backend.emitManaged(backend.managedJudgmentSettings());
    const words = await pageA.evaluate(() => t("settings.managed.error.space", "저장 공간이 부족합니다. 사용하지 않는 모델을 정리한 뒤 다시 시도하세요."));
    assertEqual(await pageA.locator("#managed-judgment-status").textContent(), words, "a storage failure is hidden behind a generic refresh message");
    assertEqual(backend.typesafeSettings().provider, before, "a failed installation replaced the provider");
  } finally {
    backend.managedModelState = previousManaged;
    await pageA.evaluate(() => { managedJudgmentController.snapshot = null; judgmentProviderController.draft = null; return refreshApiRouters(); });
  }
  return "actionable storage refusal · no configuration change";
});

await test("카탈로그 추가는 크기와 현재 목록에 묶이고 설치나 적용을 시작하지 않는다", async () => {
  const previousManaged = backend.managedModelState === undefined ? undefined : clone(backend.managedModelState);
  delete backend.managedModelState;
  backend.managedJudgmentSettings();
  backend.managedModelState.catalogFingerprint = "fixture-catalog";
  await openJudgmentSettings(pageA);
  await pageA.evaluate(() => { managedJudgmentController.snapshot = null; judgmentProviderController.localMode = "managed"; });
  try {
    await pageA.selectOption("#judgment-provider-kind", "local");
    await pageA.waitForFunction(() => managedJudgmentController.snapshot?.catalogFingerprint === "fixture-catalog");
    assertEqual(await pageA.locator("#managed-judgment-register").count(), 1, "the model library cannot add a new catalog");
    await pageA.locator("#managed-judgment-register").evaluate((element) => { element.open = true; });
    const before = clone(backend.typesafeSettings().provider);
    const callsAt = backend.calls.length;
    await pageA.locator("#managed-judgment-catalog-file").setInputFiles({ name: "too-large.json", mimeType: "application/json", buffer: Buffer.alloc(512 * 1024 + 1, 32) });
    await pageA.click("#managed-judgment-register-submit");
    assert(!backend.calls.slice(callsAt).some((entry) => entry.command === "managed_judgment_register_catalog"), "an oversized catalog crossed IPC");
    const added = { ...clone(backend.managedModelState.models[0]), id: "fixture-added", label: "Additional Native Decision" };
    const catalogJson = JSON.stringify({ version: 1, models: [added] });
    await pageA.locator("#managed-judgment-catalog-file").setInputFiles({ name: "decisions.json", mimeType: "application/json", buffer: Buffer.from(catalogJson) });
    await pageA.click("#managed-judgment-register-submit");
    const registered = await backend.waitForCall("A", "managed_judgment_register_catalog", callsAt);
    assertEqual(registered.args, { instanceId: "fixture-instance", expectedCatalogFingerprint: "fixture-catalog", catalogJson });
    await pageA.waitForFunction(() => managedJudgmentController.snapshot?.models.length === 2);
    assertEqual(backend.typesafeSettings().provider, before, "registration silently selected a model");
    assert(!backend.calls.slice(callsAt).some((entry) => ["managed_judgment_prepare_runtime", "managed_judgment_install", "managed_judgment_start", "managed_judgment_apply"].includes(entry.command)), "catalog registration executed a model lifecycle action");
  } finally {
    backend.managedModelState = previousManaged;
    await pageA.evaluate(() => { managedJudgmentController.snapshot = null; judgmentProviderController.draft = null; return refreshApiRouters(); });
  }
  return "bounded file · exact instance/catalog binding · registration is not installation or application";
});

await test("모델별 실행 환경은 서로 준비됐다고 표시하지 않는다", async () => {
  const previousManaged = backend.managedModelState === undefined ? undefined : clone(backend.managedModelState);
  delete backend.managedModelState;
  backend.managedJudgmentSettings();
  backend.managedModelState.runtime.state = "ready";
  const model = backend.managedModelState.models[0];
  model.runtime = { state: "missing", pythonRequirement: "fixture-model-runtime", downloadBytes: 2048, reason: null };
  model.installed = true;
  await openJudgmentSettings(pageA);
  await pageA.evaluate(() => { managedJudgmentController.snapshot = null; judgmentProviderController.localMode = "managed"; });
  try {
    await pageA.selectOption("#judgment-provider-kind", "local");
    await pageA.waitForFunction(() => managedJudgmentController.snapshot?.models[0]?.runtime?.state === "missing");
    assertEqual(await pageA.evaluate(() => managedJudgmentController.snapshot.models[0].runtime.state), "missing", "an unrelated runtime made this model ready");
    assert(await pageA.locator('[data-model-action="activate"]').isEnabled(), "automatic setup is not available for this supported model");
    assert(await pageA.locator("#managed-judgment-runtime").isHidden(), "a global ready message contradicts the model runtime");
    const preparedAt = backend.calls.length;
    await pageA.click('[data-model-action="prepare"]');
    const prepared = await backend.waitForCall("A", "managed_judgment_prepare_model_runtime", preparedAt);
    assertEqual(prepared.args, { modelId: "fixture-decision", revision: "fixture-revision" });
    await pageA.waitForFunction(() => managedJudgmentController.snapshot?.models[0]?.runtime?.state === "ready");
    assert(await pageA.locator('[data-model-action="activate"]').isEnabled(), "the prepared model is still blocked");
  } finally {
    backend.managedModelState = previousManaged;
    await pageA.evaluate(() => { managedJudgmentController.snapshot = null; judgmentProviderController.draft = null; return refreshApiRouters(); });
  }
  return "per-model preparation · no global-runtime false readiness";
});

await test("새 판단 연결은 적용 전에 확인하고 실패나 다른 창의 변경을 덮지 않는다", async () => {
  const previous = backend.zoSettings.smart === undefined ? undefined : clone(backend.zoSettings.smart);
  let finishProbe;
  await openJudgmentSettings(pageA);
  try {
    await pageA.selectOption("#judgment-provider-kind", "local");
    await pageA.click("#judgment-local-manual");
    await pageA.fill("#judgment-endpoint", "http://127.0.0.1:32103");
    await pageA.fill("#typesafe-model-input", "acme/decision");
    const before = clone(backend.typesafeSettings().provider);
    const identity = backend.typesafeSettings().providerIdentity;
    backend.judgmentPreflight = { candidateIdentity: "fixture-candidate", activeProviderIdentity: identity, answered: false, model: null, failure: "schema", elapsedMs: 1 };
    const failedAt = backend.calls.length;
    await pageA.click("#judgment-save-provider");
    await pageA.waitForFunction(() => !typesafeBusy);
    assertEqual(backend.typesafeSettings().provider, before, "an invalid connection replaced the working provider before it was checked");
    assert(backend.calls.slice(failedAt).some((entry) => entry.command === "preflight_judgment_provider"), "apply did not probe the candidate");
    assert(!backend.calls.slice(failedAt).some((entry) => entry.command === "set_judgment_provider"), "a failed probe reached apply");
    backend.judgmentPreflight.answered = true;
    backend.judgmentPreflight.model = "fixture-candidate:acme/decision";
    backend.judgmentPreflight.failure = null;
    const checkedAt = backend.calls.length;
    await pageA.click("#typesafe-check-btn");
    await backend.waitForCall("A", "preflight_judgment_provider", checkedAt);
    await pageA.waitForFunction(() => !typesafeBusy);
    assertEqual(backend.typesafeSettings().provider, before, "a standalone draft check applied its candidate");
    backend.judgmentPreflightWait = new Promise((resolveProbe) => { finishProbe = resolveProbe; });
    const staleAt = backend.calls.length;
    await pageA.click("#judgment-save-provider");
    await backend.waitForCall("A", "preflight_judgment_provider", staleAt);
    backend.zoSettings.smart = { ...(backend.zoSettings.smart ?? {}), [JEV_MODEL_SETTING]: "jev-other-window", judgmentProvider: { ...before, kind: "jev" } };
    finishProbe(backend.judgmentPreflight);
    await pageA.waitForFunction(() => !typesafeBusy);
    assertEqual(backend.typesafeSettings().provider.model, "jev-other-window", "a stale probe overwrote the other window's choice");
    assert(!backend.calls.slice(staleAt).some((entry) => entry.command === "set_judgment_provider"), "a stale probe reached apply");
    delete backend.judgmentPreflightWait;
    delete backend.judgmentPreflight;
    await pageA.evaluate(() => { judgmentProviderController.draft = null; return refreshApiRouters(); });
    await pageA.selectOption("#judgment-provider-kind", "local");
    await pageA.click("#judgment-local-manual");
    await pageA.fill("#judgment-endpoint", "http://127.0.0.1:32103");
    await pageA.fill("#typesafe-model-input", "acme/decision");
    const expectedProviderIdentity = backend.typesafeSettings().providerIdentity;
    const appliedAt = backend.calls.length;
    await pageA.click("#judgment-save-provider");
    const applied = await backend.waitForCall("A", "set_judgment_provider", appliedAt);
    assertEqual(applied.args.expectedProviderIdentity, expectedProviderIdentity, "apply lacks the active-identity compare-and-set guard");
    await pageA.waitForFunction(() => !typesafeBusy);
    assertEqual(backend.typesafeSettings().provider.model, "acme/decision");
  } finally {
    finishProbe?.(backend.judgmentPreflight ?? {});
    delete backend.judgmentPreflight;
    delete backend.judgmentPreflightWait;
    backend.zoSettings.smart = previous;
    await pageA.evaluate(() => { judgmentProviderController.draft = null; return refreshApiRouters(); });
  }
  return "candidate failure preserves active model · draft check is read-only · stale probe refuses · atomic apply after success";
});

await test("로컬 모델 관리는 키보드와 다섯 언어·두 테마·좁은 화면을 지원한다", async () => {
  const previous = backend.zoSettings.smart === undefined ? undefined : clone(backend.zoSettings.smart);
  const previousManaged = backend.managedModelState === undefined ? undefined : clone(backend.managedModelState);
  delete backend.managedModelState;
  const managedPage = await context.newPage();
  await installTauri(managedPage, "managed-quality");
  attachFaultRecorder(managedPage, "managed-quality");
  const traceManaged = (phase) => {
    if (process.env.SETTINGS_TRACE) process.stderr.write(`managed quality ${phase}\n`);
  };
  const deadline = setTimeout(() => {
    traceManaged("deadline exceeded");
    void managedPage.close().catch(() => {});
  }, 60_000);
  try {
    traceManaged("boot");
    await bootPage(managedPage, "managed-quality");
    await openJudgmentSettings(managedPage);
    await managedPage.selectOption("#judgment-provider-kind", "local");
    traceManaged("model list");
    await managedPage.waitForFunction(() => managedJudgmentController.snapshot?.models.length > 0);
    await managedPage.locator("#managed-judgment-prepare").focus();
    const preparedAt = backend.calls.length;
    traceManaged("keyboard prepare");
    await managedPage.keyboard.press("Enter");
    await backend.waitForCall("managed-quality", "managed_judgment_prepare_runtime", preparedAt);
    await managedPage.waitForFunction(() => managedJudgmentController.snapshot?.runtime.state === "ready");
    await managedPage.locator("#managed-judgment-import").evaluate((element) => { element.open = true; });
    await managedPage.locator("#managed-judgment-import-path").fill("/models/example");
    for (const locale of SETTINGS_QUALITY_SPEC.locales) {
      traceManaged(`locale ${locale}`);
      await setQualityLocale(managedPage, locale);
      await managedPage.evaluate(() => judgmentProviderController.render());
      for (const theme of SETTINGS_QUALITY_SPEC.themes) {
        traceManaged(`accessibility ${locale}/${theme}`);
        await setQualityTheme(managedPage, theme);
        await managedPage.locator("#managed-judgment-import-path").evaluate((input) => input.scrollIntoView({ block: "center", inline: "nearest" }));
        await renderSettled(managedPage);
        const violations = await axeViolations(managedPage, AxeBuilder, "#managed-judgment-panel");
        assertEqual(violations, [], `${locale}/${theme} local manager accessibility`);
        const contrast = await contrastTable(managedPage, AxeBuilder, "#managed-judgment-import-path");
        const importedPath = contrast.find((row) => row.target === "#managed-judgment-import-path");
        assert(importedPath?.verdict !== "hidden" && importedPath?.ratio >= 4.5
          && importedPath.fg !== null && importedPath.bg !== null,
        `${locale}/${theme} visible import path has no passing contrast measurement`, importedPath);
      }
      for (const viewport of [SETTINGS_QUALITY_SPEC.viewport, { width: 1280, height: 1069 }, { width: 1998, height: 1069 }]) {
        traceManaged(`layout ${locale}/${viewport.width}x${viewport.height}`);
        await managedPage.setViewportSize(viewport);
        await renderSettled(managedPage);
        const geometry = await managedPage.locator("#managed-judgment-panel").evaluate((panel) => ({ client: panel.clientWidth, scroll: panel.scrollWidth }));
        assert(geometry.client > 0 && geometry.scroll <= geometry.client + 1, `${locale}/${viewport.width}×${viewport.height} local manager overflows`, geometry);
      }
    }
    await setQualityLocale(managedPage, "ko");
    await managedPage.evaluate(() => judgmentProviderController.render());
    if (process.env.MANAGED_JUDGMENT_SHOTS) {
      for (const theme of SETTINGS_QUALITY_SPEC.themes) {
        await setQualityTheme(managedPage, theme);
        await managedPage.locator("#typesafe-card").screenshot({ path: resolve(process.env.MANAGED_JUDGMENT_SHOTS, `managed-${theme}.png`), animations: "disabled" });
      }
    }
    const looseButtons = await managedPage.locator("#managed-judgment-panel .btn").evaluateAll((buttons) => buttons.filter((button) => !button.closest(".settings-action-row") && button.id !== "managed-judgment-cancel").map((button) => button.id));
    assertEqual(looseButtons, [], "managed actions are not grouped with their controls");
  } finally {
    clearTimeout(deadline);
    await managedPage.close();
    backend.zoSettings.smart = previous;
    backend.managedModelState = previousManaged;
  }
  return "real Enter · 5 locales × 2 themes · supported 720×480 minimum and wider viewports · model actions grouped";
});

await test("판단 연결 시험은 입력 키를 임시로 쓰고 실패해도 기존 키를 바꾸지 않는다", async () => {
  const previous = backend.zoSettings.smart === undefined ? undefined : clone(backend.zoSettings.smart);
  const provider = { kind: "remote", protocol: "openai_json", baseUrl: "https://decision-key.example.com/v1", model: "acme/decision", requiresAuth: true };
  const service = backend.judgmentService(provider);
  const previousKey = backend.keychain.get(service);
  backend.zoSettings.smart = { ...(previous ?? {}), judgmentProvider: provider };
  backend.keychain.set(service, "kept-fixture-key");
  const identity = backend.typesafeSettings().providerIdentity;
  try {
    await openJudgmentSettings(pageA);
    backend.judgmentModelsError = "unauthorized";
    backend.judgmentPreflight = { candidateIdentity: "fixture-candidate", activeProviderIdentity: identity, answered: false, failure: "unauthorized" };
    for (const [button, command] of [["judgment-refresh-models", "judgment_models"], ["typesafe-check-btn", "preflight_judgment_provider"]]) {
      await pageA.fill("#typesafe-key-input", "rejected-fixture-key");
      const began = backend.calls.length;
      await pageA.click(`#${button}`);
      const call = await backend.waitForCall("A", command, began);
      await pageA.waitForFunction(() => !typesafeBusy && !judgmentProviderController.loading);
      assertEqual(backend.keychain.get(service), "kept-fixture-key", "a failed connection test replaced the active credential");
      assertEqual(call.args.key, "rejected-fixture-key", "the test did not use the entered candidate credential");
      assert(!backend.calls.slice(began).some((one) => ["save_typesafe_key", "set_judgment_provider"].includes(one.command)), "a read-only connection test wrote settings or a key");
      assertEqual(backend.typesafeSettings().providerIdentity, identity);
    }
  } finally {
    delete backend.judgmentModelsError;
    delete backend.judgmentPreflight;
    if (previousKey === undefined) backend.keychain.delete(service);
    else backend.keychain.set(service, previousKey);
    backend.zoSettings.smart = previous;
    await pageA.evaluate(() => { judgmentProviderController.draft = null; return refreshApiRouters(); });
  }
  return "discovery and preflight are read-only · the current credential survives failure";
});

await test("판단 서버도 주소와 키로 연결하고 모델을 고른 뒤 검증해 적용한다", async () => {
  await openJudgmentSettings(pageA);
  const previous = backend.zoSettings.smart === undefined ? undefined : clone(backend.zoSettings.smart);
  try {
    await pageA.selectOption("#judgment-provider-kind", "remote");
    assertEqual(await pageA.locator("#judgment-protocol").count(), 1, "a custom server has no protocol choice");
    await pageA.selectOption("#judgment-protocol", "openai_json");
    await pageA.fill("#judgment-endpoint", "https://decisions.example.com/v1");
    await pageA.fill("#typesafe-model-input", "");
    await pageA.fill("#typesafe-key-input", "decision-fixture-key");
    assertEqual(await pageA.locator("#typesafe-status").textContent(), "", "the new connection still claims a previous model was checked or applied");
    const candidate = await pageA.evaluate(() => judgmentProviderController.current());
    const service = backend.judgmentService(candidate);
    const storedBefore = backend.keychain.get(service);
    const began = backend.calls.length;
    await pageA.click("#judgment-refresh-models");
    await backend.waitForCall("A", "judgment_models", began);
    await pageA.waitForFunction(() => !judgmentProviderController.loading);
    const calls = backend.calls.slice(began).filter((call) => ["save_typesafe_key", "judgment_models"].includes(call.command));
    assertEqual(calls.map((call) => call.command), ["judgment_models"], "discovery wrote a credential before the connection was applied");
    assertEqual(calls[0].args.provider.protocol, "openai_json");
    assertEqual(calls[0].args.key, "decision-fixture-key", "discovery did not use the entered credential");
    assertEqual(backend.keychain.get(service), storedBefore, "discovery replaced the stored credential");
    assertEqual(backend.typesafeSettings().provider.kind, "jev", "discovery applied the candidate before its answer was checked");
    await pageA.selectOption("#judgment-listed-model", "fixture-decision-b");
    for (const theme of SETTINGS_QUALITY_SPEC.themes) {
      await setQualityTheme(pageA, theme);
      await renderSettled(pageA);
      assertEqual(await axeViolations(pageA, AxeBuilder, "#judgment-connection-settings"), [], `${theme} custom connection accessibility`);
      if (process.env.JUDGMENT_CONNECTION_SHOTS) {
        await mkdir(process.env.JUDGMENT_CONNECTION_SHOTS, { recursive: true });
        await pageA.locator("#judgment-connection-settings").screenshot({ path: resolve(process.env.JUDGMENT_CONNECTION_SHOTS, `connection-${theme}.png`), animations: "disabled" });
      }
    }
    const applyAt = backend.calls.length;
    await pageA.click("#judgment-save-provider");
    await backend.waitForCall("A", "set_judgment_provider", applyAt);
    assertEqual(backend.typesafeSettings().provider.model, "fixture-decision-b");
    assertEqual(backend.typesafeSettings().provider.protocol, "openai_json");
    await pageA.waitForFunction(() => !typesafeBusy && !typesafeRefreshing);
    assertEqual(backend.keychain.get(service), "decision-fixture-key", "applying the checked connection did not keep its credential");
    assertEqual(await pageA.inputValue("#typesafe-key-input"), "", "the applied key remained in the form");
  } finally {
    backend.zoSettings.smart = previous;
    await pageA.evaluate(() => { judgmentProviderController.draft = null; return refreshApiRouters(); });
  }
  return "temporary key → model discovery → selected model → checked settings and key commit";
});

await test("판단 서버 주소 오류는 고칠 방법을 말하고 현재 모델은 유지한다", async () => {
  await openJudgmentSettings(pageA);
  const previous = backend.zoSettings.smart === undefined ? undefined : clone(backend.zoSettings.smart);
  const identity = backend.typesafeSettings().providerIdentity;
  try {
    await pageA.selectOption("#judgment-provider-kind", "remote");
    await pageA.fill("#judgment-endpoint", "http://gateway.example.com/v1");
    backend.judgmentModelsError = "provider_https_required";
    await pageA.click("#judgment-refresh-models");
    await pageA.waitForFunction(() => !judgmentProviderController.loading);
    const status = await pageA.locator("#judgment-model-status").textContent();
    assert(status.includes("HTTPS") && !status.includes("provider_https_required"), "the address refusal is still an unexplained token", status);
    assertEqual(backend.typesafeSettings().providerIdentity, identity, "failed discovery changed the current provider");
    backend.judgmentPreflightError = "provider_https_required";
    await pageA.fill("#typesafe-model-input", "fixture-decision");
    await pageA.click("#judgment-save-provider");
    await pageA.waitForFunction(() => !typesafeBusy);
    const said = await pageA.locator("#typesafe-status .settings-status-said").textContent();
    assert(said.includes("HTTPS") && !said.includes("provider_https_required"), "apply displayed the raw address token", said);
    assertEqual(backend.typesafeSettings().providerIdentity, identity, "failed apply changed the current provider");
  } finally {
    delete backend.judgmentModelsError;
    delete backend.judgmentPreflightError;
    backend.zoSettings.smart = previous;
    await pageA.evaluate(() => { judgmentProviderController.draft = null; return refreshApiRouters(); });
  }
  return "actionable address failure · current selection unchanged";
});

await test("다른 서버의 키를 모델 선택 전에 저장하고 늦은 목록은 새 선택을 덮지 않는다", async () => {
  await openJudgmentSettings(pageA);
  const previous = backend.zoSettings.smart === undefined ? undefined : clone(backend.zoSettings.smart);
  let finishListing;
  try {
    await pageA.selectOption("#judgment-provider-kind", "remote");
    await pageA.fill("#judgment-endpoint", "https://decisions.example.com");
    await pageA.fill("#typesafe-model-input", "");
    await pageA.fill("#typesafe-key-input", "remote-fixture");
    const keyAt = backend.calls.length;
    await pageA.click("#typesafe-save-btn");
    const saved = await backend.waitForCall("A", "save_typesafe_key", keyAt);
    assertEqual(saved.args.provider.model, "", "the key required a model before discovery");
    assertEqual(backend.keychain.get(backend.judgmentService(saved.args.provider)), "remote-fixture");
    assertEqual(backend.typesafeSettings().provider.kind, "jev", "saving a draft key activated its provider");
    assert(await pageA.locator("#judgment-provider-dirty").isVisible(), "saving a key discarded the draft");
    backend.judgmentModelsWait = new Promise((resolve) => { finishListing = resolve; });
    const listAt = backend.calls.length;
    await pageA.click("#judgment-refresh-models");
    await backend.waitForCall("A", "judgment_models", listAt);
    await pageA.fill("#typesafe-key-input", "unsaved-fixture");
    await pageA.fill("#judgment-endpoint", "https://other.example.com");
    assertEqual(await pageA.inputValue("#typesafe-key-input"), "", "an unsaved key followed a different server");
    await pageA.fill("#typesafe-model-input", "new-head");
    finishListing([{ id: "old-server-head" }]);
    delete backend.judgmentModelsWait;
    await renderSettled(pageA);
    assertEqual(await pageA.locator("#judgment-model-options option").count(), 0, "a stale list filled a different server");
    assertEqual(await pageA.inputValue("#typesafe-model-input"), "new-head");
    await pageA.click("#judgment-revert-provider");
    assertEqual(await pageA.inputValue("#judgment-provider-kind"), "jev");
    assert(await pageA.locator("#judgment-provider-dirty").isHidden());
  } finally {
    finishListing?.([]);
    delete backend.judgmentModelsWait;
    backend.zoSettings.smart = previous;
    await pageA.evaluate(() => { judgmentProviderController.draft = null; return refreshApiRouters(); });
  }
  return "server-scoped credential before discovery · stale catalog discarded";
});

// While Jev is on, the card says for each feature whether it acts or still
// records, and what a recording one waits on (t-11989) — read off the
// numbers `zo jev summary` answers, the dashboard's own reading: the two
// summons seats, which start acting, say so first; one the door refused for
// consent says what it waits for; one counting its window says how much of
// it is in, and one measured under its bar says that. A feature that asks
// nothing is not listed, the list is gone while the switch is off, and a zo
// that does not answer is said. Set `JEV_STANDING_SHOTS` to a folder to keep
// the card in both treatments.
await test("Jev를 켜면 카드가 기능마다 적용 중인지, 기록 중이면 판정까지 얼마나 모였는지 zo의 숫자로 말한다", async () => {
  await openJudgmentSettings(pageA);
  const said = (key, fallback, vars) => pageA.evaluate(([one, words, values]) => t(one, words, values), [key, fallback, vars]);
  const tally = (rows, answered = rows, failures = []) => ({
    rows, answered, refused: rows - answered, applied: 0, called: answered, requests: rows, redactedLines: 0,
    inputTokens: 0, p50Ms: rows ? 300 : null, p95Ms: rows ? 600 : null, failures,
  });
  const numbers = (id, extra = {}) => ({
    id, stand: "recording", applies: false, verdict: null, rowsToNextJudgment: null, judged: null,
    today: tally(0), week: tally(0), days: [], recent: [], askersWeek: {}, ...extra,
  });
  const judged = (rows, wanted) => ({ window: tally(rows), windowWanted: wanted, agreement: { compared: 0, agreed: 0 } });
  const refusedForConsent = tally(4, 0, [{ token: "not_consented", rows: 4 }]);
  backend.keychain.set(TYPESAFE_SERVICE, "apikey_fixture");
  backend.zoSettings.smart = { [JEV_SETTINGS_KEY]: { enabled: true, workspaces: [JEV_EVERY_WORKSPACE] } };
  backend.jevSummary = {
    scope: { scope: "workspace", workspace: "/Users/dev/project", workspaceName: "project", recorded: true, projects: [], windowDays: 7 },
    seats: [
      numbers("routing", { verdict: { verdict: "hold", line: "too_few_rows" }, rowsToNextJudgment: 40, judged: judged(13, 53), week: tally(13) }),
      numbers("placement", { verdict: { verdict: "hold", line: "agreement" }, rowsToNextJudgment: 5, judged: judged(25, 25), week: tally(25) }),
      numbers("summon", { stand: "applying", applies: true, verdict: { verdict: "keep", line: null }, judged: judged(3, 53), week: tally(3) }),
      numbers("summon_difficulty", { stand: "applying", applies: true, verdict: { verdict: "keep", line: null }, judged: judged(12, 53), week: tally(12) }),
      numbers("notify", { week: refusedForConsent, today: refusedForConsent }),
      // Stopped on its own evidence: switched on, it still asks nothing.
      numbers("patch_review", { week: tally(2) }),
    ],
  };
  const summaryAt = backend.count("A", "jev_summary");
  await pageA.evaluate(() => refreshApiRouters());
  const list = pageA.locator("#typesafe-card [data-jev-standings]");
  await pageA.locator("#judgment-feature-details > summary").click();
  await pageA.waitForFunction(
    () => document.querySelectorAll("#typesafe-card [data-jev-standing]").length === 5,
    null, { timeout: UI_TIMEOUT },
  );
  assert(backend.count("A", "jev_summary") > summaryAt, "the card drew standings nobody asked zo for");
  assert(await list.isVisible(), "a card with Jev on shows no standings");
  const rows = await pageA.locator("#typesafe-card [data-jev-standing]").evaluateAll((all) => all.map((row) => {
    const standing = row.querySelector(".settings-jev-standing");
    return {
      id: row.dataset.jevStanding,
      name: row.querySelector(".settings-jev-standing-name").textContent,
      said: standing.textContent,
      status: standing.dataset.status,
      tip: standing.dataset.tip ?? null,
    };
  }));
  const applying = await said("jev.status.applying", "적용 중");
  const recording = await said("jev.status.recording", "기록 중");
  const counted = (detail) => said("settings.typesafe.standingWith", "{{state}} · {{detail}}", { state: recording, detail });
  assertEqual(rows.map((row) => [row.id, row.said, row.status]), [
    ["summon", applying, "applying"],
    ["summon_difficulty", applying, "applying"],
    ["notify", await said("jev.status.needsConsent", "동의 필요"), "blocked"],
    ["routing", await counted("13/53"), "recording"],
    ["placement", await counted(await said("jev.status.under", "기준 미달")), "recording"],
  ], "the card's standings");
  assertEqual(rows.find((row) => row.id === "routing").tip,
    await said("jev.window", "판정 표본 {{rows}}/{{wanted}}건", { rows: "13", wanted: "53" }),
    "the counted sample's tip is the table's sentence");
  const names = await pageA.evaluate(() => Object.fromEntries(["summon", "summon_difficulty"].map((id) => {
    const words = jevFeature(id).name;
    return [id, t(words.key, words.source)];
  })));
  assertEqual(rows.slice(0, 2).map((row) => row.name), [names.summon, names.summon_difficulty], "a feature is named by the template");
  // In words, not by colour alone: every state says itself, and no two
  // states say the same thing.
  assert(rows.every((row) => row.said.trim() !== ""), "a state was drawn with no words", rows);
  assertEqual(new Set(rows.map((row) => row.status)).size, new Set(rows.map((row) => row.said.split(" · ")[0])).size,
    "two states read the same");

  const shots = process.env.JEV_STANDING_SHOTS;
  if (shots) {
    const card = pageA.locator("#typesafe-card");
    for (const theme of ["dark", "light"]) {
      await pageA.evaluate((treatment) => { document.documentElement.dataset.theme = treatment; }, theme);
      await renderSettled(pageA);
      await list.scrollIntoViewIfNeeded();
      await card.screenshot({ path: resolve(shots, `jev-standings-${theme}.png`), animations: "disabled" });
    }
    await pageA.evaluate(() => { delete document.documentElement.dataset.theme; });
  }

  // A zo that does not answer is said, not drawn as nothing.
  backend.jevSummary = null;
  await pageA.evaluate(async () => {
    jevNumbers = null;
    await refreshApiRouters();
    await jevNumbersAsking;
  });
  await pageA.waitForFunction(
    () => document.querySelector("#typesafe-card [data-jev-standings-note]")?.hidden === false,
    null, { timeout: UI_TIMEOUT },
  );
  assertEqual(
    (await pageA.locator("#typesafe-card [data-jev-standings-note]").textContent()).trim(),
    await said("settings.typesafe.standingsUnanswered", "zo가 기능별 상태를 알려 주지 않았습니다."),
  );
  assertEqual(await pageA.locator("#typesafe-card [data-jev-standing]").count(), 0, "a silent zo left rows standing");

  // Off: no standings at all.
  backend.zoSettings.smart = { [JEV_SETTINGS_KEY]: { enabled: false, workspaces: [JEV_EVERY_WORKSPACE] } };
  await pageA.evaluate(() => refreshApiRouters());
  await pageA.waitForFunction(
    () => document.querySelector("#typesafe-card [data-jev-standings]")?.hidden === true,
    null, { timeout: UI_TIMEOUT },
  );
  delete backend.zoSettings.smart;
  backend.keychain.delete(TYPESAFE_SERVICE);
  await pageA.evaluate(() => refreshApiRouters());
  return `${rows.length} standings · ${rows.filter((row) => row.status === "applying").length} applying`;
});

await test("zo가 아무것도 묻지 않는다고 읽는 기능은 카드에 서지 않고, 내가 만든 결정은 결정이 묻기 시작해야 줄이 생긴다", async () => {
  await openJudgmentSettings(pageA);
  const said = (key, fallback) => pageA.evaluate(([one, words]) => t(one, words), [key, fallback]);
  const tally = (rows) => ({
    rows, answered: rows, refused: 0, applied: 0, called: rows, requests: rows, redactedLines: 0,
    inputTokens: 0, p50Ms: rows ? 300 : null, p95Ms: rows ? 600 : null, failures: [],
  });
  const numbers = (id, extra) => ({
    id, stand: "recording", applies: false, verdict: null, rowsToNextJudgment: null, judged: null,
    today: tally(0), week: tally(2), days: [], recent: [], askersWeek: {}, ...extra,
  });
  const summaryBefore = backend.jevSummary;
  backend.keychain.set(TYPESAFE_SERVICE, "apikey_fixture");
  backend.zoSettings.smart = { [JEV_SETTINGS_KEY]: { enabled: true, workspaces: [JEV_EVERY_WORKSPACE] } };
  const standingAt = async (mode, applies) => {
    backend.jevSummary = {
      scope: { scope: "workspace", workspace: "/Users/dev/project", workspaceName: "project", recorded: true, projects: [], windowDays: 7 },
      seats: [numbers("routing", { mode: "auto" }), numbers("custom", { mode, applies })],
    };
    await pageA.evaluate(async () => {
      jevNumbers = null;
      await refreshApiRouters();
      await jevNumbersAsking;
    });
    await pageA.waitForFunction(
      () => document.querySelector('#typesafe-card [data-jev-standing="routing"]') !== null,
      null, { timeout: UI_TIMEOUT },
    );
    return pageA.evaluate(() => {
      const standing = document.querySelector('#typesafe-card [data-jev-standing="custom"] .settings-jev-standing');
      return standing ? { said: standing.textContent, status: standing.dataset.status } : null;
    });
  };
  try {
    assertEqual(await standingAt("off", false), null, "a feature zo reads as asking nothing stands on the card");
    assertEqual(await standingAt("shadow", false),
      { said: await said("jev.status.recording", "기록 중"), status: "recording" },
      "a decision that only records");
    assertEqual(await standingAt("on", true),
      { said: await said("jev.status.applying", "적용 중"), status: "applying" },
      "a decision that acts");
  } finally {
    backend.jevSummary = summaryBefore;
    delete backend.zoSettings.smart;
    backend.keychain.delete(TYPESAFE_SERVICE);
    await pageA.evaluate(() => refreshApiRouters());
  }
  return "off: no line · shadow: recording · on: applying";
});

// The key typing into a page's fields asks with (t-9537): the Computer Use
// pane draws one slot per key a walk reads — today the chosen row's alone —
// from the table the product reads, keeps a saved key in the item the walk's
// writer reads and never shows it again, and a machine with no keychain is
// told so in the TypeSafe card's words.
await test("Computer Use의 글자 입력용 키는 지금 쓰는 키 하나만 칸으로 그리고, 키는 키체인에만 간다", async () => {
  const slots = typeValueSlots();
  const [slot] = slots;
  assert(slot, "the table offers no key a walk reads");
  backend.keychain.delete(slot.service);
  const from = backend.calls.length;
  await openSettings(pageA, "computer-use");
  await backend.waitForCall("A", "type_value_keys", from);
  await renderSettled(pageA);
  // A key is asked for only on the road that asks with one (t-10372).
  const keyed = backend.calls.length;
  await pageA.locator('input[data-generator-road="api_key"]').check();
  await backend.waitForCall("A", "set_computer_generator_road", keyed);
  await pageA.waitForFunction(() => document.getElementById("type-value-key-section")?.hidden === false, null, { timeout: UI_TIMEOUT });
  const said = (key, fallback, vars) => pageA.evaluate(([one, words, values]) => t(one, words, values), [key, fallback, vars]);
  const card = pageA.locator("#type-value-card");
  const drawn = card.locator("[data-type-value-key]");
  const mine = card.locator(`[data-type-value-key="${slot.name}"]`);
  const input = mine.locator("[data-type-value-input]");
  const part = (name) => mine.locator(`[data-type-value-${name}]`);
  const standsAt = (standing) => standingWord(pageA, standing).then((word) => pageA.waitForFunction(
    ([name, expected]) => document.querySelector(`[data-type-value-key="${name}"] [data-type-value-state]`)?.textContent === expected,
    [slot.name, word], { timeout: UI_TIMEOUT },
  ));

  // One slot, the key a walk reads now: its name, the rows that read it and
  // the mark — and no slot for a key only a row nothing picks reads.
  assert(await card.isVisible(), "the Computer Use pane has no key card");
  assertEqual(await drawn.count(), 1, "the card does not draw exactly the one key a walk reads");
  assertEqual(await part("name").textContent(), slot.name);
  assert(await part("now").isVisible(), "the key a walk reads now is not marked");
  const rowsSaid = await part("rows").textContent();
  for (const row of slot.rows) {
    assert(rowsSaid.includes(row.id) && rowsSaid.includes(row.model), `${row.id} is not named on its key`, rowsSaid);
  }
  const shown = await card.textContent();
  for (const row of TYPE_VALUE_TABLE.rows.filter((one) => one.credentialKey && one.credentialKey !== slot.name)) {
    assert(!shown.includes(row.credentialKey), `${row.credentialKey} has a slot no walk reads`);
  }

  // Nothing saved: the badge is the one table's word, the field hides what
  // is typed, and nothing but typing a key is offered — spaces are no key.
  await standsAt("unchecked");
  assertEqual(await input.getAttribute("type"), "password", "the key field shows what is typed");
  assert(await part("save").isDisabled(), "a save was offered with nothing typed");
  assert(await part("remove").isDisabled(), "a removal was offered with nothing saved");
  await input.fill("   ");
  assert(await part("save").isDisabled(), "a key of spaces could be saved");

  // A save sends the trimmed key once under the slot's own name, empties the
  // field, and the key lands in the item the walk reads — never on the page.
  await input.fill("  sk-fixture-typing  ");
  assert(await part("save").isEnabled(), "a typed key could not be saved");
  const savedAt = backend.calls.length;
  await part("save").click();
  const save = await backend.waitForCall("A", "save_type_value_key", savedAt);
  assertEqual(save.args, { credentialKey: slot.name, key: "sk-fixture-typing" }, "the save did not carry the slot's name and the trimmed key");
  await standsAt("keySaved");
  assertEqual(backend.keychain.get(slot.service), "sk-fixture-typing", "the key is not where the walk reads it");
  assertEqual(await input.inputValue(), "", "the key stayed in the field after the save");
  assert(
    !(await pageA.locator('.settings-pane[data-pane="computer-use"]').innerHTML()).includes("sk-fixture-typing"),
    "the key came back to the page",
  );
  assertEqual(
    await part("status").locator(".settings-status-said").textContent(),
    await said("computerUse.typeKeySaved", "키체인에 저장했습니다 — {{name}}. 다음 작업부터 빈 칸에 글자를 입력합니다.", { name: slot.name }),
  );
  assert(await part("remove").isEnabled(), "a saved key cannot be removed");

  // A name no walk reads is refused and writes nothing — one the table
  // never held, and every key only a row nothing picks reads.
  const held = [...backend.keychain.entries()];
  const others = TYPE_VALUE_TABLE.rows.map((row) => row.credentialKey).filter((name) => name && name !== slot.name);
  for (const name of ["SOME_OTHER_API_KEY", ...others]) {
    const refused = await pageA.evaluate((one) => invoke("save_type_value_key", { credentialKey: one, key: "sk-x" })
      .then(() => "saved", (error) => error?.kind ?? String(error)), name);
    assertEqual(refused, "failed", `${name} was saved`);
  }
  assertEqual([...backend.keychain.entries()], held, "a refused name wrote to the key store");

  // A removal forgets it, and says what that means.
  const removedAt = backend.calls.length;
  await part("remove").click();
  assertEqual((await backend.waitForCall("A", "remove_type_value_key", removedAt)).args, { credentialKey: slot.name });
  await standsAt("unchecked");
  assert(!backend.keychain.has(slot.service), "the key outlived its removal");
  assertEqual(
    await part("status").locator(".settings-status-said").textContent(),
    await said("computerUse.typeKeyRemoved", "지웠습니다 — {{name}}. 빈 칸은 입력하지 않고 사람에게 맡깁니다.", { name: slot.name }),
  );

  // The card draws whatever the backend lists: two keys, one a walk reads
  // now, are two slots with one mark, and the field settings lands on is the
  // one a walk reads — in the pane every named control says it lives on.
  await pageA.evaluate((name) => paintTypeValueKeys({
    keysKeptHere: true,
    keys: [
      { credentialKey: "SPARE_API_KEY", rows: [{ id: "spare", model: "spare-model" }], chosen: null, keySaved: true },
      { credentialKey: name, rows: [{ id: "asked", model: "asked-model" }], chosen: "asked", keySaved: false },
    ],
  }), slot.name);
  assertEqual(await drawn.count(), 2, "a second key the backend listed was not drawn");
  assertEqual(await card.locator("[data-type-value-now]:visible").count(), 1, "more than the key a walk reads is marked");
  const landing = await pageA.evaluate(() => ({
    pane: PANE_OF["type-value-key-input"] ?? null,
    slot: document.getElementById("type-value-key-input")?.closest("[data-type-value-key]")?.dataset.typeValueKey ?? null,
    strays: Object.entries(PANE_OF).flatMap(([id, pane]) => {
      const lives = document.getElementById(id)?.closest(".settings-pane")?.dataset.pane;
      return lives === undefined || lives === pane ? [] : [{ id, pane, lives }];
    }),
  }));
  assertEqual(landing, { pane: "computer-use", slot: slot.name, strays: [] }, "settings does not land on the key a walk reads");
  await pageA.evaluate(() => refreshTypeValueKeys());
  await pageA.waitForFunction(() => document.querySelectorAll("#type-value-card [data-type-value-key]").length === 1, null, { timeout: UI_TIMEOUT });

  // A machine with no keychain closes the field and says why, in the words
  // the TypeSafe card says it in.
  backend.routerKeychainUnavailable = true;
  try {
    await pageA.evaluate(() => refreshTypeValueKeys());
    await pageA.waitForFunction(() => document.getElementById("type-value-no-keychain-hint")?.hidden === false, null, { timeout: UI_TIMEOUT });
    assert(await input.isDisabled(), "a key field was offered with no keychain");
    assertEqual(
      (await pageA.locator("#type-value-no-keychain-hint").textContent()).trim(),
      (await pageA.locator("#typesafe-no-keychain-hint").textContent()).trim(),
      "the pane does not say it in the TypeSafe card's words",
    );
    const refusal = { kind: "keychain-unavailable", message: "no keychain for keys" };
    assertEqual(
      await pageA.evaluate((error) => keyStoreRefusal(error), refusal),
      await pageA.evaluate((error) => typesafeRefusal(error), refusal),
      "a refused save is not said in the TypeSafe card's words",
    );
  } finally {
    backend.routerKeychainUnavailable = false;
    backend.keychain.delete(slot.service);
  }
  await pageA.evaluate(() => refreshTypeValueKeys());
  await pageA.waitForFunction(() => document.getElementById("type-value-no-keychain-hint")?.hidden === true, null, { timeout: UI_TIMEOUT });
  assert(!(await input.isDisabled()), "a machine with a keychain was left without the key field");
  const back = backend.calls.length;
  await pageA.locator('input[data-generator-road="auto"]').check();
  await backend.waitForCall("A", "set_computer_generator_road", back);
  return `${await drawn.count()} slot (${slot.name}) of ${TYPE_VALUE_TABLE.rows.filter((row) => row.credentialKey).length} keyed rows`;
});

// Computer Use's generator road (t-10372): a person who never chose is on
// `auto` — the logins the window opens its agents with — and the card says,
// for each login, whose account it runs as, the model its row asks and that
// it spends that account's subscription; a login whose CLI is not on the
// machine says so; the last answer names the road that gave it and every
// road passed over with its reason; a choice is written through the one
// settings writer and comes back through the snapshot; and a key is asked
// for only on the key road.
await test("Computer Use 생성기는 기본이 로그인(자동)이고, 로그인마다 계정·구독 차감을 말하며, 넘어간 길을 보인다", async () => {
  backend.generatorAccounts = { claude_login: "ada@example.invalid" };
  backend.generatorMissingClis = ["codex_login"];
  backend.generatorLast = {
    road: "codex_login",
    model: "a-model",
    passed: ["claude_login=quota_wall"],
    atMs: 1,
  };
  try {
    await openSettings(pageA, "computer-use");
    const from = backend.calls.length;
    await pageA.evaluate(() => refreshTypeValueKeys());
    await backend.waitForCall("A", "type_value_keys", from);
    await renderSettled(pageA);
    const said = (key, fallback, vars) => pageA.evaluate(([one, words, values]) => t(one, words, values), [key, fallback, vars]);
    const checked = () => pageA.evaluate(() => document.querySelector("input[data-generator-road]:checked")?.dataset.generatorRoad ?? null);
    assertEqual(await checked(), "auto", "a person who never chose is not on the logins");
    assertEqual(
      await pageA.evaluate(() => [...document.querySelectorAll("input[data-generator-road]")].map((one) => one.dataset.generatorRoad)),
      [...GENERATOR_ROADS],
      "the card does not offer the core's roads in its order",
    );
    assert(await pageA.locator("#type-value-key-section").isHidden(), "a key was asked for on a login road");

    // Each login: its name, and whose account it runs as, the row's model and
    // the subscription it spends — or that its CLI is not here.
    const lines = await pageA.evaluate(() => [...document.querySelectorAll("#generator-logins dd")].map((one) => one.textContent));
    const claudeRow = TYPE_VALUE_TABLE.rows.find((row) => row.id === TYPE_VALUE_TABLE.claudeLogin);
    assertEqual(lines, [
      await said("computerUse.generatorLoginLine", "{{account}} · {{model}} · 이 계정의 구독 한도에서 차감", {
        account: "ada@example.invalid",
        model: claudeRow.model,
      }),
      await said("computerUse.generatorNoCli", "이 컴퓨터에 CLI가 없어 이 길은 쓸 수 없습니다"),
    ], "the logins are not said with their account, model and subscription");

    // The last answer names the road that gave it and the one passed over.
    const last = await pageA.locator("#generator-last").textContent();
    assertEqual(last, await said("computerUse.generatorLastPassed", "마지막으로 답한 길: {{road}} — 넘어간 길: {{passed}}", {
      road: await said("computerUse.generatorLoginCodex", "Codex 로그인"),
      passed: `${await said("computerUse.generatorLoginClaude", "Claude Code 로그인")} (quota_wall)`,
    }), "the road passed over is a quiet one");

    // A choice goes through the one writer and comes back in the snapshot.
    const at = backend.calls.length;
    await pageA.locator('input[data-generator-road="claude_login"]').check();
    assertEqual((await backend.waitForCall("A", "set_computer_generator_road", at)).args, { road: "claude_login" });
    assertEqual(backend.settings.computer_generator_road, "claude_login");
    await pageA.evaluate(() => applySettingsSnapshot({ computer_generator_road: "off" }));
    assertEqual(await checked(), "off", "the snapshot's road is not painted");
    await pageA.evaluate(() => applySettingsSnapshot({ computer_generator_road: "gemini_login" }));
    assertEqual(await checked(), "auto", "a road the core does not name was painted");
  } finally {
    delete backend.generatorAccounts;
    delete backend.generatorMissingClis;
    delete backend.generatorLast;
    backend.settings.computer_generator_road = "auto";
  }
  return "5 roads · 2 logins · passed over said";
});

await test("분리 전에 손으로 끈 스킬 검색은 업데이트 뒤에도 스킬 제안을 끈 채로 둔다 — 제안은 제 낱말이 생길 때까지 검색의 낱말을 따른다", async () => {
  await openJudgmentSettings(pageA);
  backend.keychain.set(TYPESAFE_SERVICE, "apikey_fixture");
  const toggle = pageA.locator("#jev-enabled");
  const suggestion = JEV_SEATS.find((seat) => seat.id === "skill_suggestion");
  const search = JEV_SEATS.find((seat) => seat.id === "skills");
  // A file written before the skill suggestion was a seat of its own: Jev on
  // in every folder and every feature turned off by hand, the skill search
  // among them, and no word for the suggestion, which read the search's
  // until the split (t-6877, the coordinator's migration contract m-8181).
  const byHand = Object.fromEntries(
    JEV_SEATS.filter((seat) => seat !== suggestion).map((seat) => [seat.setting, "off"]),
  );
  try {
    backend.zoSettings.smart = { ...byHand, [JEV_SETTINGS_KEY]: { enabled: true, workspaces: [JEV_EVERY_WORKSPACE] } };
    await pageA.evaluate(() => refreshApiRouters());
    await renderSettled(pageA);
    assert(!(await toggle.isChecked()), "the update turned the suggestion on though its person had turned the search off");
    // A word of the suggestion's own is its own: it asks, and the switch
    // says Jev is in use, while the search's own word stands.
    backend.zoSettings.smart = { ...backend.zoSettings.smart, [suggestion.setting]: "auto" };
    await pageA.evaluate(() => refreshApiRouters());
    await pageA.waitForFunction(() => document.getElementById("jev-enabled")?.checked === true, null, { timeout: UI_TIMEOUT });
    assertEqual(backend.zoSettings.smart[search.setting], "off", "the search's own word moved");
  } finally {
    delete backend.zoSettings.smart;
    backend.keychain.delete(TYPESAFE_SERVICE);
  }
  await pageA.evaluate(() => refreshApiRouters());
  return `${suggestion.id} follows ${search.setting} until it has a word of its own`;
});

await test("카드가 제시하는 판단은 실제로 불릴 수 있어야 한다 — 분류기가 라우팅 자리 앞에 선다", async () => {
  await openJudgmentSettings(pageA);
  backend.keychain.set(TYPESAFE_SERVICE, "apikey_fixture");
  delete backend.zoSettings.smart;
  await pageA.evaluate(() => refreshApiRouters());
  await renderSettled(pageA);
  const said = (key, fallback) => pageA.evaluate(([one, words]) => t(one, words), [key, fallback]);
  const unreachable = pageA.locator("[data-jev-unreachable]");
  // The classifier stands on a card of its own on the orchestration pane
  // (t-75762 v2, decision 4; the draft passed 2026-10-27).
  await openSettings(pageA, "orchestration");
  const choices = pageA.locator('#route-classifier-choices input[type="radio"]');
  const choose = async (mode) => {
    await pageA.locator(`#route-classifier-choices input[value="${mode}"]`).click();
  };
  const chosen = () => pageA.evaluate(() => document.querySelector("#route-classifier-choices input:checked")?.value ?? null);

  // The gate is on the card, and it offers the four words the classifier has —
  // named by what each does, never by the word it writes.
  assertEqual(
    await choices.evaluateAll((all) => all.map((input) => input.value)),
    CLASSIFIER_MODES.map((choice) => choice.mode),
    "the card does not offer the classifier's own words",
  );
  assertEqual(
    await pageA.locator("#route-classifier-choices .route-classifier-name").evaluateAll((all) => all.map((name) => name.textContent)),
    [
      await said("settings.classifier.modeOff", "끔"),
      await said("settings.classifier.modeWords", "키워드만"),
      await said("settings.classifier.modeMarkers", "키워드 + 과업 표시"),
      await said("settings.classifier.modeProbed", "키워드 + 빠른 모델 확인"),
    ],
    "a classifier choice is not named by what it does",
  );
  // Nothing written means the probing word, which is zo's own reading of an
  // absent key — so an untouched machine reaches the seat.
  assertEqual(await chosen(), CLASSIFIER_MODES.at(-1).mode, "an untouched settings file did not read as the probing word");

  // A word that calls no probe still reaches the routing seat (t-6346): the
  // change says only that no Fast-tier model is asked, and no warning stands.
  const quiet = CLASSIFIER_MODES.find((choice) => choice.runs && !choice.probes).mode;
  const chosenAt = backend.calls.length;
  await choose(quiet);
  assertEqual((await backend.waitForCall("A", "set_route_classifier", chosenAt)).args.mode, quiet);
  await pageA.waitForFunction(
    (words) => document.querySelector("#route-classifier-status .settings-status-said")?.textContent === words,
    await said("settings.classifier.nowQuiet", "바꿨습니다 — 이 방식은 빠른 등급 모델에게 묻지 않습니다."),
    { timeout: UI_TIMEOUT },
  );
  assertEqual(backend.zoSettings.smart.autoClassifier, quiet, "the choice was not written to zo's settings");
  assert(await unreachable.count() === 1, "exactly one warning stands beside the classifier");
  assert(
    await unreachable.isHidden(),
    "a feature that asks nothing yet was warned about a mode it is not in",
  );

  // Turn Jev on: the gated feature now stands at its recommendation and asks,
  // and a word that calls no probe still reaches it — nothing to warn about.
  const onAt = backend.calls.length;
  await openJudgmentSettings(pageA);
  await pageA.locator("#jev-enabled").click();
  await backend.waitForCall("A", "set_jev_enabled", onAt);
  await openSettings(pageA, "orchestration");
  await renderSettled(pageA);
  assert(
    await unreachable.isHidden(),
    "the card warned that a word which reaches the feature cannot reach it",
  );

  // Automatic routing off is the one word nothing reaches the feature under:
  // the change says so, and the card says why the feature asks nothing.
  const off = CLASSIFIER_MODES.find((choice) => !choice.reaches).mode;
  const offAt = backend.calls.length;
  await choose(off);
  await backend.waitForCall("A", "set_route_classifier", offAt);
  await pageA.waitForFunction(() => !document.querySelector("[data-jev-unreachable]").hidden, null, { timeout: UI_TIMEOUT });
  assertEqual(
    await statusSaid(pageA, "route-classifier-status"),
    await said("settings.classifier.nowOff", "자동 라우팅을 껐습니다 — 「모델 선택 판단」도 판단을 요청하지 않습니다."),
  );
  assertEqual(
    (await unreachable.textContent()).trim(),
    await said("settings.typesafe.routingUnreachable", "자동 라우팅이 꺼져 있어 「모델 선택 판단」은 판단을 요청하지 않습니다. 「끔」이 아닌 방식을 고르면 요청합니다."),
    "the classifier does not say why the feature it gates cannot be reached",
  );

  // Put the probing word back and the warning goes with it.
  const probing = CLASSIFIER_MODES.at(-1).mode;
  const backAt = backend.calls.length;
  await choose(probing);
  await backend.waitForCall("A", "set_route_classifier", backAt);
  await pageA.waitForFunction(() => document.querySelector("[data-jev-unreachable]").hidden, null, { timeout: UI_TIMEOUT });
  assertEqual(
    await statusSaid(pageA, "route-classifier-status"),
    await said("settings.classifier.nowProbes", "이제 빠른 등급 모델에게도 묻습니다."),
  );

  // A refused change keeps the card as it stood.
  backend.typesafeSetFailure = true;
  try {
    const refusedAt = backend.calls.length;
    await choose(quiet);
    await backend.waitForCall("A", "set_route_classifier", refusedAt);
    await pageA.waitForFunction(
      (word) => document.querySelector("#route-classifier-choices input:checked")?.value === word,
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
  // Compare the same unfocused state; the initial input focus owns a different border.
  await pageA.evaluate(() => document.activeElement?.blur());
  await settlePaint(pageA);
  // 이 칸의 select는 형제 input과 같은 얼굴이어야 한다. 오래 `settings-select`를
  // 입고 있었는데 이 창의 어떤 스타일시트도 그 이름을 선언하지 않는다 — 밑줄도
  // 얼굴도 없는 네이티브 셀렉트로 서고, 그 옆에는 `.settings-field:has(select)`가
  // 그린 셰브런만 떠 있었다. 재는 것은 클래스 목록이 아니라 그려진 결과다.
  const hostFaces = await pageA.evaluate(() => {
    const face = id => {
      const paint = getComputedStyle(document.getElementById(id));
      return [paint.borderBottomWidth, paint.borderBottomColor, paint.backgroundColor, paint.fontFamily];
    };
    return { select: face("remote-workspace-host"), input: face("remote-workspace-label") };
  });
  assertEqual(hostFaces.select, hostFaces.input, "the host select is not wearing the field face its siblings wear");
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
  // t-78091: three nights missed, the first an hour away from where counting
  // back from the row's own date in local minutes lands — the backend dated
  // it across a daylight-saving change.
  const missedFirstAt = 1_756_600_000_000 - 2 * 86_400_000 + 3_600_000;
  backend.automationRuns = [{
    id: "1756600000000-missed-auto-qa",
    automation_id: "auto-qa",
    scheduled_for_epoch_minutes: 28_996_000,
    at_epoch_ms: 1_756_600_000_000,
    root: BOOT_BASE.project_root,
    term: 0,
    made_worktree: null,
    skipped: null,
    failure: null,
    missed: { count: 3, first_epoch_minutes: 28_996_000 - 2 * 1440, first_at_epoch_ms: missedFirstAt },
    ended_at_epoch_ms: null,
    exit_code: null,
    completed_at_epoch_ms: null,
    result: null,
    evidence_dir: null,
  }, {
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
    // The first missed night is the instant the backend dated (t-78091), not
    // the row's date counted back in local minutes.
    assertEqual(
      await pageA.locator("#auto-runs-list .auto-run-missed").getAttribute("data-tip"),
      await pageA.evaluate((at) => t("auto.runMissedFirst", "처음 놓친 회차 {{at}}", { at: runClock(at) }), missedFirstAt),
      "the first missed night was counted back from the row instead of read",
    );
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
    ["claude_autoswitch", "set_claude_autoswitch_mode", "auto", "provider-accounts"],
    ["setup_script_launch_mode", "set_setup_script_launch_mode", "split-horizontal", "terminal"],
    ["agent_pane_first_view", "set_agent_pane_first_view", "terminal", "terminal"],
    ["terminal_shortcut_policy", "set_terminal_shortcut_policy", "terminal-first", "shortcuts"],
    ["workspace_directory", "patch_workspace_creation_prefs", "/tmp/zerocode-workspaces", "general"],
    ["nest_workspaces", "patch_workspace_creation_prefs", false, "general"],
    ["default_agent", "set_default_agent", "blank", "agents"],
    ["harness_mode", "set_harness_settings", "stop", "provider-accounts"],
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
  // The middle-click paste follows the platform until someone writes it (shell-term.js), and its default is off on
  // Windows: flip the box from where it stands, so the write changes the canonical state on every platform.
  await gestureAndWait(pageA, "A", "patch_editing_prefs", async () => {
    const middle = "#editing-primary-selection-middle-click-paste";
    await pageA.setChecked(middle, !(await pageA.isChecked(middle)));
  });
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
  // t-73869: the pane's grid answers `OSC 10/11/12 ; ?` with what the window
  // paints, so the renderer reports the resolved palette — theme plus the
  // person's overrides — whenever it changes.
  const paintedReport = backend.last("A", "report_terminal_colors")?.args?.report;
  assertEqual(
    {
      foreground: paintedReport?.foreground,
      background: paintedReport?.background,
      brightBlue: paintedReport?.ansi?.[12],
      named: paintedReport?.ansi?.length,
    },
    { foreground: "#f8f8f2", background: "#102030", brightBlue: "#abcdef", named: 16 },
    "the renderer painted a terminal palette it never reported to the shell",
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
  assertEqual(
    backend.settings.agent_pane_first_view,
    "terminal",
    "the agent pane's first view did not persist independently",
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
    optionAsAltHidden: document.getElementById("term-option-as-alt-field")?.hidden,
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
    // What the next agent pane opens on is the boot's word before any pane opens.
    agentPaneFirstView,
    agentPaneFirstViewChecked: document.querySelector("#agent-pane-first-view input:checked")?.value,
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
    optionAsAlt: TEST_PLATFORM === "darwin" ? "left" : "",
    optionAsAltOptions: TEST_PLATFORM === "darwin" ? ["auto", "true", "left", "right", "false"] : [],
    optionAsAltHidden: TEST_PLATFORM !== "darwin",
    jisYenToBackslash: true,
    jisYenHidden: TEST_PLATFORM !== "darwin",
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
    agentPaneFirstView: "terminal",
    agentPaneFirstViewChecked: "terminal",
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
  { kind: "claude_autoswitch", command: "set_claude_autoswitch_mode", pane: "provider-accounts", asked: "off", canonical: "auto" },
  { kind: "harness_mode", command: "set_harness_settings", pane: "provider-accounts", asked: "off", canonical: "stop" },
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
          claude_autoswitch: () => document.getElementById("account-autoswitch")?.value,
          harness_mode: () => document.getElementById("harness-gate-mode")?.value,
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

await test("the harness card shows what a person set and what the window counted, and saves each field as the person typed it (t-26583)", async () => {
  // The status sentence below is read in Korean; the persisted writes above leave
  // the page in Japanese, so the language is set first and given back at the end.
  const previousLocale = backend.settings.locale;
  await backend.externalPatch({ locale: "ko" }, ["locale"]);
  await renderSettled(pageB);
  backend.settings.harness = clone(DEFAULT_SETTINGS.harness);
  await reopenSettings(pageB, "B", "provider-accounts");
  const card = () => pageB.evaluate(() => {
    const field = (id) => document.getElementById(id);
    return {
      mode: field("harness-gate-mode")?.value ?? null,
      task: field("harness-task-usd")?.value ?? null,
      day: field("harness-day-usd")?.value ?? null,
      concurrent: field("harness-concurrent")?.value ?? null,
      perHour: field("harness-per-hour")?.value ?? null,
      perDay: field("harness-per-day")?.value ?? null,
      alerts: field("harness-alerts-landing")?.value ?? null,
      alertsHint: field("harness-alerts-landing-hint")?.textContent ?? null,
      status: field("harness-status")?.textContent ?? null,
    };
  });
  // The counts are asked for when the pane is opened and come a beat after it.
  const said = await pageB.waitForFunction(
    () => (document.getElementById("harness-status")?.textContent ?? "") !== "",
    null,
    { timeout: UI_TIMEOUT },
  ).then(() => true, () => false);
  assert(said, "the card never said what the window counted");
  const shown = await card();
  assert(
    shown.mode === "notify" && shown.task === "" && shown.day === "" &&
      shown.concurrent === "4" && shown.perHour === "300" && shown.perDay === "2000" &&
      shown.alerts === "on",
    "the card does not show the document's gate and ceilings", shown,
  );
  assert(
    /^지금 1개 실행 중 · 최근 한 시간 3번 · 오늘 12번 · 쉬는 중: [Cc]laude 1[01]분 · 오늘 워커 비용 약 \$4\.20$/.test(shown.status ?? ""),
    "the card does not say what the launch ledger and the day's spend hold", shown.status,
  );
  // What is sent, to whom, and that nothing is merged or deleted, is said beside the switch (t-34501).
  assert(
    /코디네이터/.test(shown.alertsHint ?? "") && /워커/.test(shown.alertsHint ?? "") &&
      /병합|삭제/.test(shown.alertsHint ?? ""),
    "the switch does not say what it sends and to whom", shown.alertsHint,
  );
  assert(await pageB.$("#harness-task-usd"), "the card has no task budget field");
  const budget = await gestureAndWait(pageB, "B", "set_harness_settings", () =>
    changeField(pageB, "#harness-task-usd", 12.5));
  assertEqual(
    budget.args,
    {
      harness: {
        gate: { mode: "notify", task_usd: 12.5, day_usd: null },
        launches: { concurrent: 4, per_hour: 300, per_day: 2000 },
        alerts: { landing: true },
      },
    },
    "a budget was sent as something other than the whole record with the number typed",
  );
  await waitForSettingsIdle(pageB);
  const lifted = await gestureAndWait(pageB, "B", "set_harness_settings", () =>
    changeField(pageB, "#harness-per-day", ""));
  assertEqual(
    lifted.args.harness,
    {
      gate: { mode: "notify", task_usd: 12.5, day_usd: null },
      launches: { concurrent: 4, per_hour: 300, per_day: null },
      alerts: { landing: true },
    },
    "a blank ceiling is no ceiling, and the budget saved before it stays",
  );
  await waitForSettingsIdle(pageB);
  await gestureAndWait(pageB, "B", "set_harness_settings", () =>
    changeField(pageB, "#harness-per-hour", 1.5));
  await waitForSettingsIdle(pageB);
  const refused = await card();
  assert(
    refused.perHour === "300" && refused.perDay === "" && refused.task === "12.5",
    "a ceiling the backend refused did not go back to the saved one, or took the others with it", refused,
  );
  assertEqual(
    backend.settings.harness.launches,
    { concurrent: 4, per_hour: 300, per_day: null },
    "a refused ceiling reached the stored document",
  );
  // The ledger's alerts are on until a person says otherwise, and turning them off is one saved word.
  const silenced = await gestureAndWait(pageB, "B", "set_harness_settings", () =>
    pageB.selectOption("#harness-alerts-landing", "off"));
  assertEqual(
    silenced.args.harness.alerts,
    { landing: false },
    "the switch did not send the alerts as off",
  );
  await waitForSettingsIdle(pageB);
  assertEqual(backend.settings.harness.alerts, { landing: false }, "the stored document does not hold the alerts off");
  await backend.externalPatch({ locale: previousLocale }, ["locale"]);
  await renderSettled(pageB);
});

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

  // Back to the fresh-install document the suite booted with — every item,
  // the serialized Rust default, and no switch of the person's on record.
  await backend.externalPatch({
    status_bar_items: RUST_DEFAULT_DOCUMENT.status_bar_items,
    status_bar_items_off: RUST_DEFAULT_DOCUMENT.status_bar_items_off,
  }, ["status_bar_items", "status_bar_items_off"]);
});

await test("a stored status-bar id the catalog no longer offers is skipped, never painted", async () => {
  // An install that saved its list while a gauge the catalog has since retired
  // still boots, still shows every item it DOES know, and raises no error for
  // the one it does not — the stored id is skipped, not offered.
  const stale = ["claude", "codex", "antigravity", "kimi", "grok", "opencode-go", "resource-usage", "ports", "retired-gauge"];
  try {
    await backend.externalPatch(
      { status_bar_items: stale, status_bar_items_seen: stale },
      ["status_bar_items", "status_bar_items_seen"],
    );
    await openSettings(pageA, "appearance");
    const controls = await pageA.$$eval("[data-status-bar-item]", (fields) =>
      fields.map((field) => field.dataset.statusBarItem));
    assertEqual(
      controls,
      stale.filter((item) => item !== "retired-gauge"),
      "Appearance offers a status-bar item no segment stands behind",
    );
    assertEqual(
      await pageA.evaluate(() => ({
        enabled: statusBarItemEnabled("retired-gauge"),
        segment: document.getElementById("sb-retired-gauge") !== null,
        known: [...statusBarItems].sort(),
      })),
      { enabled: false, segment: false, known: [...controls].sort() },
      "a stored id with no catalog row reached the status bar",
    );
  } finally {
    // Back to the fresh-install document — the serialized Rust default — even
    // when an assertion above fails, so the suites after this one start clean.
    await backend.externalPatch(
      {
        status_bar_items: RUST_DEFAULT_DOCUMENT.status_bar_items,
        status_bar_items_seen: RUST_DEFAULT_DOCUMENT.status_bar_items_seen,
      },
      ["status_bar_items", "status_bar_items_seen"],
    );
  }
});

/* t-70039: the Antigravity switch and the bar stand together. The person said
 * twice that the gauge was gone (10-21, 10-22): their document had it off, and
 * ticking it back showed nothing until the window's next look — the gauge was
 * asked only on focus, on an agent's turn, or every fifteen minutes. Off is
 * one item patch the backend keeps as the person's own record
 * (`status_bar_items_off`); on asks the gauge at once. The window's focus
 * listener stands down for the measure, so only the switch can ask. */
await test("a status-bar gauge switched on is asked at once, and switched off is the person's record", async () => {
  await pageA.evaluate(() => window.removeEventListener("focus", noteWindowFocused));
  try {
    await backend.externalPatch(
      {
        status_bar_items: RUST_DEFAULT_DOCUMENT.status_bar_items,
        status_bar_items_off: RUST_DEFAULT_DOCUMENT.status_bar_items_off,
      },
      ["status_bar_items", "status_bar_items_off"],
    );
    await openSettings(pageA, "appearance");
    assert(
      await pageA.isChecked("#status-bar-antigravity"),
      "the Antigravity switch does not stand as the document has it",
    );

    let from = backend.calls.length;
    await pageA.uncheck("#status-bar-antigravity");
    const off = await backend.waitForCall("A", "set_status_bar_item", from);
    assertEqual(off.args, { item: "antigravity", enabled: false }, "Antigravity off sent sibling state");
    assertEqual(
      backend.settings.status_bar_items_off,
      ["antigravity"],
      "the switch off is not kept as the person's own record",
    );
    assertEqual(
      await pageA.evaluate(() => document.getElementById("sb-antigravity").hidden),
      true,
      "the bar kept a gauge its switch turned off",
    );

    from = backend.calls.length;
    await pageA.check("#status-bar-antigravity");
    await backend.waitForCall("A", "antigravity_usage", from);
    const on = await backend.waitForCall("A", "set_status_bar_item", from);
    assertEqual(on.args, { item: "antigravity", enabled: true }, "Antigravity on sent sibling state");
    assertEqual(
      backend.settings.status_bar_items_off,
      [],
      "switching the gauge back on left it on the person's off record",
    );
  } finally {
    await pageA.evaluate(() => window.addEventListener("focus", noteWindowFocused));
    await backend.externalPatch(
      {
        status_bar_items: RUST_DEFAULT_DOCUMENT.status_bar_items,
        status_bar_items_off: RUST_DEFAULT_DOCUMENT.status_bar_items_off,
      },
      ["status_bar_items", "status_bar_items_off"],
    );
  }
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
  await withoutAmbientReads([pageA, pageB], async () => {
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
  await pageB.selectOption("#notify-finish-mode", "off");
  const completion = await backend.waitForCall("B", "set_finish_notification_mode", from);
  assertEqual(completion.args, { mode: "off" }, "finish mode sent the wrong word");
  assertEqual(
    backend.settings.notifications,
    { enabled: true, agent_attention: false, agent_completion: "off" },
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
      && (await pageA.isDisabled("#notify-finish-mode")),
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
  // Awaited below. A failed wait before that must not leave this rejection
  // unhandled: Node would end the whole suite before its summary.
  oldRequest.catch(() => {});
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

  // 주간 리뷰를 창 자동화로(t-77377) — 스위치는 저장된 볼트가 있어야 살고, 켜면
  // 백엔드가 볼트에서 zo로 도는 창의 일을 자동화 목록에 쓴 뒤 설정을 쓰며, 카드는
  // 상태를 다시 물어 영수증 줄에 그 일의 다음·마지막 실행을 적는다. 끄면 일이
  // 걷히고 줄이 사라진다.
  const switchOn = backend.calls.length;
  assert(
    await pageB.evaluate(() => document.getElementById("second-brain-weekly-review")?.disabled === false),
    "the weekly-review switch is dead although a vault is saved",
  );
  await pageB.click("#second-brain-weekly-review");
  const turnedOn = await backend.waitForCall("B", "set_second_brain_weekly_review", switchOn);
  assertEqual(turnedOn.args, { enabled: true }, "the switch did not ask to write the review's job");
  await backend.waitForCall("B", "second_brain_status", switchOn);
  const nextRun = await pageB.evaluate(() => localMinuteText(29_891_880));
  await pageB.waitForFunction((next) =>
    document.getElementById("second-brain-weekly-review-receipt")?.hidden === false
      && document.getElementById("second-brain-weekly-review-receipt").textContent.includes(next),
  nextRun, { timeout: 5_000 }).catch(() => {});
  const receipt = await pageB.evaluate(() => ({
    line: document.getElementById("second-brain-weekly-review-receipt").textContent,
    expected: t(
      "settings.secondBrain.reviewRegistered",
      "창 자동화로 등록됨 · 다음 실행 {{next}} · 마지막 실행 {{last}}",
      { next: localMinuteText(29_891_880), last: t("settings.secondBrain.reviewNever", "아직 없음") },
    ),
    checked: document.getElementById("second-brain-weekly-review").checked,
  }));
  assertEqual(
    [receipt.line, receipt.checked, backend.settings.second_brain_weekly_review],
    [receipt.expected, true, true],
    "the receipt line does not say when the review's job runs next and last ran",
  );
  // 자동화 목록에서 꺼 둔 일은 그렇다고 말한다 — 켜진 스위치가 열리지 않을 턴을
  // 약속하지 않는다.
  backend.secondBrain = {
    ...backend.secondBrain,
    weekly_review: { registered: true, enabled: false, next_run_at: null, last_run_at: 29_891_880 },
  };
  const pausedAt = backend.calls.length;
  await pageB.click("#second-brain-refresh");
  await backend.waitForCall("B", "second_brain_status", pausedAt);
  const pausedText = await pageB.evaluate(() => t(
    "settings.secondBrain.reviewPaused",
    "자동화 목록에서 꺼져 있습니다 · 거기서 켜거나 스위치를 껐다 다시 켜 주세요",
  ));
  await pageB.waitForFunction((text) =>
    document.getElementById("second-brain-weekly-review-receipt")?.textContent === text,
  pausedText, { timeout: 5_000 }).catch(() => {});
  assertEqual(
    await pageB.evaluate(() => document.getElementById("second-brain-weekly-review-receipt").textContent),
    pausedText,
    "a review switched off on the automation screen still reads as scheduled",
  );
  const switchOff = backend.calls.length;
  await pageB.click("#second-brain-weekly-review");
  const turnedOff = await backend.waitForCall("B", "set_second_brain_weekly_review", switchOff);
  assertEqual(turnedOff.args, { enabled: false }, "the switch did not ask to take the review's job away");
  await pageB.waitForFunction(() =>
    document.getElementById("second-brain-weekly-review-receipt")?.hidden === true
      && document.getElementById("second-brain-weekly-review").checked === false);
  assertEqual(backend.settings.second_brain_weekly_review, false, "switching off did not clear the setting");
  // A refusal (no saved vault, a store that cannot be written) rolls the
  // switch back to what the document says: a switch reading "on" while no
  // job stands would be the card promising a turn nobody opens.
  backend.rejectOnce("set_second_brain_weekly_review");
  const refusedAt = backend.calls.length;
  await pageB.click("#second-brain-weekly-review");
  await backend.waitForCall("B", "set_second_brain_weekly_review", refusedAt);
  await backend.waitForCall("B", "settings_snapshot", refusedAt);
  await pageB.waitForFunction(() =>
    document.getElementById("second-brain-weekly-review").checked === false
      && document.getElementById("second-brain-weekly-review-receipt")?.hidden === true);
  assertEqual(backend.settings.second_brain_weekly_review, false, "a refused switch changed the setting");

  // raw 받은편지함(t-43992 단위 1) — 에이전트가 이 창의 브라우저로 통째로 읽은
  // 바깥 페이지를 볼트의 raw/에 새 파일로 모으는 스위치. 처음엔 꺼져 있고(옵트인),
  // 저장된 볼트가 있으면 살며, 켜고 끄는 것은 설정 한 칸을 쓴다. 거절은 스위치를
  // 문서가 말하는 자리로 되돌린다.
  const rawSwitch = await pageB.evaluate(() => {
    const box = document.getElementById("second-brain-raw-capture");
    return box === null ? null : { checked: box.checked, disabled: box.disabled };
  });
  assertEqual(rawSwitch, { checked: false, disabled: false },
    "the raw-capture switch is missing, dead with a vault saved, or on before the person turned it on");
  const rawOn = backend.calls.length;
  await pageB.click("#second-brain-raw-capture");
  const rawTurnedOn = await backend.waitForCall("B", "set_second_brain_raw_capture", rawOn);
  assertEqual(rawTurnedOn.args, { enabled: true }, "the raw-capture switch did not ask to turn the feed on");
  await pageB.waitForFunction(() => document.getElementById("second-brain-raw-capture").checked === true);
  assertEqual(backend.settings.second_brain_raw_capture, true, "turning the raw feed on did not write the setting");
  const rawOff = backend.calls.length;
  await pageB.click("#second-brain-raw-capture");
  const rawTurnedOff = await backend.waitForCall("B", "set_second_brain_raw_capture", rawOff);
  assertEqual(rawTurnedOff.args, { enabled: false }, "the raw-capture switch did not ask to turn the feed off");
  await pageB.waitForFunction(() => document.getElementById("second-brain-raw-capture").checked === false);
  assertEqual(backend.settings.second_brain_raw_capture, false, "turning the raw feed off did not clear the setting");
  backend.rejectOnce("set_second_brain_raw_capture");
  const rawRefusedAt = backend.calls.length;
  await pageB.click("#second-brain-raw-capture");
  await backend.waitForCall("B", "set_second_brain_raw_capture", rawRefusedAt);
  await backend.waitForCall("B", "settings_snapshot", rawRefusedAt);
  await pageB.waitForFunction(() => document.getElementById("second-brain-raw-capture").checked === false);
  assertEqual(backend.settings.second_brain_raw_capture, false, "a refused raw-capture switch changed the setting");

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

// t-77381: 볼트의 매일 취합이 10-19에 꺼지고 여드레 동안 아무도 몰랐다. 카드는 그 자동화가
// 꺼졌거나 없으면 그렇다고 말하고 「켜기」를 세우며, 켜져 있어도 하루 넘게 밀렸으면
// 경고한다. 「켜기」는 있는 자동화를 켜고, 없으면 「raw 자동 취합」 템플릿으로 만든다.
await test("세컨드 브레인 카드의 매일 취합 줄은 꺼지거나 하루 넘게 밀린 취합을 말하고, 켜기는 그 자동화를 켜거나 템플릿으로 만든다 (t-77381)", async () => {
  const vault = "/Users/fixture/Knowledge";
  const lastRun = 29_871_060;
  const heldAutomations = backend.automations;
  backend.settings.second_brain_vault = vault;
  backend.secondBrain = {
    ...backend.secondBrain,
    saved_path: vault,
    daily_ingest: {
      automation_id: "ingest-1",
      name: "raw 자동 취합",
      enabled: false,
      last_run_at: lastRun,
      overdue_since: null,
    },
  };
  backend.automations = [{
    id: "ingest-1",
    name: "raw 자동 취합",
    enabled: false,
    workspace: vault,
    prompt: "raw/에 있지만 wiki/에 아직 반영되지 않은 항목을 모두 취합하고 wiki/log.md에 기록해 줘.",
    schedule: { cadence: "daily", time: "19:00", day_of_week: 1, custom: "" },
    last_run_at: lastRun,
  }];
  const ingestLine = () => pageB.evaluate(() => {
    const warning = document.getElementById("second-brain-ingest-warning");
    return {
      shown: document.getElementById("second-brain-ingest-field")?.hidden === false,
      line: document.getElementById("second-brain-ingest")?.textContent ?? "",
      state: document.getElementById("second-brain-ingest")?.dataset.state ?? "",
      warning: warning && !warning.hidden
        ? document.getElementById("second-brain-ingest-warning-text").textContent
        : null,
      turnOn: document.getElementById("second-brain-ingest-on")?.hidden === false,
      off: t("auto.off", "꺼짐"),
    };
  });
  const warningIs = (shown) => pageB.waitForFunction((want) => {
    const warning = document.getElementById("second-brain-ingest-warning");
    return Boolean(warning) && warning.hidden === !want;
  }, shown);

  // 꺼진 취합: 줄이 「꺼짐」을 경고색으로 말하고, 경고가 raw/가 wiki/에 닿지 않는다고
  // 말하며, 「켜기」가 선다.
  const openedAt = backend.calls.length;
  await openSettings(pageB, "integrations");
  await backend.waitForCall("B", "second_brain_status", openedAt);
  await warningIs(true);
  const off = await ingestLine();
  assert(off.shown && off.line === off.off && off.state === "warn"
    && off.warning?.includes("raw/") && off.turnOn,
  "a daily ingest that is off was not said to be off with a switch to turn it on", off);

  // 「켜기」는 그 자동화를 켜고 상태를 다시 묻는다. 경고는 사라지고 줄은 마지막 실행을 말한다.
  const turnedAt = backend.calls.length;
  await pageB.click("#second-brain-ingest-on");
  const enabled = await backend.waitForCall("B", "enable_automation", turnedAt);
  assertEqual(enabled.args, { id: "ingest-1", enabled: true },
    "the switch turned on something other than the vault's ingest");
  await backend.waitForCall("B", "second_brain_status", turnedAt);
  await warningIs(false);
  const on = await ingestLine();
  assert(on.shown && on.line !== on.off && on.line !== "" && on.state !== "warn" && !on.turnOn,
    "a daily ingest that is on still read as off, or kept its switch", on);

  // 켜져 있어도 하루 넘게 밀리면 경고한다 — 켤 것이 없으니 스위치는 없다.
  backend.secondBrain.daily_ingest = {
    ...backend.secondBrain.daily_ingest,
    enabled: true,
    overdue_since: lastRun + 24 * 60,
  };
  const lateAt = backend.calls.length;
  await pageB.click("#second-brain-refresh");
  await backend.waitForCall("B", "second_brain_status", lateAt);
  await warningIs(true);
  const late = await ingestLine();
  assert(late.state === "warn" && late.line !== late.off && late.warning && !late.turnOn,
    "a daily ingest a day behind its schedule was not warned about", late);

  // 볼트를 취합하는 자동화가 없으면 「켜기」는 템플릿으로 하나를 만든다.
  backend.automations = [];
  backend.secondBrain.daily_ingest = {
    automation_id: null,
    name: null,
    enabled: false,
    last_run_at: null,
    overdue_since: null,
  };
  const noneAt = backend.calls.length;
  await pageB.click("#second-brain-refresh");
  await backend.waitForCall("B", "second_brain_status", noneAt);
  await warningIs(true);
  const none = await ingestLine();
  assert(none.line === none.off && none.turnOn,
    "a vault with no ingest job was not said to have its daily ingest off", none);
  const madeAt = backend.calls.length;
  await pageB.click("#second-brain-ingest-on");
  const made = await backend.waitForCall("B", "save_automation", madeAt);
  const job = made.args.automation;
  assert(job.workspace === vault && job.prompt.includes("raw/") && job.enabled === true
    && job.schedule.cadence === "daily" && job.schedule.time === "19:00",
  "the switch did not make the raw ingestion job from its template", job);
  await backend.waitForCall("B", "second_brain_status", madeAt);
  await warningIs(false);

  delete backend.secondBrain.daily_ingest;
  backend.automations = heldAutomations;
});

// t-55424: the card's 「스킬」 line reads the second-brain skill's copies the
// way the Skills screen reads them, in every home an installed agent reads
// (the window's own Claude home among them), and the global-link line names
// each agent once although Claude now keeps a guide in two homes.
await test("the second-brain card says whether the installed skill is current", async () => {
  const appClaude = "/Users/fixture/Library/Application Support/ZeroCode/.claude";
  const skillAt = (root) => `${root}/skills/second-brain/SKILL.md`;
  backend.secondBrain = {
    ...backend.secondBrain,
    guides: [
      { agent: "claude", label: "Claude", path: "/Users/fixture/.claude/CLAUDE.md", state: "linked", detail: "" },
      { agent: "claude", label: "Claude", path: `${appClaude}/CLAUDE.md`, state: "linked", detail: "" },
      { agent: "codex", label: "Codex", path: "/Users/fixture/.codex/AGENTS.md", state: "linked", detail: "" },
    ],
    skill_copies: [
      { agent: "claude", label: "Claude", path: skillAt("/Users/fixture/.claude"), state: "current" },
      { agent: "claude", label: "Claude", path: skillAt(appClaude), state: "missing" },
      { agent: "codex", label: "Codex", path: skillAt("/Users/fixture/.codex"), state: "stale" },
    ],
  };
  const read = async () => {
    const from = backend.calls.length;
    await pageB.evaluate(() => refreshSecondBrain());
    await backend.waitForCall("B", "second_brain_status", from);
    return pageB.evaluate(() => {
      const line = document.getElementById("second-brain-skill");
      const count = (key, fallback) => t("skills.copyCount", "{{state}} {{count}}곳",
        { state: t(key, fallback), count: 1 });
      return {
        skill: line?.textContent ?? null,
        tip: line?.dataset.tip ?? "",
        mixed: [count("skills.copyStale", "낡음"), count("skills.copyMissing", "없음")].join(" · "),
        current: t("skills.copyCurrent", "최신"),
        guides: document.getElementById("second-brain-guides").textContent,
        linked: t("settings.secondBrain.linked", "{{agents}}에 연결됨", { agents: "Claude·Codex" }),
      };
    });
  };
  const mixed = await read();
  assertEqual(
    [mixed.skill, mixed.guides],
    [mixed.mixed, mixed.linked],
    "the card did not say which second-brain copies are outdated or missing, or named Claude twice",
  );
  assert(mixed.tip.includes(skillAt(appClaude)),
    "the skill line did not name the window's own Claude copy");
  backend.secondBrain = {
    ...backend.secondBrain,
    skill_copies: backend.secondBrain.skill_copies.map((row) => ({ ...row, state: "current" })),
  };
  const current = await read();
  assertEqual(current.skill, current.current, "the card did not say the installed skill is current");
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
    {
      id: "1".repeat(32),
      name: "일하는 계정",
      lastApply: { applied: 121, refused: 1, retained: 0, dropped: 24, atUnix: 1793150000 },
    },
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
  // 주입이 끝난 프로필은 마지막 결과를 한 줄로 든다 — 거절이 있어도 받은 수가 먼저다.
  // 문구는 그때 창의 언어로 선다 — 두 수와 거절 갈래(「·」)만 본다.
  const outcome = await pageA.locator("#browser-profile-list > *").nth(0)
    .locator(".browser-profile-applied").textContent();
  assert(
    /121\D[^·]*·\D*1\D[^·]*·\D*24\D/.test(`${outcome} `),
    `the last injection's counts did not reach the card: ${JSON.stringify(outcome)}`,
  );
  assertEqual(
    await pageA.locator("#browser-profile-list > *").nth(1).locator(".browser-profile-applied").count(),
    0,
    "a profile never injected claimed an outcome",
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

/* 주입이 끝나면 창은 목록을 다시 읽어 「적용 대기」를 「N개 적용」으로 바꾸고,
 * 다 받지 못했으면 세 수로 문장을 지어 알린다. 전부 만료인 스테이징은 「0개
 * 적용」만이 아니라 건너뛴 수도 말한다. */
await test("a finished injection repaints the cards and says what it skipped", async () => {
  const id = "1".repeat(32);
  backend.defaultProfileId = null;
  backend.browserProfiles = [{ id, name: "일하는 계정", staged: true }];
  await openSettings(pageA, "browser");
  await pageA.waitForFunction(
    () => document.querySelectorAll("#browser-profile-list .browser-profile-chip").length === 1,
    undefined,
    { timeout: UI_TIMEOUT },
  );

  // 판이 쿠키를 다 앉혔다: 칩이 사라지고 결과 줄이 선다. 목록은 이 이벤트로 다시 읽힌다.
  const readAgain = backend.count("A", "browser_profiles");
  backend.browserProfiles = [{
    id,
    name: "일하는 계정",
    lastApply: { applied: 0, refused: 0, retained: 0, dropped: 2933, atUnix: 1793150000 },
  }];
  await pageA.evaluate((profile) => window.__SETTINGS_TEST_EMIT__("browser:cookie-import-applied", {
    label: "browser-1", profile, applied: 0, refused: 0, retained: 0, dropped: 2933,
  }), id);
  await pageA.waitForFunction(
    () => document.querySelectorAll("#browser-profile-list .browser-profile-applied").length === 1,
    undefined,
    { timeout: UI_TIMEOUT },
  );
  assert(
    backend.count("A", "browser_profiles") > readAgain,
    "the end of an injection did not re-read the profiles",
  );
  assertEqual(
    await pageA.locator("#browser-profile-list .browser-profile-chip").count(),
    0,
    "the card kept saying pending after its injection ended",
  );
  const card = await pageA.locator("#browser-profile-list .browser-profile-applied").textContent();
  assert(card.includes("2933"), `an all-expired staging did not say how many it skipped: ${JSON.stringify(card)}`);

  // 다 받지 못한 주입의 알림은 세 수로 지은 문장이다. 수가 없는 옛 알림만 백엔드 문장을 그대로 쓴다.
  await pageA.evaluate(() => {
    window.__cookieNotices = [];
    window.__realShowError = window.showError;
    window.showError = (message) => window.__cookieNotices.push(String(message));
  });
  try {
    await pageA.evaluate(() => {
      window.__SETTINGS_TEST_EMIT__("browser:cookie-import-error", {
        applied: 7, refused: 2, retained: 5, message: "backend sentence",
      });
      window.__SETTINGS_TEST_EMIT__("browser:cookie-import-error", { message: "older sentence" });
    });
    const notices = await pageA.evaluate(() => window.__cookieNotices);
    assertEqual(notices.length, 2, "an incomplete injection did not reach the person");
    assert(
      notices[0] !== "backend sentence" && ["7", "2", "5"].every((count) => notices[0].includes(count)),
      `the notice did not build its sentence from the three counts: ${JSON.stringify(notices[0])}`,
    );
    assertEqual(notices[1], "older sentence", "a notice without counts lost the backend's sentence");
  } finally {
    await pageA.evaluate(() => {
      window.showError = window.__realShowError;
    });
  }

  backend.browserProfiles = [];
  return "pending chip becomes the outcome line; skipped count; notice from three counts";
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

/* ---- 모델 · 모델 연결, as the person approved them (t-75762 v2, t-75886) ----
 *
 * Every list on these two pages is zo's own list and the saved connections;
 * every state is an answer that came back. What is pinned here is what the
 * person asked for in words: the coding list shows every model zo shows,
 * an older generation marked and at the foot of its login; the judgment
 * list names every login model it does not use; a pick is checked before
 * it is applied; one press applies the recommendation; the words a
 * beginner reads carry no technical term; and the pages hold at 390px in
 * five languages and two themes. `MODEL_SCENE_SHOTS=<folder>` keeps a
 * picture of each scene. */
const MODEL_TECH_WORDS = Object.freeze(["provider", "endpoint", "OpenAI JSON", "Choice", "Score", "Noul"]);
const MODEL_FIXTURE_PRESET = Object.freeze({
  id: "fixture-router", name: "Fixture Router", base_url: "", auth: { header: "Authorization", scheme: "Bearer" },
  models: { path: "/models", id_field: "id", context_field: "context_length" },
});

/* Synthetic values a provider's answer might echo — a query token and a
   header's value on its next line, neither with a digit (t-76873). */
const LOGIN_BODY_SECRETS = ["fixtureonlyletters", "fixturemultilinevalue"];

/* A login with no credential, as zo reports it (ZO m-76597): the source is
   skipped. A report that failed any other way kept last time's list and is
   not 「로그인 안 됨」 (t-76598). */
const signedOut = (report) => ({ ...report, ok: false, count: 0, diagnosis: { status: null, kind: null, skipped: true } });

async function withModelScene(prepare, run) {
  const saved = {
    zoSettings: clone(backend.zoSettings), zoList: clone(backend.zoList), zoEnvRouters: clone(backend.zoEnvRouters),
    legacyKeys: new Map(backend.legacyKeys), keychain: new Map(backend.keychain), routerPresets: clone(backend.routerPresets),
    managed: backend.managedModelState === undefined ? undefined : clone(backend.managedModelState),
    connectionCheck: backend.connectionCheck, routerProbeRefusal: backend.routerProbeRefusal, routerProbeAnswers: backend.routerProbeAnswers,
  };
  backend.settings.locale = "ko";
  await setQualityLocale(pageA, "ko");
  try {
    await prepare?.();
    await openSettings(pageA, "judgment-models");
    await pageA.evaluate(() => {
      Object.assign(modelConnectionsController, { setup: null, setupDismissed: false, picking: null, draft: null, adopting: null });
      for (const held of [modelConnectionsController.checks, modelConnectionsController.judged, modelConnectionsController.said, modelConnectionsController.formats, modelConnectionsController.usedFormats]) held.clear();
      routerPresets = [];
      return refreshApiRouters();
    });
    await renderSettled(pageA);
    return await run();
  } finally {
    Object.assign(backend, {
      zoSettings: saved.zoSettings, zoList: saved.zoList, zoEnvRouters: saved.zoEnvRouters, legacyKeys: saved.legacyKeys,
      keychain: saved.keychain, routerPresets: saved.routerPresets, connectionCheck: saved.connectionCheck,
      routerProbeRefusal: saved.routerProbeRefusal, routerProbeAnswers: saved.routerProbeAnswers,
    });
    if (saved.managed === undefined) delete backend.managedModelState; else backend.managedModelState = saved.managed;
    await pageA.setViewportSize({ width: 1280, height: 860 });
    await setQualityTheme(pageA, "dark");
    await pageA.evaluate(() => {
      Object.assign(modelConnectionsController, { setup: null, setupDismissed: false, picking: null, draft: null, adopting: null });
      for (const held of [modelConnectionsController.checks, modelConnectionsController.judged, modelConnectionsController.said, modelConnectionsController.formats, modelConnectionsController.usedFormats]) held.clear();
      routerPresets = [];
      return refreshApiRouters();
    });
  }
}

/* A scene in both themes at 1280 and 390 wide, with nothing wider than its pane. */
async function modelScenePictures(name) {
  const pane = pageA.locator(".settings-pane:not([hidden])");
  for (const theme of ["dark", "light"]) {
    await setQualityTheme(pageA, theme);
    for (const width of [1280, 390]) {
      await pageA.setViewportSize({ width, height: 900 });
      await settlePaint(pageA);
      const wide = await pane.evaluate((root) => {
        const right = root.getBoundingClientRect().right;
        return [...root.querySelectorAll("*")].filter((node) => node.getClientRects().length && node.getBoundingClientRect().right > right + 1)
          .map((node) => [node.tagName, node.className, Math.round(node.getBoundingClientRect().right - right)]).slice(0, 5);
      });
      assertEqual(wide, [], `${name} ${theme} ${width}px reaches past its pane`);
      const scroll = await pane.evaluate((root) => root.scrollWidth - root.clientWidth);
      assert(scroll <= 1, `${name} ${theme} ${width}px scrolls sideways`, scroll);
      if (process.env.MODEL_SCENE_SHOTS) {
        await mkdir(process.env.MODEL_SCENE_SHOTS, { recursive: true });
        // The pane scrolls inside the settings frame: the window is made as
        // tall as the pane's content so the picture holds all of it.
        const tall = await pane.evaluate((root) => {
          const frame = root.closest(".settings-frame") ?? root.parentElement;
          frame.scrollTop = 0;
          return Math.ceil(frame.scrollHeight + frame.getBoundingClientRect().top + 16);
        });
        await pageA.setViewportSize({ width, height: Math.min(6000, Math.max(900, tall)) });
        await settlePaint(pageA);
        await pane.screenshot({ path: resolve(process.env.MODEL_SCENE_SHOTS, `${name}-${theme}-${width}.png`), animations: "disabled" });
      }
    }
  }
  await pageA.setViewportSize({ width: 1280, height: 860 });
  await setQualityTheme(pageA, "dark");
}

/* The chooser's rows in order, each with the group it stands under. */
function chooserRows(role) {
  return pageA.evaluate((which) => {
    const field = [...document.querySelectorAll(".model-chooser")].find((one) => one.querySelector(`input[name="model-pick-${which}"]`));
    const rows = [];
    let group = null;
    for (const node of field?.querySelectorAll(".model-pick-group, .model-pick") ?? []) {
      if (node.classList.contains("model-pick-group")) { group = node.textContent; continue; }
      const input = node.querySelector("input");
      rows.push({ group, value: input.value, disabled: input.disabled, old: Boolean(node.querySelector(".model-old")), name: node.querySelector(".model-pick-name").textContent, why: node.querySelector(".model-pick-why")?.textContent ?? "" });
    }
    return rows;
  }, role);
}

await test("모델 목록: 코딩 목록의 행은 zo가 보여 주는 모델 전부이고, 이전 세대는 표시를 달고 그 로그인 맨 아래에 선다 (덧붙임 B·D)", async () => withModelScene(() => {
  backend.zoSettings.providers = [
    { name: "Gateway", base_url: "https://gateway.example.test/v1", models: ["vendor/coding-model", "second-model"], requires_auth: false, connection: { id: "gateway", generation: 1 } },
    { name: "Bench", base_url: "http://127.0.0.1:8090/v1", models: ["bench-model"], requires_auth: false, connection: { id: "bench", generation: 1 } },
  ];
  backend.zoEnvRouters = [{ name: "Elsewhere", models: ["Elsewhere/env-only-model"], usable: true, credentialEnvVars: [] }];
}, async () => {
  const listed = backend.zoModelList();
  const expected = [...listed.models.map((row) => row.id), ...listed.customProviders.flatMap((row) => row.models)];
  await pageA.click('[data-model-change="coding"]');
  const rows = await chooserRows("coding");
  assertEqual(rows.map((row) => row.value).sort(), [...expected].sort(), "the coding list is not every model zo lists");
  assertEqual(rows.length, expected.length, "a model was listed twice or folded away");
  assert(!await pageA.locator('.model-chooser details').count(), "the coding list hides models behind a fold");
  const words = await pageA.evaluate(() => t("settings.models.older", "이전 세대"));
  for (const row of rows) {
    const superseded = listed.models.find((model) => model.id === row.value)?.band === "superseded";
    assertEqual(row.old, superseded, `${row.value} wears the wrong generation mark`);
    if (!superseded) continue;
    const group = rows.filter((other) => other.group === row.group);
    assert(group.indexOf(row) > group.findLastIndex((other) => !other.old), `${row.value} stands above a current model of its login`, group);
  }
  assert(await pageA.locator(".model-chooser .model-old").first().textContent() === words);
  await modelScenePictures("coding-list");
  return `${rows.length} rows = ${listed.models.length} login + ${listed.customProviders.flatMap((row) => row.models).length} router models · ${rows.filter((row) => row.old).length} older generation at the foot`;
}));

/* `MODEL_REAL_ZO=<a copy of this machine's zo models --json>`: the coding
   list the implemented screen draws for it, counted — names never printed. */
if (process.env.MODEL_REAL_ZO) await test("이 기계의 zo 목록: 구현된 코딩 목록의 행 수 = zo models --json의 모델 수 + 라우터 모델 수", async () => withModelScene(async () => {
  const real = JSON.parse(await readFile(process.env.MODEL_REAL_ZO, "utf8"));
  backend.zoSettings.providers = [];
  backend.zoList = { models: real.models, reports: real.reports, aliases: real.aliases };
  backend.zoEnvRouters = real.customProviders;
}, async () => {
  await pageA.click('[data-model-change="coding"]');
  const rows = await chooserRows("coding");
  const listed = backend.zoModelList();
  const models = listed.models.length;
  const routers = listed.customProviders.flatMap((row) => row.models).length;
  assertEqual(rows.length, models + routers, "the coding list on this machine is not zo's whole list");
  await pageA.click('[data-model-change="judgment"]');
  await pageA.locator("#model-judgment [data-model-logins] summary").click();
  const logins = await pageA.locator("#model-judgment [data-model-logins] .model-pick-name").count();
  assertEqual(logins, models, "the judgment list does not name every login model");
  await modelScenePictures("real-judgment-list");
  await pageA.click('[data-model-change="judgment"]');
  await pageA.click('[data-model-change="coding"]');
  await modelScenePictures("real-coding-list");
  return `zoModels ${models} · zoCustomModels ${routers} · codingRows ${rows.length} · judgeLoginExpanded ${logins} · olderGeneration ${rows.filter((row) => row.old).length}`;
}));

await test("모델 목록: 판단 목록은 로그인 모델 전부를 펼쳐 이름으로 보이고, 고를 수 없게 이유를 단다 (덧붙임 C)", async () => withModelScene(null, async () => {
  await pageA.click('[data-model-change="judgment"]');
  const fold = pageA.locator("#model-judgment [data-model-logins]");
  assertEqual(await fold.count(), 1, "the login models have no line of their own");
  await fold.locator("summary").click();
  const names = await fold.locator(".model-pick-name").allTextContents();
  assertEqual(names.sort(), backend.zoList.models.map((model) => model.displayName).sort(), "the opened line does not name every login model");
  assert(await fold.locator("input").evaluateAll((inputs) => inputs.every((input) => input.disabled)), "a login model can be picked for decisions");
  assert((await fold.textContent()).includes(await pageA.evaluate(() => t("settings.models.loginWhy", "구독 로그인은 제품의 자동 요청에 쓰지 않습니다. 코딩 모델로는 고를 수 있습니다."))), "the reason is not said");
  await modelScenePictures("judgment-list");
  return `${names.length} login models, each named and dimmed with the reason`;
}));

await test("모델 연결: 네 묶음이 실제 상태로 서고, 로그인 줄은 펼치면 모델 이름 전부가 나온다", async () => withModelScene(() => {
  backend.zoSettings.providers = [{ name: "Gateway", base_url: "https://gateway.example.test/v1", models: ["vendor/coding-model"], requires_auth: true, auth_env: "ZEROCODE_ROUTER_GATEWAY_KEY", connection: { id: "gateway", generation: 1 } }];
}, async () => {
  await openSettings(pageA, "api-routers");
  await renderSettled(pageA);
  assertEqual(await pageA.locator("#model-connection-groups > [data-connection-group]").evaluateAll((groups) => groups.map((group) => group.dataset.connectionGroup)), ["login", "key", "local", "jev"]);
  const logins = await pageA.locator('[data-connection-group="login"] [data-model-login]').evaluateAll((rows) => rows.map((row) => [row.dataset.modelLogin, row.querySelector(".model-state").dataset.modelState]));
  assertEqual(logins, [["claude", "ok"], ["openai", "ok"], ["google", "ok"], ["xai", "nologin"]], "a login's state is not its report's");
  const claude = pageA.locator('[data-model-names="claude"]');
  await claude.locator("summary").click();
  assertEqual(await claude.locator("li").allTextContents(), backend.zoList.models.filter((model) => model.provider === "claude").sort((a, b) => (a.band === "superseded") - (b.band === "superseded")).map((model) => model.displayName));
  const key = pageA.locator('[data-connection-id="gateway"]');
  assertEqual(await key.locator(".model-state").getAttribute("data-model-state"), "nokey", "a connection with no key reads as reachable");
  await modelScenePictures("connections");
  backend.keychain.set("dev.zerocode.router.ZEROCODE_ROUTER_GATEWAY_KEY", "[합성-키]");
  await pageA.evaluate(() => modelConnectionsController.refresh());
  backend.routerProbeRefusal = { kind: "http", status: 401, message: "서버 오류: HTTP 401 — fixture refused" };
  await key.locator("[data-connection-check]").click();
  await pageA.waitForFunction(() => modelConnectionsController.checks.get("gateway")?.state === "badkey");
  assertEqual(await key.locator(".model-state").getAttribute("data-model-state"), "badkey", "a 401 is not said as a wrong key");
  backend.routerProbeRefusal = { kind: "transport", message: "연결 실패: error sending request" };
  await key.locator("[data-connection-check]").click();
  await pageA.waitForFunction(() => modelConnectionsController.checks.get("gateway")?.state === "down");
  // t-77116: a 503 whose body quotes `HTTP 401` as an example is a server that
  // is down — the kind is read off the status, never off the body's words.
  backend.routerProbeRefusal = { kind: "http", status: 503, message: "서버 오류: HTTP 503 Service Unavailable — upstream busy (e.g. HTTP 401 when the key is wrong)" };
  await key.locator("[data-connection-check]").click();
  await pageA.waitForFunction(() => ["down", "badkey"].includes(modelConnectionsController.checks.get("gateway")?.state) && modelConnectionsController.checks.get("gateway")?.raw?.includes("HTTP 503"));
  assertEqual(await key.locator(".model-state").getAttribute("data-model-state"), "down", "a 503 whose body quotes HTTP 401 was said as a wrong key");
  return "login · API key · this computer · Jev, each row's state from its answer";
}));

await test("처음 질문: 연결도 로그인도 없으면 저절로 묻고, 처음 화면에는 전문 용어가 없다 (결정 7, 시험 12)", async () => withModelScene(() => {
  backend.zoSettings.providers = [];
  backend.zoList.reports = backend.zoList.reports.map(signedOut);
}, async () => {
  const setup = pageA.locator("#model-setup");
  assert(await setup.isVisible(), "a person with no connection and no login was not asked");
  assert(await pageA.locator("#typesafe-card").isHidden(), "the daily cards stand beside the first question");
  assertEqual(await setup.locator("[data-model-choice]").evaluateAll((choices) => choices.map((choice) => choice.dataset.modelChoice)), ["login", "key", "local", "jev"]);
  for (const locale of SETTINGS_QUALITY_SPEC.locales) {
    await setQualityLocale(pageA, locale);
    const seen = await setup.innerText();
    const technical = MODEL_TECH_WORDS.filter((word) => seen.includes(word));
    assertEqual(technical, [], `${locale}: the first question speaks technical terms`);
    if (locale !== "ko") assert(!/[가-힣]/.test(seen), `${locale}: the first question is not translated`, seen);
    for (const theme of SETTINGS_QUALITY_SPEC.themes) {
      await setQualityTheme(pageA, theme);
      assertEqual(await axeViolations(pageA, AxeBuilder, "#model-setup"), [], `${locale}/${theme} first question accessibility`);
    }
  }
  await setQualityLocale(pageA, "ko");
  await modelScenePictures("first-question");
  await pageA.evaluate(() => modelConnectionsController.closeSetup());
  backend.zoList.reports = clone(ZO_LIST_FIXTURE.reports);
  await pageA.evaluate(() => { modelConnectionsController.setupDismissed = false; return modelConnectionsController.refresh(); });
  assert(await setup.isHidden(), "a signed-in person was asked the first question by itself");
  await openSettings(pageA, "api-routers");
  await pageA.click("#connections-add");
  assert(await setup.isVisible(), "「연결 추가」 does not ask the question");
  return "asked only when nothing is connected · four choices · no technical word in five languages · axe clean";
}));

await test("② API 키: 연결 확인이 「연결됨 — 모델 N개」로 답하고 키는 키체인에만 간다; 401·닫힌 서버는 쉬운 말로 (시험 6·7)", async () => withModelScene(() => {
  backend.zoSettings.providers = [];
  backend.zoList.reports = backend.zoList.reports.map(signedOut);
  backend.routerPresets = [{ ...MODEL_FIXTURE_PRESET, base_url: "https://fixture-router.example.test/v1" }];
  backend.routerProbeAnswers = { "https://fixture-router.example.test/v1": ["model-alpha", "model-beta", "model-gamma", "model-delta"] };
}, async () => {
  await pageA.click('[data-model-choice="key"]');
  await pageA.selectOption("#model-setup-service", "fixture-router");
  await pageA.fill("#model-setup-key", "[합성-키]");
  backend.routerProbeRefusal = { kind: "http", status: 401, message: "서버 오류: HTTP 401 — fixture refused" };
  await pageA.click("#model-setup-check");
  await pageA.waitForFunction(() => modelConnectionsController.setup?.checked === "fail");
  const badkey = await pageA.evaluate(() => t("settings.models.state.badkey", "키가 맞지 않습니다"));
  assert((await pageA.locator("#model-setup").textContent()).includes(badkey), "a 401 is not said as a wrong key");
  assertEqual(backend.zoSettings.providers.length, 0, "a refused check saved the connection");
  backend.routerProbeRefusal = { kind: "transport", message: "연결 실패: error sending request for url" };
  await pageA.fill("#model-setup-key", "[합성-키]");
  await pageA.click("#model-setup-check");
  await pageA.waitForFunction(() => modelConnectionsController.setup?.failure === "down");
  backend.routerProbeRefusal = null;
  await pageA.fill("#model-setup-key", "[합성-키]");
  const at = backend.calls.length;
  await pageA.click("#model-setup-check");
  await pageA.waitForFunction(() => modelConnectionsController.setup?.checked === "ok");
  const connected = await pageA.evaluate(() => t("settings.models.connected", "연결됨 — 모델 {{count}}개", { count: 4 }));
  assert((await pageA.locator("#model-setup").textContent()).includes(connected), "the answer does not count the server's models");
  const saved = backend.calls.slice(at).find((call) => call.command === "save_api_router");
  assertEqual(saved.args.models.length, 4, "the connection was saved without the models the server listed");
  const row = backend.zoSettings.providers[0];
  assert(!JSON.stringify(backend.zoSettings).includes("[합성-키]"), "the key reached the settings file");
  assertEqual(backend.keychain.get(`dev.zerocode.router.${row.auth_env}`), "[합성-키]", "the key is not in the keychain");
  assertEqual(await pageA.inputValue("#model-setup-key").catch(() => ""), "", "the key stayed in the field");
  await modelScenePictures("key-ok");
  return "401 → wrong key · refused socket → no answer · 4 models counted · key in the keychain only";
}));

await test("추천대로 쓰기: 로그인 계정의 「최상위」를 코딩에, 실행 중인 이 컴퓨터 모델을 판단에 — 한 번에 적용하고 zo에 한 번 알린다 (결정 6, 시험 8)", async () => withModelScene(() => {
  backend.zoSettings.providers = [];
  backend.zoList.reports = backend.zoList.reports.map(signedOut);
  const provider = { kind: "local", protocol: "system_one", baseUrl: "http://127.0.0.1:32105", model: "fixture-decision", requiresAuth: false };
  backend.zoSettings.smart = { judgmentProvider: provider };
  backend.zoSettings.model = "fixture-openai-old";
  delete backend.managedModelState;
  backend.managedJudgmentSettings();
  Object.assign(backend.managedModelState, {
    // A manager of its own: the window keeps the newest snapshot of the one it knew.
    instanceId: "fixture-reco-instance",
    configuredModelId: provider.model, appliedModelId: provider.model, runtime: { state: "ready", downloadBytes: 0, reason: null },
    server: { id: "fixture-reco-server", state: "ready", modelId: provider.model, provider, checked: true, error: null },
  });
  backend.managedModelState.models[0].installed = true;
}, async () => {
  backend.zoList.reports = clone(ZO_LIST_FIXTURE.reports);
  await pageA.click('[data-model-choice="login"]');
  await pageA.waitForFunction(() => modelConnectionsController.setup?.checked === "ok");
  const coding = pageA.locator('[data-model-reco="coding"]');
  assert((await coding.textContent()).includes("Fixture Claude Top"), "the coding recommendation is not the signed-in account's top model");
  assert((await pageA.locator('[data-model-reco="judgment"]').textContent()).includes("Fixture Decision"), "the decision recommendation is not the running model on this computer");
  await modelScenePictures("login-reco");
  const at = backend.calls.length;
  await pageA.click("[data-model-apply-setup]");
  await pageA.waitForSelector("[data-model-done]");
  const calls = backend.calls.slice(at).map((call) => call.command);
  assertEqual(calls.filter((command) => command === "select_coding_model").length, 1, "the coding model was not applied once");
  assertEqual(calls.filter((command) => command === "sync_model_connections").length, 1, "running zo were not told once");
  assert(!calls.includes("managed_judgment_apply"), "the decision model already in use was applied again");
  assertEqual(backend.zoSettings.model, "fixture-claude-top");
  assertEqual(backend.zoSettings.smart.previousCodingSelection, "fixture-openai-old");
  assert((await pageA.locator("[data-model-done]").textContent()).includes(await pageA.evaluate(() => t("settings.models.runningKept", "이미 열린 zo 세션은 지금 모델을 그대로 씁니다."))), "the done line promises a change to sessions already open");
  await modelScenePictures("done");
  return "one press: coding → the account's top model, decision kept, one sync";
}));

await test("판단 모델: 고르면 확인이 저절로 돌고, 통과하지 못한 모델은 적용할 수 없다; 키 없는 연결은 미리 흐리게 (시험 9)", async () => withModelScene(() => {
  backend.zoSettings.providers = [
    { name: "Judge", base_url: "https://judge.example.test/v1", models: ["judge-model"], requires_auth: false, connection: { id: "judge", generation: 2 } },
    { name: "Locked", base_url: "https://locked.example.test/v1", models: ["locked-model"], requires_auth: true, auth_env: "ZEROCODE_ROUTER_LOCKED_KEY", connection: { id: "locked", generation: 1 } },
  ];
}, async () => {
  await pageA.click('[data-model-change="judgment"]');
  const rows = await chooserRows("judgment");
  const locked = rows.find((row) => row.value === JSON.stringify(["locked", "locked-model"]));
  assert(locked?.disabled, "a connection with no key can be picked for decisions");
  assert(locked.why.includes(await pageA.evaluate(() => t("settings.models.state.nokey", "키를 찾지 못했습니다"))), "the dimmed row does not say why");
  assert(!rows.find((row) => row.value === JSON.stringify(["judge", "judge-model"])).disabled, "a connection nobody has checked yet was dimmed");
  backend.connectionCheck = { answered: false, failure: "schema" };
  let at = backend.calls.length;
  await pageA.locator(`input[value='${JSON.stringify(["judge", "judge-model"])}']`).check();
  await backend.waitForCall("A", "check_model_connection", at);
  await pageA.waitForSelector('[data-verdict="schema"]');
  assert(await pageA.locator('[data-model-apply="judgment"]').isDisabled(), "a model that failed its check can be applied");
  backend.connectionCheck = { answered: true, failure: null };
  // A failed check dims the row; checking the connection again lets it be asked again.
  assert(await pageA.locator(`input[value='${JSON.stringify(["judge", "judge-model"])}']`).isDisabled(), "a failed model stays pickable as if unchecked");
  await pageA.evaluate(() => modelConnectionsController.checkConnection(modelConnectionsController.state.connections.find((row) => row.id === "judge")));
  await pageA.click('[data-model-change="judgment"]');
  await pageA.click('[data-model-change="judgment"]');
  at = backend.calls.length;
  await pageA.locator(`input[value='${JSON.stringify(["judge", "judge-model"])}']`).check();
  await pageA.waitForSelector('[data-verdict="usable"]');
  await pageA.click('[data-model-apply="judgment"]');
  const applied = await backend.waitForCall("A", "select_model_connection", at);
  assertEqual(applied.args.reference, { id: "judge", generation: 2, model: "judge-model", protocol: "openai_json" }, "the apply named another connection");
  await modelScenePictures("judgment-applied");
  return "auto check on pick · failed check blocks apply · keyless row dimmed with its reason";
}));

await test("로그인 줄: 실패한 조회는 「목록은 이전 확인 것 · 지금 조회 실패」, 건너뛴 원천만 「로그인 안 됨」, 다른 곳의 조회는 그렇게 (t-76598 ④)", async () => withModelScene(() => {
  backend.zoSettings.providers = [];
  backend.zoList.reports = backend.zoList.reports.map((report) => report.provider === "anthropic"
    // The window keeps a diagnosis of a failed check (t-76873); a body that
    // still rode along, whatever it holds, is never drawn.
    ? { ...report, ok: false, count: 0, diagnosis: { status: 401, kind: "auth", skipped: false }, age: "3h old", origin: "previous",
      detail: `HTTP 401 at https://example.test/?access_token=${LOGIN_BODY_SECRETS[0]}\nAuthorization:\n${LOGIN_BODY_SECRETS[1]}` }
    : report);
}, async () => {
  await openSettings(pageA, "api-routers");
  await renderSettled(pageA);
  const line = (provider) => pageA.locator(`[data-connection-group="login"] [data-model-login="${provider}"]`);
  const state = async (provider) => line(provider).locator(".model-state").getAttribute("data-model-state");
  assertEqual(await state("claude"), "stale", "a failed read of a listed login is not said as an earlier list");
  const said = await line("claude").innerText();
  const words = await pageA.evaluate(() => ({ nologin: t("settings.models.state.nologin", "로그인 안 됨"), failed: t("settings.models.staleFailedPlain", "지금 조회 실패"),
    auth: t("settings.models.failure.auth", "인증 거절") }));
  assert(!said.includes(words.nologin), "a signed-in login whose read failed reads 「로그인 안 됨」", said);
  assert(said.includes("목록은 이전 확인 것") && said.includes(words.failed) && said.includes(`HTTP 401 · ${words.auth}`), "the line does not say the list is earlier and why now failed", said);
  for (const secret of LOGIN_BODY_SECRETS) assert(!said.includes(secret), "a provider's answer reached the login line", said);
  assertEqual(await state("xai"), "nologin", "a skipped source is not said as signed out");
  await modelScenePictures("login-stale");
  // The coding chooser says the same beside the login's models.
  await openSettings(pageA, "judgment-models");
  await pageA.click('[data-model-change="coding"]');
  const note = await pageA.locator(".model-chooser .model-pick-note").first().innerText();
  assert(note.includes("목록은 이전 확인 것") && !note.includes(words.nologin), "the chooser calls the earlier list a missing login", note);
  for (const secret of LOGIN_BODY_SECRETS) assert(!note.includes(secret), "a provider's answer reached the chooser", note);
  await pageA.click('[data-model-change="coding"]');
  // The failure is said by the fields zo sets, never guessed from text
  // (t-76968): a kind zo calls unknown is 「알 수 없는 까닭」, a status with it
  // is kept, and an older zo that names no kind is only 「지금 조회 실패」.
  const failureLine = async (diagnosis) => {
    backend.zoList.reports = backend.zoList.reports.map((report) => report.provider === "anthropic" ? { ...report, diagnosis } : report);
    await pageA.evaluate(() => modelConnectionsController.refresh());
    await openSettings(pageA, "api-routers");
    return line("claude").innerText();
  };
  const reason = await pageA.evaluate(() => ({ unknown: t("settings.models.failure.unknown", "알 수 없는 까닭"), network: t("settings.models.failure.network", "네트워크 오류") }));
  assertEqual(reason.unknown, "알 수 없는 까닭", "the unknown kind is not said as a reason");
  let failure = await failureLine({ status: null, kind: "unknown", skipped: false });
  assert(failure.includes(`${words.failed}: ${reason.unknown}`) && !failure.includes(reason.network), "an unknown kind is not said as unknown", failure);
  failure = await failureLine({ status: 503, kind: "unknown", skipped: false });
  assert(failure.includes(`HTTP 503 · ${reason.unknown}`), "a status with an unknown kind lost one of them", failure);
  failure = await failureLine({ status: null, kind: null, skipped: false });
  assert(failure.includes(words.failed) && !failure.includes(`${words.failed}:`) && !failure.includes(reason.unknown) && !failure.includes(reason.network), "a report without a kind names one", failure);
  failure = await failureLine({ status: null, kind: "credentialUnusable", skipped: false });
  const unusable = await pageA.evaluate(() => t("settings.models.failure.credentialUnusable", "로그인은 있으나 쓸 수 없음 — 다시 로그인하세요"));
  assert(failure.includes(`${words.failed}: ${unusable}`) && !failure.includes(words.nologin), "a login that cannot be used is said as missing or unknown", failure);
  failure = await failureLine({ status: null, kind: "network", skipped: false });
  assert(failure.includes(`${words.failed}: ${reason.network}`), "zo's network kind is not said as the network", failure);
  // A report from another credential store than a zo pane's says so; an
  // older zo's report without the key is read by the rules above.
  backend.zoList.paneCredentialStore = "CLAUDE_CONFIG_DIR:/fixture/pane-home";
  backend.zoList.reports = backend.zoList.reports.map((report) => report.provider === "anthropic"
    ? { ...report, ok: true, count: 3, origin: "live", credentialStore: "keychain" } : report);
  await pageA.evaluate(() => modelConnectionsController.refresh());
  await openSettings(pageA, "api-routers");
  assertEqual(await state("claude"), "elsewhere", "a list read from another credential store passes as the pane's");
  backend.zoList.reports = backend.zoList.reports.map((report) => report.provider === "anthropic"
    ? { ...report, credentialStore: "CLAUDE_CONFIG_DIR:/fixture/pane-home" } : report);
  await pageA.evaluate(() => modelConnectionsController.refresh());
  assertEqual(await state("claude"), "ok", "the pane's own store was said as another's");
  // zo's rule (ZO m-76774): a pane given the credential folder too is named
  // by it. Two pane shapes × two report labels; another provider's label
  // (`CODEX_HOME:…`) and an empty label are never held to the Claude pane's.
  const shapes = [
    ["CLAUDE_CONFIG_DIR:/fixture/runtime-home", "CLAUDE_CONFIG_DIR:/fixture/runtime-home", "CLAUDE_SECURESTORAGE_CONFIG_DIR:/fixture/account"],
    ["CLAUDE_SECURESTORAGE_CONFIG_DIR:/fixture/account", "CLAUDE_SECURESTORAGE_CONFIG_DIR:/fixture/account", "CLAUDE_CONFIG_DIR:/fixture/runtime-home"],
  ];
  for (const [pane, same, other] of shapes) {
    backend.zoList.paneCredentialStore = pane;
    for (const [label, expected] of [[same, "ok"], [other, "elsewhere"], ["", "ok"]]) {
      backend.zoList.reports = backend.zoList.reports.map((report) => report.provider === "anthropic" ? { ...report, credentialStore: label }
        : report.provider === "openai" ? { ...report, credentialStore: "CODEX_HOME:/fixture/codex-runtime" } : report);
      await pageA.evaluate(() => modelConnectionsController.refresh());
      assertEqual(await state("claude"), expected, `pane ${pane} · report ${label || "(empty)"}`);
      assert(await pageA.evaluate(() => modelConnectionsController.logins().every((login) => login.provider === "claude" || login.state !== "elsewhere")),
        "another provider's report was held to the Claude pane's store");
    }
  }
  delete backend.zoList.paneCredentialStore;
  return "401 with an earlier list → 「목록은 이전 확인 것 · 지금 조회 실패」 · skipped → 로그인 안 됨 · keychain ≠ pane folder → 다른 환경의 조회";
}));

await test("판단 고르기: System One을 권하고, 일반 LLM을 고르면 한계 한 줄이 서며, 경로 글귀에 원시 값이 없다 (t-76598 ②⑤)", async () => withModelScene(() => {
  backend.zoSettings.providers = [
    { name: "Judge", base_url: "https://judge.example.test/v1", models: ["judge-model"], requires_auth: false, connection: { id: "judge", generation: 2 } },
  ];
}, async () => {
  await pageA.click('[data-model-change="judgment"]');
  assert(await pageA.locator("[data-judgment-advice]").isVisible(), "the chooser does not recommend System One");
  const general = JSON.stringify(["judge", "judge-model"]);
  const way = await pageA.locator(`input[value='${general}']`).locator("xpath=..").locator(".model-pick-way").innerText();
  assert(way.includes("일반 LLM"), "a general model's row does not say how it judges", way);
  await pageA.locator(`input[value='${general}']`).check();
  await pageA.waitForSelector('[data-verdict="usable"]');
  assert(await pageA.locator('[data-judgment-limit=""]').isVisible(), "a general LLM was picked without its limit");
  assert(await pageA.locator("[data-judgment-restart]").isVisible(), "a change does not say judging starts over");
  await pageA.locator("input[value='jev']").check();
  await pageA.waitForSelector('[data-verdict="usable"]');
  assertEqual(await pageA.locator('[data-judgment-limit=""]').count(), 0, "a System One pick carries the general LLM's limit");
  await pageA.click('[data-model-change="judgment"]');
  // Where each choice goes, in a beginner's words, in every language.
  backend.zoSettings.smart = { ...(backend.zoSettings.smart ?? {}), judgmentConnection: { id: "judge", generation: 2, model: "judge-model", protocol: "openai_json" },
    judgmentProvider: { kind: "remote", protocol: "openai_json", baseUrl: "https://judge.example.test/v1", model: "judge-model", requiresAuth: false } };
  await pageA.evaluate(async () => { await refreshTypeSafe(); await modelConnectionsController.refresh(); });
  for (const locale of SETTINGS_QUALITY_SPEC.locales) {
    await setQualityLocale(pageA, locale);
    const route = await pageA.evaluate(() => modelConnectionsController.routeWords());
    for (const raw of [...MODEL_TECH_WORDS, "remote", "openai_json", "system_one", "judgment-"]) assert(!route.includes(raw), `${locale}: the route says ${raw}`, route);
    assert(route.includes("Judge") && route.includes("judge-model"), `${locale}: the route does not name the connection and model`, route);
  }
  await setQualityLocale(pageA, "ko");
  const current = await pageA.locator('[data-model-current="judgment"]').innerText();
  assert(current.includes("일반 LLM") && await pageA.locator('[data-judgment-limit="current"]').isVisible(), "the general LLM judging now does not say its limit", current);
  await modelScenePictures("judgment-advice");
  return "System One advised · general LLM pick → limit line, Jev pick → none · route words carry no raw value in five languages";
}));

await test("적용 확인: 적용 뒤 설정을 다시 읽어 같으면 그렇다고, 다르면 그 자리에서 무엇이 다른지 말하고, 같을 때만 바꾼 시각을 남긴다 (t-76598 ③)", async () => {
  const savedMemory = backend.connectionMemory;
  const T0 = 1_790_000_000_000;
  backend.connectionMemory = { nowMs: T0, connections: {} };
  try {
    return await withModelScene(() => {
      backend.zoSettings.providers = [
        { name: "Judge", base_url: "https://judge.example.test/v1", models: ["judge-model", "judge-two"], requires_auth: false, connection: { id: "judge", generation: 2 } },
      ];
    }, async () => {
      const pick = async (model) => {
        await pageA.click('[data-model-change="judgment"]');
        await pageA.locator(`input[value='${JSON.stringify(["judge", model])}']`).check();
        await pageA.waitForSelector('[data-verdict="usable"]');
        await pageA.click('[data-model-apply="judgment"]');
        await pageA.waitForSelector('[data-model-role="judgment"] .model-result');
      };
      await pick("judge-model");
      const said = await pageA.locator('[data-model-role="judgment"] .model-result').innerText();
      assert(said.includes("다시 읽은 설정도") && said.includes("Judge · judge-model · 일반 LLM"), "the apply does not say what the settings read back", said);
      const identity = backend.typesafeSettings().providerIdentity;
      assertEqual(backend.connectionMemory.applied, { [identity]: T0 }, "the apply's time was not kept for the provider read back");
      backend.connectionApplyLost = true;
      await pick("judge-two");
      const differs = pageA.locator("[data-applied-differs]");
      assert(await differs.isVisible(), "an apply the settings do not hold was said as done");
      const words = await differs.innerText();
      assert(words.includes("judge-model") && words.includes("judge-two"), "the difference does not name what was read and what was picked", words);
      assertEqual(Object.keys(backend.connectionMemory.applied), [identity], "an apply that did not land was kept as one");
      await modelScenePictures("judgment-applied-differs");
      // The write lands, and another window changes the connection's address
      // under the same id and model: the generation read back is not the one
      // picked, and that is a difference too (t-76598 r2 ②).
      delete backend.connectionApplyLost;
      backend.connectionApplyMoved = true;
      await pick("judge-two");
      assertEqual(backend.zoSettings.smart.judgmentConnection.generation, 3, "the scene did not move the connection");
      assert(await differs.isVisible(), "an apply read back at another generation of the connection was said as done");
      const moved = await differs.innerText();
      const movedWords = await pageA.evaluate(() => t("settings.models.appliedGeneration", "{{words}} (그 사이 연결의 주소나 키가 바뀜)", { words: "" }).trim());
      assert(moved.includes(movedWords), "the difference does not say the connection changed meanwhile", moved);
      assertEqual(Object.keys(backend.connectionMemory.applied).length, 1, "an apply read back at another generation was kept as one");
      return "read back equal → 「다시 읽은 설정도 …」 + time kept · lost write → read and picked named, nothing kept · another generation → said, nothing kept";
    });
  } finally {
    delete backend.connectionApplyLost;
    delete backend.connectionApplyMoved;
    if (savedMemory === undefined) delete backend.connectionMemory; else backend.connectionMemory = savedMemory;
  }
});

await test("적용 확인: 이 컴퓨터 모델도 적용 뒤 설정을 다시 읽고, 못 읽거나 다른 모델이면 그렇다고 말하며, 같을 때만 바꾼 시각을 남긴다 (t-76598 r2 ①)", async () => {
  const savedMemory = backend.connectionMemory;
  const T0 = 1_790_000_000_000;
  backend.connectionMemory = { nowMs: T0, connections: {} };
  try {
    return await withModelScene(() => {
      const provider = { kind: "local", protocol: "system_one", baseUrl: "http://127.0.0.1:32106", model: "fixture-decision", requiresAuth: false };
      delete backend.managedModelState;
      backend.managedJudgmentSettings();
      Object.assign(backend.managedModelState, {
        instanceId: "fixture-readback-instance", runtime: { state: "ready", downloadBytes: 0, reason: null },
        server: { id: "fixture-readback-server", state: "ready", modelId: provider.model, provider, checked: true, error: null },
      });
      backend.managedModelState.models[0].installed = true;
    }, async () => {
      const apply = async (then) => {
        backend.managedApplyThen = then;
        const at = backend.calls.length;
        try {
          await pageA.evaluate(async () => { await refreshTypeSafe(); await managedJudgmentController.refresh(); });
          const result = await pageA.evaluate(() => modelConnectionsController.applyJudgment("managed:fixture-decision", null));
          const kept = backend.calls.slice(at).filter((call) => call.command === "remember_judgment_applied").length;
          return { result, kept };
        } finally {
          delete backend.managedApplyThen;
          delete backend.settingsReadBroken;
        }
      };
      const unread = await apply("unreadable");
      const unreadWords = await pageA.evaluate(() => t("settings.models.appliedUnread", "설정을 다시 읽지 못했습니다"));
      assert(unread.result.applied && unread.result.differs === unreadWords, "a local apply whose settings could not be read again was said as read back", JSON.stringify(unread.result));
      assertEqual(unread.kept, 0, "a local apply read back from nothing kept its time");
      const overwritten = await apply("overwritten");
      const jevWords = await pageA.evaluate(() => t("jev.place.jev", "Jev 서비스"));
      assert(overwritten.result.applied && String(overwritten.result.differs ?? "").includes(jevWords), "a local apply the settings no longer hold was said as done", JSON.stringify(overwritten.result));
      assertEqual(overwritten.kept, 0, "a local apply another window overwrote kept its time");
      // The same model id, read back from another local server or asked as a
      // general model, is another provider: said, nothing kept (t-76873 P2 ①).
      const serverWords = await pageA.evaluate(() => t("settings.models.appliedServer", "{{words}} (같은 모델 이름이지만 다른 판단 서버)", { words: "" }).trim());
      for (const then of ["anotherServer", "anotherProtocol"]) {
        const moved = await apply(then);
        assert(moved.result.applied && moved.result.differs, `a local apply read back as ${then} was said as done`, JSON.stringify(moved.result));
        assertEqual(moved.kept, 0, `a local apply read back as ${then} kept its time`);
        if (then === "anotherServer") assert(String(moved.result.differs).includes(serverWords), "the difference does not say the server is another", moved.result.differs);
      }
      const held = await apply(null);
      assertEqual(held.result.differs, null, "a local apply the settings hold was said as another", JSON.stringify(held.result));
      assertEqual(held.kept, 1, "a local apply read back as picked did not keep its time");
      const identity = backend.typesafeSettings().providerIdentity;
      assertEqual(backend.connectionMemory.applied?.[identity], T0, "the local apply's time was not kept for the provider read back");
      return "local apply → read back like any other: unreadable → said, nothing kept · overwritten → named, nothing kept · same model on another server or protocol → said, nothing kept · held → time kept";
    });
  } finally {
    if (savedMemory === undefined) delete backend.connectionMemory; else backend.connectionMemory = savedMemory;
    // The page read a local model applied; the scenes after start from the
    // settings and the managed state put back.
    await pageA.evaluate(async () => {
      if (typesafeRefreshing) await typesafeRefreshing.promise;
      await refreshTypeSafe();
      await managedJudgmentController.refresh();
      await refreshApiRouters();
    });
    await renderSettled(pageA);
  }
});

await test("저장 실패: 창이 확인 결과를 기록하지 못하면 화면이 그렇다고 말하고 기록은 그대로다 (t-76500 r4 ①)", async () => {
  const savedMemory = backend.connectionMemory;
  const T0 = 1_790_000_000_000;
  backend.connectionMemory = { nowMs: T0, connections: {} };
  // Refused from the record's first moment: a quiet check still running from
  // the scene before, or the scene's own, is refused too.
  backend.connectionMemoryRefusal = "model_connection_checks_full";
  try {
    return await withModelScene(() => {
      backend.zoSettings.providers = [
        { name: "Judge", base_url: "https://judge.example.test/v1", models: ["judge-model"], requires_auth: false, connection: { id: "judge", generation: 2 } },
      ];
    }, async () => {
      await pageA.click('[data-model-change="judgment"]');
      await pageA.locator(`input[value='${JSON.stringify(["judge", "judge-model"])}']`).check();
      await pageA.waitForSelector('[data-verdict="usable"] [data-check-unsaved]');
      assertEqual(backend.connectionMemory.connections, {}, "a refused record changed");
      await pageA.click('[data-model-change="judgment"]');
      await openSettings(pageA, "api-routers");
      await pageA.locator('[data-connection-id="judge"] [data-connection-check]').click();
      await pageA.waitForSelector('[data-check-unsaved="judge"]');
      return "a refused judgment check and connection check each say they were not kept; the record is as it was";
    });
  } finally {
    delete backend.connectionMemoryRefusal;
    if (savedMemory === undefined) delete backend.connectionMemory; else backend.connectionMemory = savedMemory;
  }
});

await test("검수 F1: 연결 확인의 거절에 키가 되돌아와도 화면·「자세히」·로그 어디에도 그 키가 없다 (t-76028)", async () => withModelScene(() => {
  backend.zoSettings.providers = [];
  backend.zoList.reports = backend.zoList.reports.map(signedOut);
  backend.routerPresets = [{ ...MODEL_FIXTURE_PRESET, base_url: "https://fixture-router.example.test/v1" }];
}, async () => {
  const key = "[합성-키]-echo-7f3a";
  const logged = [];
  const listen = (message) => logged.push(message.text());
  pageA.on("console", listen);
  try {
    await pageA.click('[data-model-choice="key"]');
    await pageA.selectOption("#model-setup-service", "fixture-router");
    await pageA.fill("#model-setup-key", key);
    // An answer that carries the key back whole: whatever road brought it,
    // the screen keeps it off every line it draws.
    backend.routerProbeRefusal = { kind: "http", status: 401, message: `서버 오류: HTTP 401 Unauthorized — invalid token Bearer ${key}` };
    const at = backend.calls.length;
    await pageA.click("#model-setup-check");
    await pageA.waitForFunction(() => modelConnectionsController.setup?.checked === "fail");
    await pageA.locator("#model-setup details summary").first().click().catch(() => {});
    const held = await pageA.evaluate(() => JSON.stringify(modelConnectionsController.setup));
    const drawn = await pageA.evaluate(() => document.body.innerHTML);
    const noted = backend.calls.slice(at).filter((call) => call.command === "note_webview_error").map((call) => JSON.stringify(call.args));
    assert(!held.includes(key), "the page holds the key it was handed back");
    assert(!drawn.includes(key), "the key the server said back is drawn on the screen");
    assert(!noted.some((text) => text.includes(key)), "the key reached the window's log");
    assert(!logged.some((text) => text.includes(key)), "the key reached the console");
    assert((await pageA.locator("#model-setup").textContent()).includes(await pageA.evaluate(() => t("settings.models.state.badkey", "키가 맞지 않습니다"))), "the 401 is not said as a wrong key");
    assert(drawn.includes("invalid token"), "the server's reason is gone with the key");
    await modelScenePictures("key-refused");
  } finally {
    pageA.off("console", listen);
  }
  return "401 echoing the pasted key: page state 0 · DOM 0 · window log 0 · console 0 hits; reason kept";
}));

await test("검수 F2: 빈 zo 목록을 먼저 읽은 뒤 새 연결을 저장하면 곧바로 「추천대로 쓰기」가 그 연결의 모델로 적용된다 (t-76028)", async () => withModelScene(() => {
  backend.zoSettings.providers = [];
  backend.zoList.reports = backend.zoList.reports.map(signedOut);
  backend.routerPresets = [{ ...MODEL_FIXTURE_PRESET, base_url: "https://fixture-router.example.test/v1" }];
  backend.routerProbeAnswers = { "https://fixture-router.example.test/v1": ["model-alpha", "model-beta"] };
  // The window's zo list is `slash_catalog`'s cache: what was read stands
  // until a change the window makes forgets it.
  backend.zoListCaching = true;
  backend.zoListHeld = null;
}, async () => {
  try {
    assertEqual(backend.zoListHeld?.customProviders.length, 0, "the screen did not read the empty list first");
    await pageA.click('[data-model-choice="key"]');
    await pageA.selectOption("#model-setup-service", "fixture-router");
    await pageA.fill("#model-setup-key", "[합성-키]");
    await pageA.click("#model-setup-check");
    await pageA.waitForFunction(() => modelConnectionsController.setup?.checked === "ok");
    const coding = pageA.locator('[data-model-reco="coding"]');
    assert((await coding.textContent()).includes("model-alpha"), "the new connection's model is not offered for coding", await coding.textContent());
    const at = backend.calls.length;
    await pageA.click("[data-model-apply-setup]");
    await pageA.waitForSelector("[data-model-done]");
    const applied = backend.calls.slice(at).find((call) => call.command === "select_coding_model");
    assertEqual(applied?.args.model, "Fixture Router/model-alpha", "the recommendation was not applied");
    assertEqual(backend.zoSettings.model, "Fixture Router/model-alpha", "the coding model is not the new connection's");
    assert(await pageA.locator("[data-model-done].is-ok").count(), "the done line says the coding model was not applied", await pageA.locator("[data-model-done]").textContent());
  } finally {
    backend.zoListCaching = false;
    backend.zoListHeld = null;
  }
  return "empty list read → connection saved → recommendation applied as Fixture Router/model-alpha";
}));

await test("검수 F3: 판단 형식은 연결이 쓰던 그대로 — System One과 OpenAI JSON 각각 확인·적용이 같은 형식이고, 바꾸는 것은 「고급」에서만 (t-76028)", async () => withModelScene(() => {
  backend.zoSettings.providers = [
    { name: "Bench", base_url: "http://127.0.0.1:8091/v1", models: ["bench-a", "bench-b"], requires_auth: false, connection: { id: "bench", generation: 4 } },
    { name: "Judge", base_url: "https://judge.example.test/v1", models: ["judge-model"], requires_auth: false, connection: { id: "judge", generation: 2 } },
  ];
  const reference = { id: "bench", generation: 4, model: "bench-a", protocol: "system_one" };
  backend.zoSettings.smart = {
    judgmentConnection: reference,
    judgmentProvider: { kind: "remote", protocol: "system_one", baseUrl: "http://127.0.0.1:8091/v1", model: "bench-a", requiresAuth: false },
  };
}, async () => {
  const pickAndApply = async (connection, model) => {
    if (!await pageA.locator('[data-model-apply="judgment"]').count()) await pageA.click('[data-model-change="judgment"]');
    const at = backend.calls.length;
    await pageA.locator(`input[name="model-pick-judgment"][value='${JSON.stringify([connection, model])}']`).check();
    const checked = await backend.waitForCall("A", "check_model_connection", at);
    await pageA.waitForSelector('[data-verdict="usable"]');
    await pageA.click('[data-model-apply="judgment"]');
    const applied = await backend.waitForCall("A", "select_model_connection", at);
    await renderSettled(pageA);
    return [checked.args.reference, applied.args.reference];
  };
  // System One: another model of the connection judgment already uses.
  const [systemCheck, systemApply] = await pickAndApply("bench", "bench-b");
  assertEqual(systemCheck, { id: "bench", generation: 4, model: "bench-b", protocol: "system_one" }, "the check asked another format than the connection's");
  assertEqual(systemApply, systemCheck, "the apply wrote another reference than the one checked");
  // OpenAI JSON: a connection judgment never used, in its default format.
  const [openCheck, openApply] = await pickAndApply("judge", "judge-model");
  assertEqual(openCheck, { id: "judge", generation: 2, model: "judge-model", protocol: "openai_json" });
  assertEqual(openApply, openCheck, "the apply wrote another reference than the one checked");
  // The everyday list names no format; 「고급」 changes one, and the same
  // model can then be applied in it.
  const everyday = await pageA.locator('[data-model-role="judgment"]').innerText();
  assert(!everyday.match(/OpenAI JSON|System One/), "the everyday list speaks a format", everyday);
  await pageA.evaluate(() => { document.getElementById("model-advanced").open = true; document.getElementById("model-advanced-format-fold").open = true; });
  const format = pageA.locator('#model-advanced [data-judgment-format="judge"]');
  assertEqual(await format.inputValue(), "openai_json", "「고급」 does not show the connection's format");
  await format.selectOption("system_one");
  await renderSettled(pageA);
  await modelScenePictures("judgment-format");
  const [againCheck, againApply] = await pickAndApply("judge", "judge-model");
  assertEqual(againCheck.protocol, "system_one", "the format picked in 「고급」 was not asked");
  assertEqual(againApply, againCheck, "the apply wrote another reference than the one checked");
  return "System One kept for check+apply · OpenAI JSON kept for check+apply · 「고급」 switches judge to System One and re-applies";
}));

await test("재검수 R3: System One 연결 → 다른 연결 → 원래 연결로 돌아와도, 화면을 다시 열어도 그 연결의 판단 형식은 그대로다 (t-76028)", async () => withModelScene(() => {
  backend.zoSettings.providers = [
    { name: "Bench", base_url: "http://127.0.0.1:8091/v1", models: ["bench-a", "bench-b"], requires_auth: false, connection: { id: "bench", generation: 4 } },
    { name: "Judge", base_url: "https://judge.example.test/v1", models: ["judge-model"], requires_auth: false, connection: { id: "judge", generation: 2 } },
  ];
  backend.zoSettings.smart = {
    judgmentConnection: { id: "bench", generation: 4, model: "bench-a", protocol: "system_one" },
    judgmentProvider: { kind: "remote", protocol: "system_one", baseUrl: "http://127.0.0.1:8091/v1", model: "bench-a", requiresAuth: false },
  };
}, async () => {
  const pickAndApply = async (connection, model) => {
    if (!await pageA.locator('[data-model-apply="judgment"]').count()) await pageA.click('[data-model-change="judgment"]');
    const at = backend.calls.length;
    await pageA.locator(`input[name="model-pick-judgment"][value='${JSON.stringify([connection, model])}']`).check();
    const checked = await backend.waitForCall("A", "check_model_connection", at);
    await pageA.waitForSelector('[data-verdict="usable"]');
    await pageA.click('[data-model-apply="judgment"]');
    const applied = await backend.waitForCall("A", "select_model_connection", at);
    await renderSettled(pageA);
    return [checked.args.reference, applied.args.reference];
  };
  const [judgeCheck] = await pickAndApply("judge", "judge-model");
  assertEqual(judgeCheck.protocol, "openai_json", "a connection judgment never used was asked in another format");
  await openSettings(pageA, "api-routers");
  await openSettings(pageA, "judgment-models");
  await renderSettled(pageA);
  await pageA.evaluate(() => { document.getElementById("model-advanced").open = true; document.getElementById("model-advanced-format-fold").open = true; });
  assertEqual(await pageA.locator('#model-advanced [data-judgment-format="bench"]').inputValue(), "system_one", "「고급」 shows Bench in another format once judgment moved on");
  const [benchCheck, benchApply] = await pickAndApply("bench", "bench-a");
  assertEqual(benchCheck, { id: "bench", generation: 4, model: "bench-a", protocol: "system_one" }, "coming back to Bench asked it in another format");
  assertEqual(benchApply, benchCheck, "the apply wrote another reference than the one checked");
  assertEqual(await pageA.locator('#model-advanced [data-judgment-format="judge"]').inputValue(), "openai_json", "Judge's format changed with nobody touching 「고급」");

  // t-76211: judgment on Judge again, then the window starts over — its
  // memory of formats gone, the connections read afresh from the settings.
  await pickAndApply("judge", "judge-model");
  await pageA.evaluate(() => {
    for (const held of [modelConnectionsController.checks, modelConnectionsController.judged, modelConnectionsController.formats, modelConnectionsController.usedFormats]) held.clear();
    return refreshApiRouters();
  });
  await renderSettled(pageA);
  const [restartedCheck] = await pickAndApply("bench", "bench-a");
  assertEqual(restartedCheck.protocol, "system_one", "after a restart Bench was asked in another format than it was last applied in");
  return "Bench system_one → Judge openai_json → (screen reopened) Bench system_one for check+apply → Judge → (restart) Bench system_one";
}));

await test("검수 F4: 멈춘 이 컴퓨터 모델을 고르면 요청 없이 「실행 뒤 확인합니다」라고만 말하고, 확인 전에는 통과라고 하지 않는다 (t-76028)", async () => withModelScene(() => {
  delete backend.managedModelState;
  backend.managedJudgmentSettings();
  Object.assign(backend.managedModelState, { instanceId: "fixture-stopped-instance", runtime: { state: "ready", downloadBytes: 0, reason: null } });
  backend.managedModelState.models[0].installed = true;
}, async () => {
  await pageA.click('[data-model-change="judgment"]');
  const at = backend.calls.length;
  await pageA.locator(`input[name="model-pick-judgment"][value="managed:fixture-decision"]`).check();
  await renderSettled(pageA);
  const asked = backend.calls.slice(at).filter((call) => /^(check_model_connection|preflight_judgment_provider|managed_judgment_)/.test(call.command));
  assertEqual(asked.length, 0, "picking asked something it does not wait for");
  const verdict = pageA.locator('[data-model-role="judgment"] .model-verdict');
  const words = await verdict.textContent();
  const later = await pageA.evaluate(() => t("settings.models.state.later", "실행 뒤 확인합니다"));
  const passed = await pageA.evaluate(() => t("settings.models.usableSub", "판단 확인을 통과했습니다. 적용을 누르면 다음 판단부터 이 모델을 씁니다."));
  assert(words.includes(later), "a stopped model does not say it is checked once it runs", words);
  assert(!words.includes(passed) && !words.includes(await pageA.evaluate(() => t("settings.models.state.usable", "쓸 수 있음"))), "a model nobody checked is said to have passed", words);
  assert(!await pageA.locator('[data-model-apply="judgment"]').isDisabled(), "the apply that checks it cannot be pressed");
  await modelScenePictures("managed-pick-later");
  return `stopped model picked: ${asked.length} requests · says 「${later}」 · no pass words`;
}));

await test("⑥ 창이 못 읽는 키 변수의 연결은 미리 그렇다고 말하고, 이전 판단 키는 어디서 어디로 옮기는지 보인 뒤 사람이 누를 때만 옮긴다", async () => withModelScene(() => {
  backend.zoSettings.providers = [{ name: "Private gateway", base_url: "https://private.example.test/v1", models: ["reason-model"], requires_auth: true, auth_env: "FIXTURE_FOREIGN_LLM_KEY", connection: { id: "private", generation: 3 } }];
  backend.legacyKeys.set("private", "[합성-이전-판단-키]");
}, async () => {
  await openSettings(pageA, "api-routers");
  await renderSettled(pageA);
  const row = pageA.locator('[data-connection-id="private"]');
  assertEqual(await row.locator(".model-state").getAttribute("data-model-state"), "unreadable");
  assert((await row.textContent()).includes("FIXTURE_FOREIGN_LLM_KEY"), "the row does not name the variable the window cannot read");
  await modelScenePictures("key-unreadable");
  const at = backend.calls.length;
  await row.locator("[data-legacy-move]").click();
  const confirm = row.locator('[data-legacy-confirm="private"]');
  assert(await confirm.isVisible(), "the move does not say from where to where first");
  assert((await confirm.textContent()).includes("https://private.example.test/v1"), "the move does not name the address the old key served");
  assertEqual(backend.calls.slice(at).filter((call) => call.command === "adopt_legacy_judgment_key").length, 0, "the key moved before the person confirmed");
  await confirm.locator("[data-legacy-confirm-move]").click();
  await backend.waitForCall("A", "adopt_legacy_judgment_key", at);
  await pageA.waitForFunction(() => !document.querySelector('[data-connection-id="private"] [data-legacy-move]'));
  const moved = backend.zoSettings.providers[0];
  assert(moved.auth_env.startsWith("ZEROCODE_ROUTER_"), "the row still names a variable the window cannot read");
  assertEqual(backend.keychain.get(`dev.zerocode.router.${moved.auth_env}`), "[합성-이전-판단-키]");
  return "unreadable variable said before any request · from→to line · moved on the person's press";
}));

await test("이 컴퓨터 판단 서버: 실행 중 · 다시 띄우는 중 · 멈춤 · 다른 작업 중 · 설정이 방금 바뀜을 실제 상태로 말하고 단추가 실제로 동작한다 (t-59744 ④⑥)", async () => withModelScene(() => {
  const provider = { kind: "local", protocol: "system_one", baseUrl: "http://127.0.0.1:32106", model: "fixture-decision", requiresAuth: false };
  backend.zoSettings.smart = { judgmentProvider: provider };
  delete backend.managedModelState;
  backend.managedJudgmentSettings();
  Object.assign(backend.managedModelState, {
    instanceId: "fixture-five-instance",
    configuredModelId: provider.model, appliedModelId: provider.model, runtime: { state: "ready", downloadBytes: 0, reason: null },
    server: { id: "fixture-five-server", state: "ready", modelId: provider.model, provider, checked: true, error: null },
  });
  backend.managedModelState.models[0].installed = true;
}, async () => {
  await openSettings(pageA, "api-routers");
  await renderSettled(pageA);
  const row = pageA.locator('[data-connection-id="managed:fixture-decision"]');
  const state = () => row.locator(".model-state").getAttribute("data-model-state");
  assertEqual(await state(), "running");
  const at = backend.calls.length;
  await row.locator('[data-managed-action="stop"]').click();
  assertEqual((await backend.waitForCall("A", "managed_judgment_stop", at)).args.serverId, "fixture-five-server", "the stop did not name the running server");
  await pageA.waitForFunction(() => document.querySelector('[data-connection-id="managed:fixture-decision"] .model-state')?.dataset.modelState === "stopped");
  assert(await row.locator('[data-managed-action="start"]').isVisible(), "a stopped server offers no restart");
  await modelScenePictures("managed-stopped");
  const snapshot = backend.managedJudgmentSettings();
  snapshot.revision += 1;
  snapshot.server = { ...snapshot.server, state: "starting", exit: { cause: "runtime_exited", code: 7, signal: null, task: null } };
  await backend.emitManaged(snapshot);
  await pageA.waitForFunction(() => document.querySelector('[data-connection-id="managed:fixture-decision"] .model-state')?.dataset.modelState === "restarting");
  assert(await row.locator('[data-managed-action="stop"]').isVisible(), "a restart that waits offers no stop");
  snapshot.revision += 1;
  snapshot.server = { ...snapshot.server, state: "stopped", exit: null };
  snapshot.operation = { id: "fixture-download", kind: "download", state: "running", modelId: "fixture-decision", receivedBytes: 1, totalBytes: 4 };
  await backend.emitManaged(snapshot);
  await pageA.waitForFunction(() => document.querySelector('[data-connection-id="managed:fixture-decision"] .model-state')?.dataset.modelState === "busy");
  assert(await row.locator('[data-managed-action="stop"]').isDisabled(), "a busy server can be stopped");
  assert(await row.locator('[data-managed-action="cancel"]').isVisible(), "a download offers no cancel");
  snapshot.revision += 1;
  snapshot.operation = null;
  snapshot.server = { ...snapshot.server, state: "ready", exit: null };
  await backend.emitManaged(snapshot);
  await pageA.waitForFunction(() => document.querySelector('[data-connection-id="managed:fixture-decision"] .model-state')?.dataset.modelState === "running");
  backend.managedStopRefusal = "settings_changed";
  await row.locator('[data-managed-action="stop"]').click();
  await pageA.waitForFunction(() => document.querySelector('[data-connection-id="managed:fixture-decision"] .model-state')?.dataset.modelState === "changed");
  assert(await row.locator('[data-managed-action="reread"]').isVisible(), "a refusal for moved settings offers no fresh read");
  delete backend.managedStopRefusal;
  return "running · restarting · stopped · busy · changed, each from the snapshot or the refusal";
}));

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
    await pageA.evaluate(() => modelConnectionsController.addManual());
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
    backend.routerProbeRefusal = { kind: "http", status: 401, message: body };
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

    // ---- 5. one switch, and where the features are (§6.1) ---------------
    const jevCard = await pageA.evaluate(() => {
      const card = document.getElementById("typesafe-card");
      const toggle = card.querySelector("[data-jev-switch]");
      return {
        switches: card.querySelectorAll("[data-jev-switch]").length,
        role: toggle?.getAttribute("role") ?? null,
        type: toggle?.type ?? null,
        rows: card.querySelectorAll("[data-jev-row]").length,
        link: card.querySelector("#jev-open-dashboard")?.classList.contains("btn") ?? null,
      };
    });
    assertEqual(
      jevCard,
      { switches: 1, role: "switch", type: "checkbox", rows: 0, link: false },
      "the card is not one switch and a way to the dashboard",
    );

    // ---- 6. one thing to finish, and it is the only filled button --------
    // The judgment editor is one item of the model pane's 고급 fold now
    // (t-75762 v2); the rule holds for it as it held for the old card.
    for (const [card, finish] of [["router-card", "router-save-btn"], ["judgment-connection-settings", "judgment-save-provider"]]) {
      const bottom = await pageA.evaluate((id) => {
        const box = document.getElementById(id);
        const last = id === "judgment-connection-settings" ? box.querySelector(".settings-judgment-actions") : box.lastElementChild;
        const filled = [...box.querySelectorAll(".btn--primary")].filter((one) => !one.closest(".judgment-model-source")).map((one) => one.id);
        return {
          actions: last.className,
          // A notice's own action (the switch's 「모든 폴더에서 사용」) is the
          // notice's, not a button loose above the card's footer.
          buttonsOutside: [...box.querySelectorAll(".btn")]
            .filter((one) => one.closest(".settings-action-row") !== last && !one.closest(".settings-notice") && !one.closest("#judgment-local-mode, #managed-judgment-panel, .judgment-model-source, .judgment-connect-actions"))
            .map((one) => one.id),
          filled,
        };
      }, card);
      assert(bottom.actions.includes("settings-action-row"), `${card} does not group its connection actions`, bottom);
      assertEqual(bottom.buttonsOutside, [], `${card} leaves a button loose above its footer`);
      assertEqual(bottom.filled, [finish], `${card} does not fill exactly the one thing to finish`);
    }
    return `key ${Math.round(key.width)}px in a ${Math.round(pane.width)}px column`;
  } finally {
    await pageA.setViewportSize({ width: 1280, height: 860 });
    await renderSettled(pageA);
  }
});

// The integration queue's switch in the project pane (t-45315, test 29): off until a person turns
// it on, saved for the repository by `set_integration_queue_enabled` and read back when the pane
// opens again. Where this machine does not run the queue (Windows for now) the row is locked and
// says why.
await test("the project pane's integration queue switch starts off, and turning it on and off is saved", async () => {
  backend.integrationQueue = { enabled: false, supported: true };
  try {
    await pageA.evaluate(() => setSettingsOpen(false));
    const from = backend.calls.length;
    await openSettings(pageA, "project");
    await backend.waitForCall("A", "integration_queue_setting", from);
    await renderSettled(pageA);
    const box = pageA.locator("#project-integration-queue");
    assert(await box.isVisible(), "the project pane shows no integration queue switch");
    assert(!(await box.isChecked()), "the integration queue switch was on before anyone turned it on");
    assert(!(await box.isDisabled()), "the integration queue switch was locked on macOS");
    assert(await pageA.locator("#project-integration-queue-why").isHidden(), "the switch gave a lock reason on macOS");
    const on = await gestureAndWait(pageA, "A", "set_integration_queue_enabled", () => box.click());
    assertEqual(on.args, { enabled: true }, "turning the switch on did not save it on");
    await pageA.waitForFunction(
      () => document.getElementById("project-integration-queue")?.checked === true,
      undefined,
      { timeout: UI_TIMEOUT },
    );
    assertEqual(backend.integrationQueue, { enabled: true, supported: true }, "the backend did not hold the switch on");
    const reread = backend.calls.length;
    await reopenSettings(pageA, "A", "project");
    await backend.waitForCall("A", "integration_queue_setting", reread);
    await renderSettled(pageA);
    assert(await box.isChecked(), "the switch did not read back on when the pane opened again");
    const off = await gestureAndWait(pageA, "A", "set_integration_queue_enabled", () => box.click());
    assertEqual(off.args, { enabled: false }, "turning the switch off did not save it off");
    await pageA.waitForFunction(
      () => document.getElementById("project-integration-queue")?.checked === false,
      undefined,
      { timeout: UI_TIMEOUT },
    );
    assertEqual(backend.integrationQueue, { enabled: false, supported: true }, "the backend did not hold the switch off");
  } finally {
    backend.integrationQueue = { enabled: false, supported: true };
    await pageA.evaluate(() => setSettingsOpen(false));
  }
});

await test("on Windows the integration queue switch is locked and says why", async () => {
  backend.integrationQueue = { enabled: false, supported: false };
  try {
    await pageA.evaluate(() => setSettingsOpen(false));
    const from = backend.calls.length;
    await openSettings(pageA, "project");
    await backend.waitForCall("A", "integration_queue_setting", from);
    await pageA.waitForFunction(
      () => document.getElementById("project-integration-queue")?.disabled === true,
      undefined,
      { timeout: UI_TIMEOUT },
    );
    const why = pageA.locator("#project-integration-queue-why");
    assert(await why.isVisible(), "the locked switch did not say why");
    const said = await why.textContent();
    assert(said.includes("macOS"), "the lock reason did not name macOS", said);
    const before = backend.count("A", "set_integration_queue_enabled");
    await pageA.locator("#project-integration-queue").click({ force: true, timeout: 2_000 }).catch(() => {});
    await renderSettled(pageA);
    assertEqual(backend.count("A", "set_integration_queue_enabled"), before, "the locked switch still asked to save");
    assert(!(await pageA.locator("#project-integration-queue").isChecked()), "the locked switch turned on");
  } finally {
    backend.integrationQueue = { enabled: false, supported: true };
    await pageA.evaluate(() => setSettingsOpen(false));
  }
});

/* The page that shows Jev's judgments is named for what it holds — the person's decision models — not
 * for the engine (10-17, the person: 「Jev 대시보드」→「의사결정 모델」). Visible words only: the rail's tab,
 * the page title and the TypeSafe card's line, in every language; ids, keys and file names keep their
 * names. The settings' own "Decision model(s)" (settings.judgment.*, settings.managed.*) are another
 * thing and stay; the page is "Decisions" in English so the two never read as one. */
await test("the Jev page is named for the decisions it shows — the tab, the title and the TypeSafe card's line, in every language", async () => {
  const names = { ko: "의사결정 모델", en: "Decisions", ja: "意思決定", zh: "决策", es: "Decisiones" };
  const engineName = /대시보드|dashboard|ダッシュボード|仪表板|panel de jev/i;
  const seen = {};
  try {
    for (const locale of SETTINGS_QUALITY_SPEC.locales) {
      await setQualityLocale(pageA, locale);
      // Read where the person reads it, not through t() with a fallback of the test's own: Korean keeps no
      // table row for these keys, so its words are the surfaces' own (the tab label, the page the rail
      // opens, the card's line), and a fallback written here would only ever echo itself.
      const words = await pageA.evaluate(() => {
        const page = buildJevView();
        return {
          tab: tabLabel({ kind: "jev" }),
          title: page.querySelector(".jev-title")?.textContent ?? null,
          aria: page.getAttribute("aria-label"),
          line: document.querySelector('[data-i18n="settings.typesafe.dashboardLine"]')?.textContent ?? null,
          navSr: document.querySelector("#nav-jev .sr")?.textContent ?? null,
          navTip: document.getElementById("nav-jev")?.dataset.tip ?? null,
        };
      });
      seen[locale] = words;
      const want = names[locale];
      assert(words.tab === want && words.title === want && words.aria === want, `${locale}: the tab and the page's title say its name`, words);
      assert(words.navSr === want, `${locale}: the rail's tab reads the page's name`, words);
      assert(words.line?.includes(want) && !engineName.test(words.line), `${locale}: the TypeSafe card's line names the page`, words);
      if (locale === "ko") assert(words.navTip === want, "ko: the rail's tip is the page's name", words);
    }
  } finally {
    backend.settings.locale = "ko";
  await setQualityLocale(pageA, "ko");
  }
  return JSON.stringify(seen);
});

/* The Google card names what uses the sign-in: zo, and the Antigravity gauge that reads the same login.
 * Its signed-out line and its hint name both, in every language. */
await test("the Google card names zo and the gauge as the sign-in's users — in every language", async () => {
  const gauge = /게이지|gauge|ゲージ|用量条|medidor/i;
  const seen = {};
  try {
    for (const locale of SETTINGS_QUALITY_SPEC.locales) {
      await setQualityLocale(pageA, locale);
      const words = await pageA.evaluate(() => {
        const was = googleAccount;
        try {
          googleAccount = { signed_in: false, store: "" };
          const none = googleLoginUnder();
          googleAccount = { signed_in: true, renewable: true, store: "file", email: "dev@example.com" };
          return { none, hint: googleAccountNote() };
        } finally {
          googleAccount = was;
        }
      });
      seen[locale] = words;
      assert(typeof words.none === "string" && words.none.includes("zo") && gauge.test(words.none), `${locale}: the signed-out line names zo and the gauge`, words);
      assert(typeof words.hint === "string" && words.hint.includes("zo") && gauge.test(words.hint), `${locale}: the hint names zo and the gauge`, words);
    }
  } finally {
    backend.settings.locale = "ko";
  await setQualityLocale(pageA, "ko");
  }
  return JSON.stringify(seen);
});

/* The pages booted before a scene keeps a record have each made their one
   start-time read of it already (and found none): a scene's record is read
   only by the windows the scene starts. */
async function pastStartQuietChecks() {
  await pageA.waitForFunction(() => modelConnectionsController.memory !== null, null, { timeout: 30_000 });
}

/* t-76500: a connection checked once stays 「연결됨」 across a window start
   — the window keeps each connection's last check (state words, model count,
   time, generation; never a key or a server's words) — and the ones whose
   check is old, never made or made against another generation are checked
   again one at a time without a press; a fresh one is not asked again. */
await test("모델 연결: 확인한 연결은 창을 다시 켜도 「연결됨 · N시간 전 확인」이고, 오래됐거나 안 한 것만 누르지 않아도 하나씩 다시 확인한다 (t-76500)", async () => {
  const HOUR = 60 * 60 * 1000;
  const T0 = Date.UTC(2026, 9, 26, 12);
  await pastStartQuietChecks();
  const saved = { providers: clone(backend.zoSettings.providers), answers: backend.routerProbeAnswers, memory: backend.connectionMemory, locale: backend.settings.locale };
  const row = (name, base, id, generation = 1) => ({ name, base_url: base, models: [`${id}-one`, `${id}-two`], requires_auth: false, connection: { id, generation } });
  backend.zoSettings.providers = [
    row("Gateway", "https://gateway.example.test/v1", "gateway"),
    row("Bench", "https://bench.example.test/v1", "bench"),
    row("Stale", "https://stale.example.test/v1", "stale"),
  ];
  backend.routerProbeAnswers = Object.fromEntries(backend.zoSettings.providers.map((provider) => [provider.base_url, provider.models]));
  backend.connectionMemory = { nowMs: T0, connections: {} };
  const probes = (from) => backend.calls.slice(from).filter((call) => call.command === "test_router_connection").map((call) => call.args.sourceId);
  const states = (page) => page.evaluate(() => Object.fromEntries([...document.querySelectorAll('#model-connection-groups .model-line[data-connection-id]')]
    .map((line) => [line.dataset.connectionId, { state: line.querySelector(".model-state").dataset.modelState, notes: [...line.querySelectorAll(".model-line-note")].map((note) => note.textContent) }])));
  const pages = [];
  const boot = async (id) => {
    const page = await context.newPage();
    pages.push(page);
    await installTauri(page, id);
    attachFaultRecorder(page, id);
    await bootPage(page, id);
    return page;
  };
  try {
    // ---- the first start: nothing kept, so each is checked once, unasked --
    const firstAt = backend.calls.length;
    const first = await boot("memory-first");
    await openSettings(first, "api-routers");
    await first.waitForFunction(() => ["gateway", "bench", "stale"].every((id) => modelConnectionsController.checks.get(id)?.state === "ok"), null, { timeout: UI_TIMEOUT });
    assertEqual(probes(firstAt).sort(), ["bench", "gateway", "stale"], "the first start did not check each connection exactly once");
    assertEqual(Object.keys(backend.connectionMemory.connections).sort(), ["bench", "gateway", "stale"], "a check was not kept");
    assertEqual(backend.connectionMemory.connections.gateway, { generation: 1, state: "ok", count: 2, checkedMs: T0 }, "the kept check holds more or less than its words");
    const format = await first.evaluate(() => modelConnectionsController.judgmentFormat(modelConnectionsController.state.connections.find((one) => one.id === "gateway")));
    await first.close();

    // ---- three hours later the window starts again ---------------------------
    backend.connectionMemory.nowMs = T0 + 3 * HOUR;
    backend.connectionMemory.connections.stale.checkedMs = T0 - 4 * HOUR;
    backend.connectionMemory.connections.gateway.judgments = [{ model: "gateway-two", protocol: format, state: "schema", checkedMs: T0 }];
    backend.zoSettings.providers.find((provider) => provider.name === "Bench").connection.generation = 2;
    backend.zoSettings.providers.push(row("Fresh", "https://fresh.example.test/v1", "fresh"));
    backend.routerProbeAnswers["https://fresh.example.test/v1"] = ["fresh-one"];
    let release;
    backend.routerProbeHold = new Promise((done) => { release = done; });
    const secondAt = backend.calls.length;
    const second = await boot("memory-second");
    await openSettings(second, "api-routers");
    await backend.waitForCall("memory-second", "test_router_connection", secondAt);
    await renderSettled(second);
    const held = await states(second);
    assertEqual(held.gateway.state, "ok", "a connection checked three hours ago says 「아직 확인 안 함」 after a start");
    const ago = await second.evaluate(() => t("settings.models.connectedAgo", "연결됨 · {{ago}} 확인 — 모델 {{count}}개", { ago: new Intl.RelativeTimeFormat("ko", { numeric: "auto" }).format(-3, "hour"), count: 2 }));
    assert(held.gateway.notes.includes(ago), "the kept check does not say how long ago it was made", held.gateway);
    const schema = await second.evaluate(() => modelStateWords("schema"));
    assert(held.gateway.notes.some((note) => note.includes(schema.word)), "the kept judgment check of a model was forgotten", held.gateway);
    assertEqual(Object.values(held).filter((one) => one.state === "checking").length, 1, "more than one connection is checked at a time", held);
    assert(["changed", "checking"].includes(held.bench.state), "a check made against the old setting was drawn as if it still stood", held.bench);
    assert(["unknown", "checking"].includes(held.fresh.state), "a connection never checked wears another state", held.fresh);
    assert(["ok", "checking"].includes(held.stale.state), "an old check was dropped before the new one came", held.stale);
    release();
    delete backend.routerProbeHold;
    await second.waitForFunction(() => ["bench", "stale", "fresh"].every((id) => modelConnectionsController.checks.get(id)?.state === "ok"), null, { timeout: UI_TIMEOUT });
    await renderSettled(second);
    assertEqual(probes(secondAt).sort(), ["bench", "fresh", "stale"], "the start did not check exactly the old, changed and never-checked connections, once each");
    assertEqual(backend.connectionMemory.connections.stale.checkedMs, T0 + 3 * HOUR, "the quiet check was not kept");
    assertEqual(backend.connectionMemory.connections.gateway.judgments?.length, 1, "a quiet check of another connection cost a judgment check");
    // Opening the page again asks nothing more: every check is fresh.
    const againAt = backend.calls.length;
    await second.evaluate(() => setSettingsOpen(false));
    await openSettings(second, "api-routers");
    await backend.waitForCall("memory-second", "remembered_model_connection_checks", againAt);
    await renderSettled(second);
    assertEqual(probes(againAt), [], "a fresh check was asked again");
    // Five languages, 390 px: the new words stay inside the pane.
    for (const locale of ["ko", "en", "ja", "zh", "es"]) {
      await setQualityLocale(second, locale);
      await second.setViewportSize({ width: 390, height: 900 });
      await second.evaluate(() => modelConnectionsController.render());
      await renderSettled(second);
      const wide = await second.locator(".settings-pane:not([hidden])").evaluate((root) => {
        const right = root.getBoundingClientRect().right;
        return [...root.querySelectorAll("*")].filter((node) => node.getClientRects().length && node.getBoundingClientRect().right > right + 1).map((node) => node.className).slice(0, 3);
      });
      assertEqual(wide, [], `${locale} 390px reaches past its pane`);
    }
    return `first start 3 quiet checks · start +3 h: gateway 「${ago}」, 3 checked one at a time (old, changed, never), 0 asked of the fresh one`;
  } finally {
    backend.zoSettings.providers = saved.providers;
    backend.routerProbeAnswers = saved.answers;
    if (saved.memory === undefined) delete backend.connectionMemory; else backend.connectionMemory = saved.memory;
    delete backend.routerProbeHold;
    backend.settings.locale = saved.locale;
    for (const page of pages) if (!page.isClosed()) await page.close();
  }
});

/* t-76500 B: the window's start checks a due connection with nobody opening
   settings — after its first paint and an idle moment, never before. */
await test("모델 연결: 설정을 열지 않아도 창이 뜬 뒤 한가할 때 오래된 연결을 한 번 확인하고, 첫 그림 전에는 묻지 않는다 (t-76500)", async () => {
  const HOUR = 60 * 60 * 1000;
  const T0 = Date.UTC(2026, 9, 27, 0);
  await pastStartQuietChecks();
  const saved = { providers: clone(backend.zoSettings.providers), answers: backend.routerProbeAnswers, memory: backend.connectionMemory };
  backend.zoSettings.providers = [
    { name: "Gateway", base_url: "https://gateway.example.test/v1", models: ["one"], requires_auth: false, connection: { id: "gateway", generation: 1 } },
    { name: "Recent", base_url: "https://recent.example.test/v1", models: ["one"], requires_auth: false, connection: { id: "recent", generation: 1 } },
  ];
  backend.routerProbeAnswers = { "https://gateway.example.test/v1": ["one"], "https://recent.example.test/v1": ["one"] };
  backend.connectionMemory = { nowMs: T0, connections: {
    gateway: { generation: 1, state: "ok", count: 1, checkedMs: T0 - 7 * HOUR },
    recent: { generation: 1, state: "ok", count: 1, checkedMs: T0 - HOUR },
  } };
  const page = await context.newPage();
  try {
    await installTauri(page, "memory-boot");
    attachFaultRecorder(page, "memory-boot");
    /* The page's start holds its next frame, its 10 s timer and its idle
       moment in the test's hands: nothing here waits on a real frame, a real
       10 s or a real idle callback. Only what shell-connections.js asks for
       is held; every other frame and timer runs as it does. */
    await page.addInitScript(() => {
      const held = { frames: [], timers: [], idles: [] };
      window.__BOOT_CLOCK__ = held;
      const mine = () => new Error().stack.includes("shell-connections.js");
      const frame = window.requestAnimationFrame.bind(window);
      window.requestAnimationFrame = (callback) => (mine() ? held.frames.push(callback) : frame(callback));
      const timer = window.setTimeout.bind(window);
      window.setTimeout = (callback, delay, ...rest) => (mine() && delay >= 10_000 ? held.timers.push({ callback, delay }) : timer(callback, delay, ...rest));
      const idle = window.requestIdleCallback.bind(window);
      window.requestIdleCallback = (callback, options) => (mine() ? held.idles.push({ callback, options }) : idle(callback, options));
    });
    const from = backend.calls.length;
    await bootPage(page, "memory-boot");
    const probes = () => backend.calls.slice(from).filter((call) => call.command === "test_router_connection").map((call) => call.args.sourceId);
    const clock = () => page.evaluate(() => ({ frames: window.__BOOT_CLOCK__.frames.length, timers: window.__BOOT_CLOCK__.timers.map((one) => one.delay), idles: window.__BOOT_CLOCK__.idles.map((one) => one.options?.timeout ?? null) }));
    const settle = () => page.evaluate(() => new Promise((done) => window.setTimeout(done, 0)));
    // The first frame has not come: no timer, no idle moment, no provider asked.
    assertEqual(await clock(), { frames: 1, timers: [], idles: [] }, "the start did not wait for its first frame");
    assertEqual(probes(), [], "the window asked a provider before its first paint settled");
    // The frame comes: the 10 s wait is set, and still nothing is asked.
    await page.evaluate(() => { const [frame] = window.__BOOT_CLOCK__.frames.splice(0); frame(performance.now()); });
    assertEqual(await clock(), { frames: 0, timers: [10_000], idles: [] }, "the first frame did not set exactly one 10 s wait");
    await settle();
    assertEqual(probes(), [], "the window asked a provider before its 10 s wait was over");
    // The 10 s pass: an idle moment is asked for, and still nothing is asked.
    await page.evaluate(() => { const [one] = window.__BOOT_CLOCK__.timers.splice(0); one.callback(); });
    assertEqual(await clock(), { frames: 0, timers: [], idles: [10_000] }, "the 10 s wait did not ask for one idle moment");
    await settle();
    assertEqual(probes(), [], "the window asked a provider before an idle moment came");
    // The idle moment comes: the one old connection is checked, once.
    await page.evaluate(() => { const [one] = window.__BOOT_CLOCK__.idles.splice(0); one.callback({ didTimeout: false, timeRemaining: () => 50 }); });
    await backend.waitForCall("memory-boot", "test_router_connection", from);
    await page.waitForFunction(() => !modelConnectionsController.quieting, null, { timeout: UI_TIMEOUT });
    assertEqual(probes(), ["gateway"], "the start did not check exactly the old connection once");
    assertEqual(backend.connectionMemory.connections.gateway.checkedMs, T0, "the start's quiet check was not kept");
    assert(await page.evaluate(() => !document.querySelector(".settings-pane:not([hidden])")?.offsetParent), "the quiet check opened settings");
    return "0 asked before the frame, the 10 s wait and the idle moment; then 1 quiet check; the 1 h old one not asked";
  } finally {
    backend.zoSettings.providers = saved.providers;
    backend.routerProbeAnswers = saved.answers;
    if (saved.memory === undefined) delete backend.connectionMemory; else backend.connectionMemory = saved.memory;
    await page.close();
  }
});

await (await import("./settings-gguf.mjs")).runSettingsGguf({ test, assert, assertEqual, backend, withJudgmentStatus, setQualityLocale });
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
endRun(failed ? 1 : 0);
