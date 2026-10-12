/**
 * A session turn that failed, through the real ConversationView (the phase-flow fixture, its
 * second request failed): the calm error row, its Details, Try again, and the notice that no
 * longer repeats the error above the composer. A bare ErrorState sits under it. `?drive=1`
 * writes what it saw into `#turn-error-result`; requests never reach a daemon.
 */
// oxlint-disable-next-line import/no-unassigned-import
import "@/fixtures/flow";
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ErrorState } from "@/components/assistant-ui/elements/error-state";
import type { Notice, Request } from "@/ipc/generated";
import { useBoard } from "@/state/board";

const RAW = 'API Error: 500 {"type":"error","error":{"type":"api_error","message":"Internal server error"}}';
const OTHER = "The sandbox isn't available here: commands run without it.";

const calls: Request[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  calls.push(req);
  // A sent message stays on its way: the fixture only records it.
  if (req.method === "sendMessage") return new Promise(() => {});
  if (req.method === "getRunDiff") return { method: req.method, diff: null };
  if (req.method === "getPullRequest") return { method: req.method, pullRequest: null };
  if (req.method === "listFiles") return { method: req.method, files: [], truncated: false };
  return { method: req.method };
});

// The second request failed, and the run's error and an old notice repeat its words (as data
// stored before the daemon said it only once).
useBoard.setState(({ board }) => {
  if (!board) return {};
  const failed = board.requests["r2"]!;
  const notices: Notice[] = [
    { level: "info", text: OTHER, atMs: Date.now() - 20_000 },
    { level: "warning", text: RAW, atMs: Date.now() - 10_000 },
  ];
  return {
    board: {
      ...board,
      requests: { ...board.requests, r2: { ...failed, state: { type: "failed", error: RAW }, endedAtMs: failed.startedAtMs + 34_000 } },
      run: "failed",
      runError: RAW,
      runRequest: null,
      notices,
    },
  };
});

const bare = document.createElement("div");
bare.className = "bg-background text-foreground w-160 p-4";
document.body.append(bare);
createRoot(bare).render(<ErrorState data-fixture="bare" summary="The search failed." />);

/** Painted red: a destructive colour applied as is, not a state variant like `aria-invalid:`. */
const red = (element: Element) => /(^|\s)(text|bg|border)-destructive/.test(element.getAttribute("class") ?? "");

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
  const details = row.querySelector<HTMLDetailsElement>('[data-slot="error-state-details"]');
  const seen: Record<string, unknown> = {
    header: header?.textContent ?? null,
    headerDestructive: !!header && red(header),
    rows: block.querySelectorAll('[data-slot="error-state"]').length,
    destructive: red(row) || [...row.querySelectorAll("*")].some(red),
    summary: row.querySelector('[data-slot="error-state-summary"]')?.textContent ?? null,
    detailsOpen: details?.open ?? null,
    rawShownBefore: document.body.innerText.includes("Internal server error"),
    notices: [...document.querySelectorAll('ul[aria-label="Notices"] > li')].map((item) => item.textContent),
  };
  details?.querySelector("summary")?.click();
  await pause();
  seen.rawShownAfter = row.innerText.includes("Internal server error");
  seen.copy = details?.querySelector("button")?.textContent ?? null;
  const again = [...row.querySelectorAll("button")].find((button) => button.textContent?.includes("Try again"));
  seen.tryAgain = !!again;
  again?.click();
  await pause(300);
  seen.sent = calls.flatMap((call) => (call.method === "sendMessage" ? [call.text] : []));
  const plain = document.querySelector<HTMLElement>('[data-fixture="bare"]');
  seen.bare = {
    text: plain?.innerText ?? null,
    details: plain?.querySelectorAll("details").length ?? null,
    buttons: plain?.querySelectorAll("button").length ?? null,
  };
  const result = document.createElement("pre");
  result.id = "turn-error-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

if (new URLSearchParams(location.search).has("drive")) void drive();
