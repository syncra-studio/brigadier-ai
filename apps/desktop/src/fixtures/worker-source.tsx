/**
 * A worker's changes in Source: its +N −N in the context card opens the right sidebar's Source on
 * them (the row itself still opens its panel); the switch under Source's title goes back to the
 * session's checkout and on to another worker; a worker's files are read only, each opening its
 * part of the worker's diff in the Review tab.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import { SessionSource } from "@/app/side-panel/WorkerSource";
import { WorkersSummary } from "@/app/side-panel/WorkerSummary";
import { TooltipProvider } from "@/components/overlays";
import type { Request, SourceState, ThreadView, WorkerRow, WorkerSource } from "@/ipc/generated";
import { useCliThread } from "@/state/cliThread";
import { sessionTabs } from "@/state/sessionTabs";
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
      return { method: req.method, source: SOURCES[req.taskId] };
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
      setResult(JSON.stringify(seen));
    })().catch((error: unknown) => setResult(JSON.stringify({ error: String(error), seen, body: document.body.textContent?.slice(0, 2000) })));
  }, []);
  return (
    <TooltipProvider>
      <AgentsPanelContext.Provider value={{ panel: undefined, setPanel: (asked) => panels.push(asked ?? null) }}>
        <div className="bg-background text-foreground flex h-screen">
          <div className="w-80"><WorkersSummary conversationId={id} /></div>
          <div className="flex w-80 flex-col"><SessionSource conversationId={id} /></div>
        </div>
        {result && <pre id="worker-source-result">{result}</pre>}
      </AgentsPanelContext.Provider>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
