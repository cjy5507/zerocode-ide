/* 발행 레인이 제 `status.json`에 적는 단계 목록(`plan`, t-83513)을 레인의 표에서 그대로 읽는다 —
 * 장면의 레인이 진짜 레인과 같은 말을 하게. 창은 단계 이름의 표를 들고 있지 않으므로, 시험이
 * 이름을 따로 적으면 창이 아니라 시험이 레인의 말을 지어내는 셈이다.
 *
 * `tools/release/lane.sh`의 `PHASE_LANGS`와 `PHASE_WORDS`(한 줄에 `이름|말마다 이름`)를 읽어
 * `lane.sh --plan`이 내는 모양 그대로 돌려준다: [{ name, words: { ko, en, … } }, …]. */
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const LANE = fileURLToPath(new URL("../../tools/release/lane.sh", import.meta.url));

export function lanePlan() {
  const lane = readFileSync(LANE, "utf8");
  const langs = /^PHASE_LANGS="([^"]+)"$/m.exec(lane)?.[1].split(/\s+/) ?? [];
  const table = /^PHASE_WORDS='([^']+)'$/m.exec(lane)?.[1] ?? "";
  return table.split("\n").filter(Boolean).map((line) => {
    const [name, ...words] = line.split("|");
    return { name, words: Object.fromEntries(langs.map((lang, at) => [lang, words[at]])) };
  });
}
