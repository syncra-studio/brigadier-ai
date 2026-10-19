/**
 * A standalone Vite entry, excluded from the app entry and production build: the card under an
 * answer whose files wait for review. IPC is mocked, no daemon. The result goes to
 * `<pre id="turn-diff-review-result">`; `stop=1` holds the page for a screenshot, `still=1` shows the first state only.
 *
 * - Waiting: "3 Files", "+a −d · 2 to review", Undo All, Keep All and Review Changes; each row
 *   with Undo and Keep while it waits, and "Kept" once kept.
 * - Review Changes opens the first waiting file's tab; a row's Keep keeps that file; Undo All
 *   undoes the rest.
 * - Done: "Edited 3 files", "· kept 2, undid 1", View changes only.
 * - Merged while two files waited: "Edited 3 files", "· kept 1", View changes only, those rows
 *   with no word (the merge says it once).
 * - A file put back as it was after the merge (#333), which the request's own diff leaves out:
 *   "4 Files", "· 1 to review", its row with Undo and Keep; alone, "1 File", "· 1 to review".
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { TurnDiff } from "@/app/conversation/TurnDiff";
import { ViewContext } from "@/app/conversation/viewContext";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, FileReview, Request, UserRequest } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const CHAT = "turn-diff-review";

let review: Record<string, FileReview> = { "src/a.ts": "pending", "src/b.ts": "kept", "src/c.ts": "pending" };
const asked: string[] = [];
const FILES = [
  { path: "src/a.ts", insertions: 3, deletions: 1, binary: false },
  { path: "src/b.ts", insertions: 1, deletions: 1, binary: false },
  { path: "src/c.ts", insertions: 2, deletions: 0, binary: false },
];
let files = FILES;

function show() {
  const request = {
    id: "r1", conversationId: CHAT, preview: "Tidy", state: { type: "done" }, startedAtMs: 0, endedAtMs: 1, review,
    changes: {
      settled: true,
      stat: {
        files,
        insertions: files.reduce((sum, file) => sum + file.insertions, 0),
        deletions: files.reduce((sum, file) => sum + file.deletions, 0),
      },
    },
  } as unknown as UserRequest;
  useBoard.setState({ board: { ...emptyBoard(CHAT), loaded: true, requests: { r1: request } } });
}

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "keepChanges" || req.method === "undoPendingChanges") {
    asked.push(`${req.method} ${req.path ?? "all"}`);
    if (req.path) review = { ...review, [req.path]: req.method === "keepChanges" ? "kept" : "undone" };
    show();
    return req.method === "keepChanges" ? { method: req.method } : { method: req.method, outcome: { restored: [req.path], held: [] } };
  }
  return { method: req.method };
});
useApp.setState({ selection: { type: "conversation", id: CHAT } });
show();

const view = { selection: { type: "conversation" as const, id: CHAT }, conversation: { id: CHAT } as Conversation, embedded: false };

const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
const wait = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));
async function until(check: () => boolean, ms = 8000) {
  for (const end = performance.now() + ms; performance.now() < end && !check(); ) await frame();
  await wait(100);
}
const words = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, " ").trim() ?? null;
const rows = () => [...document.querySelectorAll<HTMLElement>("[data-slot=turn-diff-file]")];

function look() {
  const card = document.querySelector("[data-slot=turn-diff]");
  return {
    title: words(card?.querySelector("[data-slot=turn-diff-title]")),
    review: words(card?.querySelector("[data-slot=turn-diff-review]")),
    buttons: [...(card?.querySelectorAll("button") ?? [])]
      .filter((button) => !button.closest("[data-slot=turn-diff-file]"))
      .map((button) => button.getAttribute("aria-label") ?? words(button)),
    rows: rows().map((row) => [row.dataset.review ?? null, [...row.querySelectorAll("button")].slice(1).map(words).join(" ") || words(row.querySelector("[data-slot=turn-diff-reviewed]"))]),
  };
}

async function measure() {
  const out: Record<string, unknown> = {};
  await until(() => rows().length === 3);
  out.waiting = look();
  [...document.querySelectorAll("button")].find((button) => words(button) === "Review Changes")?.click();
  await wait(50);
  out.opened = useSessionTabs.getState().sessions[CHAT]?.active ?? null;
  [...(rows()[0]?.querySelectorAll("button") ?? [])].find((button) => words(button) === "Keep")?.click();
  await until(() => rows()[0]?.dataset.review === "kept");
  out.keptOne = look();
  [...document.querySelectorAll("button")].find((button) => words(button) === "Undo All")?.click();
  await until(() => look().title === "Edited 3 files");
  out.done = look();
  review = { "src/a.ts": "merged", "src/b.ts": "kept", "src/c.ts": "merged" };
  show();
  await until(() => rows()[0]?.dataset.review === "merged");
  out.merged = look();
  review = { ...review, "src/d.ts": "pending" };
  show();
  await until(() => look().review === "· 1 to review");
  [...document.querySelectorAll("button")].find((button) => words(button) === "Show 1 more file")?.click();
  await until(() => rows().length === 4);
  out.putBack = look();
  files = [];
  review = { "src/d.ts": "pending" };
  show();
  await until(() => look().title === "1 File");
  out.putBackAlone = look();
  out.asked = asked;
  return out;
}

createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <ViewContext.Provider value={view}>
      <div className="w-160 p-6">
        <TurnDiff requestIds={["r1"]} />
      </div>
    </ViewContext.Provider>
  </TooltipProvider>,
);

// `still=1`: the first state only, for a screenshot.
void (query.get("still") === "1" ? new Promise<never>(() => {}) : measure())
  .catch((error: unknown) => ({ error: String(error) }))
  .then(async (result) => {
    if (query.get("stop") === "1") await new Promise(() => {});
    const out = document.createElement("pre");
    out.id = "turn-diff-review-result";
    out.textContent = JSON.stringify(result);
    document.body.append(out);
  });
