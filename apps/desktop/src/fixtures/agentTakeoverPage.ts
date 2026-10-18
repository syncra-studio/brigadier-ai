/**
 * A Browser tab's user-input script (src-tauri/src/browser_user_input.js) on a test page: the
 * user's own clicks, keys and scrolls tell the app, at most once a second; a page's scripts and
 * the browser tool's own clicks and typing never do. Writes what it saw to
 * #agent-takeover-result (AgentTakeoverRender.test.ts reads it).
 */
import agentSource from "../../src-tauri/src/browser_agent.js?raw";
import source from "../../src-tauri/src/browser_user_input.js?raw";
import { fixtureClick, fixtureInput } from "./input";

const posted: string[] = [];
(window as unknown as { ipc: { postMessage: (message: string) => void } }).ipc = { postMessage: (message) => posted.push(message) };
// oxlint-disable-next-line no-eval
(0, eval)(source);
// oxlint-disable-next-line no-eval
const agent = (op: Record<string, unknown>) => (0, eval)(`(${agentSource.trim().replace(/;$/, "")})(${JSON.stringify(op)})`) as unknown;
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const button = document.getElementById("add")!;

const seen: Record<string, unknown> = {};
// A page's own events, and the browser tool working the page.
button.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
document.dispatchEvent(new KeyboardEvent("keydown", { key: "a", bubbles: true }));
window.dispatchEvent(new WheelEvent("wheel", { deltaY: 100 }));
button.click();
agent({ kind: "read", max: 20000 });
const ref = (id: string) => document.getElementById(id)?.getAttribute("data-brigadier-ref") ?? "";
seen.agentClick = agent({ kind: "click", target: ref("add") });
seen.agentType = agent({ kind: "type", target: ref("name"), text: "Ada", submit: true });
seen.scripts = [...posted];

/** Waits for the `count`th message, 5 s at most (a busy machine hands input on late). */
const posts = async (count: number) => {
  for (let tries = 0; tries < 100 && posted.length < count; tries++) await wait(50);
  return [...posted];
};

await fixtureClick(button);
const clicked = Date.now();
seen.click = await posts(1);
// Within the second: no more.
await fixtureInput({ type: "key", key: "a" });
await wait(100);
seen.soon = { posted: [...posted], within: Date.now() - clicked < 1000 };
await wait(1100);
await fixtureInput({ type: "wheel", x: 100, y: 200, deltaY: 300 });
seen.wheel = await posts(posted.length + 1);
await wait(1100);
await fixtureInput({ type: "key", key: "b" });
seen.key = await posts(posted.length + 1);

document.getElementById("agent-takeover-result")!.textContent = JSON.stringify(seen);
