/** The existing card layout with turn snapshots, merge groups and confirmed Revert. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";
import { ChangesCard } from "@/app/conversation/ChangesCard";
import { ViewContext } from "@/app/conversation/viewContext";
import { TooltipProvider } from "@/components/overlays";
import { driveFold } from "@/fixtures/cardFold";
import type { Conversation, FileStat, Request, UserRequest } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { sessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const CHAT = "changes-card";
const query = new URLSearchParams(location.search);
const files: FileStat[] = [
  { path: "public/index.html", insertions: 11, deletions: 2, binary: false },
  { path: "src/server.js", insertions: 22, deletions: 5, binary: false },
  { path: "src/store.js", insertions: 5, deletions: 0, binary: false },
  { path: "src/dates.js", insertions: 30, deletions: 0, binary: false },
  { path: "test/dates.test.js", insertions: 18, deletions: 0, binary: false },
  { path: "README.md", insertions: 2, deletions: 0, binary: false },
];
const stat = { files, insertions: 88, deletions: 7 };
const entry = (id: string, changes: UserRequest["changes"]): UserRequest => ({ id, conversationId: CHAT, preview: "Due dates", state: { type: "done" }, startedAtMs: 1, endedAtMs: 2, steeredInto: null, steeredAfter: null, undo: null, worked: [], quotaWait: false, review: {}, ...(changes && { changes }) });
const group = { branch: "worker/one", tip: "f1f1f1f1", mergeCommit: "c1c1c1c1", stat: { files: [{ path: "worker.txt", insertions: 9, deletions: 1, binary: false }], insertions: 9, deletions: 1 } };
const requests = {
  own: entry("own", { stat, settled: true }),
  all: entry("all", { stat, settled: true }),
  empty: entry("empty", { stat: { files: [], insertions: 0, deletions: 0 }, settled: true }),
  legacy: entry("legacy", undefined),
  worker: entry("worker", { stat: { files: [files[0]!], insertions: 11, deletions: 2 }, settled: true, merged: [group] }),
  binary: entry("binary", { stat: { files: [{ path: "photo.png", insertions: 0, deletions: 0, binary: true }], insertions: 0, deletions: 0 }, settled: true }),
};
useBoard.setState({ board: { ...emptyBoard(CHAT), loaded: true, requests } });
useApp.setState({ selection: { type: "conversation", id: CHAT } });
const calls: Request[] = [];
let release: (() => void) | null = null;
mockIPC(async (command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  calls.push(req);
  if (req.method === "prepareTurnRevert") return { method: req.method, preview: { expectedTree: "confirmed-tree", changedAfter: ["later.txt", "src/user-edit.ts"] } };
  if (req.method === "revertTurn") {
    await new Promise<void>((resolve) => { release = resolve; });
    useBoard.setState((s) => ({ board: s.board ? { ...s.board, requests: { ...s.board.requests, [req.requestId]: { ...s.board.requests[req.requestId]!, undo: { reverted: true, commits: [] } } } } : null }));
    return { method: req.method, outcome: { restored: files.map((file) => file.path), held: [] } };
  }
  return { method: req.method };
});
const conversation = { id: CHAT, setup: { type: "session", repo: "/repo", environment: { type: "localCheckout", branch: "main" } } } as Conversation;
const only = query.get("case");
createRoot(document.getElementById("root")!).render(
  <TooltipProvider><ViewContext value={{ conversation, selection: { type: "conversation", id: CHAT }, embedded: false }}>
    <main className="flex w-full flex-col gap-4 p-6">{Object.keys(requests).filter((id) => !only || only === id).map((id) => <section key={id} data-case={id}><ChangesCard requestIds={[id]} /></section>)}</main>
  </ViewContext></TooltipProvider>,
);
const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
async function until(check: () => boolean) {
  for (let n = 0; n < 240 && !check(); n++) await frame();
  if (!check()) throw new Error("fixture did not reach expected state");
}
const card = (id: string) => document.querySelector<HTMLElement>(`[data-case="${id}"]`)!;
const button = (id: string, label: string) => [...card(id).querySelectorAll<HTMLButtonElement>("button")].find((item) => item.textContent === label || item.getAttribute("aria-label") === label)!;
const rows = (id: string) => card(id)?.querySelectorAll("[data-slot=turn-diff-file]").length ?? 0;
async function drive() {
  await until(() => rows(only ?? "own") > 0);
  if (query.get("drive") === "fold") {
    button("all", "Show 3 more files").click();
    await until(() => rows("all") === 6);
    const result = await driveFold(card("all").querySelector<HTMLElement>("[data-slot=turn-diff]")!, button("all", "Revert"), () => rows("all"));
    return { ...result, afterButton: result.afterButton, reopened: result.reopened };
  }
  const own = card("own");
  const before = { counts: own.querySelector("[data-slot=turn-diff-counts]")?.textContent, rows: rows("own"), empty: rows("empty"), legacy: !!card("legacy").querySelector("[data-slot=turn-diff]"), binary: card("binary").textContent, workerCounts: card("worker").querySelector("[data-slot=turn-diff-counts]")?.textContent, group: card("worker").querySelector("[data-slot=turn-merged-branch]")?.textContent };
  own.querySelector<HTMLButtonElement>("[data-slot=turn-diff-file] button")!.click();
  await frame();
  const tabs = sessionTabs(CHAT);
  const tab = tabs.tabs.find((item) => item.id === tabs.active);
  const target = tab?.kind === "review" ? tab.target : null;
  button("own", "Revert").click();
  await until(() => !!own.querySelector("[data-slot=merge-revert-confirm]"));
  const confirm = own.querySelector("[data-slot=merge-revert-confirm]")?.textContent;
  const writesBeforeConfirm = calls.filter((call) => call.method === "revertTurn").length;
  button("own", "Cancel").click();
  await frame();
  const cancelled = !own.querySelector("[data-slot=merge-revert-confirm]");
  button("own", "Revert").click();
  await until(() => !!own.querySelector("[data-slot=merge-revert-go]"));
  own.querySelector<HTMLButtonElement>("[data-slot=merge-revert-go]")!.click();
  await until(() => release !== null);
  const reverting = own.querySelector("[role=status]")?.textContent;
  release!();
  await until(() => own.querySelector("[data-slot=turn-diff]")?.getAttribute("data-state") === "reverted");
  return { before, target, confirm, writesBeforeConfirm, cancelled, reverting, call: calls.find((call) => call.method === "revertTurn"), finalTitle: own.querySelector("[data-slot=turn-diff-title]")?.textContent };
}
if (query.has("drive")) void drive().catch((error: unknown) => ({ error: String(error) })).then((result) => {
  const output = document.createElement("pre"); output.id = "changes-card-result"; output.textContent = JSON.stringify(result); document.body.append(output);
});
