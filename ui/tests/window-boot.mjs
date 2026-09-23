/* Shared window bootstrap. The full window gate and baseline use one backend. */
import { createRequire } from "node:module";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { readdir, readFile } from "node:fs/promises";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
const UI = resolve(dirname(fileURLToPath(import.meta.url)), "..");
/* The primary modifier as a synthetic-event spread — ⌘ on macOS, Ctrl
 * elsewhere. `shell.js` reads the same platform; a test that builds a ⌘/Ctrl
 * click spreads `window.__TEST_PRIMARY_EVENT__` in. */
export const PRIMARY_EVENT = process.platform === "darwin"
  ? Object.freeze({ metaKey: true })
  : Object.freeze({ ctrlKey: true });
const TERMINAL_THEME_CATALOG = Object.freeze(JSON.parse(await readFile(resolve(UI, "..", "crates/zerocode-shell/src/terminal_themes.json"), "utf8")));
/* Claude Code's spinner verbs, read off the core catalog (`CLAUDE_SPINNER_
 * VERBS`, agent.rs) rather than copied — the stub row speaks the one list. */
const CLAUDE_SPINNER_VERBS = Object.freeze(
  [...((await readFile(resolve(UI, "..", "crates/zerocode-core/src/agent.rs"), "utf8"))
    .match(/pub const CLAUDE_SPINNER_VERBS: &\[&str\] = &\[([\s\S]*?)\];/)?.[1] ?? "")
    .matchAll(/"([^"]+)"/g)].map((hit) => hit[1]),
);
let chromium;
try {
  // Resolved from wherever node can see it, including a global install —
  // `import "playwright"` alone misses that, and its CommonJS export has no
  // named `chromium` to destructure from an ESM import.
  const require = createRequire(import.meta.url);
  let entry;
  try {
    entry = require.resolve("playwright");
  } catch {
    const roots = await new Promise((done) => {
      const npm = spawn("npm", ["root", "-g"], { stdio: ["ignore", "pipe", "ignore"] });
      let out = "";
      npm.stdout.on("data", (chunk) => (out += chunk));
      npm.on("close", () => done(out.trim()));
      npm.on("error", () => done(""));
    });
    if (!roots) throw new Error("no global npm root");
    entry = require.resolve(join(roots, "playwright"));
  }
  ({ chromium } = require(entry));
} catch {
  console.error(
    "FAIL  Playwright is unavailable — run `npm ci && npx playwright install chromium`",
  );
  process.exit(1);
}

const BOOT = {
  /* 이 하네스의 볼트(`/vault`)는 마지막으로 전체 지도로 본 볼트다 — 지식 그래프를 여는
     창 검사는 전체 그림을 읽는다(부팅 보고의 한 줄, 실제 설정 문서와 같은 자리). 진짜
     첫 방문(줄 없음 → 주변 탐색)은 그래프 하네스의 S2가 검사한다. */
  second_brain_explore: { "/vault": JSON.stringify({ mode: "global" }) },
  explorer_policy: JSON.parse(await readFile(resolve(UI, "..", "crates/zerocode-shell/src/explorer_policy.json"), "utf8")),
  claude_spinner_verbs: CLAUDE_SPINNER_VERBS,
  project_root: "/tmp/zerocode-window-test",
  active_root: "/tmp/zerocode-window-test",
  project: "zerocode",
  hidden_shortcuts: [],
  keybindings: {
    "tab.close": [],
    "view.toggleAside": [],
    "terminal.newTab": [],
    "terminal.toggle": [],
  },
  zo: "/usr/local/bin/zo",
  lanes: [],
  worktrees: [],
  recent_projects: [],
  theme: "dark",
  locale: "ko",
  ui_zoom_level: 0,
  ui_zoom_spec: {
    min_level: -3,
    max_level: 5,
    step: 0.5,
    default_level: 0,
    scale_base: 1.2,
  },
  app_font_family: "Geist",
  status_bar_items: ["claude", "codex", "resource-usage", "ports"],
  usage_percentage_display: "used",
  status_bar_usage_mode: "verbose",
  show_titlebar_app_name: true,
  show_menu_bar_icon: true,
  minimize_to_tray_on_close: false,
  compact_worktree_cards: false,
  agent_activity_display: "compact",
  worktree_card_properties: ["branch", "ports", "agents", "default-badge"],
  workspace_board: {
    statuses: [
      { id: "todo", label: "Todo", color: "neutral", icon: "circle" },
      { id: "in-progress", label: "In progress", color: "conductor-progress", icon: "conductor-progress" },
      { id: "in-review", label: "In review", color: "conductor-review", icon: "conductor-review" },
      { id: "completed", label: "Done", color: "conductor-done", icon: "conductor-done" },
    ],
    cards: {},
    column_width: 308,
  },
  sidebar_view: { group_by: "repo", sort_by: "default", project_order: "default" },
  show_git_ignored_files: true,
  source_control_group_order: "changes-first",
  source_control_compare_base: "repository-default",
  refresh_local_base_ref_on_worktree_create: false,
  left_sidebar_appearance_mode: "default",
  left_sidebar_tint_color: "#18181b",
  left_sidebar_tint_opacity: 0.08,
  left_sidebar_appearance_spec: {
    tint_color_default: "#18181b",
    tint_opacity: { min: 0, max: 0.35, step: 0.01, default: 0.08 },
  },
  panel_widths: { sidebar: 280, aside: 350 },
  floating_workspace: { enabled: true, cwd: "~", trigger_location: "floating-button" },
  serve: "down",
  system_locale: "ko",
  terminal_command: "zo",
  setup_script_launch_mode: "new-tab",
  terminal_shortcut_policy: "orca-first",
  editing_prefs: {
    editor_auto_save: false,
    editor_auto_save_delay_ms: 1000,
    editor_minimap_enabled: false,
    editor_word_wrap: true,
    diff_word_wrap: false,
    rich_markdown_spellcheck_enabled: true,
    markdown_review_tools_enabled: true,
    editor_font_family: "",
    combined_diff_file_tree_visible_by_default: false,
    primary_selection_middle_click_paste: null,
    editor_font_zoom: 0,
  },
  editing_prefs_spec: {
    auto_save_delay_ms: { min: 250, max: 10000, step: 250 },
    editor_font_zoom: {
      min_level: -6, max_level: 18, step: 1, default_level: 0, px_min: 8, px_max: 32,
    },
  },
  diff_side_by_side: true,
  conversation_focus_view: false,
  terminal_prefs: {
    font_size: 14,
    font_family: "",
    ligatures: "auto",
    mac_option_as_alt: "auto",
    jis_yen_to_backslash: false,
    allow_osc52_clipboard: true,
    windows_shell: "powershell.exe",
    windows_powershell_implementation: "auto",
    theme_dark: "Ghostty Default Style Dark",
    use_separate_light_theme: true,
    theme_light: "Builtin Tango Light",
    custom_themes: [],
    color_overrides: {},
    leading: 1.15,
    weight: 500,
    cursor_blink: true,
    cursor_style: "block",
    cursor_opacity: 1,
    padding_x: 4,
    padding_y: 4,
    sensitivity: 1.15,
    scrollback: 5000,
    word_separators: " ()[]{}',\"`",
    fast_scroll_sensitivity: 5,
    tui_scroll_sensitivity: 1,
    focus_follows_mouse: false,
    hide_mouse_while_typing: false,
    copy_on_select: false,
    right_click_paste: false,
    inactive_pane_opacity: 0.9,
    divider_color_dark: "#3f3f46",
    divider_color_light: "#d4d4d8",
    divider_thickness_px: 3,
  },
  terminal_prefs_spec: {
    font_size: { min: 10, max: 24, step: 1 },
    font_family_defaults: {
      macos: "SF Mono",
      windows: "Cascadia Mono",
      linux: "DejaVu Sans Mono",
    },
    ligature_modes: ["auto", "on", "off"],
    mac_option_as_alt_modes: ["auto", "true", "left", "right", "false"],
    windows_shells: ["powershell.exe", "cmd.exe", "git-bash"],
    windows_powershell_implementations: ["auto", "powershell.exe", "pwsh.exe"],
    ligature_font_tokens: [
      "fira code", "fira mono", "jetbrains mono", "jetbrainsmono",
      "cascadia code", "cascadia mono", "iosevka", "victor mono",
      "hasklig", "monoid", "operator mono", "dank mono", "mononoki",
      "pragmatapro", "recursive", "monolisa", "commit mono", "geist mono",
      "maple mono", "departure mono",
    ],
    theme_defaults: {
      dark: "Ghostty Default Style Dark",
      light: "Builtin Tango Light",
    },
    terminal_themes: TERMINAL_THEME_CATALOG,
    color_override_groups: [
      {
        id: "base",
        keys: [
          "foreground", "background", "cursor", "cursorAccent",
          "selectionBackground", "selectionForeground", "bold",
        ],
      },
      {
        id: "normal",
        keys: ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"],
      },
      {
        id: "bright",
        keys: [
          "brightBlack", "brightRed", "brightGreen", "brightYellow",
          "brightBlue", "brightMagenta", "brightCyan", "brightWhite",
        ],
      },
    ],
    leading: { min: 1, max: 3, step: 0.1 },
    leading_base: 1.15,
    weight: { min: 100, max: 900, step: 100 },
    bold_weight_floor: 700,
    bold_weight_offset: 200,
    cursor_styles: ["block", "bar", "underline"],
    cursor_opacity: { min: 0, max: 1, step: 0.05 },
    padding_x: { min: 0, max: 512, step: 1 },
    padding_y: { min: 0, max: 512, step: 1 },
    sensitivity: { min: 0.5, max: 3, step: 0.05 },
    fast_scroll_sensitivity: { min: 1, max: 10, step: 0.5 },
    tui_scroll_sensitivity: { min: 1, max: 10, step: 1 },
    scrollback: { min: 1000, max: 50000, step: 100 },
    scrollback_presets: [5000, 10000, 25000, 50000],
    word_separators_max_chars: 128,
    inactive_pane_opacity: { min: 0, max: 1, step: 0.05 },
    divider_color_defaults: { dark: "#3f3f46", light: "#d4d4d8" },
    divider_thickness_px: { min: 1, max: 32, step: 1 },
    divider_hit_padding_px: 6,
    max_custom_terminal_themes: 200,
    custom_theme_selection_prefix: "custom:",
  },
  confirm_close_pinned: true,
  skip_close_terminal_with_running_process_confirm: false,
  ctrl_tab_order_mode: "mru",
  workspace_creation_prefs: {
    directory: "~/zerocode/workspaces",
    nest_workspaces: true,
  },
  open_in_applications: [
    { id: "vscode", label: "VS Code", command: "code" },
  ],
  open_in_applications_spec: {
    max: 8,
    presets: [
      { id: "vscode", label: "VS Code", command: "code" },
      { id: "cursor", label: "Cursor", command: "cursor" },
      { id: "zed", label: "Zed", command: "zed" },
    ],
  },
  browser: {
    home_page: "",
    search_engine: "google",
    restore_tabs: false,
    open_links_in_app: false,
    open_links_in_app_modifier_inverts: false,
    open_links_in_app_prompted: false,
    terminal_link_action_popover: true,
    default_zoom_level: 1.5,
    open_tabs: [],
    user_agents: [],
  },
  browser_zoom_spec: {
    min_level: -3,
    max_level: 5,
    step: 0.5,
    default_level: 0,
    scale_base: 1.2,
  },
  skip_delete_worktree_confirm: false,
  skip_delete_automation_confirm: false,
  "vault.sessionLimit": 200,
  vault_limits: { choices: [50, 100, 200], absolute_max: 1000, default: 200, children_per_parent: 50 },
  artifacts_retention_days: 0,
  artifacts_retention_spec: { min: 0, max: 365, step: 1 },
};

const POLLER_DEFINITION =
  /^(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)|^(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=/;
const POLLER_CLOSES = /^[)}\]]/;
const POLLER_SIBLING_KEY = /^\s{0,4}[A-Za-z_$][\w$]*:/;
const POLLER_ASKS = /invoke\(\s*(?:["']|[A-Za-z_$][\w$]*\.command\b)/;
const POLLER_LITERAL_ASK = /invoke\(\s*["']([a-z0-9_]+)["']/g;
const POLLER_TABLE_ASK = /invoke\(\s*[A-Za-z_$][\w$]*\.command\b/;
const POLLER_TABLE_NAME = /^\s*command:\s*"([a-z0-9_]+)"/;
const POLLER_CALLS = /([A-Za-z_$][\w$]*)\s*\(/g;
const POLLER_BEAT_GUARD = /-\s*[A-Za-z_$][\w$]*\s*<\s*([A-Z][A-Z0-9_]*_BEAT)\b/;
const POLLER_REACH = 3;

const derivePollerCommands = async () => {
  /* 한 정의의 몸통. 이 파일들은 최상위 정의를 첫 칸에서 열고 첫 칸에서
   * 닫으므로, 「다음에 오는, 첫 칸에서 닫는 괄호」가 곧 그 끝이다 — 하네스가
   * 자바스크립트 파서를 들이는 것보다 이 규약을 읽는 편이 정직하다. */
  const bodyAt = (lines, at) => {
    const held = [lines[at]];
    for (let n = at + 1; n < lines.length && !POLLER_CLOSES.test(lines[n]); n += 1) held.push(lines[n]);
    return held;
  };
  const sources = [];
  for (const name of (await readdir(UI)).filter((one) => /^shell.*\.js$/.test(one)).sort()) {
    sources.push({ name, lines: (await readFile(join(UI, name), "utf8")).split("\n") });
  }
  const bodies = new Map();
  for (const source of sources) {
    for (let n = 0; n < source.lines.length; n += 1) {
      const named = POLLER_DEFINITION.exec(source.lines[n]);
      if (!named) continue;
      const name = named[1] ?? named[2];
      if (!bodies.has(name)) bodies.set(name, []);
      bodies.get(name).push({ source, body: bodyAt(source.lines, n) });
    }
  }
  const callsIn = (body) => {
    const names = new Set();
    for (const line of body) {
      for (const call of line.matchAll(POLLER_CALLS)) if (bodies.has(call[1])) names.add(call[1]);
    }
    return names;
  };

  const beats = new Map();
  for (const source of sources) {
    for (let n = 0; n < source.lines.length; n += 1) {
      const line = source.lines[n];
      if (/idlePoller\(\{/.test(line) && !/^\s*(?:export\s+)?(?:async\s+)?function\s/.test(line)) {
        const body = bodyAt(source.lines, n);
        for (let at = 0; at < body.length; at += 1) {
          if (!/^\s*(?:tick|onResume):/.test(body[at])) continue;
          const bare = /^\s*(?:tick|onResume):\s*([A-Za-z_$][\w$]*)\s*,?\s*$/.exec(body[at]);
          if (bare && bodies.has(bare[1])) {
            beats.set(bare[1], `${source.name}:${n + 1} idlePoller`);
            continue;
          }
          // 인라인 화살표: 그 속성 하나가 끝날 때까지가 그 박자의 몸이다.
          const held = [body[at]];
          for (let more = at + 1; more < body.length && !POLLER_SIBLING_KEY.test(body[more]); more += 1) {
            held.push(body[more]);
          }
          for (const name of callsIn(held)) beats.set(name, `${source.name}:${n + 1} idlePoller`);
        }
      }
      if (/\bsetInterval\(/.test(line)) {
        for (const name of callsIn(bodyAt(source.lines, n))) beats.set(name, `${source.name}:${n + 1} setInterval`);
      }
      const guard = POLLER_BEAT_GUARD.exec(line);
      if (!guard) continue;
      for (let up = n; up >= 0; up -= 1) {
        const named = POLLER_DEFINITION.exec(source.lines[up]);
        if (!named) continue;
        beats.set(named[1] ?? named[2], `${source.name}:${up + 1} ${guard[1]}`);
        break;
      }
    }
  }

  const commands = new Map();
  const reach = (name, depth, trail, walked) => {
    if (depth > POLLER_REACH || walked.has(name)) return;
    walked.add(name);
    for (const { source, body } of bodies.get(name) ?? []) {
      const text = body.join("\n");
      for (const ask of text.matchAll(POLLER_LITERAL_ASK)) {
        if (!commands.has(ask[1])) commands.set(ask[1], `${trail} > ${name}`);
      }
      // 공급자 표를 들고 묻는 자리(`invoke(provider.command)`) — 이름은 그
      // 파일의 표에 적혀 있다.
      if (POLLER_TABLE_ASK.test(text)) {
        for (const line of source.lines) {
          const listed = POLLER_TABLE_NAME.exec(line);
          if (listed && !commands.has(listed[1])) commands.set(listed[1], `${trail} > ${name} (표)`);
        }
      }
      if (POLLER_ASKS.test(text)) continue;
      for (const callee of callsIn(body)) if (callee !== name) reach(callee, depth + 1, `${trail} > ${name}`, walked);
    }
  };
  for (const [beat, why] of beats) reach(beat, 0, why, new Set());
  return { beats: [...beats.keys()].sort(), commands: new Map([...commands].sort()) };
};

const pollers = await derivePollerCommands();
const POLLER_COMMANDS = [...pollers.commands.keys()];

const stubBackend = ({ boot, pollers }) => {
  window.__CALLS__ = [];
  window.__LISTENERS__ = {};
  window.__CLIPBOARD_TEXT__ = "";
  window.__CLIPBOARD_WRITES__ = [];
  window.__CLIPBOARD_READS__ = 0;
  window.__CLIPBOARD_READ_HOLD__ = false;
  window.__CLIPBOARD_RELEASE__ = null;
  window.__CLIPBOARD_READ_FAIL__ = false;
  window.__CLIPBOARD_WRITE_FAIL__ = false;
  // Commands that should refuse. A window's answer to a failure is a screen
  // of its own, and there is no other way to reach it from here.
  window.__FAIL__ = new Set();
  // Answers a test swaps in mid-run, checked before the stub's own table —
  // how a test hands the window a real project catalog, then takes it back.
  window.__ANSWER__ = {};
  // The store path is a real one on purpose: the row shows it, and the logout
  // question names it.
  const GOOGLE_SIGNED_OUT = {
    signed_in: false,
    expired: false,
    renewable: false,
    scoped_for_agents: true,
    store: "/h/.zo/credentials.json",
  };
  window.__COUNTS__ = {};
  // 켜 둔 시험만 커맨드의 순서를 받아 적는다.
  window.__ORDER__ = null;
  // 첫 실행 마법사가 저장한 것들, 그리고 그 화면이 읽는 두 기계 상태.
  window.__ONB_STEPS__ = [];
  window.__ONB_CLOSED__ = null;
  window.__GH__ = "missing";
  // `env_line` beside `env` for the reason the row carries both: `env` is the
  // resolved plan and `env_line` is the line the field shows and writes back.
  const defaultAgentLaunchPlans = [
    { agent: "claude", args: "--dangerously-skip-permissions", env: [], env_line: "",
      permission: "unattended", has_switch: true, is_default: true },
    { agent: "codex", args: "--dangerously-bypass-approvals-and-sandbox", env: [], env_line: "",
      permission: "unattended", has_switch: true, is_default: true },
    { agent: "opencode", args: "", env: [], env_line: "", permission: "asks",
      has_switch: false, is_default: true },
    { agent: "goose", args: "", env: [["GOOSE_MODE", "auto"]], env_line: "GOOSE_MODE=auto",
      permission: "unattended", has_switch: true, is_default: true },
  ];
  // 마법사 다음의 세 표면이 읽고 쓰는 저장분. 실제 백엔드는 이것을
  // `onboarding.json` 한 파일에 든다 — 여기서도 하나의 객체다: 목록을 셋으로
  // 쪼개면 커맨드마다 다른 사실을 보게 되고, 그런 목 위의 통과는 아무것도
  // 증명하지 않는다.
  window.__ONB__ = {
    flow_version: 1,
    closed_at: 1,
    outcome: "completed",
    last_completed_step: 4,
    checklist: {
      chose_agent: false,
      dismissed: false,
      notified: false,
      ran_second_agent: false,
      reviewed_diff: false,
      opened_pr: false,
      used_palette: false,
      shaped_sidebar: false,
      opened_file: false,
    },
    guide_dismissed: false,
    tours_auto: true,
    // 기본은 **이미 다 배운 프로필**이다. 팁은 앱 오픈당 하나가 저절로 뜨고
    // 코치마크는 화면을 열 때마다 후보가 되므로, 비워 두면 이 파일의 모든 시험이
    // 그 위에서 돌게 된다 — 실제로 Escape 하나를 팁이 가로채 두 시험이 깨졌다.
    // Orca도 개발·E2E에서 같은 자리를 같은 방법으로 막는다
    // (`suppressDevEducationForStore`가 두 목록을 전부 채운다). 이것을 보는
    // 시험은 자기가 이 둘을 비운다.
    tours_seen: ["board", "tasks", "automations"],
    tips_seen: ["palette", "panels", "editor"],
    wall_seen: [],
  };
  window.__TOUR_REFUSALS__ = [];
  window.__STATUS_BAR_WRITES__ = [];
  window.__FLOATING_WORKSPACE_WRITES__ = [];
  window.__USAGE_PERCENTAGE_WRITES__ = [];
  window.__STATUS_BAR_USAGE_MODE_WRITES__ = [];
  window.__TITLEBAR_APP_NAME_WRITES__ = [];
  window.__MENU_BAR_ICON_WRITES__ = [];
  window.__MINIMIZE_TO_TRAY_WRITES__ = [];
  window.__WORKTREE_CARD_LAYOUT_WRITES__ = [];
  window.__GIT_IGNORED_VISIBILITY_WRITES__ = [];
  window.__SOURCE_CONTROL_GROUP_ORDER_WRITES__ = [];
  window.__SOURCE_CONTROL_COMPARE_BASE_WRITES__ = [];
  window.__WORKTREE_COMPARE_BASE_WRITES__ = [];
  window.__LOCAL_BASE_REF_REFRESH_WRITES__ = [];
  window.__LEFT_SIDEBAR_APPEARANCE_WRITES__ = [];
  window.__UI_ZOOM_WRITES__ = [];
  window.__UI_ZOOM_APPLIES__ = [];
  window.__OVERRIDES__ = { ...boot.keybindings };
  /* `scm_tree_rows`의 스텁판. 규칙의 주인은 `zerocode_core::scm_tree`이고,
   * 여기서는 창이 그 답을 어떻게 그리는지 재기 위해 같은 계약만 지킨다 —
   * 디렉터리 먼저·이름순, 파일은 들어온 순서, 자식이 디렉터리 하나뿐인
   * 사슬은 한 행. */
  const scmTreeRows = (area, paths, folded) => {
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
        if (left.dir && right.dir) return left.name.localeCompare(right.name);
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
        rows.push({ type: "file", key: `${area}::${node.path}`, name: node.name,
          path: node.path, depth, at: node.at });
        return;
      }
      const held = under(node);
      const key = `dir::${area}::${node.path}`;
      rows.push({ type: "directory", key, name: node.name, path: node.path, depth,
        file_count: held.length, paths: held });
      if (folded.includes(key)) return;
      node.children.forEach((child) => walk(child, depth + 1));
    };
    roots.map(compact).forEach((root) => walk(root, 0));
    return rows;
  };
  /* 합성 아티팩트 카탈로그 — 백엔드 `artifacts_list`의 답 모양 그대로.
   *
   * 결정적이다: 같은 `spec`이면 같은 행이고, 그래서 천 개짜리 첫 그림을 다섯
   * 라운드 재도 같은 그림을 잰다. 종류는 여섯을 돌아가며 입고, 출처는 워커
   * 넷·작업 넷·워크트리 둘을 섞어 칩과 왕복이 한 픽스처에서 읽힌다. */
  window.__buildArtifacts__ = (spec) => {
    const count = spec.count ?? 0;
    const kinds = ["report", "screenshot", "evidence", "export", "transcript", "other"];
    const rows = [];
    /* 갤러리의 세 종류(t-3233): `spec.gallery`장을 page → document → web 순으로
       앞에 세운다 — 백엔드가 web 행에 url·favicon을, 페이지 행에 세션·프로젝트를
       싣는 모양 그대로. 시각은 오늘·어제·지난달로 흩어 아래 줄의 낱말 셋을 다 본다. */
    const gallery = spec.gallery ?? 0;
    const galleryKinds = ["page", "document", "web"];
    const day = 24 * 60 * 60 * 1000;
    const now = spec.now ?? Date.now();
    const whenOf = (at) => now - [0, day, 40 * day][at % 3];
    for (let at = 0; at < gallery; at += 1) {
      const kind = galleryKinds[at % galleryKinds.length];
      const remote = kind === "web";
      rows.push({
        id: `gal-${at}`,
        kind,
        title: remote ? `claude.ai 아티팩트 ${at}` : `${kind}-${at}.${kind === "page" ? "html" : "md"}`,
        path: remote ? "" : `/tmp/zerocode-window-test/project/${kind}-${at}.${kind === "page" ? "html" : "md"}`,
        bytes: remote ? 0 : 2000 + at,
        created_ms: whenOf(at),
        modified_ms: whenOf(at),
        ...(remote ? { url: `https://claude.ai/code/artifact/00000000-0000-4000-8000-${String(at).padStart(12, "0")}`, favicon: "🧱" } : {}),
        origin: {
          agent: remote ? "claude" : (at % 2 === 0 ? "claude" : "zo"),
          session: `session-${at}`,
          project: "/tmp/zerocode-window-test/project",
        },
        tags: [],
        preview: kind === "document"
          ? { kind: "markdown", text: `# 문서 ${at}\n\n첫 블록의 **활자**가 얼굴이다\n\n둘째 블록` }
          : remote ? { kind: "text", text: `설명 ${at}` } : { kind: "none" },
        source: remote ? "remote" : "agent_page",
      });
    }
    for (let at = 0; at < count; at += 1) {
      const kind = kinds[at % kinds.length];
      const worker = `w-${at % 4}`;
      const preview = kind === "report"
        ? { kind: "markdown", text: `# 보고서 ${at}\n\nlanded artifact ${at}` }
        : kind === "screenshot" ? { kind: "image", w: 1280, h: 800 }
        : kind === "export" ? { kind: "none" }
        : { kind: "text", text: `{"n":${at}}` };
      rows.push({
        id: `art-${at}`,
        kind,
        title: `${kind}-${at}.${kind === "screenshot" ? "png" : kind === "report" ? "md" : "txt"}`,
        path: `/data/artifacts/run/art-${at}/${kind}-${at}`,
        bytes: 1000 + at,
        created_ms: 1_700_000_000_000 + at * 60_000,
        modified_ms: 1_700_000_000_000 + at * 60_000,
        origin: {
          run: "run-1",
          task: `t-${at % 4}`,
          worker,
          pane: `team-1/%${at % 4}`,
          agent: at % 2 === 0 ? "claude" : "codex",
          model: at % 2 === 0 ? "fable" : "gpt-5.6-sol",
          worktree: at % 2 === 0 ? "/tmp/zerocode-window-test" : "/tmp/zerocode-window-test/wt-b",
        },
        tags: [],
        preview,
        source: kind === "report" ? "worker_report" : "evidence",
      });
    }
    return rows;
  };
  /* 합성 볼트를 백엔드의 답 모양 그대로 짓는다.
   *
   * 결정적이다: 같은 `spec`이면 같은 그래프이고, 그래서 성능 측정이 라운드마다
   * 다른 그림을 재지 않는다. 링크는 「가까운 페이지」와 「멀리 한 곳」을 섞어
   * 건다 — 사슬만 걸면 배치가 실을 늘어놓은 그림이 되고, 그것은 진짜 볼트가
   * 그리는 뭉치와 다른 부하다. */
  /* 백엔드의 lint 표(`second_brain_lint::assess`)를 합성 볼트 위에서 같은 정의로
     짓는다: 합성 볼트에는 index.md가 없으므로 모든 페이지가 색인 누락이고, 고아는
     어느 페이지도 가리키지 않는 페이지, 미선언 관계는 같은 쌍에 타입 간선이 없는
     본문 링크다. 시험이 `spec.lint`를 주면 그 표를 그대로 싣는다 —
     fixtures/vault-lint/expected.json이 그 길로 카드에 닿는다. */
  const lintOf = (nodes, edges, spec) => {
    if (spec.lint) return spec.lint;
    const isPage = (at) => nodes[at]?.kind === "page";
    const inbound = new Set();
    const typed = new Set();
    const ghosts = new Map();
    const superseded = new Set();
    let contradictions = 0;
    for (const edge of edges) {
      const kind = edge.kind ?? "mentions";
      if (nodes[edge.to]?.kind === "ghost") {
        const target = nodes[edge.to].id.slice("ghost:".length);
        if (!ghosts.has(target)) ghosts.set(target, []);
        ghosts.get(target).push(nodes[edge.from].id);
        continue;
      }
      if (!isPage(edge.to)) continue;
      if (isPage(edge.from)) inbound.add(edge.to);
      if (kind !== "mentions") {
        typed.add(`${edge.from}>${edge.to}`);
        if (kind === "contradicts") contradictions += 1;
        if (kind === "supersedes") superseded.add(nodes[edge.to].id);
      }
    }
    const undeclared = new Map();
    for (const edge of edges) {
      if ((edge.kind ?? "mentions") !== "mentions" || !isPage(edge.from) || !isPage(edge.to)) continue;
      if (typed.has(`${edge.from}>${edge.to}`)) continue;
      const page = nodes[edge.from].id;
      if (!undeclared.has(page)) undeclared.set(page, []);
      undeclared.get(page).push(nodes[edge.to].id);
    }
    const pages = nodes.map((node, at) => [node, at]).filter(([node]) => node.kind === "page");
    const table = {
      index_gaps: pages.map(([node]) => node.id),
      ghost_links: [...ghosts].sort(([left], [right]) => left.localeCompare(right))
        .map(([target, from]) => ({ target, from: [...new Set(from)].sort() })),
      orphans: pages.filter(([, at]) => !inbound.has(at)).map(([node]) => node.id),
      missing_frontmatter: [],
      undeclared_relations: [...undeclared].sort(([left], [right]) => left.localeCompare(right))
        .map(([page, targets]) => ({ page, targets: [...new Set(targets)].sort() })),
      unlogged_raw: [],
      unsourced_edges: [],
      contradictions,
      superseded: [...superseded].sort(),
      merge_candidates: null,
    };
    table.counts = {
      index_gaps: table.index_gaps.length,
      ghost_links: table.ghost_links.length,
      orphans: table.orphans.length,
      missing_frontmatter: 0,
      undeclared_relations: table.undeclared_relations.length,
      unlogged_raw: 0,
      unsourced_edges: 0,
      contradictions,
      superseded: table.superseded.length,
      merge_candidates: null,
    };
    table.findings = table.counts.index_gaps + table.counts.ghost_links + table.counts.orphans
      + table.counts.missing_frontmatter + table.counts.undeclared_relations
      + table.counts.unlogged_raw + table.counts.unsourced_edges;
    return table;
  };
  window.__buildVaultGraph__ = (args, spec) => {
    const pages = spec.pages ?? 0;
    const perPage = spec.linksPer ?? 2;
    const ghosts = spec.ghosts ?? 0;
    const tags = spec.tags ?? ["core", "reading", "tools"];
    const nodes = [];
    for (let at = 0; at < pages; at += 1) {
      nodes.push({
        id: `wiki/Page-${String(at).padStart(4, "0")}.md`,
        title: spec.titles?.[at] ?? `개념 ${at}`,
        tags: tags.length === 0 ? [] : [tags[at % tags.length]],
        kind: "page",
        /* 수정 시각은 옛날이 기본이고, 시험이 `spec.modified[at]`로 한 장씩 「방금」
           으로 옮긴다(활동 렌즈·펄스가 그 차이를 읽는다). */
        modified_ms: spec.modified?.[at] ?? 1700000000000 + at,
        out_links: 0,
        in_links: 0,
        source: args.sources ? `raw/source-${at % 5}.md` : null,
      });
    }
    const ghostFrom = nodes.length;
    for (let at = 0; at < ghosts; at += 1) {
      nodes.push({
        id: `ghost:아직 없는 ${at}`,
        title: `아직 없는 ${at}`,
        tags: [],
        kind: "ghost",
        modified_ms: 0,
        out_links: 0,
        in_links: 0,
        source: null,
      });
    }
    const sourceFrom = nodes.length;
    if (args.sources) {
      for (let at = 0; at < Math.min(5, pages); at += 1) {
        nodes.push({
          id: `raw/source-${at}.md`,
          title: `source-${at}.md`,
          tags: [],
          kind: "source",
          modified_ms: 0,
          out_links: 0,
          in_links: 0,
          source: `raw/source-${at}.md`,
        });
      }
    }
    const seen = new Set();
    const edges = [];
    const join = (from, to, kind = "mentions") => {
      if (from === to || from >= nodes.length || to >= nodes.length) return;
      const key = `${from}>${to}`;
      if (seen.has(key)) return;
      seen.add(key);
      /* 낡은 답을 흉내 내는 갈래. `kind`도 `graph.kinds`도 없던 시절의 페이로드를
         창이 아직 그리는지는 픽스처가 그 시절을 지을 수 있어야만 재어진다. */
      /* 근거(t-5966)는 백엔드의 세 길 그대로: 본문 링크는 inferred, 키(원본 `source:` 포함)는
         declared. */
      const provenance = kind !== "mentions" || nodes[to]?.kind === "source" ? "declared" : "inferred";
      edges.push(spec.untyped ? { from, to } : { from, to, kind, provenance });
    };
    /* 고아는 양쪽이 다 비어야 고아다: 다섯 장에 한 장은 아무것도 걸지 않고,
       아무도 그것을 가리키지 않는다 — 나가는 선만 없애면 들어오는 선이 남아
       「고아만」 필터가 아무것도 남기지 않는다. */
    const lonely = (at) => at % 5 === 4;
    const nearest = (at, step) => {
      for (let ahead = 1; ahead <= pages; ahead += 1) {
        const seat = (at + ahead + step * 3) % Math.max(1, pages);
        if (!lonely(seat)) return seat;
      }
      return at;
    };
    /* 「멀리 한 곳」은 프론트매터가 이름을 준 관계로 건다. 대부분은 본문의
       `[[위키링크]]`이고 — 진짜 볼트가 그렇다 — 타입 관계는 소수여야 「타입
       관계만」이 실제로 좁히는지가 재어진다. 종류를 돌려 쓰는 것은 한 종류만
       나오는 픽스처가 `kind-<name>` 옷을 한 벌밖에 시험하지 못하기 때문이다. */
    const typedKinds = ["related", "implements", "depends_on", "supersedes", "contradicts"];
    for (let at = 0; at < pages; at += 1) {
      if (lonely(at)) continue;
      for (let step = 0; step < perPage; step += 1) join(at, nearest(at, step));
      if (at % 7 === 0) {
        const far = (at * 37 + 11) % Math.max(1, pages);
        if (!lonely(far)) join(at, far, typedKinds[(at / 7) % typedKinds.length]);
      }
      if (args.sources && at < Math.min(5, pages)) join(at, sourceFrom + at);
    }
    /* 유령은 누군가 그것을 가리켜서 생긴 것이다. 스캐너가 만들 수 없는 「아무도
       안 가리키는 유령」을 픽스처가 만들면, 그 픽스처로 통과한 필터는 진짜
       볼트에서 다르게 움직인다. */
    for (let at = 0; at < ghosts; at += 1) {
      let from = at % Math.max(1, pages);
      for (let ahead = 0; ahead < pages && lonely(from); ahead += 1) {
        from = (from + 1) % Math.max(1, pages);
      }
      join(from, ghostFrom + at);
    }
    for (const edge of edges) {
      nodes[edge.from].out_links += 1;
      nodes[edge.to].in_links += 1;
    }
    const counted = new Map();
    for (const node of nodes) {
      for (const tag of node.tags) counted.set(tag, (counted.get(tag) ?? 0) + 1);
    }
    /* 백엔드가 세어 주는 종류별 수. 많이 쓰인 순, 같으면 계약의 순서다 —
       「타입 관계만」 칩이 설지 말지를 이 목록 하나로 답한다. */
    const kindOrder = ["mentions", ...typedKinds];
    const kindCount = new Map();
    for (const edge of edges) {
      const kind = edge.kind ?? "mentions";
      kindCount.set(kind, (kindCount.get(kind) ?? 0) + 1);
    }
    const kinds = [...kindCount].map(([kind, count]) => ({ kind, count }))
      .sort((left, right) => right.count - left.count
        || kindOrder.indexOf(left.kind) - kindOrder.indexOf(right.kind));
    const lint = lintOf(nodes, edges, spec);
    return {
      vault: args.path ?? "/vault",
      scanned_ms: spec.scannedMs ?? 7,
      empty: pages === 0,
      graph: {
        nodes,
        edges,
        tags: [...counted].map(([tag, count]) => ({ tag, count }))
          .sort((left, right) => right.count - left.count || left.tag.localeCompare(right.tag)),
        // 낡은 답에는 이 열쇠가 아예 없다.
        ...(spec.untyped ? {} : { kinds }),
        /* 근거별 수(t-5966): 세 길 전부, enum 순서로, 0도 한 줄. */
        ...(spec.untyped ? {} : { provenances: ["measured", "declared", "inferred"].map((provenance) => ({
          provenance, count: edges.filter((edge) => edge.provenance === provenance).length })) }),
        pages,
        ghosts,
        orphans: lint.orphans.length,
        lint,
        capped: false,
        truncated: 0,
        reparsed: pages,
      },
      /* 라이브 층(t-2931). 시험이 `spec.live`로 준 것만 싣는다 — 이 열쇠가 없던
         옛 답을 창이 여전히 그리는지는 그 열쇠를 **안 주는** 픽스처로만 재어진다. */
      ...(spec.live ? { live: spec.live } : {}),
    };
  };

  const groupedBoardColumns = (args) => {
    window.__ASKED_BOARD__ = args;
    return (window.__COLUMNS__ ?? []).map((column) => ({
      ...column,
      cards: (column.cards ?? []).map((card) => ({
        lineage: { depth: 0, is_first_sibling: true, is_last_sibling: true, child_count: 0 },
        ...card,
      })),
    }));
  };
  const answers = {
    boot_report: () => ({ ...boot, keybindings: window.__OVERRIDES__ }),
    list_system_fonts: () => ["Fira Code", "JetBrains Mono", "Menlo"],
    terminal_keyboard_layout: () => ({ category: "us" }),
    terminal_windows_status: () => ({
      supported: true,
      pwsh_available: true,
      git_bash_available: true,
    }),
    list_dir: () => [],
    // 업데이트(t-3191): 이 픽스처의 빌드는 피드를 묻지 않은 채 서 있고, 이력은
    // 아직 공개된 버전이 없다. 케이스가 제 답을 깔아 쓴다.
    update_check: () => ({
      phase: { phase: "idle" },
      next: null,
      running: { version: "0.1.0", commit: "9b576e43aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", ui_digest: "ui" },
      prefs: { policy: "ask", channel: "stable", last_checked: null, skipped_version: null },
    }),
    update_download: () => ({
      phase: { phase: "idle" },
      next: null,
      running: { version: "0.1.0", commit: "9b576e43aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", ui_digest: "ui" },
      prefs: { policy: "ask", channel: "stable", last_checked: null, skipped_version: null },
    }),
    update_install: () => ({
      phase: { phase: "idle" },
      next: null,
      running: { version: "0.1.0", commit: "9b576e43aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", ui_digest: "ui" },
      prefs: { policy: "ask", channel: "stable", last_checked: null, skipped_version: null },
    }),
    update_history: () => ({ releases: [], fetched_at: null, source: "none", failure: null }),
    // 창 안 경로 브라우저(t-2982)의 두 물음. 시작 화면은 최근 프로젝트 하나와
    // 홈, 볼륨 하나; 어느 폴더든 빈 폴더로 답한다 — 시험이 제 목록을 깔아 쓴다.
    browse_places: () => ({
      home: "/Users/tester",
      places: [
        { kind: "recent", name: "zerocode", path: boot.project_root },
        { kind: "home", name: "tester", path: "/Users/tester" },
        { kind: "volume", name: "Macintosh HD", path: "/Volumes/Macintosh HD" },
      ],
    }),
    browse_dir: ({ path }) => ({
      path,
      parent: path === "/" ? null : path.replace(/\/[^/]+$/, "") || "/",
      entries: [],
      total: 0,
      truncated: false,
    }),
    // 하나는 있다. 실제 백엔드의 `project_catalog`는 저장된 목록에 **이 창이
    // 열려 있는 프로젝트를 언제나 덧붙여** 답하므로(main.rs), 빈 목록은 이
    // 하네스만 만들어 낼 수 있는 상태였다 — 그리고 프로젝트 0개는 이제 다른
    // 화면(랜딩)을 뜻하므로, 목 하나가 모든 시험을 그 화면 뒤에서 돌리게 된다.
    project_catalog: () => [
      {
        name: "zerocode",
        path: boot.project_root,
        // 어느 저장소인가. 실제 백엔드는 프로젝트마다 이것을 실어 보내고
        // (`ProjectEntry.slug`), 원격이 없는 저장소에서만 `null`이다.
        slug: "cjy5507/zerocode-ide",
        worktrees: [
          {
            path: boot.project_root,
            branch: "main",
            is_main: true,
            active: true,
            is_folder: false,
            ownership: "zerocode-managed",
            external_hidden: false,
          },
        ],
      // 이 창이 만들지 않은 워크트리에 대한 답. 실제 백엔드는 프로젝트마다 이것을
      // 언제나 실어 보내므로(`ProjectEntry.external`) 여기서도 그렇게 한다 —
      // 빠뜨린 목은 카드도 카운트도 없는, 백엔드가 만들 수 없는 상태를 만든다.
        external: {
          visibility: "show",
          legacy: true,
          authoritative: true,
          shown: 0,
          hidden: [],
          prompt: false,
          inbox: [],
        },
      },
    ],
    // 첫 실행. 기본은 **이미 닫힌** 상태다 — 마법사가 뜨는 조건은 딱 하나이고,
    // 그 하나를 켠 시험만 마법사를 본다.
    onboarding_state: () => ({
      flow_version: 1,
      closed_at: 1,
      outcome: "completed",
      last_completed_step: 4,
      checklist: { chose_agent: false, dismissed: false },
    }),
    github_status: () => {
      const standing = window.__GH__ ?? "missing";
      // 로그인은 섰지만 스코프가 모자란 keyring 계정 — refresh 한 번의 얼굴.
      if (standing === "scope-gap") {
        return {
          availability: "available",
          connected: true,
          credential_protection: "external_cli",
          repo_host: "github.com",
          repo_standing: "connected",
          active_account_id: "gh-narrow",
          accounts: [{
            id: "gh-narrow",
            host: "github.com",
            login: "octocat",
            active: true,
            standing: "connected",
            selectable: true,
            disconnectable: true,
            credential_source: "gh",
            env_credential: null,
            scope_gaps: ["repo"],
          }],
        };
      }
      // 환경 변수 자격증명의 세 사정: 가려서 못 서는 계정, 키링이 뒤에서
      // 기다리는 같은 사정, 그리고 환경 출신이지만 멀쩡히 선 계정.
      if (standing === "env-broken" || standing === "env-covered" || standing === "env-fine") {
        const fine = standing === "env-fine";
        const envAccount = {
          id: "gh-env",
          host: "github.com",
          login: "ci-bot",
          active: true,
          standing: fine ? "connected" : "auth_error",
          selectable: false,
          disconnectable: false,
          credential_source: "environment",
          env_credential: "GH_TOKEN",
          // env가 스코프 부족까지 겹친 상태 — 사다리는 env를 먼저 말해야 한다.
          scope_gaps: fine ? [] : ["repo"],
        };
        const spare = standing === "env-covered" ? [{
          id: "gh-keyring",
          host: "github.com",
          login: "octocat",
          active: false,
          standing: "connected",
          selectable: true,
          disconnectable: true,
          credential_source: "gh",
          env_credential: null,
          scope_gaps: [],
        }] : [];
        return {
          availability: "available",
          connected: fine,
          credential_protection: "external_cli",
          repo_host: "github.com",
          repo_standing: fine ? "connected" : "auth_error",
          active_account_id: "gh-env",
          accounts: [envAccount, ...spare],
        };
      }
      const connected = standing === "connected";
      return {
        availability: standing === "missing" ? "missing" : "available",
        connected,
        credential_protection: "external_cli",
        repo_host: "github.com",
        repo_standing: connected ? "connected" : "auth_error",
        active_account_id: connected ? "gh-test" : null,
        accounts: connected ? [{
          id: "gh-test",
          host: "github.com",
          login: "octocat",
          active: true,
          standing: "connected",
          selectable: true,
          disconnectable: true,
          credential_source: "gh",
          env_credential: null,
          scope_gaps: [],
        }] : [],
      };
    },
    // 켜기만 한다. 모르는 이름은 백엔드가 거절하므로 여기서도 거절한다 —
    // 아무 이름이나 받는 목은 오타를 통과시킨다.
    mark_onboarding: (args) => {
      const held = window.__ONB__.checklist;
      if (!(args.mark in held)) throw new Error(`refused: ${args.mark}`);
      held[args.mark] = true;
      return window.__ONB__;
    },
    set_guide_dismissed: (args) => {
      window.__ONB__.guide_dismissed = args.dismissed === true;
      return window.__ONB__;
    },
    mark_first_run_seen: (args) => {
      const list = { tour: "tours_seen", tip: "tips_seen", wall: "wall_seen" }[args.kind];
      if (!list) throw new Error(`refused: ${args.kind}`);
      if (!window.__ONB__[list].includes(args.id)) window.__ONB__[list].push(args.id);
      return window.__ONB__;
    },
    // 완료는 저장되지 않는다 — 실제 백엔드와 같이 넘겨받은 사실에서 매번
    // 파생한다. 저장했다면 이 목이 제품이 만들 수 없는 상태를 만들게 된다.
    setup_guide: (args) => {
      const check = window.__ONB__.checklist;
      const steps = [
        { id: "default-agent", section: "setup", done: window.__CHOSE_AGENT__ === true },
        { id: "notifications", section: "setup", done: check.notified },
        { id: "github", section: "setup", done: args.github === true },
        { id: "two-projects", section: "setup", done: args.projects >= 2 },
        { id: "two-worktrees", section: "milestone", done: args.sideWorktrees >= 1 },
        { id: "parallel-agents", section: "milestone", done: check.ran_second_agent },
        { id: "review-diff", section: "milestone", done: check.reviewed_diff },
        { id: "open-pr", section: "milestone", done: check.opened_pr },
      ];
      const complete = steps.every((one) => one.done);
      return {
        steps,
        complete,
        dismissed: window.__ONB__.guide_dismissed,
        entry: args.ready === true && !complete && !window.__ONB__.guide_dismissed,
      };
    },
    tour_decision: (args) => {
      const ask = args.ask;
      const why =
        !["board", "tasks", "automations"].includes(ask.id)
          ? "Unknown"
          : !ask.here
            ? "NotHere"
            : !ask.ready
              ? "NotReady"
              : window.__ONB__.tours_auto !== true
                ? "AutoDisabled"
                : window.__ONB__.closed_at === null
                  ? "Onboarding"
                  : ask.modalOpen
                    ? "Modal"
                    : ask.activeTour
                      ? "ActiveTour"
                      : ask.spentThisSession
                        ? "SessionSpent"
                        : window.__ONB__.tours_seen.includes(ask.id)
                          ? "Seen"
                          : !ask.hasTarget
                            ? "NoTarget"
                            : null;
      if (why !== null) window.__TOUR_REFUSALS__.push(`${ask.id}:${why}`);
      return why;
    },
    tip_verdict: (args) => {
      if (window.__ONB__.closed_at === null) {
        return { verdict: "suppress", id: null, settle: [] };
      }
      if (args.sealed || args.spentThisOpen || !args.ready || args.modalOpen) {
        return { verdict: "skip", id: null, settle: [] };
      }
      const check = window.__ONB__.checklist;
      const tips = [
        { id: "palette", met: check.used_palette },
        { id: "panels", met: check.shaped_sidebar },
        { id: "editor", met: check.opened_file },
      ].filter((one) => !window.__ONB__.tips_seen.includes(one.id));
      const settle = tips.filter((one) => one.met).map((one) => one.id);
      const show = tips.find((one) => !one.met);
      return show
        ? { verdict: "show", id: show.id, settle }
        : { verdict: "skip", id: null, settle };
    },
    notification_probe: () => window.__NOTIFY_OK__ !== false,
    save_onboarding_step: (args) => {
      window.__ONB_STEPS__.push(args);
      return {
        flow_version: 1,
        closed_at: null,
        outcome: null,
        last_completed_step: args.step,
        checklist: { chose_agent: args.choseAgent === true, dismissed: false },
      };
    },
    close_onboarding: (args) => {
      window.__ONB_CLOSED__ = args.outcome;
      return {
        flow_version: 1,
        closed_at: 2,
        outcome: args.outcome,
        last_completed_step: args.outcome === "completed" ? 4 : -1,
        checklist: { chose_agent: false, dismissed: args.outcome === "dismissed" },
      };
    },
    reopen_onboarding: () => ({
      flow_version: 1,
      closed_at: null,
      outcome: null,
      last_completed_step: -1,
      checklist: { chose_agent: false, dismissed: false },
    }),
    scm_status: () => ({ changed: [], ignored: [] }),
    scm_tree_rows: (args) => scmTreeRows(args.area, args.paths, args.folded ?? []),
    // 토큰 원장 둘 — 스캔 전 상태가 이 스텁의 기본값이다(게이지의 답과
    // 모양이 다르다: 원장은 report·scanning·scanned_at을 준다).
    stats_summary: () => ({ first_event_at_ms: null, agents_spawned: 0, agent_time_ms: 0, prs_created: 0 }),
    claude_usage_stats: () => ({ enabled: true, report: null, scanning: false, scanned_at: null, files: 0, capped: false }),
    codex_usage_stats: () => ({ enabled: true, report: null, scanning: false, scanned_at: null, files: 0, capped: false }),
    opencode_usage_stats: () => ({ enabled: true, report: null, scanning: false, scanned_at: null, files: 0, capped: false }),
    git_history: () => ({ rows: [], has_more: false, current: null, remote: null }),
    list_agents: () => [
      ...(window.__GEMINI_VENDOR_FIXTURE__ ? [{ id: "gemini", name: "Gemini", favicon_domain: "geminicli.com",
        homepage_url: "https://geminicli.com/docs/", installed: true,
        found_as: "gemini", unsupported_here: false, missing_requirement: null,
        takes_a_paste: true, ready: "quiet" }] : []),
      { id: "claude", name: "Claude", favicon_domain: "claude.ai",
        homepage_url: "https://docs.anthropic.com/claude/docs/claude-code", installed: true,
        found_as: "claude", unsupported_here: false, missing_requirement: null,
        takes_a_paste: false, ready: "quiet", glyph: "✻", busy_word: "Pondering…", models_provider: "claude", model_command: "/model", model_command_takes_id: true, permission_road: "shift-tab",
        // The catalog's own rows (core `AGENT_VOICES`): Shift+Tab's order and
        // Claude Code's words for each mode (t-6323 A3).
        permission_modes: [
          { mode: "dontAsk", reach: "ask", label: "Don't ask", cycles: false, aliases: [] },
          { mode: "default", reach: "ask", label: "Manual", cycles: true, aliases: ["manual"] },
          { mode: "acceptEdits", reach: "edits", label: "Edit automatically", cycles: true, aliases: [] },
          { mode: "plan", reach: "plan", label: "Plan", cycles: true, aliases: [] },
          { mode: "auto", reach: "bypass", label: "Auto", cycles: true, aliases: [] },
          { mode: "bypassPermissions", reach: "bypass", label: "Bypass permissions", cycles: true, aliases: [] },
        ],
        wire: "claude-stream", wire_resumes: true, compact_command: "/compact", read_offset_base: 1, interrupt_key: "Escape",
        spinner_verbs: boot.claude_spinner_verbs, todo_tool: "TodoWrite" },
      { id: "codex", name: "Codex", favicon_domain: "openai.com",
        homepage_url: "https://github.com/openai/codex", installed: true,
        found_as: "codex", unsupported_here: false, missing_requirement: null,
        takes_a_paste: true, ready: "composer-prompt", glyph: "◎", busy_word: "Thinking…", models_provider: "openai", model_command: "/model", model_command_takes_id: false, permission_road: "/permissions", compact_command: "/compact" },
      { id: "opencode", name: "OpenCode", favicon_domain: "opencode.ai",
        homepage_url: "https://opencode.ai/docs/cli/", installed: false,
        found_as: null, unsupported_here: false, missing_requirement: null,
        takes_a_paste: true, ready: "cursor-shown" },
      { id: "goose", name: "Goose", favicon_domain: "goose-docs.ai",
        homepage_url: "https://block.github.io/goose/docs/quickstart/", installed: false,
        found_as: null, unsupported_here: true, missing_requirement: null,
        takes_a_paste: true, ready: "quiet" },
    ],
    // 프로젝트 추가 다이얼로그가 읽는 둘. 실제 백엔드에서 이름은
    // `zerocode_core::clone::clone_dir_name`이 정하고 기본 부모는 지금 선
    // 저장소의 부모다 — 규칙 자체는 Rust에서 표로 시험되고, 여기서는 창이 그
    // 답을 어떻게 그리는지만 본다.
    default_project_parent: () => "/tmp/zerocode-window-test-parent",
    clone_target_name: (args) => {
      const url = String(args.url ?? "")
        .trim()
        .replace(/[?#].*$/, "")
        .replace(/\/+$/, "");
      const rest = url.includes("://") ? url.slice(url.indexOf("://") + 3) : url;
      const tail = rest.split(/[/:]/).pop();
      // 마디가 하나뿐이면 호스트일 뿐이다 — Rust와 같은 거절.
      if (!tail || tail === rest) return null;
      const name = tail.endsWith(".git") ? tail.slice(0, -4) : tail;
      return name && ![".", ".."].includes(name) && !name.startsWith("-") ? name : null;
    },
    list_diff_notes: () => window.__NOTES__ ?? [],
    save_diff_note: (args) => {
      window.__NOTES__ = window.__NOTES__ ?? [];
      const at = window.__NOTES__.findIndex((one) => one.id === args.note.id);
      if (at >= 0) window.__NOTES__[at] = args.note;
      else window.__NOTES__.push(args.note);
      return null;
    },
    delete_diff_note: (args) => {
      window.__NOTES__ = (window.__NOTES__ ?? []).filter((one) => one.id !== args.id);
      return null;
    },
    clear_delivered_diff_notes: (args) => {
      window.__NOTES__ = (window.__NOTES__ ?? []).filter(
        (held) => !args.delivered.some((sent) => sent.id === held.id && sent.body === held.body),
      );
      return null;
    },
    agent_terms: () => window.__AGENT_TERMS__ ?? [],
    // The screen of one shell, whole. The window reads it outside the watch
    // (the skills page), and the pull below pays a newly declared shell's
    // floor with the same picture — a real backend's debt IS this snapshot.
    // `null` is the backend saying it holds no such shell, which is the whole
    // basis of the "no live terminal" line.
    term_has_running_process: () => false,
    term_snapshot: (args) =>
      window.__DEAD_TERMS__?.includes(args.term)
        ? null
        : {
            rows: [
              {
                index: 0,
                cells: [...(window.__SNAPSHOT_TEXT__ ?? "restored")].map((ch) => ({ ch })),
              },
            ],
            scrolled_lines: 0,
            cursor: [0, 0],
            title: "",
            alt_screen: false,
            // The real backend's snapshot wears the grid the shell was told
            // (`GridDelta::size` comes off the emulator). A fixture that wears
            // any other size models a resize this window lost, and the window
            // now notices and re-tells — a round trip no budget test asked
            // for. The fallback pair is for shells nothing ever told.
            size: (() => {
              const told = typeof termGrids !== "undefined" && termGrids.get(args.term);
              return told ? [told.rows, told.cols] : [24, 80];
            })(),
            full: true,
            cursor_visible: true,
            // `MouseTracking`은 문자열 열거형이다(`grid.rs`: off/click/drag/
            // motion). 여기 `false`가 들어 있던 동안 이 목은 백엔드가 만들 수
            // 없는 프레임을 만들고 있었고, 스냅샷이 화면을 깨우는 길이 되자
            // 곧바로 "아무도 안 물었는데 포인터를 보고하는 터미널"이 되었다.
            mouse_tracking: "off",
            mouse_sgr: false,
            focus_reporting: false,
            bell: false,
            view_offset: 0,
            scrollback_len: 0,
          },
    // The board's two halves. `pane_agents` is what the backend knows about
    // running agents; `board_columns` is the Rust grouping, whose RULES are
    // tested in Rust — a JS reimplementation here would be a second copy of the
    // thing the split exists to avoid. So the fixture below is real output of
    // `zerocode_core::board::columns`, and what these tests assert is the render.
    pane_agents: () => window.__PANES__ ?? [],
    // And the third source: every worker the ORCHESTRATION LEDGER still holds,
    // seat or no seat. `pane_agents` can only answer for terminals this window
    // has; a worker whose pane it never had — or has lost — exists nowhere else.
    ledger_agents: () => window.__LEDGER__ ?? [],
    /* 아티팩트 카탈로그 (t-2720). 목록은 사람이 연 화면의 값이라 폴러가 아니다;
       필터는 창 안에서 고르므로 `artifacts_list`는 문을 열 때 한 번이다. */
    artifacts_list: (args) => {
      window.__ARTIFACT_ASKS__ = (window.__ARTIFACT_ASKS__ ?? 0) + 1;
      window.__ARTIFACT_ASKED__ = args;
      const rows = window.__buildArtifacts__(window.__ARTIFACTS__ ?? { count: 0 });
      window.__ARTIFACTS_ANSWERED_AT__ = performance.now();
      // 썸네일 표는 백엔드의 것(`Limits`): 창은 이 수로 큐를 잰다.
      return { rows, total: rows.length, truncated: false, thumb: { width: 320, height: 240, queue_max: window.__THUMB_QUEUE_MAX__ ?? 24 } };
    },
    artifact_preview: (args) => {
      window.__PREVIEW_ASKS__ = (window.__PREVIEW_ASKS__ ?? []);
      window.__PREVIEW_ASKS__.push(args.id);
      const at = Number(String(args.id).replace("art-", ""));
      const kinds = ["report", "screenshot", "evidence", "export", "transcript", "other"];
      const kind = kinds[at % kinds.length];
      if (kind === "report") {
        return { kind: "markdown", text: `# 보고서 ${at}\n\n본문 **굵게** landed`, bytes: 40, truncated: false };
      }
      if (kind === "screenshot") {
        return {
          kind: "image",
          data_url: "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7",
          bytes: 43,
          truncated: false,
        };
      }
      if (kind === "export") return { kind: "none", bytes: 12, truncated: false };
      return { kind: "text", text: `{"n":${at}}\n`, bytes: 8, truncated: false };
    },
    artifact_counts: () => {
      const rows = window.__buildArtifacts__(window.__ARTIFACTS__ ?? { count: 0 });
      const counts = {
        total: rows.length, by_worker: {}, by_task: {}, by_run: {}, by_worktree: {}, by_automation: {},
        report_by_worker: {},
      };
      for (const row of rows) {
        const bump = (map, key) => { if (key) map[key] = (map[key] ?? 0) + 1; };
        bump(counts.by_worker, row.origin.worker);
        bump(counts.by_task, row.origin.task);
        bump(counts.by_run, row.origin.run);
        bump(counts.by_worktree, row.origin.worktree);
        if (row.kind === "report") counts.report_by_worker[row.origin.worker] = row.id;
      }
      return counts;
    },
    /* 렌더 썸네일(t-3233 §3): 청한 id를 세고, 한 번에 몇이 떠 있었는지의 최댓값을
       적는다 — 창의 큐가 동시 1인지는 이 수가 말한다. `gal-3`(page)은 실패로 답해
       글리프로 물러나는 길을 본다. */
    artifact_thumbnail: async (args) => {
      window.__THUMB_ASKS__ = window.__THUMB_ASKS__ ?? [];
      window.__THUMB_ASKS__.push(args.id);
      window.__THUMB_INFLIGHT__ = (window.__THUMB_INFLIGHT__ ?? 0) + 1;
      window.__THUMB_INFLIGHT_MAX__ = Math.max(window.__THUMB_INFLIGHT_MAX__ ?? 0, window.__THUMB_INFLIGHT__);
      await new Promise((done) => setTimeout(done, 8));
      window.__THUMB_INFLIGHT__ -= 1;
      if (args.id === "gal-3") return { data_url: null, cached: false };
      return {
        data_url: "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7",
        cached: false,
      };
    },
    artifact_versions: (args) => ((window.__VERSION_ASKS__ = (window.__VERSION_ASKS__ ?? 0) + 1), [
      { n: 1, path: `/data/artifacts/versions/${args.id}/1/page.html`, bytes: 10, modified_ms: 1, sha256: "1".repeat(64) },
      { n: 2, path: `/data/artifacts/versions/${args.id}/2/page.html`, bytes: 12, modified_ms: 2, sha256: "2".repeat(64) },
    ]),
    artifact_import_transcripts: () => ((window.__IMPORT_ASKS__ = (window.__IMPORT_ASKS__ ?? 0) + 1),
      { files: 0, bytes: 0, remote: 0, pages: 0, truncated: false }),
    artifact_open: (args) => ((window.__ARTIFACT_OPENED__ = args), null),
    artifact_reveal: (args) => ((window.__ARTIFACT_REVEALED__ = args), null),
    artifact_copy_path: (args) => ((window.__ARTIFACT_COPIED__ = args), `/data/artifacts/${args.id}`),
    artifact_delete: (args) => ((window.__ARTIFACT_DELETED__ = args), true),
    artifact_register: (args) => ((window.__ARTIFACT_REGISTERED__ = args), null),
    // The helpers running inside those agents, for a window that opened after
    // they started. Seeded once at boot and kept true by `hook:subagent` — the
    // stub answers nothing by default, which is the state every other test
    // here runs in.
    pane_subagents: () => window.__SUBAGENTS__ ?? [],
    // And what those agents have been DOING — the ring the backend keeps per
    // card, for a window that opened after the work started. Empty by default,
    // which is the state every other test here runs in.
    pane_activities: () => window.__ACTIVITIES__ ?? [],
    board_columns: (args) => {
      // `zerocode_core::board::columns` stamps a lineage on every card it
      // returns — a root, unless another card started this one. The fixtures
      // below spell it out only where the TREE is what is being tested, so
      // this fills in the root the grouping would have written. Standing in
      // for Rust is this stub's whole job; a default in the window itself
      // would be a rule with no producer behind it.
      return groupedBoardColumns(args);
    },
    board_snapshot: (args) => {
      const columns = window.__ANSWER__.board_columns
        ? window.__ANSWER__.board_columns(args)
        : groupedBoardColumns(args);
      return {
        columns,
        attention_count: columns.find((column) => column.bucket === "attention")?.cards.length ?? 0,
        total_count: (args.cards ?? []).length,
        overlays: window.__OVERLAYS__ ?? {
          latest: null, mail: [], dependencies: [], merge: [],
        },
      };
    },
    // The review each checkout is on. `gh`, its four-word mapping and its
    // one-minute cache are all Rust's and tested there; what this side owes is
    // the join — the answer is keyed by checkout path and the cards are not.
    github_review_states: (args) => {
      window.__ASKED_REVIEWS__ = args;
      return window.__REVIEWS__ ?? [];
    },
    // Answering from a card. The keys and their pacing are Rust's
    // (`zerocode_core::ask`, tested there); what this side owes is the picks,
    // verbatim, through this one door.
    answer_ask: (args) => {
      window.__ANSWERED__ = JSON.parse(JSON.stringify(args));
      return null;
    },
    // A permission request's one-key answer — which byte means Allow is the
    // backend's; this side only says which way the person pressed.
    answer_approval: (args) => {
      window.__APPROVED__ = JSON.parse(JSON.stringify(args));
      return null;
    },
    // The vault, the same way: the filtering, sorting and grouping are Rust's and
    // tested there, so the fixture below is real output of
    // `zerocode_core::vault::view` and what is asserted here is the render.
    vault_sessions: (args) => {
      window.__ASKED_VAULT__ = args;
      if (window.__VAULT_LIMIT_ROWS__) {
        const rows = window.__VAULT_LIMIT_ROWS__;
        const shown = Math.min(args.query.limit || rows.length, rows.length);
        return { ...window.__VAULT__, shown, truncated: shown < rows.length,
          groups: [{ key: "fixture", label: "Vault 검증", sessions: rows.slice(0, shown) }] };
      }
      return window.__VAULT__ ?? { groups: [], issues: [], shown: 0, total: 0, truncated: false };
    },
    set_vault_session_limit: (args) => ({ "vault.sessionLimit": args.limit }),
    resume_vault_session: (args) => {
      window.__RESUMED_VAULT__ = args;
      return 91;
    },
    reveal_vault_session: (args) => {
      window.__REVEALED_VAULT__ = args;
      return null;
    },
    // What each agent will be launched with. The fixture mirrors the measured
    // defaults for the two installed agents so the pane can be checked against
    // what a real machine would show.
    project_scripts: () =>
      window.__PROJECT__ ?? {
        file: "/tmp/zerocode/zerocode.yaml", exists: false, unreadable: false,
        runs_setup: true, source: "shared-only", setup_run_policy: "run-by-default",
      },
    set_project_script_policy: (args) => {
      window.__SAVED_PROJECT__ = args;
      return null;
    },
    agent_launch_plans: () =>
      JSON.parse(JSON.stringify(window.__LAUNCH__ ?? defaultAgentLaunchPlans)),
    set_agent_permission_mode: (args) => {
      window.__SAVED_PERMISSION_MODE__ = args.mode;
      const measured = new Map(defaultAgentLaunchPlans.map((row) => [row.agent, row]));
      window.__LAUNCH__ = (window.__LAUNCH__ ?? defaultAgentLaunchPlans).map((row) => {
        if (!row.has_switch || row.permission === "mixed") return row;
        const defaults = measured.get(row.agent);
        if (args.mode === "yolo") return { ...defaults, env: [...defaults.env] };
        return {
          ...row,
          args: defaults.args ? "" : row.args,
          env: defaults.env.length > 0 ? [] : row.env,
          permission: "asks",
          is_default: false,
        };
      });
      return JSON.parse(JSON.stringify(window.__LAUNCH__));
    },
    // Three doors, three records — the point of the split is that each writes
    // only its own half, and a stub that folded them into one could not show it.
    save_agent_launch: (args) => {
      window.__SAVED_LAUNCH__ = args;
      return null;
    },
    save_agent_launch_env: (args) => {
      window.__SAVED_LAUNCH_ENV__ = args;
      return null;
    },
    reset_agent_launch: (args) => {
      window.__RESET_LAUNCH__ = args;
      return null;
    },
    // The agent hook bridge's state. A fixture rather than real installs: the
    // real ones write into `~/.claude/settings.json`, which a test suite has no
    // business touching — what is checked here is the section and the badge.
    skills_list: () => ({ families: [], sources: [], agents: [], required: [], policy: { terminal_rows: 24, terminal_cols: 96 }, evidence_scope: "hooks-only" }),
    skills_rescan: () => window.__ANSWER__.skills_list(),
    hooks_report: () =>
      window.__HOOKS__ ?? { enabled: true, listening: true, agents: [] },
    set_hooks_enabled: (args) => {
      window.__HOOKS__ = { ...window.__HOOKS__, enabled: args.enabled };
      return window.__HOOKS__;
    },
    install_hooks: () => {
      window.__HOOK_INSTALLS__ = (window.__HOOK_INSTALLS__ ?? 0) + 1;
      return window.__HOOKS__ ?? { enabled: true, listening: true, agents: [] };
    },
    // The Claude accounts a machine holds. A fixture rather than a login: the
    // real add runs the CLI's browser flow, which is exactly the part this
    // suite must not have — what is checked here is the list, the choice, and
    // the button's three states.
    claude_accounts: () => window.__ACCOUNTS__ ?? { accounts: [], can_add: true },
    /* 링크가 될 수 있는지 묻는 문. 기본은 「다 있다」 — 이 하네스의 화면들은
     * 존재하지 않는 파일 이름을 예시로 쓰는 곳이 아니고, 없는 것으로 답하면
     * 링크에 대한 모든 단면이 링크의 부재를 재게 된다. 없는 경우는 그것을 재는
     * 단면이 `window.__GONE__` 으로 이름을 대어 만든다. */
    // `window.__SHORT_ANSWER__` 는 Rust 쪽 한 번 물음 상한(`TERM_LINK_PROBE_CAP`)
    // 을 흉내낸다: 답이 물음보다 짧게 온다. 창은 없는 자리를 「없다」로 적어서는
    // 안 된다 — 적으면 있는 파일이 다시 물어보지도 못한 채 링크를 잃는다.
    paths_exist: (args) => {
      const asked = args?.paths ?? [];
      const said = asked.map((path) =>
        !(window.__GONE__ ?? []).some((gone) => path.endsWith(gone)));
      const cap = window.__SHORT_ANSWER__;
      return typeof cap === "number" ? said.slice(0, cap) : said;
    },
    // The oracles behind the two account lists. Answered here rather than left
    // to fall through to `null`, because a row is only marked when the STORE
    // says it has credentials AND the oracle says they are dead — a silent
    // `null` reads as "nobody was asked", which is a different thing.
    verify_codex_accounts: () => window.__CODEX_VERDICTS__ ?? [],
    add_claude_account: () => {
      window.__ADD_CALLS__ = (window.__ADD_CALLS__ ?? 0) + 1;
      return window.__ACCOUNTS__ ?? { accounts: [], can_add: true };
    },
    select_claude_account: (args) => {
      window.__ACCOUNTS__ = { ...window.__ACCOUNTS__, active: args.id };
      return window.__ACCOUNTS__;
    },
    // 「시스템 기본값」: no managed account at all. The report answers with no
    // active id — the backend resolves the selection before it replies, and
    // that state is what the first row means.
    use_system_claude_login: () => {
      window.__ACCOUNTS__ = { ...window.__ACCOUNTS__, active: undefined };
      return window.__ACCOUNTS__;
    },
    remove_claude_account: (args) => {
      const left = (window.__ACCOUNTS__?.accounts ?? []).filter((one) => one.id !== args.id);
      const active = window.__ACCOUNTS__?.active === args.id ? left[0]?.id : window.__ACCOUNTS__?.active;
      window.__ACCOUNTS__ = { ...window.__ACCOUNTS__, accounts: left, active };
      return window.__ACCOUNTS__;
    },
    // Codex holds accounts by its own mechanism. Same reason for a fixture,
    // and one difference that is the point of the surface: `active: null` is a
    // real state here — the machine's own `~/.codex` login — so the mock has to
    // be able to be in it.
    codex_account_list: () => window.__CODEX_ACCOUNTS__ ?? { accounts: [], can_add: true },
    add_codex_account: () => window.__CODEX_ACCOUNTS__ ?? { accounts: [], can_add: true },
    relogin_codex_login: () => window.__CODEX_ACCOUNTS__ ?? { accounts: [], can_add: true },
    logout_codex_login: () => window.__CODEX_ACCOUNTS__ ?? { accounts: [], can_add: true },
    select_codex_account: (args) => {
      window.__CODEX_ACCOUNTS__ = { ...window.__CODEX_ACCOUNTS__, active: args.id ?? undefined };
      return window.__CODEX_ACCOUNTS__;
    },
    remove_codex_account: (args) => {
      const left = (window.__CODEX_ACCOUNTS__?.accounts ?? []).filter((one) => one.id !== args.id);
      window.__CODEX_ACCOUNTS__ = { ...window.__CODEX_ACCOUNTS__, accounts: left, active: undefined };
      return window.__CODEX_ACCOUNTS__;
    },
    // Google is the third card and the only one whose login this window
    // MAKES: two calls with a consent screen between them. So `finish` parks
    // a resolver rather than answering — a fixture that answered immediately
    // could never show the middle state, which is where the 취소 button and
    // the system-browser escape hatch live.
    google_account: () => window.__GOOGLE__ ?? GOOGLE_SIGNED_OUT,
    google_login_start: () => {
      window.__GOOGLE_STARTS__ = (window.__GOOGLE_STARTS__ ?? 0) + 1;
      return "https://accounts.google.com/o/oauth2/v2/auth?state=fixture";
    },
    google_login_finish: () =>
      new Promise((settle) => {
        window.__GOOGLE_FINISH__ = settle;
      }),
    cancel_google_login: () => {
      window.__GOOGLE_CANCELS__ = (window.__GOOGLE_CANCELS__ ?? 0) + 1;
      return null;
    },
    google_logout: () => {
      window.__GOOGLE__ = GOOGLE_SIGNED_OUT;
      return window.__GOOGLE__;
    },
    // Counted forced, like the other two gauges: a login that moved has to be
    // re-read past the backend's own floor.
    antigravity_usage: (args) => {
      if (args?.force) window.__ANTIGRAVITY_FORCED__ = (window.__ANTIGRAVITY_FORCED__ ?? 0) + 1;
      return { usage: window.__ANTIGRAVITY_USAGE__ ?? null, fetching: false };
    },
    // The checks panel's two reads. A fixture rather than a fetch: `gh` is not
    // on a test machine, and what is being checked here is the panel — how it
    // sorts, folds, tallies and opens — not GitHub.
    pr_checks: () => window.__CHECKS__ ?? { checks: [] },
    pr_check_details: (args) => {
      window.__DETAIL_CALLS__ = (window.__DETAIL_CALLS__ ?? 0) + 1;
      return window.__CHECK_DETAILS__?.[args.name] ?? { name: args.name, annotations: [], jobs: [] };
    },
    open_url: (args) => {
      window.__OPENED__ = args.url;
      return null;
    },
    // Forced asks are counted apart from the rest: a login that moved has to
    // be re-read past the backend's five-minute floor, and an unforced ask in
    // that place is the bar keeping the previous login's figure.
    claude_usage: (args) => {
      if (args?.force) window.__USAGE_FORCED__ = (window.__USAGE_FORCED__ ?? 0) + 1;
      return { usage: window.__USAGE__ ?? null, fetching: window.__USAGE_FETCHING__ ?? false };
    },
    codex_usage: (args) => {
      if (args?.force) window.__CODEX_USAGE_FORCED__ = (window.__CODEX_USAGE_FORCED__ ?? 0) + 1;
      return {
        usage: window.__CODEX_USAGE__ ?? null,
        fetching: window.__CODEX_USAGE_FETCHING__ ?? false,
      };
    },
    list_quick_commands: () => window.__QUICK__ ?? [],
    second_brain_status: () => ({
      saved_path: "",
      vaults: [],
      vault: null,
      obsidian_installed: false,
      created: [],
      quick_commands_added: 0,
      skill_installs: [],
    }),
    second_brain_setup: (args) => ({
      saved_path: args.path,
      vaults: [],
      vault: {
        path: args.path,
        exists: true,
        setup_complete: true,
        raw_items: 0,
        wiki_pages: 0,
        last_ingested: null,
        counts_capped: false,
      },
      obsidian_installed: false,
      created: [],
      quick_commands_added: 0,
      skill_installs: [],
    }),
    second_brain_open: () => null,
    /* 합성 볼트. 테스트가 `window.__VAULT__`로 크기를 정하면 여기서 결정적으로
       지어 낸다 — 천 페이지짜리 픽스처를 하네스 파일에 적으면 그 파일이
       메가바이트로 자라고, 그 크기는 재려는 것과 아무 상관이 없다. */
    second_brain_graph: (args) => {
      window.__GRAPH_ASKS__ = (window.__GRAPH_ASKS__ ?? 0) + 1;
      window.__GRAPH_ASKED__ = args;
      const built = window.__buildVaultGraph__(args, window.__VAULT__ ?? { pages: 0 });
      /* 답이 도착한 시각. 첫 그림의 예산은 창의 것이므로, 이 목이 천 개의
         노드를 지어 내는 시간은 그 예산에 들어가지 않아야 한다. */
      window.__GRAPH_ANSWERED_AT__ = performance.now();
      return built;
    },
    second_brain_page: (args) => {
      window.__GRAPH_PAGE_ASKED__ = args;
      return `${args.path}/${args.id}`;
    },
    save_quick_command: (args) => {
      window.__QUICK__ = window.__QUICK__ ?? [];
      window.__QUICK__.push(args.command);
      return null;
    },
    delete_quick_command: (args) => {
      window.__QUICK__ = (window.__QUICK__ ?? []).filter((one) => one.id !== args.id);
      return null;
    },
    // 비활성 워크스페이스 스캔. 티어 판정은 Rust의 것이고 거기서 시험되므로,
    // 이 픽스처는 그 출력의 모양이고 여기서 재는 것은 렌더와 삭제 경로다.
    // `only`가 붙은 물음은 preflight — 테스트가 그 답만 따로 바꿔 끼울 수
    // 있도록 다른 서랍에서 읽는다.
    // 공간. 실제 백엔드는 크기와 판정을 함께 실어 보내므로(`ready`는 Rust의
    // 분류기가 답한 값이다) 이 목도 그렇게 한다 — 창이 판정을 다시 만들지
    // 않는다는 것이 이 화면의 약속이고, 목이 그 칸을 비우면 그 약속을 시험할
    // 수 없다.
    workspace_space_scan: () => window.__SPACE__ ?? null,
    workspace_space_git: (args) => {
      window.__SPACE_ASKED__ = args.paths.slice();
      return (window.__SPACE_GIT__ ?? []).filter((row) => args.paths.includes(row.id));
    },
    workspace_space_cancel: () => {
      window.__SPACE_CANCELLED__ = (window.__SPACE_CANCELLED__ ?? 0) + 1;
      return null;
    },
    workspace_cleanup_scan: (args) => {
      window.__CLEANUP_CALLS__ = (window.__CLEANUP_CALLS__ ?? []).concat([
        JSON.parse(JSON.stringify(args ?? {})),
      ]);
      const held = args?.only ? (window.__CLEANUP_PREFLIGHT__ ?? window.__CLEANUP__) : window.__CLEANUP__;
      const rows = (held ?? []).filter(
        (row) => !args?.only || args.only.includes(row.path),
      );
      return { rows, scanned_at_ms: 1_700_000_000_000, classifier_version: 1 };
    },
    // The 1-du gate's ordinary answer: nothing repo-supplied to consent to,
    // so worktree creation flows on. Tests that exercise the dialog mock a
    // real "ask" themselves.
    repo_trust_standing: () => ({ standing: "nothing", changed: false, content: "" }),
    record_repo_trust: () => null,
    // The 1-dt shape, plus the 1-eh one: lines ride inside a view that can
    // also carry the render limit instead of them, and — only when the
    // ceiling passed and git found changed lines — the two documents the
    // merge view mounts, with the stamp a save has to hand back.
    file_diff: () => ({
      lines: [
        { kind: "ctx", old: "1", new: "1", text: "fn main() {" },
        { kind: "add", old: null, new: "2", text: '    println!("hi");' },
        { kind: "ctx", old: "2", new: "3", text: "}" },
      ],
      texts: {
        original: "fn main() {\n}\n",
        modified: 'fn main() {\n    println!("hi");\n}\n',
        version: "diff1",
      },
    }),
    launch_agent_tab: (args) => {
      window.__LAUNCHED__ = args;
      return (window.__NEXT_TERM__ = (window.__NEXT_TERM__ ?? 0) + 1);
    },
    list_claude_sessions: () => [],
    // 만들기 다이얼로그가 읽는 넷. 판정 규칙은 Rust의 것이고
    // (`zerocode_core::workitem`, 거기서 표로 시험된다) 여기 있는 것은 그
    // 답의 모양뿐이다 — 이 목이 규칙을 다시 구현하면 두 판정기가 생기고,
    // 그 위의 통과는 아무것도 증명하지 않는다.
    work_item_seed: (args) => {
      const text = (args.text ?? "").trim();
      const fallback = window.__FALLBACK_NAME__ ?? "quartz";
      const said = (item, label, display) => ({
        item,
        label,
        display_name: display,
        seed_name: display.toLowerCase().replaceAll(" ", "-"),
        // 손으로 고친 이름은 덮어쓰지 않는다는 판정도 Rust의 것이다
        // (`should_apply_auto_name`). 목은 그 답의 자리만 맡는다.
        apply_auto_name:
          (args.current ?? "") === "" || (args.current ?? "") === (args.lastAuto ?? null),
        fallback,
      });
      const number = /^#?(\d+)$/.exec(text);
      if (number) {
        return said(
          { kind: "issue", number: Number(number[1]), key: null, repo: null },
          `Issue ${number[1]}`,
          `Fix Issue ${number[1]}`,
        );
      }
      const link = /^https:\/\/[^/]+\/([^/]+)\/([^/]+)\/(pull|issues)\/(\d+)/.exec(text);
      if (link) {
        // The verb follows the kind, the way Rust's `WorkItemKind::verb` does:
        // a pull request is read and an issue is fixed.
        const pull = link[3] === "pull";
        return said(
          {
            kind: pull ? "pr" : "issue",
            number: Number(link[4]),
            key: null,
            repo: `${link[1]}/${link[2]}`,
          },
          `${pull ? "PR" : "Issue"} ${link[4]}`,
          `${pull ? "Review PR" : "Fix Issue"} ${link[4]}`,
        );
      }
      const words = text.split(/\s+/).filter(Boolean).slice(0, 5).join(" ");
      return said(null, "", words);
    },
    list_branches: () => window.__BRANCHES__ ?? [],
    // GitHub 탭이 읽는 둘. 검색 문법도 JSON 매핑도 Rust의 것이고(`gh::search_plan`,
    // `gh::read_work_items` — 거기서 표로 시험된다), 이 목이 맡는 것은 답의
    // 모양과 "좁혀진다"는 사실뿐이다. 실패는 문자열이 아니라 `{ kind, message }`로
    // 거절된다(`GhFailure`) — 조용한 줄을 가르는 판정이 실제로 시험되도록.
    // 칩이 검색칸에 써 넣을 문장(1-g77a). 진짜 문법은 Rust의 표 하나이고
    // (`gh::preset_query` — 쌍마다 거기서 시험된다), 이 목이 맡는 것은 「창은
    // 받은 문장을 그대로 보여 주고 그대로 다시 묻는다」는 사실뿐이다. 그래서
    // 여기서 오는 것은 GitHub의 문법이 아니라 알아볼 수 있는 표식이다.
    github_preset_query: (args) => `${args.kind}/${args.preset}`,
    github_web_urls: (args) => ({
      repo: `https://github.com/acme/${args.project ? "app" : "none"}`,
      issues: "https://github.com/acme/app/issues",
      pulls: "https://github.com/acme/app/pulls",
      new_issue: "https://github.com/acme/app/issues/new",
    }),
    github_work_items: (args) => {
      window.__GH_ASKED__ = { project: args.project, preset: args.preset, query: args.query };
      window.__GH_ASKS__ = (window.__GH_ASKS__ ?? 0) + 1;
      const said = window.__GH_ITEMS__;
      if (said && said.refuse) throw said.refuse;
      const rows = said ?? [];
      const needle = (args.query ?? "").trim().toLowerCase();
      return needle ? rows.filter((row) => row.title.toLowerCase().includes(needle)) : rows;
    },
    resolve_pr_base: (args) => {
      window.__PR_BASE_ASKED__ = args;
      if (window.__PR_BASE_REFUSES__) throw new Error("could not fetch the PR head");
      // A fork's head is fetched from the contributor's own repository, which
      // the resolve added as a remote — so the push target names that remote,
      // and the compare base still names the base repository's.
      const fork = args.crossRepo === true;
      return {
        start_point: "deadbeefcafe",
        branch: args.headRef,
        compare_base: args.baseRef ? `refs/remotes/origin/${args.baseRef}` : null,
        push_remote: fork ? "pr-outsider-tool" : "origin",
        push_branch: args.headRef,
        push_fork: fork,
        maintainer_can_modify: fork ? false : null,
      };
    },
    validate_branch_name: () => null,
    worktree_prefs: () =>
      window.__WT_PREFS__ ?? { branch_prefix: "git-username", custom_prefix: null },
    save_worktree_prefs: (args) => {
      window.__WT_PREFS__ = { branch_prefix: args.mode, custom_prefix: args.custom ?? null };
      return window.__WT_PREFS__;
    },
    // The backend's contract, in miniature: the template renders around the
    // base prompt and the agent resolves. Tests that care mock it themselves.
    launch_plan_for_action: (args) => ({
      agent: "claude",
      prompt: (args.templateOverride ?? "{basePrompt}")
        .replaceAll("{basePrompt}", args.basePrompt)
        .trim(),
      args: [],
    }),
    launch_recipes: () => ({}),
    // 연결 여부와 목록은 시험이 정한다. 기본은 미연결 — 첫 프레임이 그리는
    // 것이 연결 카드라는 사실 자체가 여러 시험의 전제다.
    jira_status: () => window.__JIRA_STATUS__ ?? { connected: false, sites: [] },
    linear_status: () => window.__LINEAR_STATUS__ ?? {
      connected: false,
      credential: "missing",
      connection: null,
    },
    linear_issues: () => window.__LINEAR_ISSUES__ ?? [],
    linear_search_issues: () => window.__LINEAR_ISSUES__ ?? [],
    linear_issue_detail: (_window, args) => window.__LINEAR_DETAIL__ ?? {
      id: args.id,
      key: "ENG-1",
      title: "Linear issue",
      status: "Todo",
      category: "todo",
      priority: null,
      assignee: null,
      updated: null,
      url: "https://linear.app/acme/issue/ENG-1/linear-issue",
      team_id: "team-1",
      team_name: "Engineering",
      description: "",
      created: null,
      labels: [],
      attachments: [],
    },
    linear_issue_comments: () => [],
    linear_comment_issue: (_window, args) => ({
      id: "comment-1", author: "Fixture", body: args.body, created: null,
    }),
    linear_issue_options: () => ({ teams: [], states: [], priorities: [] }),
    linear_agent_context: () => null,
    ssh_hosts: () => window.__SSH_HOSTS__ ?? { hosts: [] },
    remote_workspaces: () => window.__REMOTE_WORKSPACES__ ?? { workspaces: [] },
    probe_ssh_host: () => ({
      keyAlgorithm: "ssh-ed25519",
      encodedKey: "AAAAC3NzaC1lZDI1NTE5AAAAIFixturePublicKeyOnly",
      fingerprint: "SHA256:fixture-host-key",
    }),
    probe_remote_workspace: (_window, args) => ({ root: args.input.root }),
    // 실패는 문자열이 아니라 `{ kind, message }`로 거절된다(Rust의
    // `jira::Failure`). 목이 그 모양을 그대로 던져야 조용한 상태와 오류 띠를
    // 가르는 판정이 실제로 시험된다.
    jira_issues: () => {
      const said = window.__JIRA_ISSUES__;
      if (said && said.refuse) throw said.refuse;
      return said ?? [];
    },
    terminal_command: () => "zo",
    terminal_command_argv: () => ["zo"],
    process_memory: () => 0,
    // The floating shell's seat, as the real backend answers it: where the
    // shell starts (the header names this), and the display resolution the
    // settings field asks for.
    open_terminal: () => "~",
    floating_workspace_seat: () => "/Users/tester",
    patch_floating_workspace: (args) => {
      window.__FLOATING_WORKSPACE_WRITES__.push(JSON.parse(JSON.stringify(args)));
      return null;
    },
    set_status_bar_item: (args) => {
      window.__STATUS_BAR_WRITES__.push(JSON.parse(JSON.stringify(args)));
      return null;
    },
    set_usage_percentage_display: (args) => {
      window.__USAGE_PERCENTAGE_WRITES__.push(JSON.parse(JSON.stringify(args)));
      return null;
    },
    set_status_bar_usage_mode: (args) => {
      window.__STATUS_BAR_USAGE_MODE_WRITES__.push(JSON.parse(JSON.stringify(args)));
      return null;
    },
    set_show_titlebar_app_name: (args) => {
      window.__TITLEBAR_APP_NAME_WRITES__.push(JSON.parse(JSON.stringify(args)));
      return null;
    },
    set_show_menu_bar_icon: (args) => {
      window.__MENU_BAR_ICON_WRITES__.push(JSON.parse(JSON.stringify(args)));
      return null;
    },
    set_minimize_to_tray_on_close: (args) => {
      window.__MINIMIZE_TO_TRAY_WRITES__.push(JSON.parse(JSON.stringify(args)));
      return null;
    },
    set_compact_worktree_cards: (args) => {
      window.__WORKTREE_CARD_LAYOUT_WRITES__.push(JSON.parse(JSON.stringify(args)));
      return null;
    },
    set_show_git_ignored_files: (args) => {
      window.__GIT_IGNORED_VISIBILITY_WRITES__.push(JSON.parse(JSON.stringify(args)));
      return null;
    },
    set_source_control_group_order: (args) => {
      window.__SOURCE_CONTROL_GROUP_ORDER_WRITES__.push(JSON.parse(JSON.stringify(args)));
      return null;
    },
    set_source_control_compare_base: (args) => {
      window.__SOURCE_CONTROL_COMPARE_BASE_WRITES__.push(JSON.parse(JSON.stringify(args)));
      return null;
    },
    set_worktree_compare_base: (args) => {
      window.__WORKTREE_COMPARE_BASE_WRITES__.push(JSON.parse(JSON.stringify(args)));
      return {
        head: "feature/compare",
        base_ref: args.reference || "origin/main",
        source: args.reference ? "worktree" : "repository-default",
        options: ["origin/feature", "origin/main"],
        error: null,
      };
    },
    set_refresh_local_base_ref_on_worktree_create: (args) => {
      window.__LOCAL_BASE_REF_REFRESH_WRITES__.push(JSON.parse(JSON.stringify(args)));
      return null;
    },
    patch_left_sidebar_appearance: (args) => {
      window.__LEFT_SIDEBAR_APPEARANCE_WRITES__.push(JSON.parse(JSON.stringify(args)));
      return null;
    },
    set_ui_zoom: (args) => {
      window.__UI_ZOOM_WRITES__.push(JSON.parse(JSON.stringify(args)));
      return null;
    },
    apply_ui_zoom: (args) => {
      window.__UI_ZOOM_APPLIES__.push(JSON.parse(JSON.stringify(args)));
      return null;
    },
    sessions: () => [],
    // A fresh id per shell, the way the backend hands them out. A constant
    // was enough while a terminal tab was one shell; a tab that can be split
    // asks again, and two panes answering to one id is two panes sharing a
    // screen.
    open_term_tab: () => (window.__NEXT_TERM__ = (window.__NEXT_TERM__ ?? 0) + 1),
    // 이 창이 지금 읽고 있는 셸들. 실제 백엔드는 이 집합에 든 셸의 프레임만
    // 내보내므로(main.rs `pump_loop`), 이 목은 그 집합을 그대로 들고 있다가
    // 프레임을 흘리는 시험이 같은 문을 지나게 한다 — 창이 아무 말도 하지
    // 않은 동안에는 `undefined`이고, 그것이 곧 "전부 배달"이던 옛 시절이다.
    set_watched_terms: (args) => {
      const previous = window.__WATCHED_TERMS__ ?? new Set();
      window.__WATCHED_TERMS__ = new Set(args.terms ?? []);
      window.__WATCH_CALLS__ = (window.__WATCH_CALLS__ ?? 0) + 1;
      // A shell this window starts reading is owed its floor
      // (`FrameReaders::declare`); one it stops reading takes its share and
      // its debt with it.
      for (const term of window.__WATCHED_TERMS__) {
        if (!previous.has(term)) termShare.owed.add(term);
      }
      for (const term of termShare.owed) {
        if (!window.__WATCHED_TERMS__.has(term)) termShare.owed.delete(term);
      }
      termShare.frames = termShare.frames.filter(({ term, fed }) => {
        const keep = window.__WATCHED_TERMS__.has(term);
        if (!keep) fed();
        return keep;
      });
      // Frames printed while this declaration was on its way were printed for
      // a reader: they follow its floor, in the order they came.
      for (const frame of termShare.parked.splice(0)) {
        if (window.__WATCHED_TERMS__.has(frame.term)) termShare.frames.push(frame);
        else frame.fed();
      }
      return null;
    },
    // What this window's shells owe it, in one answer — the readers' share as
    // `zerocode-pty` readers.rs keeps it: a newly declared shell's floor first,
    // then the frames fed since, oldest first, as the bytes `term_pull` answers
    // with. `resync` owes every declared shell its floor again. A declared
    // shell the backend does not hold is named in `missing` on every pull, as
    // `term_pull` does (board.rs).
    term_pull: (args) => {
      const watched = window.__WATCHED_TERMS__ ?? new Set();
      if (args?.resync) {
        for (const term of watched) termShare.owed.add(term);
      }
      const screens = new Map();
      const missing = [];
      for (const term of watched) {
        const floor = serve("term_snapshot", { term });
        if (floor == null) missing.push(term);
        else if (termShare.owed.has(term)) screens.set(term, [floor]);
      }
      termShare.owed.clear();
      const served = termShare.frames.splice(0);
      for (const { term, delta } of served) {
        if (!screens.has(term)) screens.set(term, []);
        screens.get(term).push(delta);
      }
      // Settled once the window has taken the answer in: its `await invoke`
      // resumes in a microtask, and this runs after every microtask.
      setTimeout(() => {
        for (const { fed } of served) fed();
      });
      const answer = {
        screens: [...screens].map(([term, frames]) => ({ term, frames })),
        missing,
      };
      return new TextEncoder().encode(JSON.stringify(answer)).buffer;
    },
    // 그리고 보드가 카드로 들고 있는 셸들 — 두 번째 티어. 실제 백엔드는 이
    // 집합에 든 셸에만 `term:preview`를 보내고, 감시 집합에 든 셸은 여기서
    // 빠진다(main.rs `pump_loop`). 목이 같은 표를 들고 있어야 "선언한 것만
    // 그려진다"를 이쪽에서도 물을 수 있다.
    set_previewed_terms: (args) => {
      window.__PREVIEWED_TERMS__ = new Set(args.terms ?? []);
      window.__PREVIEW_CALLS__ = (window.__PREVIEW_CALLS__ ?? 0) + 1;
      return null;
    },
  };
  class TestChannel {
    constructor() {
      this.onmessage = () => {};
      this.cleaned = false;
    }
    cleanupCallback() {
      this.cleaned = true;
      this.onmessage = () => {};
    }
  }
  /* 이 창의 터미널 몫 — 실제 백엔드가 판마다 이 창에 들고 있는 것
   * (`FrameReaders`)의 목. `owed`는 바닥을 빚진 셸, `frames`는 아직 안 당겨 간
   * 프레임, `parked`는 창이 읽기 시작했으나 그 선언이 아직 닿지 않은 셸의
   * 프레임(다음 선언이 몫으로 옮긴다). */
  const termShare = { owed: new Set(), frames: [], parked: [], noticing: false };
  /* 알림은 백엔드의 펌프에서 오고, 창에는 이벤트로 — 흘린 자의 호출 안이 아니라
   * 그 다음에 — 닿는다. 한 틱에 흘린 장들에는 알림이 하나다: 그 장들은 다음
   * 당김에 함께 실린다. */
  const noticeTermFrames = () => {
    if (termShare.noticing) return;
    termShare.noticing = true;
    queueMicrotask(() => {
      termShare.noticing = false;
      for (const [name, handlers] of Object.entries(window.__LISTENERS__)) {
        if (!name.startsWith("term:dirty:")) continue;
        for (const handler of handlers) handler({ payload: null });
      }
    });
  };
  /* 시험이 셸의 출력을 흘리는 **하나뿐인** 문.
   *
   * 백엔드가 하는 일을 그대로 한다: 아무 창도 읽지 않는 셸의 프레임은 여기서
   * 멈추고(펌프가 가져가지 않는다), 읽히는 셸의 프레임은 이 창의 몫에 쌓인 뒤
   * 알림(`term:dirty:<라벨>`)이 간다. 창은 제 박자대로 당겨 간다 — 조용하면
   * 곧바로, 흐르는 중이면 다음 화면 프레임에. 돌려주는 약속은 이 프레임을 실은
   * 당김이 창에 닿은 뒤에 풀린다: 흘린 뒤 화면을 읽는 시험은 그것을 기다린다.
   * 한 틱에 여러 장을 흘리면 한 당김에 함께 실려 한 번 칠해진다 — 그것이
   * 이 길의 요점이다.
   *
   * 창이 방금 드러낸 셸(`readingTerms`)의 선언이 아직 오는 중이면, 그 사이에
   * 흘린 프레임은 버리지 않고 그 선언 뒤로 넘긴다. 탭을 열자마자 흘리는 시험이
   * 뜻하는 것은 「읽히는 셸이 출력했다」이지, 선언과 경주하는 출력이 아니다. */
  window.__TERM_FEED__ = (term, delta) => new Promise((fed) => {
    const frame = { term, delta, fed };
    const watched = window.__WATCHED_TERMS__;
    if (watched === undefined || watched.has(term)) {
      termShare.frames.push(frame);
      noticeTermFrames();
      return;
    }
    if (typeof readingTerms === "function" && readingTerms().has(term)) {
      termShare.parked.push(frame);
      void syncWatchedTerms();
      return;
    }
    fed();
  });
  /* 창이 시계로 두드리는 명령들 — 목록은 창 소스에서 유도해 이 판에 실려 온다
   * (하네스 머리의 `derivePollerCommands`). */
  window.__POLLERS__ = new Set(pollers ?? []);
  window.__HOLD_POLLERS__ = false;
  window.__PARKED__ = [];
  /* 실제로 답하는 자리. 주차장을 비울 때도 같은 문을 지나야 하므로 이름이 있다. */
  const serve = (command, args, options) => {
    // The third argument is Tauri's own: a raw body rides `args` and its
    // headers ride here, so a stub asked about a picture can read both.
    if (window.__ANSWER__[command]) return window.__ANSWER__[command](args, options);
    if (command === "set_keybinding") {
      window.__CALLS__.push(JSON.parse(JSON.stringify(args)));
      const { actionId, bindings } = args;
      if (bindings === null || bindings === undefined) delete window.__OVERRIDES__[actionId];
      else window.__OVERRIDES__[actionId] = bindings;
      return { ...window.__OVERRIDES__ };
    }
    return answers[command] ? answers[command](args) : null;
  };
  /* 주차장을 비운다: 한 명령에 한 번만 진짜로 묻고 그 답을 그 명령으로 주차된
   * 모든 약속에 나눠 준다. 폴러는 같은 것을 되묻기 때문이고, 긴 주차를 하나씩
   * 되돌려주면 풀리는 순간이 그 자체로 폭풍이 된다. 답을 **주는** 것이 요점이다
   * — 영원히 안 답하면 `machineInFlight` 같은 걸쇠가 걸린 채 남아 그 뒤의
   * 케이스에서 그 박자가 영영 돌지 않는다. */
  window.__RELEASE_POLLERS__ = () => {
    const parked = window.__PARKED__;
    window.__PARKED__ = [];
    const served = new Map();
    for (const one of parked) {
      if (!served.has(one.command)) {
        let answer = null;
        try {
          answer = serve(one.command, one.args, one.options);
        } catch {
          answer = null;
        }
        served.set(one.command, answer);
      }
      one.answer(served.get(one.command));
    }
    return parked.length;
  };
  window.__TAURI__ = {
    core: {
      Channel: TestChannel,
      invoke: async (command, args, options) => {
        /* 시계가 부르는 것은 몸짓의 값이 아니다 (t-2412). 「이 몸짓은 아무것도
         * 묻지 않는다」를 재는 케이스는 이 집합을 켠 채 재고, 그동안 박자는
         * 세어지지도 답해지지도 않는다 — 세면 전환의 값이 되고, 답하면 그
         * 답의 다시 그리기가 프레임 예산 안으로 들어온다. */
        if (window.__HOLD_POLLERS__ && window.__POLLERS__.has(command)) {
          return new Promise((answer) => window.__PARKED__.push({ command, args, options, answer }));
        }
        // Counted here because `shell.js` destructures `invoke` once at load,
        // so a test cannot wrap it from the outside afterwards. What a gesture
        // costs in subprocesses is a fact worth being able to assert.
        window.__COUNTS__[command] = (window.__COUNTS__[command] ?? 0) + 1;
        // 순서까지 물어야 하는 시험 하나를 위해 (1-ep: 먼저 지켜본다고 말하고,
        // 그 다음에 스냅샷). 켠 시험만 기록하므로 나머지에는 값이 없다.
        window.__ORDER__?.push(command);
        if (window.__FAIL__.has(command)) throw new Error(`refused: ${command}`);
        if (window.__IN_FLIGHT__) {
          const flights = window.__IN_FLIGHT__;
          flights.set(command, (flights.get(command) ?? 0) + 1);
          try { return await serve(command, args, options); }
          finally { flights.set(command, flights.get(command) - 1); }
        }
        return serve(command, args, options);
      },
    },
    event: {
      listen: async (name, handler) => {
        (window.__LISTENERS__[name] ??= []).push(handler);
        return () => {};
      },
    },
    clipboardManager: {
      readText: async () => {
        window.__CLIPBOARD_READS__ += 1;
        if (window.__CLIPBOARD_READ_HOLD__) {
          await new Promise((done) => (window.__CLIPBOARD_RELEASE__ = done));
        }
        if (window.__CLIPBOARD_READ_FAIL__) throw new Error("clipboard read secret");
        return window.__CLIPBOARD_TEXT__;
      },
      writeText: async (text) => {
        if (window.__CLIPBOARD_WRITE_FAIL__) throw new Error("clipboard write secret");
        window.__CLIPBOARD_TEXT__ = text;
        window.__CLIPBOARD_WRITES__.push(text);
      },
    },
  };
};

/* 목과, 그 목이 주차할 폴러 이름표를 함께 건다 — 판이 여럿이라 한 문으로
 * 모은다(팝아웃도 같은 백엔드 위에 서야 한다). */
const standBackend = (surface, boot = BOOT) =>
  surface.addInitScript(stubBackend, { boot, pollers: POLLER_COMMANDS });


/* The harness's hands on a page — one set, installed on every page this door
 * opens, so a suite on its own page has the same hands as the shared flow.
 *
 * The editor, reached the way the window reaches it. The file surface used to
 * be a `<textarea>`, and every test drove it by assigning `.value` and firing
 * an `input` event — which was the real path, because that is exactly what a
 * keystroke did. It is CodeMirror now, so the real path is a transaction:
 * `dispatch` is what a keypress produces, and it runs the same update
 * listener, the same dirty comparison and the same strip repaint. Nothing
 * here is a stand-in for the editor; it IS the editor.
 *
 * Installed as an init script, lazy so it can name globals `ui/shell.js` has
 * not declared yet at injection time. */
const installHarnessHands = (surface) => surface.addInitScript((primaryEvent) => {
  window.__TEST_PRIMARY_EVENT__ = primaryEvent;
  const owner = () => tabs.find((held) => held.id === activeTabId);
  // The live EditorView in front of the active tab, or null when the tab in
  // front is not a file.
  window.__EDITOR__ = () => {
    const tab = owner();
    return tab ? editorShowing(tab) : null;
  };
  window.__SHOWN__ = () => window.__EDITOR__()?.state.doc.toString() ?? null;
  // What typing into the whole document does. Selecting everything and
  // replacing it is one transaction, which is what ⌘A followed by a paste is.
  window.__TYPE__ = (text) => {
    const editor = window.__EDITOR__();
    editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: text } });
  };
  window.__CARET__ = (at) => window.__EDITOR__().dispatch({ selection: { anchor: at } });
  window.__AT__ = () => window.__EDITOR__().state.selection.main.head;

  /* 에이전트 소식이 그림이 되기까지 기다리는 문 (1-ep).
   *
   * 탭 배지·카드 행·문 배지·보드는 이제 이벤트마다가 아니라 **한 프레임에 한
   * 번** 그려진다. 그래서 훅을 흘린 직후의 DOM은 아직 옛 그림이고, 그것을 읽는
   * 시험은 임의의 밀리초가 아니라 프레임 하나를 기다려야 한다. 두 번 기다리는
   * 것은 예약이 이번 프레임의 콜백 뒤에 걸렸을 수도 있기 때문이다. */
  window.__PAINTED__ = () =>
    new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));

  /* 「값이 0」을 재기 전의 한 박자 (t-2412 규칙 3).
   *
   * 케이스는 서로의 상태를 물려받지 않는다: 앞 케이스가 남긴 프레임 예약과,
   * 이미 떠난 물음의 `.then`이 뒷 케이스의 계수에 섞이면 그 숫자는 케이스의
   * 것이 아니다. 프레임 둘과 매크로태스크 하나를 흘려보낸 뒤 계수기를 0으로
   * 잡는다. 폴러의 한 주기를 따로 기다리지 않는 것은 `__HOLD_POLLERS__`가
   * 이미 그것을 주차했기 때문이다 — 기다림은 그 주차가 없을 때의 대용이다. */
  window.__SETTLED__ = async () => {
    await window.__PAINTED__();
    await new Promise((done) => setTimeout(done, 0));
    await window.__PAINTED__();
    window.__COUNTS__ = {};
  };
}, PRIMARY_EVENT);

/* A fresh document for scenarios that own their timing, adapters or pane
 * layout — and, since t-4017, for every suite that owns its state: the page
 * dies with the suite, so nothing the suite moved (the active workspace, the
 * pane helpers, the tabs) has to be put back for a neighbour.
 *
 *   faults — where the page's renderer errors go: the caller's own list when
 *            one check reads several pages' faults (the shared window flow),
 *            a fresh one otherwise.
 *   before — a hand on the page after the backend stands and before the
 *            window boots: the init script a fixture needs. */
export async function openWindowTestPage(browser, origin, { faults = [], before = null } = {}) {
  // A context of its own, not `browser.newPage()`: a context a page owns
  // refuses `context.newPage()`, and the axe check opens its blank page that
  // way ("Please use browser.newContext()"). The context leaves with the page.
  const context = await browser.newContext({ viewport: { width: 1280, height: 860 } });
  const page = await context.newPage();
  page.on("close", () => context.close().catch(() => {}));
  page.on("pageerror", (error) => faults.push(error?.stack ?? String(error)));
  // `WINDOW_TRACE=1`: a page-side `console.log("TRACE …")` reaches stderr, so
  // a scenario that never returns can say how far it got.
  if (process.env.WINDOW_TRACE) {
    page.on("console", (message) => {
      const said = message.text();
      if (said.startsWith("TRACE ")) process.stderr.write(`${said}\n`);
    });
  }
  await standBackend(page);
  await installHarnessHands(page);
  if (before) await before(page);
  await page.goto(`${origin}/index.html`);
  try {
    await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
  } catch (error) {
    throw new Error(`window did not finish loading: ${faults.join(" | ") || error}`);
  }
  return { page, faults };
}

export async function createWindowServer({ bootstrapSource = null } = {}) {
const files = createServer(async (request, response) => {
  const asked = decodeURIComponent((request.url ?? "/").split("?")[0]);
  if (asked === "/__window-fixture.js" && bootstrapSource !== null) {
    response.writeHead(200, { "content-type": "text/javascript" });
    response.end(bootstrapSource);
    return;
  }
  const path = join(UI, asked === "/" ? "index.html" : asked);
  // The server only ever answers from `ui/`, so a crafted path cannot walk out.
  if (!path.startsWith(UI)) {
    response.writeHead(403).end();
    return;
  }
  // The file is read before any header goes out: a read that fails after a
  // 200 was written could only answer with a second `writeHead`, which node
  // throws on (ERR_HTTP_HEADERS_SENT) and which took a whole suite down with
  // it once (2026-09-24). A miss is a 404 written exactly once.
  let content;
  try {
    content = await readFile(path);
  } catch {
    response.writeHead(404).end();
    return;
  }
  const type = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css" };
  response.writeHead(200, { "content-type": type[extname(path)] ?? "text/plain" });
  response.end(bootstrapSource !== null && extname(path) === ".html"
    ? content.toString("utf8").replace("<head>", '<head><script src="/__window-fixture.js"></script>')
    : content);
});
await new Promise((done) => files.listen(0, "127.0.0.1", done));
const origin = `http://127.0.0.1:${files.address().port}`;


return { files, origin };
}
export { BOOT, chromium, pollers, POLLER_COMMANDS, standBackend };
