/* 창 하나를 시험대에 세우는 픽스처 (6차 P2에서 따로 나옴).
 *
 * 부팅 보고 하나(`BOOT`)와, 그것을 문서보다 먼저 판에 심는 손(`seedKnowledgeWindow`).
 * 두 하네스가 이것을 나눠 쓴다: 계약을 묻는 `test-knowledge-graph.mjs`와, 진짜
 * GPU에서 시간을 재는 `knowledge-gpu.mjs`. 한 벌이어야 하는 이유는 간단하다 —
 * 두 픽스처는 두 창이고, 두 창에서 잰 수는 견줄 수 없다.
 *
 * 옮긴 것이지 고친 것이 아니다. */
export const BOOT = {
  project_root: "/tmp/zerocode-window-test",
  active_root: "/tmp/zerocode-window-test",
  project: "zerocode",
  hidden_shortcuts: [],
  keybindings: { "tab.close": [], "view.toggleAside": [], "terminal.newTab": [], "terminal.toggle": [] },
  zo: "/usr/local/bin/zo",
  lanes: [],
  worktrees: [],
  recent_projects: [],
  theme: "dark",
  locale: "ko",
  ui_zoom_level: 0,
  ui_zoom_spec: { min_level: -3, max_level: 5, step: 0.5, default_level: 0, scale_base: 1.2 },
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
  workspace_board: { statuses: [], cards: {}, column_width: 308 },
  sidebar_view: { group_by: "repo", sort_by: "default", project_order: "default" },
  show_git_ignored_files: true,
  source_control_group_order: "changes-first",
  source_control_compare_base: "repository-default",
  refresh_local_base_ref_on_worktree_create: false,
  left_sidebar_appearance_mode: "default",
  left_sidebar_tint_color: "#18181b",
  left_sidebar_tint_opacity: 0.08,
  panel_widths: { sidebar: 280, aside: 350 },
  floating_workspace: { enabled: true, cwd: "~", trigger_location: "floating-button" },
  serve: "down",
  system_locale: "ko",
  terminal_command: "zo",
  setup_script_launch_mode: "new-tab",
  terminal_shortcut_policy: "orca-first",
  editing_prefs: {
    editor_auto_save: false, editor_auto_save_delay_ms: 1000, editor_minimap_enabled: false,
    editor_word_wrap: true, diff_word_wrap: false, rich_markdown_spellcheck_enabled: true,
    markdown_review_tools_enabled: true, editor_font_family: "",
    combined_diff_file_tree_visible_by_default: false,
    primary_selection_middle_click_paste: null, editor_font_zoom: 0,
  },
  editing_prefs_spec: { auto_save_delay_ms: { min: 250, max: 10000, step: 250 }, editor_font_zoom: { min_level: -6, max_level: 18, step: 1, default_level: 0, px_min: 8, px_max: 32 } },
  diff_side_by_side: true,
  terminal_prefs: { font_size: 14, font_family: "", ligatures: "auto", mac_option_as_alt: "auto", jis_yen_to_backslash: false, allow_osc52_clipboard: true, windows_shell: "powershell.exe", windows_powershell_implementation: "auto", theme_dark: "Ghostty Default Style Dark", use_separate_light_theme: true, theme_light: "Builtin Tango Light", custom_themes: [], color_overrides: {}, leading: 1.15, weight: 500, cursor_blink: true, cursor_style: "block", cursor_opacity: 1, padding_x: 4, padding_y: 4, sensitivity: 1.15, scrollback: 5000, word_separators: " ()[]{}',\"`", fast_scroll_sensitivity: 5, tui_scroll_sensitivity: 1, focus_follows_mouse: false, hide_mouse_while_typing: false, copy_on_select: false, right_click_paste: false, inactive_pane_opacity: 0.9, divider_color_dark: "#3f3f46", divider_color_light: "#d4d4d8", divider_thickness_px: 3 },
  terminal_prefs_spec: { font_size: { min: 10, max: 24, step: 1 }, font_family_defaults: { macos: "SF Mono", windows: "Cascadia Mono", linux: "DejaVu Sans Mono" }, ligature_modes: ["auto", "on", "off"], mac_option_as_alt_modes: ["auto", "true", "left", "right", "false"], windows_shells: ["powershell.exe", "cmd.exe", "git-bash"], windows_powershell_implementations: ["auto", "powershell.exe", "pwsh.exe"], ligature_font_tokens: ["fira code"], theme_defaults: { dark: "Ghostty Default Style Dark", light: "Builtin Tango Light" }, terminal_themes: [], color_override_groups: [], leading: { min: 1, max: 3, step: 0.1 }, leading_base: 1.15, weight: { min: 100, max: 900, step: 100 }, bold_weight_floor: 700, bold_weight_offset: 200, cursor_styles: ["block", "bar", "underline"], cursor_opacity: { min: 0, max: 1, step: 0.05 }, padding_x: { min: 0, max: 512, step: 1 }, padding_y: { min: 0, max: 512, step: 1 }, sensitivity: { min: 0.5, max: 3, step: 0.05 }, fast_scroll_sensitivity: { min: 1, max: 10, step: 0.5 }, tui_scroll_sensitivity: { min: 1, max: 10, step: 1 }, scrollback: { min: 1000, max: 50000, step: 100 }, scrollback_presets: [5000], word_separators_max_chars: 128, inactive_pane_opacity: { min: 0, max: 1, step: 0.05 }, divider_color_defaults: { dark: "#3f3f46", light: "#d4d4d8" }, divider_thickness_px: { min: 1, max: 32, step: 1 }, divider_hit_padding_px: 6, max_custom_terminal_themes: 200, custom_theme_selection_prefix: "custom:" },
  confirm_close_pinned: true,
  skip_close_terminal_with_running_process_confirm: false,
  ctrl_tab_order_mode: "mru",
  workspace_creation_prefs: { directory: "~/zerocode/workspaces", nest_workspaces: true },
  open_in_applications: [{ id: "vscode", label: "VS Code", command: "code" }],
  open_in_applications_spec: { max: 8, presets: [{ id: "vscode", label: "VS Code", command: "code" }] },
  browser: { home_page: "", search_engine: "google", restore_tabs: false, open_links_in_app: false, open_links_in_app_modifier_inverts: false, open_links_in_app_prompted: false, terminal_link_action_popover: true, default_zoom_level: 1.5, open_tabs: [] },
  browser_zoom_spec: { min_level: -3, max_level: 5, step: 0.5, default_level: 0, scale_base: 1.2 },
  skip_delete_worktree_confirm: false,
  skip_delete_automation_confirm: false,
};

export const seedKnowledgeWindow = (target) => target.addInitScript((boot) => {
  window.__CALLS__ = [];
  window.__LISTENERS__ = {};
  window.__CLIPBOARD_TEXT__ = "";
  /* 서 있는 이름표의 상자들과 그 겹친 쌍 — 페인터가 무엇이든 화면에 선 글자를 DOM에게
     묻는다: SVG는 점의 <text>와 주제의 이름판, GL은 오버레이다. 격자가 어림한 폭이
     아니라 실제로 그려진 글자가 겹치는지가 답이다. 두 재는 자(장면 표와 GL 대조)가
     이 한 벌을 쓴다. */
  window.__knowledgeLabelBoxes__ = (view) => {
    const boxes = [];
    for (const word of view.querySelectorAll(
      ".knowledge-node.is-named .knowledge-label, .knowledge-gl-label:not([hidden]),"
      + " .knowledge-cluster-label:not(.is-folded)",
    )) {
      const seat = word.getBoundingClientRect();
      if (seat.width === 0 || seat.height === 0) continue;
      boxes.push([seat.left, seat.top, seat.right, seat.bottom]);
    }
    return boxes;
  };
  window.__knowledgeOverlapPairs__ = (boxes, slack) => {
    let pairs = 0;
    for (let left = 0; left < boxes.length; left += 1) {
      for (let right = left + 1; right < boxes.length; right += 1) {
        const one = boxes[left];
        const two = boxes[right];
        if (one[2] - two[0] > slack && two[2] - one[0] > slack
          && one[3] - two[1] > slack && two[3] - one[1] > slack) {
          pairs += 1;
        }
      }
    }
    return pairs;
  };
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
      contradictions,
      superseded: table.superseded.length,
      merge_candidates: null,
    };
    table.findings = table.counts.index_gaps + table.counts.ghost_links + table.counts.orphans
      + table.counts.missing_frontmatter + table.counts.undeclared_relations
      + table.counts.unlogged_raw;
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
        /* 이름을 준 자리는 그 이름으로 — 관리용 문서(`wiki/index.md`)를 세우는 시험의 길(t-4140 S6). */
        id: spec.ids?.[at] ?? `wiki/Page-${String(at).padStart(4, "0")}.md`,
        title: spec.titles?.[at] ?? `개념 ${at}`,
        tags: tags.length === 0 ? [] : [tags[at % tags.length]],
        kind: "page",
        modified_ms: spec.modifiedMs?.[at] ?? 1700000000000 + at,
        out_links: 0,
        in_links: 0,
        source: args.sources ? `raw/source-${at % 5}.md` : null,
        excerpt: `개념 ${at}의 첫 문단 요약입니다.`,
        folder: at % 3 === 0 ? "topics" : "",
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
        excerpt: "",
        folder: "",
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
          excerpt: "",
          folder: "",
        });
      }
    }
    /* 공급망 종류(모양 시험): 구성요소는 사각, 취약점은 삼각으로 서야 한다
     * (`KNOWLEDGE_NODE_SHAPES`). 선은 첫 페이지 → 구성요소, 취약점 → 구성요소 — 시험이 선을
     * 손으로 주면(`customEdges`) 그 선만 선다(픽셀 대조는 선 없는 다섯 점을 쓴다). */
    const componentFrom = nodes.length;
    const components = spec.supply?.components ?? 0;
    for (let at = 0; at < components; at += 1) {
      nodes.push({ id: `sbom:crate/part-${at}@1.0.${at}`, title: `part-${at}`, tags: [],
        kind: "component", modified_ms: 0, out_links: 0, in_links: 0, source: null,
        excerpt: "", folder: "" });
    }
    const vulnerabilityFrom = nodes.length;
    const vulnerabilities = spec.supply?.vulnerabilities ?? 0;
    for (let at = 0; at < vulnerabilities; at += 1) {
      nodes.push({ id: `osv:RUSTSEC-2026-${String(at).padStart(4, "0")}`,
        title: `RUSTSEC-2026-${String(at).padStart(4, "0")}`, tags: [], kind: "vulnerability",
        modified_ms: 0, out_links: 0, in_links: 0, source: null, excerpt: "", folder: "" });
    }
    const seen = new Set();
    const edges = [];
    const join = (from, to, kind = "mentions") => {
      if (from === to || from >= nodes.length || to >= nodes.length) return;
      const key = `${from}>${to}`;
      if (seen.has(key)) return;
      seen.add(key);
      edges.push(spec.untyped ? { from, to } : { from, to, kind });
    };
    const lonely = (at) => at % 5 === 4;
    const nearest = (at, step) => {
      for (let ahead = 1; ahead <= pages; ahead += 1) {
        const seat = (at + ahead + step * 3) % Math.max(1, pages);
        if (!lonely(seat)) return seat;
      }
      return at;
    };
    const typedKinds = ["related", "implements", "depends_on", "supersedes", "contradicts"];
    if (spec.customEdges) {
      for (const e of spec.customEdges) join(e.from, e.to, e.kind);
    } else {
      for (let at = 0; at < pages; at += 1) {
        if (lonely(at)) continue;
        for (let step = 0; step < perPage; step += 1) join(at, nearest(at, step));
        if (at % 7 === 0) {
          const far = (at * 37 + 11) % Math.max(1, pages);
          if (!lonely(far)) join(at, far, typedKinds[(at / 7) % typedKinds.length]);
        }
        if (args.sources && at < Math.min(5, pages)) join(at, sourceFrom + at);
      }
      if (spec.allTyped) {
        for (let at = 0; at < typedKinds.length; at += 1) {
          join(at, Math.max(0, pages - at - 1), typedKinds[at]);
        }
      }
      for (let at = 0; at < ghosts; at += 1) {
        let from = at % Math.max(1, pages);
        for (let ahead = 0; ahead < pages && lonely(from); ahead += 1) {
          from = (from + 1) % Math.max(1, pages);
        }
        join(from, ghostFrom + at);
      }
      for (let at = 0; at < components; at += 1) join(0, componentFrom + at);
      for (let at = 0; at < vulnerabilities; at += 1) {
        join(vulnerabilityFrom + at, componentFrom + (at % Math.max(1, components)));
      }
    }
    for (const edge of edges) {
      nodes[edge.from].out_links += 1;
      nodes[edge.to].in_links += 1;
    }
    const counted = new Map();
    for (const node of nodes) {
      for (const tag of node.tags) counted.set(tag, (counted.get(tag) ?? 0) + 1);
    }
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
      vault: args.path,
      graph: {
        pages,
        ghosts,
        orphans: lint.orphans.length,
        lint,
        tags: [...counted].map(([tag, count]) => ({ tag, count }))
          .sort((left, right) => right.count - left.count || left.tag.localeCompare(right.tag)),
        kinds: spec.untyped ? [] : kinds,
        nodes,
        edges,
      },
      scanned_ms: 4,
      /* 라이브 층은 시험이 준 그대로 — 창 하네스의 같은 픽스처와 같은 손(t-4140 S4). */
      ...(spec.live ? { live: spec.live } : {}),
    };
  };
  /* 공급망의 답(`supply_chain_graph`, docs/design/knowledge-supply-chain-20260917.md §5.3) —
     필드 이름·값 낱말·정렬(구성요소 id순, 취약점 심각도순 → id순, 선 kind → from → to순)을
     계약 그대로 짓는다. 창이 계약의 무엇을 읽는지 시험하려면 픽스처가 계약 밖의 모양을
     가지면 안 된다.
       members       멤버 이름 — `npm:`으로 시작하면 npm 루트(판 없음), 아니면 Cargo 경로 멤버
       deps          레지스트리 구성요소 수(다섯째마다 npm)
       direct        멤버마다 직접 의존 수(제 생태계의 앞쪽부터)
       fanout        레지스트리 구성요소마다 의존 수(이진 나무: 풀 자리 p → 2p+1, 2p+2)
       unreachable   아무도 의존하지 않는 레지스트리 구성요소 수(풀 밖에 덧붙음)
       vulnerabilities  취약점 수 — 심각도는 계약의 여섯 낱말을 돌고, 영향은 풀의 깊은 쪽
       names         이름을 손으로 준 레지스트리 구성요소(풀 자리 → 이름) — 유령 링크 시험의 길
       lookup        `Lookup`에 덮을 칸들 */
  const SUPPLY_SEVERITIES = ["critical", "high", "medium", "low", "none", "unknown"];
  const SUPPLY_SCORES = { critical: 9.8, high: 7.5, medium: 5.3, low: 3.1, none: 0, unknown: null };
  window.__buildSupplyAnswer__ = (args, spec) => {
    const members = spec.members ?? ["app-core", "app-shell", "npm:ui"];
    const deps = spec.deps ?? 0;
    const direct = spec.direct ?? 2;
    const fanout = spec.fanout ?? 2;
    const pad = (value) => String(value).padStart(4, "0");
    const rows = [];
    for (const name of members) {
      const npm = name.startsWith("npm:");
      const bare = npm ? name.slice(4) : name;
      rows.push(npm
        ? { id: `pkg:npm/${bare} (private)`, purl: `pkg:npm/${bare}`, ecosystem: "npm", name: bare, version: "",
          origin: "private", source: null, member: true, lockfiles: ["package-lock.json"] }
        : { id: `pkg:cargo/${bare}@0.1.0 (path)`, purl: `pkg:cargo/${bare}@0.1.0`, ecosystem: "cargo", name: bare,
          version: "0.1.0", origin: "path", source: null, member: true, lockfiles: ["Cargo.lock"] });
    }
    const pools = { cargo: [], npm: [] };
    const registry = (at) => {
      const ecosystem = at % 5 === 4 ? "npm" : "cargo";
      const name = spec.names?.[at] ?? `dep-${pad(at)}`;
      const version = `1.${at % 7}.${at % 11}`;
      const purl = `pkg:${ecosystem}/${name}@${version}`;
      return { id: purl, purl, ecosystem, name, version, origin: "registry", source: null, member: false,
        lockfiles: [ecosystem === "npm" ? "package-lock.json" : "Cargo.lock"] };
    };
    for (let at = 0; at < deps; at += 1) {
      const row = registry(at);
      rows.push(row);
      pools[row.ecosystem].push(row);
    }
    const strays = [];
    for (let at = 0; at < (spec.unreachable ?? 0); at += 1) {
      const row = registry(deps + at);
      rows.push(row);
      strays.push(row);
    }
    const links = [];
    /* 멤버의 직접 의존은 제 생태계의 풀에서, 같은 생태계의 멤버 순서대로 이어 붙인 자리다 —
       풀의 앞자리(나무의 뿌리들)가 모두 멤버에 닿아 뜻하지 않은 「닿지 않는 구성요소」가 없다. */
    const rankIn = { cargo: 0, npm: 0 };
    members.forEach((name, at) => {
      const ecosystem = name.startsWith("npm:") ? "npm" : "cargo";
      const pool = pools[ecosystem];
      const rank = rankIn[ecosystem];
      rankIn[ecosystem] += 1;
      for (let step = 0; step < direct && pool.length > 0; step += 1) {
        links.push([rows[at], pool[(rank * direct + step) % pool.length]]);
      }
    });
    for (const pool of Object.values(pools)) {
      pool.forEach((row, at) => {
        for (let step = 1; step <= fanout; step += 1) {
          const child = pool[at * fanout + step];
          if (child !== undefined) links.push([row, child]);
        }
      });
    }
    const components = [...rows].sort((left, right) => (left.id < right.id ? -1 : left.id > right.id ? 1 : 0));
    const seat = new Map(components.map((row, at) => [row, at]));
    const vulnerabilities = [];
    const affects = [];
    const deep = [...pools.cargo].reverse();
    for (let at = 0; at < (spec.vulnerabilities ?? 0) && deep.length > 0; at += 1) {
      const severity = SUPPLY_SEVERITIES[at % SUPPLY_SEVERITIES.length];
      const target = deep[(at * 3) % deep.length];
      const id = `RUSTSEC-2026-${pad(at)}`;
      vulnerabilities.push({ row: { id, aliases: [`CVE-2026-${pad(at)}`, `GHSA-test-${pad(at)}`],
        summary: `Test advisory ${at} in ${target.name}`, severity, score: SUPPLY_SCORES[severity],
        fixed: [{ ecosystem: target.ecosystem, name: target.name, version: `9.${at}.0` }],
        informational: severity === "unknown" && at % 12 === 11 ? "unmaintained" : null,
        url: `https://osv.dev/vulnerability/${id}` }, target });
    }
    vulnerabilities.sort((left, right) => SUPPLY_SEVERITIES.indexOf(left.row.severity)
      - SUPPLY_SEVERITIES.indexOf(right.row.severity) || (left.row.id < right.row.id ? -1 : 1));
    vulnerabilities.forEach((held, at) => affects.push({ from: at, to: seat.get(held.target), kind: "affects" }));
    const edges = [
      ...links.map(([from, to]) => ({ from: seat.get(from), to: seat.get(to), kind: "depends_on" })),
      ...affects,
    ].sort((left, right) => (left.kind < right.kind ? -1 : left.kind > right.kind ? 1 : 0)
      || left.from - right.from || left.to - right.to);
    const lockfiles = [
      { path: "Cargo.lock", ecosystem: "cargo", componentCount: components.filter((row) => row.ecosystem === "cargo").length, unreadable: null },
      { path: "package-lock.json", ecosystem: "npm", componentCount: components.filter((row) => row.ecosystem === "npm").length, unreadable: null },
    ];
    return {
      root: spec.root ?? args?.root ?? "/repo",
      lockfiles,
      components,
      vulnerabilities: vulnerabilities.map((held) => held.row),
      edges,
      lookup: { state: "fresh", reason: null, checkedAt: 1789628477294,
        asked: components.filter((row) => row.origin === "registry").length, batches: 1,
        records: vulnerabilities.length, elapsedMs: 812, ...(spec.lookup ?? {}) },
    };
  };
  /* 설정 문서의 한 칸(`second_brain_scenes`: 볼트 → JSON 한 줄). 백엔드처럼 볼트의
     줄을 통째로 갈고 스냅샷을 돌려준다. 창을 새로 읽어도(page.reload) 남도록
     sessionStorage에 든다 — 디스크의 설정 파일이 서는 자리다. */
  const sceneLines = () => JSON.parse(sessionStorage.getItem("__KNOWLEDGE_SCENE_LINES__") ?? "{}");
  window.__SCENE_LINES__ = sceneLines;
  /* 탐색의 마지막 자리(t-4140 S2): 시야와 같은 꼴의 문, 볼트 → JSON 한 줄. */
  const exploreLines = () => JSON.parse(sessionStorage.getItem("__KNOWLEDGE_EXPLORE_LINES__") ?? "{}");
  window.__EXPLORE_LINES__ = exploreLines;
  window.__ANSWER__ = {
    boot: () => boot,
    set_second_brain_scenes: ({ vault, scenes }) => {
      const lines = sceneLines();
      lines[vault] = scenes.trim();
      sessionStorage.setItem("__KNOWLEDGE_SCENE_LINES__", JSON.stringify(lines));
      return { second_brain_scenes: lines };
    },
    set_second_brain_explore: ({ vault, explore }) => {
      const lines = exploreLines();
      if (explore.trim() === "" || explore.trim() === "{}") delete lines[vault];
      else lines[vault] = explore.trim();
      sessionStorage.setItem("__KNOWLEDGE_EXPLORE_LINES__", JSON.stringify(lines));
      return { second_brain_explore: lines };
    },
    detected_agents: () => [],
    read_text_file: (args) => {
      if (args.path?.startsWith("/vault/")) {
        return { text: `# ${args.path}\n\n[[Page-0001]]\n`, version: "1:1" };
      }
      return { text: "", version: "1:1" };
    },
    second_brain_page: (args) => {
      window.__GRAPH_PAGE_ASKED__ = args;
      return `/vault/${args.id}`;
    },
    /* 관계 쓰기 문(t-4140 S5): 호출을 적고 쓴 줄을 답한다 — 볼트는 픽스처라 바뀌지 않는다. */
    second_brain_relate: (args) => {
      (window.__RELATE__ ??= []).push(args);
      return { page: args.from, target: args.to, kind: args.kind, removed: args.remove === true, changed: true,
        line: `${args.kind}: ["[[${args.to}]]"]` };
    },
    second_brain_status: () => ({
      saved_path: "/vault",
      vaults: [{ path: "/vault", name: "vault", exists: true, is_configured: true }],
      vault: { path: "/vault", name: "vault", valid: true, wiki_pages: 12, raw_documents: 0, unindexed: 0, agent_guides: [] },
      obsidian_installed: true,
      guides: [],
    }),
    /* 공급망의 문(§5.3). 물은 인자를 적고, 시험이 원하면 답을 붙들거나(`__SUPPLY_HOLD__` —
       「확인 중」의 줄을 읽는 길) 문자열 오류로 거절한다(`__SUPPLY_FAIL__`, 명령의 `Err(String)`). */
    supply_chain_graph: (args) => {
      (window.__SUPPLY_ASKED__ ??= []).push(args ?? {});
      if (window.__SUPPLY_FAIL__) return Promise.reject(window.__SUPPLY_FAIL__);
      const answer = window.__buildSupplyAnswer__(args, window.__SUPPLY__ ?? { members: [], deps: 0 });
      if (!window.__SUPPLY_HOLD__) return answer;
      return new Promise((done) => {
        window.__SUPPLY_RELEASE__ = () => done(answer);
      });
    },
    second_brain_graph: (args) => {
      window.__GRAPH_ASKS__ = (window.__GRAPH_ASKS__ ?? 0) + 1;
      window.__GRAPH_ASKED__ = args;
      const built = window.__buildVaultGraph__(args, window.__VAULT__ ?? { pages: 0 });
      window.__GRAPH_ANSWERED_AT__ = performance.now();
      return built;
    },
    quick_commands: () => window.__QUICK__ ?? [],
    list_quick_commands: () => window.__QUICK__ ?? [],
    launch_agent_tab: (args) => { window.__LAUNCHED__ = args; return (window.__NEXT_TERM__ = (window.__NEXT_TERM__ ?? 0) + 1); },
    launch_agent: (args) => { window.__LAUNCHED__ = args; return "ok"; },
  };
  /* 창을 새로 읽는 마지막 케이스만 부팅 보고를 받는다 — 앞의 케이스들은 보고 없이
     선 판에서 재어 왔고, 그 판을 바꾸지 않는다. 보고는 설정 문서를 납작하게 편 것이다. */
  if (sessionStorage.getItem("__KNOWLEDGE_REBOOT__") === "1") {
    window.__ANSWER__.boot_report = () => ({
      ...boot,
      second_brain_vault: "/vault",
      second_brain_scenes: sceneLines(),
      second_brain_explore: exploreLines(),
    });
  }
  window.__TAURI__ = {
    core: {
      invoke: (cmd, args) => Promise.resolve(window.__ANSWER__[cmd] ? window.__ANSWER__[cmd](args) : (boot[cmd] ?? null)),
    },
    event: {
      listen: (name, cb) => {
        (window.__LISTENERS__[name] = window.__LISTENERS__[name] ?? []).push(cb);
        return Promise.resolve(() => {});
      },
      emit: () => Promise.resolve(),
    },
  };
}, BOOT);
