/* The GitHub macOS runner's browser, reproduced on any machine (t-21351).
 *
 * A browser answers some questions from its host, and the runner's host answers
 * differently from a developer's Mac: it offers WebGL2 in headless (a software
 * rasteriser, SwiftShader) and has the OS's Reduce Motion switch on (simulated here per context, below). A harness
 * that does not pin those two judges another page there, and nineteen checks of
 * the window suite went red on the runner while the same suite was whole here.
 *
 *   node --import ./ui/tests/runner-host.mjs ui/tests/window.mjs
 *   taskpolicy -b node --import ./ui/tests/runner-host.mjs ui/tests/window.mjs   (and slow cores)
 *
 * This file puts the runner's two answers in the browser's own command line
 * BEFORE the harness's flags, so what the harness fixes (`WINDOW_BROWSER_ARGS`)
 * still wins and what it leaves to the host is read as the runner reads it. A
 * check that passes under this and under no preload is honest on both hosts. */
import { chromium } from "./playwright-chromium.mjs";

/** The runner's host answer that is a Chromium switch: WebGL2 present. */
export const RUNNER_HOST_ARGS = Object.freeze([
  "--use-gl=angle",
  "--use-angle=swiftshader",
  "--enable-unsafe-swiftshader",
]);

/* Reduce Motion is the OS's, and Chromium has no switch that stands in for it (`--force-prefers-reduced-motion`
 * is not honoured by headless). What the host's answer means to a page is: a context that does not say otherwise
 * is reduced. So a context opened without a `reducedMotion` of its own — or with `null`, which Playwright
 * documents as "no override, ask the host" — gets "reduce", exactly as the runner's gets it from the OS; one that sets it (the harness's `WINDOW_MOTION_REST`) is left alone. */
const HOST_MOTION = "reduce";
const hostMotion = (options) => (options?.reducedMotion == null ? { ...options, reducedMotion: HOST_MOTION } : options);

const launch = chromium.launch.bind(chromium);
chromium.launch = async (options = {}) => {
  const browser = await launch({ ...options, args: [...RUNNER_HOST_ARGS, ...(options.args ?? [])] });
  const newContext = browser.newContext.bind(browser);
  const newPage = browser.newPage.bind(browser);
  browser.newContext = (contextOptions) => newContext(hostMotion(contextOptions));
  browser.newPage = (pageOptions) => newPage(hostMotion(pageOptions));
  return browser;
};
