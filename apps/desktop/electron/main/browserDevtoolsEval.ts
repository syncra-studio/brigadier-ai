// An agent's eval verb: the script the user read on its card runs as the page's own script, in the page's main world
// and top frame, so it can do what the page can and nothing more: no protocol, no debugger, no world of ours. What
// comes back goes through section 3's scrubbing, with the page looked at before and after, as a script may fill a field.
import { Script } from "node:vm";

import type { WebContents } from "electron";

import { agentScriptBody, sweptAndScrubbed } from "./browserAgentSecrets";

/** Under the daemon's wait for the app (browser_tool.rs's CALL_TIMEOUT). */
const SCRIPT_WAIT_MS = 30_000;
const ANSWER_MOST = 20_000;

const parses = (code: string): boolean => {
  try {
    void new Script(code);
    return true;
  } catch {
    return false;
  }
};

/** The script as what an async function does: an expression's value, or statements' `return`. Null when it isn't JavaScript. */
function bodyOf(script: string): string | null {
  if (parses(`(async () => (${script}\n));`)) return `return (${script}\n);`;
  if (parses(`(async () => {${script}\n});`)) return `${script}\n`;
  return null;
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

async function run(guest: WebContents, script: string): Promise<{ text: string } | { error: string }> {
  const body = bodyOf(script);
  if (body === null) return { error: "That isn't JavaScript this can run: it didn't parse as an expression or as statements. Nothing ran." };
  const code = `(async () => { const __answer = await (async () => {${body}})(); return (${agentScriptBody})({ kind: "evalAnswer", value: __answer }); })()`;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<{ error: string }>((resolve) => {
    timer = setTimeout(() => resolve({ error: `The script hadn't finished after ${SCRIPT_WAIT_MS / 1000} seconds. It may still be running in the page.` }), SCRIPT_WAIT_MS);
  });
  try {
    return await Promise.race([guest.executeJavaScript(code) as Promise<{ text: string }>, late]);
  } catch (error) {
    return { error: `The script threw: ${messageOf(error)}` };
  } finally {
    clearTimeout(timer);
  }
}

export function evalOnPage(guest: WebContents, script: string): Promise<{ text: string; url: string } | { error: string }> {
  return sweptAndScrubbed(guest, async () => {
    const answer = await run(guest, script);
    const url = guest.isDestroyed() ? "" : guest.getURL();
    return sweptAndScrubbed(guest, () => {
      if ("error" in answer) return answer;
      const whole = answer.text;
      const text = whole.length > ANSWER_MOST ? `${whole.slice(0, ANSWER_MOST)}\n(Cut: the answer was ${whole.length} characters.)` : whole;
      return { text, url };
    });
  });
}
