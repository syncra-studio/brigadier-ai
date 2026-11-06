/**
 * A session turn that failed, through the real ConversationView (the phase-flow fixture, its
 * second request failed): the calm error row, its Details, Try again, and the notice that no
 * longer repeats the error above the composer. A bare ErrorState sits under it. `?drive=1`
 * writes what it saw into `#turn-error-result`; requests never reach a daemon. With
 * `?interrupted=1` the turn was cut off by Brigadier quitting instead (UI audit H1). With
 * `?launch=1` its CLI never began it (its launch failed): the CLI's thread is read and has no
 * turn of it, and Try again answers the request again instead of sending "Try again." (#660).
 */
// oxlint-disable-next-line import/no-unassigned-import
import "@/fixtures/flow";
import { mockIPC } from "@/ipc/nativeMocks";
import { createRoot } from "react-dom/client";

import { ErrorState } from "@/components/conversation";
import type { Notice, Request, ThreadView } from "@/ipc/generated";
import { useBoard } from "@/state/board";
import { useCliThread } from "@/state/cliThread";

const INTERRUPTED = new URLSearchParams(location.search).has("interrupted");
const LAUNCH = new URLSearchParams(location.search).has("launch");
/** The CLI's thread with no turn yet: none of the session's messages ever reached it. */
const NOTHING_HEARD: ThreadView = { entries: [], turns: [] };
const RAW = INTERRUPTED
  ? "Brigadier quit while this was running."
  : 'API Error: 500 {"type":"error","error":{"type":"api_error","message":"Internal server error"}}';
const OTHER = "The sandbox isn't available here: commands run without it.";

const calls: Request[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  calls.push(req);
  // A sent message stays on its way: the fixture only records it.
  if (req.method === "sendMessage") return new Promise(() => {});
  if (req.method === "getPullRequest") return { method: req.method, pullRequest: null };
  if (req.method === "listFiles") return { method: req.method, files: [], truncated: false };
  if (LAUNCH && req.method === "threadView") return { method: req.method, view: NOTHING_HEARD };
  return { method: req.method };
});

// The second request failed, and the run's error and an old notice repeat its words (as data
// stored before the daemon said it only once).
useBoard.setState(({ board }) => {
  if (!board) return {};
  const failed = board.requests["r2"]!;
  const ended = { ...failed, state: { type: "failed" as const, error: RAW }, endedAtMs: failed.startedAtMs + 34_000 };
  // As the daemon has it, a request's id is its user message's (`m10` here): the CLI never
  // having had the message, its block is that message's.
  const { r2: _launched, ...others } = board.requests;
  const notices: Notice[] = [
    { level: "info", text: OTHER, atMs: Date.now() - 20_000 },
    { level: "warning", text: RAW, atMs: Date.now() - 10_000 },
  ];
  return {
    board: {
      ...board,
      requests: LAUNCH ? { ...others, m10: { ...ended, id: "m10" } } : { ...board.requests, r2: ended },
      run: INTERRUPTED ? "idle" : "failed",
      runError: INTERRUPTED ? null : RAW,
      runRequest: null,
      notices,
    },
  };
});

if (LAUNCH) useCliThread.setState((state) => ({ views: { ...state.views, "flow-session": NOTHING_HEARD } }));

const bare = document.createElement("div");
bare.className = "bg-background text-foreground w-160 p-4";
document.body.append(bare);
createRoot(bare).render(<ErrorState data-fixture="bare" summary="The search failed." />);

/** Painted red: a destructive colour applied as is, not a state variant like `aria-invalid:`. */
const red = (element: Element) => /(^|\s)(text|bg|border)-destructive/.test(element.getAttribute("class") ?? "");

/** A quiet line of the thread: no box of any kind, opened or not. */
const unboxed = (element: Element | null) => element && !/(^|\s)(bg-|border|rounded-|p[xy]?-)/.test(element.getAttribute("class") ?? "");

const pause = (ms = 100) => new Promise((resolve) => setTimeout(resolve, ms));

async function drive() {
  let row: HTMLElement | null = null;
  for (let tries = 0; tries < 80 && !row; tries += 1) {
    await pause();
    row = [...document.querySelectorAll<HTMLElement>('[data-role="assistant"] [data-slot="error-state"]')].at(-1) ?? null;
  }
  if (!row) throw new Error("No error row rendered");
  const block = row.closest<HTMLElement>('[data-role="assistant"]')!;
  const header = block.querySelector<HTMLElement>('[data-slot="request-work-label"]');
  const summary = row.querySelector('[data-slot="error-state-summary"]');
  const details = row.querySelector<HTMLElement>('[data-slot="error-state-details"]');
  const seen: Record<string, unknown> = {
    header: header?.textContent ?? null,
    headerDestructive: !!header && red(header),
    rows: block.querySelectorAll('[data-slot="error-state"]').length,
    // Only the sentence takes the error colour.
    destructive: red(row) || [...row.querySelectorAll('*:not([data-slot="error-state-summary"])')].some(red),
    summaryRed: !!summary && red(summary),
    summary: summary?.textContent ?? null,
    unboxed: unboxed(row),
    detailsOpen: details ? details.getAttribute("aria-expanded") === "true" : null,
    rawShownBefore: document.body.innerText.includes("Internal server error"),
    notices: [...document.querySelectorAll('[aria-label="Notices"] li[data-level] [data-slot="notice-text"]')].map((item) => item.textContent),
    // One tray on the composer's rail (#506): the notices' card is a rail card, its rows inside it.
    tray: document.querySelector('[data-slot="composer-rail"] > [data-slot="composer-rail-item"][aria-label="Notices"]')?.getAttribute("data-variant") ?? null,
  };
  details?.click();
  await pause();
  const raw = row.querySelector('[data-slot="error-state-raw"]');
  seen.rawUnboxed = unboxed(raw);
  seen.rawShownAfter = row.innerText.includes("Internal server error");
  seen.copy = raw?.querySelector("button")?.textContent ?? null;
  seen.buttons = [...row.querySelectorAll("button")].map((button) => button.textContent?.trim());
  const again = [...row.querySelectorAll("button")].find((button) => button.textContent?.includes(INTERRUPTED ? "Continue" : "Try again"));
  seen.tryAgain = !!again;
  again?.click();
  await pause(300);
  seen.sent = calls.flatMap((call) => (call.method === "sendMessage" ? [call.text] : []));
  seen.resumed = calls.filter((call) => call.method === "resume").length;
  seen.regenerated = calls.flatMap((call) => (call.method === "regenerate" ? [call.requestId] : []));
  const plain = document.querySelector<HTMLElement>('[data-fixture="bare"]');
  seen.bare = {
    text: plain?.innerText ?? null,
    details: plain?.querySelectorAll('[data-slot="error-state-details"]').length ?? null,
    buttons: plain?.querySelectorAll("button").length ?? null,
  };
  const result = document.createElement("pre");
  result.id = "turn-error-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

if (new URLSearchParams(location.search).has("drive")) void drive();
