/**
 * A standalone Vite entry, excluded from the app entry and production build: a file tab whose
 * file has agents' changes waiting for review, kept and undone change by change. IPC is mocked,
 * no daemon. The result goes to `<pre id="file-review-result">`; `stop=1` holds the page for a
 * screenshot, `still=1` shows the first state only.
 *
 * - The bar: where the change and the file are (1 of 2, 1 of 2 Files), Undo File, Keep File.
 * - Each change: its steps, Undo and Keep over its first line; the one in view names its keys.
 * - ⌘Y keeps the change in view; Undo on the other one undoes it; the file is then done, and
 *   the bar offers "Keep all changes" and "Review next file"; ⌥L opens the next file.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { FileTabView } from "@/app/conversation/FileTabView";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, FileReview, PendingFile, Request, UserRequest } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const CHAT = "file-review";
const PATH = "src/cart.ts";

/** Lines 2 to 61, long enough that the last change scrolls out of view of the first. */
const MIDDLE = Array.from({ length: 60 }, (_, index) => `// note ${index + 1}\n`).join("");
const BASE = `export const rate = 0.2;\n${MIDDLE}export function tax(amount: number): number {\n  return amount * rate;\n}\n`;
const AFTER = `export const RATE = 0.2;\n${MIDDLE}export function tax(amount: number): number {\n  return amount * RATE;\n}\n\nexport const zero = 0;\n`;

/** What the engine holds: the file's base and its changes left, by id. */
let base = BASE;
let hunks = [
  { id: "h1", oldStart: 1, oldLines: 1, newStart: 1, newLines: 1, lines: "export const RATE = 0.2;\n" },
  { id: "h2", oldStart: 63, oldLines: 1, newStart: 63, newLines: 1, lines: "  return amount * RATE;\n" },
  { id: "h3", oldStart: 64, oldLines: 0, newStart: 65, newLines: 2, lines: "\nexport const zero = 0;\n" },
];
let current = AFTER;
const asked: string[] = [];

function pendingFile(): PendingFile {
  return {
    path: PATH, requestId: "r1", base, current, text: true,
    hunks: hunks.map(({ id, oldStart, oldLines, newStart, newLines }) => ({ id, oldStart, oldLines, newStart, newLines })),
    insertions: hunks.reduce((sum, hunk) => sum + hunk.newLines, 0),
    deletions: hunks.reduce((sum, hunk) => sum + hunk.oldLines, 0),
  };
}

function setReview(review: Record<string, FileReview>) {
  const board = emptyBoard(CHAT);
  const request = { id: "r1", conversationId: CHAT, preview: "Rename the rate", state: { type: "done" }, startedAtMs: 0, endedAtMs: 1, review } as unknown as UserRequest;
  useBoard.setState({ board: { ...board, loaded: true, requests: { r1: request } } });
}

/** A whole-file text edit: keeping a hunk puts its new lines in the base, undoing puts its old ones back. */
function take(id: string, keep: boolean) {
  const hunk = hunks.find((each) => each.id === id)!;
  const old = base.split(/(?<=\n)/);
  const now = current.split(/(?<=\n)/);
  if (keep) {
    old.splice(hunk.oldLines === 0 ? hunk.oldStart : hunk.oldStart - 1, hunk.oldLines, ...hunk.lines.split(/(?<=\n)/));
    base = old.join("");
  } else {
    now.splice(hunk.newLines === 0 ? hunk.newStart : hunk.newStart - 1, hunk.newLines, ...old.slice(hunk.oldStart - 1, hunk.oldStart - 1 + hunk.oldLines));
    current = now.join("");
  }
  hunks = hunks.filter((each) => each !== hunk);
  setReview({ [PATH]: hunks.length === 0 ? "kept" : "pending", "src/total.ts": "pending" });
}

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "getPendingChanges") return { method: req.method, pending: { files: hunks.length > 0 ? [pendingFile()] : [] } };
  if (req.method === "keepChanges" || req.method === "undoPendingChanges") {
    asked.push(`${req.method} ${req.path ?? "all"} ${(req.hunks ?? ["file"]).join(",")}`);
    for (const id of req.hunks ?? []) take(id, req.method === "keepChanges");
    return req.method === "keepChanges" ? { method: req.method } : { method: req.method, outcome: { restored: [PATH], held: [] } };
  }
  if (req.method === "readFile") return { method: req.method, file: { path: req.path, text: current, size: current.length, truncated: false } };
  return { method: req.method };
});
useApp.setState((state) => ({
  conversations: {
    [CHAT]: {
      id: CHAT, kind: "session", title: "Review", projectId: null, pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, lifecycle: "active",
      forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
      setup: { type: "session", repo: "/repo", environment: { type: "local" }, permission: "fullAccess", planMode: false, workersSeeUncommitted: null, orchestrator: { provider: "codex", model: null, effort: null } },
    } as unknown as Conversation,
  },
  selection: { type: "conversation", id: CHAT },
  info: { ...state.info, platform: "macos" } as typeof state.info,
}));
setReview({ [PATH]: "pending", "src/total.ts": "pending" });

const tab = { kind: "file" as const, id: `file:${PATH}`, path: PATH, line: null, reveal: 0, preview: false, opener: null };

const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
const wait = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));
async function until(check: () => boolean, ms = 8000) {
  for (const end = performance.now() + ms; performance.now() < end && !check(); ) await frame();
  await wait(150);
}
const text = (selector: string) => document.querySelector(selector)?.textContent?.replace(/\s+/g, " ").trim() ?? null;
const changes = () => [...document.querySelectorAll<HTMLElement>("[data-slot=review-change]")];

function look() {
  return {
    changes: text("[data-slot=review-changes]"),
    files: text("[data-slot=review-files]"),
    buttons: [...document.querySelectorAll("[data-slot=review-bar] button")].map((button) => button.textContent?.trim() ?? "").filter((words) => words !== ""),
    bars: changes().map((bar) => [...bar.querySelectorAll("button")].map((button) => button.textContent?.trim() ?? "").filter((words) => words !== "")),
    current: changes().findIndex((bar) => bar.dataset.current !== undefined),
    diff: !!document.querySelector("diffs-container"),
  };
}

async function measure() {
  const out: Record<string, unknown> = {};
  await until(() => changes().length === 3);
  out.first = look();
  // Scrolled to the end, the change in view is the first one there; back at the top, the first again.
  const box = document.querySelector<HTMLElement>("[data-slot=file-review]")!;
  // Again each frame: the diff may still be growing when its changes first show.
  await until(() => {
    box.scrollTop = box.scrollHeight;
    return look().current !== 0;
  });
  out.scrolled = look().current;
  box.scrollTop = 0;
  await until(() => look().current === 0);
  window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyY", key: "y", metaKey: true, bubbles: true }));
  await until(() => changes().length === 2);
  out.kept = look();
  // The next change down steps into view; Undo on it.
  changes()[0]?.querySelector<HTMLButtonElement>("button[aria-label='Next change']")?.click();
  await until(() => look().current === 1);
  out.stepped = look().current;
  [...(changes()[1]?.querySelectorAll("button") ?? [])].find((button) => button.textContent?.startsWith("Undo"))?.click();
  await until(() => changes().length === 1);
  window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyY", key: "y", metaKey: true, bubbles: true }));
  await until(() => changes().length === 0 && look().buttons.includes("Keep all changes"));
  out.done = look();
  window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyL", key: "¬", altKey: true, bubbles: true }));
  await wait(100);
  out.next = useSessionTabs.getState().sessions[CHAT]?.active ?? null;
  out.asked = asked;
  out.base = base;
  out.current = current;
  out.expected = `export const RATE = 0.2;\n${MIDDLE}export function tax(amount: number): number {\n  return amount * RATE;\n}\n`;
  return out;
}

createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <FileTabView conversationId={CHAT} tab={tab} active />
  </TooltipProvider>,
);

// `still=1`: the first state only, for a screenshot.
void (query.get("still") === "1" ? new Promise<never>(() => {}) : measure())
  .catch((error: unknown) => ({ error: String(error) }))
  .then(async (result) => {
    if (query.get("stop") === "1") await new Promise(() => {});
    const out = document.createElement("pre");
    out.id = "file-review-result";
    out.textContent = JSON.stringify(result);
    document.body.append(out);
  });
