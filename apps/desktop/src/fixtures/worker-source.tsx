/**
 * A session's Source with workers: a stacked row for its checkout ("This session") and one for
 * each worker, in place of a switch. A worker's +N −N in the context card opens the right
 * sidebar's Source with its row open (the row in the card still opens its panel); a worker's files
 * are read only, each opening its part of the worker's diff in the Review tab. A worker that is
 * gone (the daemon no longer finds it, or it leaves the list) closes its Review tab and loses its
 * row, with no error. Without workers there are no rows.
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
  branch: "brigadier/session", remote: "origin", upstream: null, ahead: 0, behind: 0,
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
const rowNames = () => all('[data-slot="source-row-name"]').map(text);
const panelOf = (name: string) => all('[data-slot="source-panel"]').find((one) => text(one.querySelector('[data-slot="source-row-name"]')) === name) as HTMLElement | undefined;
const groupsOf = (name: string) =>
  all("section[aria-label]", panelOf(name)).map((group) => text(group.querySelector("header > button")) + text(group.querySelector("header > span:last-child")));
function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    const seen: Record<string, unknown> = {};
    void (async () => {
      // The session's checkout first and open, a row for each worker folded; no switch.
      await until(() => groupsOf("This session").length === 1);
      seen.start = {
        rows: rowNames(),
        groups: groupsOf("This session"),
        folded: [groupsOf("Cart fixes"), groupsOf("Readme fixes")],
        title: text(document.querySelector('[data-slot="source-rows"] [data-slot="source-title"]')),
        titleActions: all('[data-slot="source-title"] button').length,
        switcher: !!document.querySelector('[data-slot="source-scope"]'),
      };

      // (a) The row opens the worker's panel; its +N −N opens Source with the worker's row open.
      const rows = all('[data-slot="workers-summary-worker"]');
      (rows[0] as HTMLElement).click();
      seen.rowOpens = [...panels];
      const changes = all('[data-slot="workers-summary-changes"]') as HTMLButtonElement[];
      seen.counts = changes.map((button) => [text(button), button.getAttribute("aria-label")]);
      changes[0]!.click();
      await until(() => groupsOf("Cart fixes").length === 1);
      const pane = useWorkspacePaneState.getState();
      const worker = panelOf("Cart fixes")!;
      seen.opened = {
        pane: { open: pane.open, tab: pane.tabs[id] },
        asked: [...new Set(calls.filter((call) => call.method === "getWorkerSource").map((call) => call.method === "getWorkerSource" && call.taskId))],
        branch: text(worker.querySelector('[data-slot="source-row-branch"]')),
        branchTitle: worker.querySelector('[data-slot="source-row-branch"]')?.getAttribute("title"),
        scope: worker.getAttribute("data-scope"),
        groups: groupsOf("Cart fixes"),
        files: all("li", worker).map(text),
        request: useSourceScopes.getState()[id] ?? null,
      };

      // (c) Read only: no message box, Commit, More Actions, or a row's stage and discard.
      seen.readOnly = {
        box: !!worker.querySelector('[aria-label="Commit message"]'),
        buttons: all("button .sr-only", worker).map(text),
      };
      // A file opens its part of the worker's diff in the Review tab.
      (all("li button", worker)[0] as HTMLButtonElement).click();
      const opened = sessionTabs(id);
      seen.review = opened.tabs.find((tab) => tab.id === opened.active);

      // The session's row still commits; another worker's opens too.
      seen.parent = { box: !!panelOf("This session")!.querySelector('[aria-label="Commit message"]') };
      (panelOf("Readme fixes")!.querySelector("button[aria-expanded]") as HTMLButtonElement).click();
      await until(() => groupsOf("Readme fixes").length === 1);
      seen.other = { files: all("li", panelOf("Readme fixes")).map(text) };

      // Gone: its Review tab closes, and its row goes on Refresh, quietly.
      gone.add("t2");
      (all("li button", panelOf("Readme fixes"))[0] as HTMLButtonElement).click();
      await until(() => calls.some((call) => call.method === "getReviewDiff" && call.scope.type === "worker" && call.scope.taskId === "t2"));
      await until(() => !sessionTabs(id).tabs.some((tab) => tab.id === REVIEW_TAB));
      seen.reviewGone = { active: sessionTabs(id).active, error: text(document.querySelector('[data-slot="review-tab"] .text-destructive')) };
      (all("button", panelOf("Readme fixes")).find((button) => text(button) === "Refresh") as HTMLButtonElement).click();
      await until(() => !panelOf("Readme fixes"));
      seen.sourceGone = {
        rows: rowNames(),
        error: text(document.querySelector('[data-slot="source-rows"] p.text-destructive')),
      };
      // With no worker left in the list: one checkout, no rows, the branch under Commit.
      useCliThread.setState((state) => ({
        views: { ...state.views, [id]: { ...state.views[id]!, workers: state.views[id]!.workers!.map((one) => ({ ...one, diff: null })) } },
      }));
      await until(() => !document.querySelector('[data-slot="source-rows"]'));
      seen.noRows = { rows: rowNames(), branch: text(document.querySelector('[data-slot="source-branch"]')) };
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
