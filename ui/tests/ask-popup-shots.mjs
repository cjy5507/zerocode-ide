/* The ask popup, photographed — one picture per kind of question the window
 * puts to the person, taken from the window harness with fake asks.
 *
 *   node ui/tests/ask-popup-shots.mjs <folder> [--variant before|after] [--theme dark|light]
 *
 * Nothing here reads the popup's markup: it fires the same frames and events
 * the backend does and takes the whole window, so the same script draws the
 * window as it stood (`before`: the question is a tool prompt whose tool is
 * the classifier's decline) and as it stands (`after`: the frame says what
 * kind of ask it is). A picture of the window is the only proof that a
 * question is where the eye is; compiling proves nothing about it. */
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

const folder = resolve(process.argv[2] ?? "output/ask-popup");
const option = (name, fallback) =>
  process.argv.includes(name) ? process.argv[process.argv.indexOf(name) + 1] : fallback;
const variant = option("--variant", "after");
const theme = option("--theme", "dark");
await mkdir(folder, { recursive: true });

const { files, origin } = await createWindowServer();
const browser = await chromium.launch();

/* The frames as zo writes them. The tool prompt is what it always was; the
 * question is the ladder's, in the two shapes the window has seen. */
const TOOL_FRAME = {
  type: "permission_prompt",
  prompt_id: 101,
  tool_name: "bash",
  reasoning: "git status를 실행해 작업 트리의 상태를 확인합니다.",
  audit_hint: "risk: medium; explicitly unblock with [y] Allow once",
  choices: [
    { key: "y", label: "한 번 허용", decision: "allow_once" },
    { key: "a", label: "항상 허용", decision: "allow_always" },
    { key: "n", label: "거부", decision: "deny" },
  ],
};
const QUESTION_TEXT = "이 요청은 지금 쓰는 모델의 안전 분류기가 거절했습니다. 다른 모델로 바꿔 다시 시도할까요?";
const QUESTION_CHOICES = [
  { key: "y", label: "모델 바꾸기", decision: "allow_once" },
  { key: "n", label: "그대로 두기", decision: "deny" },
];
const QUESTION_FRAME = variant === "before"
  ? {
      type: "permission_prompt",
      prompt_id: 102,
      tool_name: "safety-classifier decline",
      reasoning: QUESTION_TEXT,
      audit_hint: "risk: high; explicitly unblock with [y] 모델 바꾸기",
      choices: QUESTION_CHOICES,
    }
  : {
      type: "permission_prompt",
      prompt_id: 102,
      kind: "question",
      topic: "model_switch",
      title: "Switch models?",
      tool_name: "",
      reasoning: QUESTION_TEXT,
      audit_hint: null,
      choices: QUESTION_CHOICES,
    };
const CONFIRM = { id: "confirm-1", kind: "payment", label: "주문하기", verb: "mouse-click", timeoutMs: 120000 };
const HANDOFF = { id: "handoff-1", reason: "휴대폰으로 온 2FA 코드를 화면에 입력해 주세요.", timeoutMs: 600000 };
const COVER = { id: "handoff-2", reason: "", reasonKey: "computer.cover.ask", reasonArgs: { app: "Safari" }, timeoutMs: 600000 };

const shots = [];
const shoot = async (page, name) => {
  await page.waitForTimeout(450);
  const path = resolve(folder, `${variant}${theme === "dark" ? "" : `-${theme}`}-${name}.png`);
  await page.screenshot({ path });
  shots.push(path);
};

/* One page per picture: a picture never inherits what the last one raised. */
const fresh = async (setup) => {
  const { page, faults } = await openWindowTestPage(browser, origin);
  await page.evaluate((one) => {
    window.__ANSWER__.respond_permission = () => null;
    window.__ANSWER__.computer_confirm_answer = () => true;
    document.documentElement.dataset.theme = one;
  }, theme);
  await setup(page);
  return { page, faults };
};

const frame = (page, session, payload) => page.evaluate(([one, body]) => {
  for (const fire of window.__LISTENERS__["session:frame"] ?? []) fire({ payload: { session: one, frame: body } });
}, [session, payload]);
const event = (page, name, payload) => page.evaluate(([one, body]) => {
  for (const fire of window.__LISTENERS__[one] ?? []) fire({ payload: body });
}, [name, payload]);

const faultsSeen = [];
const take = async (name, setup) => {
  const { page, faults } = await fresh(async () => {});
  try {
    await setup(page);
    await shoot(page, name);
  } finally {
    faultsSeen.push(...faults);
    await page.close();
  }
};

await take("1-tool", (page) => frame(page, "s-tool", TOOL_FRAME));
await take("2-model-switch", (page) => frame(page, "s-zo", QUESTION_FRAME));
await take("3-computer-confirm", (page) => event(page, "computer:confirm", CONFIRM));
await take("4-computer-handoff", (page) => event(page, "computer:handoff", HANDOFF));
await take("5-computer-handoff-cover", (page) => event(page, "computer:handoff", COVER));
/* The project-switch question, as `mayLeaveLiveAgents` puts it: its own
 * words, through the window's one generic confirm. */
await take("6-generic-confirm", (page) => page.evaluate(() => {
  void askConfirm({
    title: t("project.leaveLiveTitle", "프로젝트를 옮길까요?"),
    body: t("project.leaveLiveBody", "에이전트 {{count}}개가 이 프로젝트에서 아직 일하고 있습니다. 옮겨도 계속 돌지만, 돌아올 때까지 이 창에서는 보이지 않습니다.", { count: 2 }),
    confirm: t("project.leaveLiveConfirm", "옮기기"),
    deny: t("app.cancel", "취소"),
    cancel: false,
  });
}));
/* Two asks at once: the oldest stands, the count says one waits behind it. */
await take("7a-two-waiting", async (page) => {
  await frame(page, "s-tool", TOOL_FRAME);
  await frame(page, "s-zo", QUESTION_FRAME);
});
await take("7b-three-waiting", async (page) => {
  await frame(page, "s-tool", TOOL_FRAME);
  await frame(page, "s-zo", QUESTION_FRAME);
  await event(page, "computer:confirm", CONFIRM);
});
await take("8-band-stays", (page) => event(page, "computer:activity", { active: true, verb: "mouse-click", actions: 3, stopped: null }));
await take("9-model-switch-en", async (page) => {
  await page.evaluate(() => {
    locale = "en";
    applyLocale();
  });
  await frame(page, "s-zo", QUESTION_FRAME);
});

await browser.close();
files.close();
console.log(JSON.stringify({ variant, shots, faults: faultsSeen }, null, 2));
process.exit(faultsSeen.length === 0 ? 0 : 1);
