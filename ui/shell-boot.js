/* The view layer, and only that (ADR 0002). Rust owns the grids, the VT state
 * machine, input encoding and the protocol; this file paints what it is sent
 * and reports intent. If logic wants to live here, it belongs in a crate
 * behind a Tauri command instead.
 *
 * Two surfaces paint cells under GridDelta's shift contract
 * (crates/zerocode-pty/src/grid.rs): the stage — the focused lane's real TUI,
 * unwrapped, tmux-style — and the floating shell terminal. The structured
 * channel (zo's SerializableRenderBlock frames) is signals only: row
 * states, the permission modal, status-bar numbers. Unknown frame types flow
 * past silently (제품 원칙 5). */

const { invoke, Channel: TauriChannel } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const TERMINAL_CLIPBOARD_WRITE_EVENT = "terminal:clipboard-write";

/* ---- 웹뷰 오류의 착륙장 (1-fm) ----
 *
 * A release webview has no console anybody can read, so a thrown listener
 * dies in silence and the symptom arrives as "안 보임" with nothing to hold
 * it to. Every uncaught error and rejection lands in the backend's own log
 * file instead (`note_webview_error`). Registered FIRST, before anything
 * that could throw at module scope — the crash this exists to catch is
 * exactly the one that would stop a later registration from running. */
for (const { kind, read } of [
  { kind: "error", read: (event) => `${event.message} @ ${event.filename}:${event.lineno}` },
  {
    kind: "unhandledrejection",
    read: (event) => String(event.reason?.stack ?? event.reason ?? "rejection"),
  },
]) {
  window.addEventListener(kind, (event) => {
    invoke("note_webview_error", { text: kind + ": " + read(event) }).catch(() => {});
  });
}

/* ---- 타이틀바가 손잡이다 ----
 *
 * `data-tauri-drag-region` 속성은 마크업에 있지만(1-dq) 실기기에서 조용히
 * 듣지 않았다 — 속성 방식은 웹뷰 내장 스크립트가 정확히 그 요소를 밟는
 * mousedown에만 반응하고, 실패해도 아무 말이 없다. 그래서 창을 끄는 일은
 * 여기서 직접 말한다: 왼쪽 버튼이 타이틀바의 드래그 표면을 누르면
 * `start_dragging`, 두 번이면 최대화 토글. 탭·버튼 같은 대화형 자식은
 * 제외 — 그 위의 누름은 그 컨트롤의 것이다.
 */
/* 왜 `event.detail`이 아닌가: `start_dragging`이 창을 넘기면 OS가 드래그를
 * 가져가고, 그 드래그의 mouseup은 이 웹뷰에 **영영 도착하지 않는다**. 그러면
 * WebKit의 클릭 카운터는 다음 누름을 더블클릭의 둘째 타로 세고, 두 번째
 * 드래그 시도가 전부 최대화 분기로 빠진다 — "처음엔 되는데 그다음부터 안
 * 된다"가 정확히 이것이다. 시계는 우리가 직접 찬다.
 *
 * 표식도 우리 것이다(`data-drag-surface`): `data-tauri-drag-region`은 웹뷰
 * 내장 핸들러도 듣는 이름이라, 실기기에서 그쪽이 간헐적으로 살아나면 같은
 * 제스처의 주인이 둘이 된다. 캡처 단계에서 듣는 이유도 같다 — 자식의
 * stopPropagation이 손잡이를 가릴 수 없어야 한다. */
/* 빗나간 누름은 드래그가 아니다 (여섯 번째 ＋ 실종, 2026-08-26). tabstrip은
 * 드래그 표면이면서 탭과 ＋ 버튼의 마당이라, 컨트롤을 몇 px 빗나간 누름이
 * `start_dragging`으로 창에 넘어가면 그 클릭은 mouseup째로 증발하고 —
 * 아무 일지도 없이 "눌렀는데 안 열려"가 된다 — 350ms 안의 재시도는 아래
 * 시계가 더블클릭으로 읽어 최대화 토글로 빠진다. 대화형 자식의 슬하
 * 몇 px은 흘려보내되, 다음 판독을 위해 어느 컨트롤 곁이었는지 남긴다. */
const GRIP_MISS_SLOP = 8;
function pressBesideControl(grip, x, y) {
  if (grip.id !== "tabstrip") return null;
  for (const child of grip.querySelectorAll("button, [role=\"tab\"], input, select, a")) {
    const box = child.getBoundingClientRect();
    if (
      x >= box.left - GRIP_MISS_SLOP && x <= box.right + GRIP_MISS_SLOP
      && y >= box.top - GRIP_MISS_SLOP && y <= box.bottom + GRIP_MISS_SLOP
    ) return child;
  }
  return null;
}
let gripPressedAt = 0;
document.addEventListener(
  "mousedown",
  (event) => {
    if (event.button !== 0) return;
    const grip = event.target.closest?.("[data-drag-surface]");
    if (!grip) return;
    if (event.target.closest("button, [role=\"tab\"], input, select, a")) return;
    const beside = pressBesideControl(grip, event.clientX, event.clientY);
    if (beside) {
      void invoke("log_window_error", {
        message: `tabstrip: bare press ${event.clientX},${event.clientY} beside `
          + `${beside.tagName.toLowerCase()}${beside.classList[0] ? `.${beside.classList[0]}` : ""}`
          + " — kept from drag",
      }).catch(() => {});
      return;
    }
    const now = Date.now();
    const again = now - gripPressedAt < 350;
    gripPressedAt = again ? 0 : now;
    const door = again ? "plugin:window|internal_toggle_maximize" : "plugin:window|start_dragging";
    void invoke(door).catch((error) => {
      // 조용한 실패가 이 손잡이의 원죄였다 — 안 되면 왜 안 되는지 남긴다.
      console.error(`titlebar ${again ? "maximize" : "drag"} failed`, error);
    });
  },
  true,
);


const el = (id) => document.getElementById(id);

/* 이 창의 라벨 — 백엔드가 이 창 앞으로 부치는 이름의 절반.
 *
 * 터미널 프레임을 당기러 오라는 알림은 `term:dirty:<label>`로 온다. 이벤트
 * 이름에 라벨이 들어가는 것은 Tauri가 **이름별 리스너 맵을 먼저 보고** 그 이름을
 * 듣지 않는 웹뷰는 아예 건너뛰기 때문이다(`emit_js_filter`) — `listen`이
 * `EventTarget::Any`로 등록되는 이상 `emit_to`의 필터로는 좁힐 수 없는 일을,
 * 이름이 대신 한다.
 *
 * **창이 아니라 웹뷰의 라벨이다.** Tauri는 둘을 따로 심고(`currentWindow`,
 * `currentWebview`), 이름을 거르는 쪽은 웹뷰 라벨로 키를 단다 —
 * `emit_js_filter`가 보는 것은 `js_listeners[webview.label()]`이고,
 * `set_watched_terms`·`term_pull`이 독자로 적는 것도 `webview.label()`이다. 지금
 * 이 앱의 모든 창은 `WebviewWindowBuilder`가 만들어 둘이 같은 글자지만, 한 창에
 * 웹뷰가 둘인 날이 오면 창 라벨로 부친 알림은 아무도 듣지 않는 이름으로 나가고 —
 * 조용하던 화면이 영영 깨어나지 않는, 이 파일이 막으려는 바로 그 고장이 된다.
 *
 * Tauri가 부팅 스크립트에 심어 주는 값이므로 여기서 한 번만 읽는다. 없으면
 * `main`: 이 창이 하나뿐인 것이 이 앱의 보통이다. 틀렸을 때의 벌은 알림을 놓치는
 * 것이다 — 선언 뒤의 당김과 흐르는 화면의 박자 당김은 알림 없이도 오지만, 조용한
 * 화면은 깨어날 이름을 잃는다. 그래서 읽는 곳은 Tauri 자신의 메타데이터뿐이다. */
const WINDOW_LABEL =
  window.__TAURI_INTERNALS__?.metadata?.currentWebview?.label
  ?? window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label
  ?? "main";


/* ---- 이 웹뷰가 무엇인가 ----
 *
 * 이 파일은 두 창에서 돈다. 메인 창과, 보드 하나만 담은 팝아웃 창
 * (`open_board_popout`, crates/zerocode-shell/src/main.rs). 두 창은 **같은
 * 문서와 같은 스크립트**를 싣고 질의 문자열로만 갈린다.
 *
 * 두 번째 파일을 만들지 않는 이유는 절약이 아니라 정확성이다. 카드 렌더러를
 * 복사하면 그 순간부터 두 보드는 서로 다르게 자라고, 어느 쪽이 맞는지 아무도
 * 모르게 된다. Orca도 같은 `AgentKanbanBoard`를 인-윈도우와 팝아웃 양쪽에서
 * 쓴다(스펙 §4d) — 다른 것은 기본 핸들러뿐이다. 여기서도 같다: 갈라지는 곳은
 * 부트 하나(`bootPopout`)와, 카드가 눌렀을 때 어디로 가느냐 둘뿐이다. */
const isPopout = new URLSearchParams(window.location.search).get("surface") === "popout";
// 선택자 하나로 크롬 전체를 접기 위해, 무엇보다 먼저 문서에 적는다 — 팝아웃이
// 사이드바를 한 프레임 그렸다가 지우는 것은 창이 자기를 고치는 모습이다.
if (isPopout) document.documentElement.dataset.surface = "popout";

/* The canonical `mod` key follows the operating system.
 *
 * Keep the platform decision in one place. On Apple keyboards `mod` is
 * Command; on Windows (and the other desktop targets) it is Control. Reading
 * the browser's platform is the only synchronous fact available before the
 * first shortcut can be pressed, and both WebKit and Chromium expose it in a
 * desktop webview. `userAgentData` is preferred where Chromium provides it;
 * `platform` keeps WebKit and older webviews on the same path. */
const reportedPlatform = navigator.userAgentData?.platform || navigator.platform || "";
const usesCommandModifier = /^(mac|iphone|ipad|ipod)/i.test(reportedPlatform);
const usesWindowsPlatform = /^win/i.test(reportedPlatform);
const usesLinuxPlatform = /^linux/i.test(reportedPlatform);

function hasPrimaryModifier(event) {
  return usesCommandModifier ? event.metaKey : event.ctrlKey;
}

function primaryModifierLabel() {
  return usesCommandModifier ? "⌘" : "Ctrl";
}

/* A default binding the original ships on macOS and NOWHERE ELSE.
 *
 * `platformBindings` in the original takes a table per platform, and one entry
 * uses it to say something this window has to say too — its own comment, at
 * `shared/keybindings.ts:551-553`: "macOS only — Windows Ctrl+Alt is AltGr and
 * Linux Ctrl+Alt+T is the desktop 'open terminal', so no safe default there."
 * Binding it anyway on those two would swallow a chord the platform already
 * owns, which is worse than shipping the command unbound: the palette still
 * reaches it, and the keybinding pane still lets somebody choose their own.
 *
 * `null` rather than `[]` because that is the spelling `chordsFor` and the
 * backend's registry already agree on for "no default". */
function darwinOnly(chord) {
  return usesCommandModifier ? chord : null;
}

/* Anything this window throws, said out loud on the process's own stderr.
 *
 * A webview that dies during boot dies quietly — the window is up, the panels
 * are drawn, and nothing on screen explains why the stage never filled. It is
 * the hardest class of bug to see from outside, so the window stops keeping
 * it to itself. */
function describeWindowFault(event) {
  if (event.reason !== undefined) return `unhandled rejection: ${event.reason}`;
  return `${event.message} (${event.filename}:${event.lineno})`;
}

for (const fault of ["error", "unhandledrejection"]) {
  window.addEventListener(fault, (event) => {
    try {
      window.__TAURI__.core.invoke("log_window_error", {
        message: describeWindowFault(event),
      });
    } catch {
      // Reporting must never be the thing that fails.
    }
  });
}
/* One idle-aware poller for every background beat this window keeps.
 *
 * A poller has a target — something on screen worth ticking for — and it
 * runs only while the window is visible and that target exists: the beat
 * stops when the last target goes and starts again when one appears, so an
 * empty or hidden window pays no timer round trip at all. Every beat used to
 * spell this out for itself (stop when nothing is there, guard on
 * `document.hidden`, re-check inside the tick, listen for visibility) — eight
 * copies of one rule is how the ninth forgets a clause. `sync()` is the only
 * verb: call it wherever the target set may have changed, and the poller
 * settles itself. `onResume` runs once when the window comes back with a
 * target standing, for the beats that owe an immediate look after an absence. */
function idlePoller({ wanted, every, tick, onResume }) {
  let handle = null;
  // Both spellings of "nobody is looking": `hidden` is the boolean, and
  // `visibilityState` is what a window that was never shown reports first.
  const away = () => document.hidden || document.visibilityState !== "visible";
  const stop = () => {
    if (handle === null) return;
    clearInterval(handle);
    handle = null;
  };
  const sync = () => {
    if (away() || !wanted()) {
      stop();
      return;
    }
    if (handle !== null) return;
    handle = window.setInterval(() => {
      if (away() || !wanted()) {
        stop();
        return;
      }
      tick();
    }, every);
  };
  document.addEventListener("visibilitychange", () => {
    sync();
    if (!away() && wanted()) onResume?.();
  });
  return { sync };
}

const tabstrip = el("tabstrip");
const stage = el("stage");
const placeholder = el("placeholder");
const attention = el("attention");
const attentionCount = el("attention-count");
const termFloat = el("term-float");
const keySink = el("key-sink");
const fileTree = el("file-tree");
const permScrim = el("perm-scrim");

