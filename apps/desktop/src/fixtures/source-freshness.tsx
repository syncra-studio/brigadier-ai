/**
 * Whether the summary card, the Source panel and the toggle's dot tell the current changes: the
 * production views over a scripted checkout, driven through each thing that changes it. Writes a
 * pass or fail for each into `#source-freshness-result`; `SourceFreshnessRender.test.ts` checks
 * them. `?draft=1` shows a project's draft instead of a session. Synthetic IPC, no daemon.
 */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@/components/conversation";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { PinnedSummary } from "@/app/conversation/PinnedSummary";
import { useWorkspacePane, WorkspacePane, WorkspacePaneContext, WorkspacePaneToggle } from "@/app/side-panel/WorkspacePane";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import night from "@/fixtures/boards/overnight-2026-10-03.json";
import type { Conversation, Request, SourceFile, SourceState, Task } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { useCliThread } from "@/state/cliThread";
import { useReview } from "@/state/review";
import { useApp } from "@/state/store";
import { emitWorkspaceChanged } from "@/state/workspaceCache";
import { openWorkspacePane, setWorkspacePaneOpen } from "@/state/workspacePane";

const id = "freshness-session";
const draft = new URLSearchParams(location.search).has("draft");
const file = (path: string): SourceFile => ({ path, status: "modified", oldPath: null });
let source: SourceState = { branch: "source-tab", remote: null, upstream: null, ahead: 0, behind: 0, staged: [], changes: draft ? [] : [file("a.ts"), file("b.ts"), file("c.ts")], merging: null, rebasing: null };
let insertions = 380;
const reads = { source: 0, diff: 0 };
/** How long each coming diff read takes to answer, in order; then at once. */
const diffDelays: number[] = [];
/** While set, a source read answers what it saw when it began, only once it is let go. */
let held: (() => void)[] | null = null;

const conversation: Conversation = { id, kind: "session", title: "Freshness", projectId: "p", pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null,
  setup: { type: "session", repo: "/work/repo", environment: { type: "newWorktree", branch: "source-tab", base: "main", path: "/work/copy", start: null }, permission: "ask", planMode: false, workersSeeUncommitted: null, orchestrator: { provider: "claude", model: null, effort: null } } };
const task = { ...(Object.values(night.tasks)[0] as unknown as Task), id: "landing", conversationId: id, landed: null } as Task;
useBoard.setState({ board: { ...emptyBoard(id), loaded: true, tasks: { landing: task } } });
useApp.setState({
  summaryShown: { [id]: true }, selection: draft ? { type: "draft", kind: "session", projectId: "p" } as never : { type: "conversation", id }, conversations: { [id]: conversation },
  projects: { p: { id: "p", name: "brigadier-ai", repos: [{ path: "/work/repo", name: "repo" }] } } as never,
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { status: "connected", daemon: null, reason: null },
});

function answer(req: Request): unknown {
  switch (req.method) {
    case "getSourceState":
    case "getProjectSourceState": {
      reads.source++;
      // A new object each read, as the daemon's answers are.
      const seen = { method: req.method, state: structuredClone(source) };
      return held ? new Promise((resolve) => held!.push(() => resolve(seen))) : seen;
    }
    case "getSessionDiff":
      reads.diff++;
      {
        const seen = { method: req.method, stat: { files: [], insertions, deletions: 2 }, commits: 1 };
        const delay = diffDelays.shift();
        return delay ? new Promise((resolve) => setTimeout(() => resolve(seen), delay)) : seen;
      }
    case "getWorkspaceTree":
      return { method: req.method, tree: { files: ["a.ts"], truncated: false, ignored: [], ignoredTruncated: false } };
    case "stageFiles": {
      const paths = req.paths ?? [];
      source = { ...source, staged: [...source.staged, ...source.changes.filter((one) => paths.includes(one.path))], changes: source.changes.filter((one) => !paths.includes(one.path)) };
      return { method: req.method, state: source };
    }
    case "discardFiles":
      source = { ...source, changes: source.changes.filter((one) => !(req.paths ?? []).includes(one.path)) };
      insertions -= 10;
      return { method: req.method, state: source };
    case "commitChanges":
      source = { ...source, staged: [], ahead: source.ahead + 1 };
      return { method: req.method, outcome: { commit: "abc", message: req.message ?? "", branch: "source-tab", pushed: false, note: null } };
    case "getTrackedWork":
      return { method: req.method, work: { pullRequests: [], prHistory: [], waitingOn: [] } };
    case "getBackgroundHistory":
      return { method: req.method, history: { records: [], next: null, total: 0 } };
    case "getPullRequest":
      return { method: req.method, pullRequest: null };
    default:
      return { method: req.method };
  }
}
mockIPC((command, payload) => (command === "ipc_request" ? answer((payload as { request: Request }).request) : null));

const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const pause = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(check: () => boolean, ms = 800): Promise<boolean> {
  for (const start = performance.now(); performance.now() - start < ms; await pause(15)) if (check()) return true;
  return check();
}
const buttons = (root: ParentNode = document) => [...root.querySelectorAll<HTMLButtonElement>("button")];
const named = (name: string, root?: ParentNode) => buttons(root).find((each) => each.getAttribute("aria-label") === name || text(each) === name);
const card = () => text(document.querySelector('[data-row="branch"]'));
const groups = () => [...document.querySelectorAll('[data-slot="source-panel"] section[aria-label]')].map((group) => `${text(group.querySelector("header > button"))} ${text(group.querySelector("header > span:last-child"))}`);
const dot = () => !!document.querySelector('[data-slot="source-changes-dot"]');
const focus = () => window.dispatchEvent(new Event("focus"));
const land = (commit: string) => useBoard.setState((s) => ({ board: { ...s.board!, tasks: { landing: { ...task, landed: commit } } } }));
const endTurn = () => useCliThread.setState((s) => {
  const view = s.views[id] as { turns?: unknown[] } | undefined;
  const turns = [...(view?.turns ?? []), { end: "done" }];
  return { views: { ...s.views, [id]: { ...view, entries: [], turns } as never } };
});
const commitButton = () => document.querySelector('[aria-label="Commit message"]')!.closest("div")!.parentElement!.parentElement!.querySelector<HTMLButtonElement>(":scope > div:nth-child(2) > button")!;
const discardFirst = async (path: string) => {
  named("Discard Changes", [...document.querySelectorAll('[data-slot="source-panel"] section[aria-label="Changes"] > *')].find((row) => text(row).includes(path)) ?? document)!.click();
  await until(() => !!document.querySelector('[role="alertdialog"], [role="dialog"]'));
  const dialog = document.querySelector('[role="alertdialog"], [role="dialog"]')!;
  buttons(dialog).find((each) => text(each).startsWith("Discard"))!.click();
};
const stageFirst = (path: string) =>
  named("Stage Changes", [...document.querySelectorAll('[data-slot="source-panel"] section[aria-label="Changes"] > *')].find((row) => text(row).includes(path)) ?? document)!.click();

/** A read that began before `mutate` answers after it, with no watcher event following. */
async function raced(mutate: () => void | Promise<void>, settled: () => boolean): Promise<boolean> {
  held = [];
  focus();
  await until(() => held!.length > 0);
  const old = held;
  held = null;
  await mutate();
  await pause(120);
  for (const release of old) release();
  await pause(150);
  return settled();
}

async function driveSession(): Promise<Record<string, boolean>> {
  const checks: Record<string, boolean> = {};
  setWorkspacePaneOpen(true);
  openWorkspacePane(id, "source");
  await until(() => groups().length > 0 && /\+380/.test(card()), 3000);
  await pause(100);
  const reread = async (name: string, act: () => void | Promise<void>, shows: () => boolean) => {
    const diff = reads.diff;
    await act();
    checks[name] = await until(() => reads.diff > diff && shows());
  };
  checks.cardShowsUncommittedCount = await until(() => /\+380\s*−2\s*·\s*3 uncommitted/.test(card()));
  await reread("cardRereadsAfterStage", () => stageFirst("a.ts"), () => groups().some((one) => one.startsWith("Staged Changes 1")));
  await reread("cardRereadsAfterDiscard", () => discardFirst("b.ts"), () => /\+370/.test(card()) && !groups().join().includes("Changes 2"));
  await reread("cardRereadsAfterCommit", () => commitButton().click(), () => !groups().some((one) => one.startsWith("Staged Changes")) && /1 uncommitted/.test(card()));
  await reread("cardRereadsAfterOutsideEdit", () => {
    source = { ...source, changes: [...source.changes, file("d.ts")] };
    insertions += 25;
    emitWorkspaceChanged({ type: "conversation", conversationId: id });
  }, () => /\+395/.test(card()) && groups().join().includes("Changes 2"));
  await reread("cardRereadsAfterLanding", () => { insertions += 5; land("landed-1"); }, () => /\+400/.test(card()));
  await reread("cardRereadsOnFocus", () => { insertions += 5; focus(); }, () => /\+405/.test(card()));
  await reread("cardRereadsAfterTurnEnd", () => { insertions += 5; endTurn(); }, () => /\+410/.test(card()));

  // An older stat answering last doesn't replace a newer one.
  const change = (added: number) => {
    insertions += added;
    emitWorkspaceChanged({ type: "conversation", conversationId: id });
  };
  diffDelays.push(300);
  change(1);
  await until(() => diffDelays.length === 0);
  change(1);
  await pause(450);
  checks.olderStatDoesNotReplaceNewer = /\+412/.test(card());
  // Readings faster than a stat can answer still show one, before they stop.
  diffDelays.push(90, 90, 90, 90, 90, 90);
  let shownDuring = false;
  for (let index = 0; index < 6; index++) {
    change(1);
    await pause(45);
    shownDuring ||= !/\+412/.test(card());
  }
  checks.burstOfReadingsStillUpdatesTheCard = shownDuring;
  diffDelays.length = 0;
  await pause(150);

  // The pane on Files: nothing reads Source, yet its count must follow.
  openWorkspacePane(id, "files");
  await pause(50);
  let before = reads.source;
  source = { ...source, changes: [...source.changes, file("e.ts")] };
  focus();
  checks.countRereadsOnFocusWithFilesShown = await until(() => reads.source > before);

  openWorkspacePane(id, "search");
  await pause(50);
  before = reads.source;
  focus();
  checks.countRereadsOnFocusWithSearchShown = await until(() => reads.source > before);

  // The pane closed.
  setWorkspacePaneOpen(false);
  await pause(450);
  const clean = () => { source = { ...source, staged: [], changes: [] }; };
  before = reads.source;
  clean();
  focus();
  checks.countRereadsOnFocusWithPaneClosed = await until(() => reads.source > before && !dot());
  before = reads.source;
  source = { ...source, changes: [file("f.ts")] };
  land("landed-2");
  checks.countRereadsAfterLandingWithPaneClosed = await until(() => reads.source > before && dot());
  before = reads.source;
  clean();
  endTurn();
  checks.countRereadsAfterTurnEndWithPaneClosed = await until(() => reads.source > before && !dot());

  // A read that began before a mutation, answering after it.
  source = { ...source, staged: [], changes: [file("g.ts"), file("h.ts"), file("i.ts")] };
  setWorkspacePaneOpen(true);
  openWorkspacePane(id, "source");
  focus();
  await until(() => groups().join().includes("Changes 3"), 2000);
  await pause(100);
  const reviewed = () => JSON.stringify([source.staged, source.changes]) === useReview.getState().checkouts[id];
  checks.staleReadDoesNotUndoStage = await raced(() => stageFirst("g.ts"), () => groups().some((one) => one.startsWith("Staged Changes 1")) && reviewed());
  checks.staleReadDoesNotUndoDiscard = await raced(() => discardFirst("h.ts"), () => !groups().join().includes("Changes 2") && reviewed());
  checks.staleReadDoesNotUndoCommit = await raced(() => commitButton().click(), () => !groups().some((one) => one.startsWith("Staged Changes")) && reviewed());
  return checks;
}

async function driveDraft(): Promise<Record<string, boolean>> {
  const checks: Record<string, boolean> = {};
  setWorkspacePaneOpen(false);
  await until(() => reads.source > 0, 2000);
  await pause(100);
  let before = reads.source;
  source = { ...source, changes: [file("a.ts")] };
  focus();
  checks.draftCountRereadsOnFocusWithPaneClosed = await until(() => reads.source > before && dot());
  setWorkspacePaneOpen(true);
  openWorkspacePane("draft:p", "files");
  await pause(100);
  before = reads.source;
  source = { ...source, changes: [] };
  focus();
  checks.draftCountRereadsOnFocusWithFilesShown = await until(() => reads.source > before && !dot());
  return checks;
}

function Fixture() {
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  const pane = useWorkspacePane(draft ? null : id, "p", true);
  const [result, setResult] = useState("");
  useEffect(() => {
    void (draft ? driveDraft() : driveSession())
      .then((checks) => setResult(JSON.stringify({ checks })))
      .catch((error: unknown) => setResult(JSON.stringify({ error: String(error), card: card(), groups: groups() })));
  }, []);
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <WorkspacePaneContext.Provider value={pane}>
        <div className="bg-background text-foreground flex h-screen min-w-0 flex-1">
          <main className="relative flex min-w-0 flex-1 flex-col">
            <header className="h-titlebar flex items-center justify-end px-2"><WorkspacePaneToggle /></header>
            {!draft && <PinnedSummary conversation={conversation} />}
          </main>
          <WorkspacePane />
        </div>
        {result && <pre id="source-freshness-result">{result}</pre>}
      </WorkspacePaneContext.Provider>
    </AssistantRuntimeProvider>
  );
}
createRoot(document.getElementById("root")!).render(<TooltipProvider><SidebarProvider defaultOpen={false}><Fixture /></SidebarProvider></TooltipProvider>);
