/**
 * A worker's changes in Source: its +N −N in the context card opens the right sidebar's Source on
 * them (the row itself still opens its panel); the switch under Source's title goes back to the
 * session's checkout and on to another worker; a worker's files are read only, each opening its
 * part of the worker's diff in the Review tab. A worker that is gone (the daemon no longer finds
 * it, or it leaves the rows) closes its Review tab and leaves the session's scope, with no error.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import { ReviewTab } from "@/app/side-panel/ReviewTab";
import { useSourceScopes } from "@/app/side-panel/sourceTarget";
import { SessionSource } from "@/app/side-panel/WorkerSource";
import { WorkersSummary } from "@/app/side-panel/WorkerSummary";
import { TooltipProvider } from "@/components/overlays";
import type { Request, SourceState, ThreadView, WorkerRow, WorkerSource } from "@/ipc/generated";
import { useCliThread } from "@/state/cliThread";
import { REVIEW_TAB, sessionTabs, useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { useWorkspacePaneState } from "@/state/workspacePane";

const id = "worker-source-fixture";
const NOW = Date.now();
const row = (task: string, title: string, diff: { insertions: number; deletions: number }): WorkerRow => ({
  id: task, number: Number(task.slice(1)), title, agent: "claude", model: null, startedAtMs: 1000,
  working: true, lastEnd: null, gone: false, lastEventAtMs: NOW, limit: null, freshContext: false, waiting: [], mcp: [],
  branch: `brigadier/w/${task}`, base: "brigadier/session", diff: { files: [], ...diff }, ahead: 1, merged: false,
});
useCliThread.setState({
  views: {
    [id]: {
      entries: [],
      turns: [],
      workers: [row("t1", "Cart fixes", { insertions: 12, deletions: 3 }), row("t2", "Readme fixes", { insertions: 4, deletions: 0 })],
    } as unknown as ThreadView,
  },
});
const parent: SourceState = {
  branch: "brigadier/session", remote: "origin", upstream: null, ahead: 0,
  staged: [], changes: [{ path: "notes.txt", oldPath: null, status: "modified" }], merging: null, rebasing: null,
};
const SOURCES: Record<string, WorkerSource> = {
  t1: {
    branch: "brigadier/w/t1", base: "brigadier/session", insertions: 12, deletions: 3,
    files: [
      { path: "src/cart/total.ts", from: null, status: "modified", insertions: 10, deletions: 3, binary: false },
      { path: "src/cart/loose.ts", from: null, status: "untracked", insertions: 2, deletions: 0, binary: false },
    ],
  },
  t2: {
    branch: "brigadier/w/t2", base: "brigadier/session", insertions: 4, deletions: 0,
    files: [{ path: "README.md", from: null, status: "modified", insertions: 4, deletions: 0, binary: false }],
  },
};
const calls: Request[] = [];
/** Workers the daemon no longer finds: closed, or their worktree removed. */
const gone = new Set<string>();
const notFound = (taskId: string) => ({ code: "notFound", message: `worker ${taskId} not found` });
/** The worker panels the summary's rows asked to open. */
const panels: (string | null)[] = [];
mockIPC((command, args) => {
  if (command !== "ipc_request") return null;
  const req = (args as { request: Request }).request;
  calls.push(req);
  switch (req.method) {
    case "getSourceState":
      return { method: req.method, state: parent };
    case "getWorkerSource":
      if (gone.has(req.taskId)) throw notFound(req.taskId);
      return { method: req.method, source: SOURCES[req.taskId] };
    case "getReviewDiff":
      if (req.scope.type === "worker" && gone.has(req.scope.taskId)) throw notFound(req.scope.taskId);
      return {
        method: req.method,
        review: { scope: req.scope, files: [], insertions: 0, deletions: 0, patch: "", fullFiles: true, commits: [], branch: null, base: null },
      };
    default:
      return { method: req.method };
  }
});
useApp.setState({ info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 } });

function until(check: () => boolean, timeoutMs = 4000): Promise<void> {
  const start = performance.now();
  return new Promise((resolve, reject) => {
    const tick = () => {
      if (check()) resolve();
      else if (performance.now() - start > timeoutMs) reject(new Error("timed out"));
      else setTimeout(tick, 20);
    };
    tick();
  });
}

const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const all = (selector: string, root: ParentNode = document) => [...root.querySelectorAll(selector)];
const panel = () => document.querySelector('[data-slot="source-panel"]');
const scope = () => panel()?.getAttribute("data-scope") ?? "session";
const groups = () =>
  all('[data-slot="source-panel"] section[aria-label]').map(
    (group) => text(group.querySelector("header > button")) + text(group.querySelector("header > span:last-child")),
  );
const openMenu = (trigger: Element | null) => {
  (trigger as HTMLElement | null)?.focus();
  trigger?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
};
const menuItem = (name: RegExp) =>
  all('[role="menu"][data-state="open"] [role^="menuitem"]').find((item) => name.test(text(item))) as HTMLElement | undefined;
const pick = async (name: RegExp) => {
  openMenu(document.querySelector('[data-slot="source-scope"]'));
  await until(() => !!menuItem(name));
  const items = all('[role="menu"][data-state="open"] [role^="menuitem"]').map(text);
  menuItem(name)!.click();
  return items;
};

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    const seen: Record<string, unknown> = {};
    void (async () => {
      // The session's own checkout first, with the switch over it.
      await until(() => groups().length === 1);
      seen.start = { scope: scope(), switch: text(document.querySelector('[data-slot="source-scope"]')), groups: groups() };

      // (a) The row opens the worker's panel; its +N −N opens Source on the worker's changes.
      const rows = all('[data-slot="workers-summary-worker"]');
      (rows[0] as HTMLElement).click();
      seen.rowOpens = [...panels];
      const changes = all('[data-slot="workers-summary-changes"]') as HTMLButtonElement[];
      seen.counts = changes.map((button) => [text(button), button.getAttribute("aria-label")]);
      changes[0]!.click();
      await until(() => scope() === "worker" && groups().length === 1);
      const pane = useWorkspacePaneState.getState();
      seen.opened = {
        pane: { open: pane.open, tab: pane.tabs[id] },
        asked: calls.filter((call) => call.method === "getWorkerSource").map((call) => call.method === "getWorkerSource" && call.taskId),
        switch: text(document.querySelector('[data-slot="source-scope"]')),
        branch: text(document.querySelector('[data-slot="source-branch"]')),
        groups: groups(),
        files: all('[data-slot="source-panel"] li').map(text),
      };

      // (c) Read only: no message box, Commit, More Actions, or a row's stage and discard.
      seen.readOnly = {
        box: !!document.querySelector('[aria-label="Commit message"]'),
        buttons: all('[data-slot="source-panel"] button .sr-only').map(text),
      };
      // A file opens its part of the worker's diff in the Review tab.
      (all('[data-slot="source-panel"] li button')[0] as HTMLButtonElement).click();
      const opened = sessionTabs(id);
      seen.review = opened.tabs.find((tab) => tab.id === opened.active);

      // (b) Back to the session's checkout, then to the other worker.
      seen.menu = await pick(/^This session/);
      await until(() => scope() === "session" && groups().length === 1);
      seen.parent = { switch: text(document.querySelector('[data-slot="source-scope"]')), groups: groups(), box: !!document.querySelector('[aria-label="Commit message"]') };
      await pick(/^Readme fixes/);
      await until(() => scope() === "worker" && groups().join() === "Changes1");
      seen.other = { switch: text(document.querySelector('[data-slot="source-scope"]')), files: all('[data-slot="source-panel"] li').map(text) };

      // Gone: its Review tab closes, and Source goes back to the session's checkout, quietly.
      gone.add("t2");
      (all('[data-slot="source-panel"] li button')[0] as HTMLButtonElement).click();
      await until(() => calls.some((call) => call.method === "getReviewDiff" && call.scope.type === "worker" && call.scope.taskId === "t2"));
      await until(() => !sessionTabs(id).tabs.some((tab) => tab.id === REVIEW_TAB));
      seen.reviewGone = { active: sessionTabs(id).active, error: text(document.querySelector('[data-slot="review-tab"] .text-destructive')) };
      (all('[data-slot="source-panel"] button').find((button) => text(button) === "Refresh") as HTMLButtonElement).click();
      await until(() => scope() === "session" && groups().length === 1);
      seen.sourceGone = {
        switch: text(document.querySelector('[data-slot="source-scope"]')),
        scope: useSourceScopes.getState()[id] ?? null,
        error: text(document.querySelector('[data-slot="source-panel"] .text-destructive')),
      };
      // A worker that leaves the rows (closed, its worktree gone) leaves the session's scope too.
      await pick(/^Cart fixes/);
      await until(() => scope() === "worker" && groups().join() === "Changes2");
      useCliThread.setState((state) => ({
        views: { ...state.views, [id]: { ...state.views[id]!, workers: state.views[id]!.workers!.map((row) => row.id === "t1" ? { ...row, diff: null } : row) } },
      }));
      await until(() => scope() === "session" && groups().length === 1);
      seen.closedGone = { switch: text(document.querySelector('[data-slot="source-scope"]')), scope: useSourceScopes.getState()[id] ?? null };
      setResult(JSON.stringify(seen));
    })().catch((error: unknown) => setResult(JSON.stringify({ error: String(error), seen, body: document.body.textContent?.slice(0, 2000) })));
  }, []);
  return (
    <TooltipProvider>
      <AgentsPanelContext.Provider value={{ panel: undefined, setPanel: (asked) => panels.push(asked ?? null) }}>
        <div className="bg-background text-foreground flex h-screen">
          <div className="w-80"><WorkersSummary conversationId={id} /></div>
          <div className="flex w-80 flex-col"><SessionSource conversationId={id} /></div>
          <ReviewHost />
        </div>
        {result && <pre id="worker-source-result">{result}</pre>}
      </AgentsPanelContext.Provider>
    </TooltipProvider>
  );
}

/** The session's Review tab while it is open, as the tab bar shows it. */
function ReviewHost() {
  const tab = useSessionTabs((s) => s.sessions[id]?.tabs.find((entry) => entry.id === REVIEW_TAB));
  return tab?.kind === "review" ? <div className="flex w-80 flex-col"><ReviewTab conversationId={id} target={tab.target} /></div> : null;
}

createRoot(document.getElementById("root")!).render(<Fixture />);
