/** The existing card layout with turn snapshots, merge groups and confirmed Revert. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";
import { ChangesCard } from "@/app/conversation/ChangesCard";
import { ViewContext } from "@/app/conversation/viewContext";
import { RowScope } from "@/components/conversation";
import { TooltipProvider } from "@/components/overlays";
import { driveFold } from "@/fixtures/cardFold";
import { fixtureInput } from "@/fixtures/input";
import type { Conversation, FileStat, MergedTurnChanges, Request, Task, UserRequest } from "@/ipc/generated";
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
const sha = "93773efb8b60c162861c2fa748047fd2ba769b45";
const stressFiles: FileStat[] = Array.from({ length: 49 }, (_, i) => ({ path: `apps/desktop/src/app/conversation/activity/session-titles/ChangedFile${i + 1}.tsx`, insertions: i + 1, deletions: i % 3, binary: false }));
const stressStat = { files: stressFiles, insertions: 4792, deletions: 307 };
const emptyStat = { files: [], insertions: 0, deletions: 0 };
const stressGroups: MergedTurnChanges[] = Array.from({ length: 6 }, (_, i) => ({
  branch: i === 0 ? sha : i === 1 ? "worker/known" : i === 2 ? "worker/ambiguous" : `brigadier/session/worker-with-a-long-branch-name-${i}`,
  tip: i === 0 || i === 5 ? sha : String(i).repeat(40),
  mergeCommit: i === 4 ? "a".repeat(40) : String.fromCharCode(97 + i).repeat(40),
  stat: { files: i === 0 ? stressFiles : stressFiles.slice(0, 5), insertions: 2107, deletions: 228 },
}));
const requests = {
  own: entry("own", { stat, settled: true }),
  all: entry("all", { stat, settled: true }),
  empty: entry("empty", { stat: { files: [], insertions: 0, deletions: 0 }, settled: true }),
  legacy: entry("legacy", undefined),
  worker: entry("worker", { stat: { files: [files[0]!], insertions: 11, deletions: 2 }, settled: true, merged: [group] }),
  binary: entry("binary", { stat: { files: [{ path: "photo.png", insertions: 0, deletions: 0, binary: true }], insertions: 0, deletions: 0 }, settled: true }),
  stress: entry("stress", { stat: stressStat, settled: true, merged: [stressGroups[0]!, stressGroups[0]!, stressGroups[1]!, stressGroups[2]!] }),
  "stress-repeat": entry("stress-repeat", { stat: emptyStat, settled: true, merged: [stressGroups[0]!, ...stressGroups.slice(3)] }),
};
// Only the fields the card reads; ambiguous branch matches must keep their branch label.
const worker = (id: string, title: string, branch: string) => ({ id, title, workspace: { branch } }) as Task;
const tasks = { known: worker("known", "Review session titles", "worker/known"), ambiguous1: worker("ambiguous1", "Do not guess one", "worker/ambiguous"), ambiguous2: worker("ambiguous2", "Do not guess two", "worker/ambiguous") };
useBoard.setState({ board: { ...emptyBoard(CHAT), loaded: true, requests, tasks } });
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
const root = createRoot(document.getElementById("root")!);
let mount = 0;
const renderCards = () => root.render(
  <TooltipProvider><ViewContext value={{ conversation, selection: { type: "conversation", id: CHAT }, embedded: false }}>
    <main className={`flex w-full flex-col gap-4 p-6${only === "stress" ? " mx-auto max-w-184" : ""}`}>{Object.keys(requests).filter((id) => only ? only === id : !id.startsWith("stress")).map((id) => <RowScope key={`${id}:${mount}`} rowKey={`${CHAT}:${id}`}><section data-case={id}><ChangesCard requestIds={id === "stress" ? ["stress", "stress-repeat"] : [id]} /></section></RowScope>)}</main>
  </ViewContext></TooltipProvider>,
);
renderCards();
const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
async function until(check: () => boolean) {
  for (let n = 0; n < 240 && !check(); n++) await frame();
  if (!check()) throw new Error("fixture did not reach expected state");
}
const card = (id: string) => document.querySelector<HTMLElement>(`[data-case="${id}"]`)!;
const button = (id: string, label: string) => [...card(id).querySelectorAll<HTMLButtonElement>("button")].find((item) => item.textContent === label || item.getAttribute("aria-label") === label)!;
const rows = (id: string) => card(id)?.querySelectorAll("[data-slot=turn-diff-file]").length ?? 0;
async function driveStress() {
  await fixtureInput({ type: "viewport", width: 1728, height: 1117 });
  const id = "stress";
  const select = <T extends HTMLElement>(slot: string) => card(id).querySelector<T>(`[data-slot=${slot}]`)!;
  const groups = () => [...card(id).querySelectorAll<HTMLElement>("[data-slot=turn-merged-branch]")];
  const groupRows = (index = 0) => groups()[index]?.querySelectorAll("[data-slot=turn-diff-file]").length ?? 0;
  const initial = { ownRows: rows("stress"), groups: groups().length, groupRows: groupRows(), label: select("turn-merged-title").textContent, tooltip: select("turn-merged-title").title, labels: groups().map((g) => g.querySelector("[data-slot=turn-merged-title]")?.textContent), more: select("turn-merged-more").textContent };
  select("turn-merged-fold").focus();
  await fixtureInput({ type: "key", key: "Enter" });
  await until(() => groupRows() === 3);
  const opened = { rows: groupRows(), expanded: select("turn-merged-fold").getAttribute("aria-expanded") };
  groups()[0]!.querySelector<HTMLButtonElement>("[data-slot=turn-diff-file] button")!.click();
  await frame();
  const tabs = sessionTabs(CHAT);
  const tab = tabs.tabs.find((item) => item.id === tabs.active);
  const target = tab?.kind === "review" ? tab.target : null;
  select("turn-merged-more-files").click();
  await until(() => groupRows() === 49);
  const allFiles = groupRows();
  select("turn-merged-more-files").click();
  await until(() => groupRows() === 3);
  const fewerFiles = groupRows();
  select("turn-merged-more").click();
  await until(() => groups().length === 6);
  const uniqueGroups = new Set(groups().map((g) => g.dataset.merge)).size;
  // A group outside the first three keeps its own file expansion when hidden and shown.
  groups()[4]!.querySelector<HTMLButtonElement>("[data-slot=turn-merged-fold]")!.click();
  await until(() => groupRows(4) === 3);
  groups()[4]!.querySelector<HTMLButtonElement>("[data-slot=turn-merged-more-files]")!.click();
  await until(() => groupRows(4) === 5);
  select("turn-merged-more").click();
  await until(() => groups().length === 3);
  select("turn-merged-more").click();
  await until(() => groups().length === 6 && groupRows(4) === 5);
  const restoredHidden = groupRows(4);
  button("stress", "Show 46 more files").click();
  await until(() => select("turn-diff-files").querySelectorAll("[data-slot=turn-diff-file]").length === 49);
  // Simulate the virtual thread removing and remounting the card under the same row scope.
  mount += 1;
  renderCards();
  await frame();
  await until(() => groups().length === 6 && groupRows(4) === 5);
  const remounted = { ownRows: select("turn-diff-files").querySelectorAll("[data-slot=turn-diff-file]").length, groups: groups().length, firstRows: groupRows(), laterRows: groupRows(4) };
  const fold = select<HTMLButtonElement>("turn-diff-fold");
  fold.click();
  await until(() => rows("stress") === 0);
  const foldedRows = rows("stress");
  fold.click();
  await until(() => groupRows(4) === 5);
  button("stress", "Show less").click();
  await until(() => select("turn-diff-files").querySelectorAll("[data-slot=turn-diff-file]").length === 3);
  const ownLess = select("turn-diff-files").querySelectorAll("[data-slot=turn-diff-file]").length;
  const overflow = document.documentElement.scrollWidth > innerWidth;
  return { initial, opened, target, allFiles, fewerFiles, uniqueGroups, restoredHidden, remounted, foldedRows, ownLess, overflow };
}
async function drive() {
  await until(() => rows(only ?? "own") > 0);
  if (query.get("drive") === "stress") return driveStress();
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
