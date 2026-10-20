/**
 * The production context card's Background rows for five previews (three still there, two stopped)
 * and the output tab a row opens, with Pause, Resume, Stop and Open; synthetic IPC, no daemon.
 */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { OutputTab } from "@/app/side-panel/OutputTab";
import { PinnedSummary } from "@/app/conversation/PinnedSummary";
import { previewActive } from "@/app/conversation/previewStatus";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, Preview, Request } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { sessionTabs, useSessionTabsOf } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const id = "running-fixture";
const conversation: Conversation = { id, kind: "session", title: "Build the web app", projectId: null,
  pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: { type: "session", repo: "/fixture", environment: { type: "localCheckout", branch: "fixture" }, permission: "askForApproval", planMode: false, workersSeeUncommitted: null, orchestrator: { provider: "claude", model: null, effort: null } }, lifecycle: "active",
  forkedFrom: null, sideOf: null, fallback: null, quotaWait: null };
const params = new URLSearchParams(location.search);
const now = Date.now();
const rows: Preview[] = [
  { id: "preview-5", name: "Brigadier dev app", command: "pnpm tauri dev", kind: "app", state: { type: "starting" }, startedAtMs: now - 90000, endedAtMs: null },
  { id: "preview-4", name: "Web app", command: "pnpm dev --host 127.0.0.1", state: { type: "running" }, url: "http://localhost:5173/", startedAtMs: now - 154000, endedAtMs: null },
  { id: "preview-3", name: "Component explorer", command: "pnpm storybook --port 6006", state: { type: "paused" }, url: "http://127.0.0.1:6006/", startedAtMs: now - 302000, endedAtMs: null },
  { id: "preview-2", name: "Static preview", command: "pnpm preview", state: { type: "stopped", reason: "stopped by the user" }, startedAtMs: now - 500000, endedAtMs: now - 488000 },
  { id: "preview-1", name: "Desktop app", command: "pnpm tauri dev --config src-tauri/tauri.dev.conf.json", kind: "app", state: { type: "stopped", reason: "the session closed" }, url: "http://localhost:1420/", startedAtMs: now - 600000, endedAtMs: now - 350000 },
].map((row) => ({ conversationId: id, pid: 42, workdir: "/work/session", workspace: "/work/session", log: null, kind: "web", ...row }) as Preview);
useBoard.setState({ board: { ...emptyBoard(id), loaded: true, previews: Object.fromEntries(rows.map((row) => [row.id, row])) } });
useApp.setState({
  summaryShown: { [id]: true }, selection: { type: "conversation", id }, conversations: { [id]: conversation },
  info: { platform: params.get("platform") === "windows" ? "windows" : "macos", version: "fixture", processStartMs: now, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { status: "connected", daemon: null, reason: null },
});
const calls: Request[] = [];
let failPause = false;
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  calls.push(req);
  if (req.method === "getPullRequest") return { method: req.method, pullRequest: null };
  if (req.method === "previewLog") return {
    method: req.method,
    tail: req.previewId === "preview-5" ? "  VITE ready in 286 ms\n  Local: http://localhost:1420/\n   \u001b[1m\u001b[32mCompiling\u001b[0m brigadier-core v0.1.0\n    Building [=====>  ] 520/731: brigadier-core\r    Building [======> ] 521/731: brigadier-core\r\u001b[K" : req.previewId === "preview-4" ? "VITE ready in 286 ms\nLocal: http://localhost:5173/\nWaiting for file changes…" : req.previewId === "preview-2" ? "Build output not found. Run pnpm build first." : "Preview output is ready.",
    url: rows.find((row) => row.id === req.previewId)?.url ?? null,
  };
  if (req.method === "pausePreview" && failPause) throw new Error("The preview has ended.");
  const board = useBoard.getState().board!;
  const previews = { ...board.previews };
  if (req.method === "pausePreview" || req.method === "resumePreview" || req.method === "stopPreview") {
    for (const [key, row] of Object.entries(previews)) {
      if ((req.previewId === null || key === req.previewId) && previewActive(row.state)) {
        if (req.method === "stopPreview") delete previews[key];
        else previews[key] = { ...row, state: req.method === "pausePreview" ? { type: "paused" } : { type: "running" } };
      }
    }
  }
  useBoard.setState({ board: { ...board, previews } });
  return { method: req.method };
});
/** A preview's Background row, by its id. */
const row = (previewId: string) => document.querySelector<HTMLButtonElement>(`[data-slot="summary-card"] button[data-activity="${previewId}"]`);
const tab = () => document.querySelector<HTMLElement>('[data-slot="output-tab"]');
const press = (label: string, within: ParentNode | null = document) => {
  const button = [...(within?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find((candidate) => candidate.textContent?.trim() === label);
  button?.focus();
  button?.click();
};
const labels = () => [...(tab()?.querySelectorAll("header button") ?? [])].map((button) => button.textContent?.trim());
/** A row's action, in its time's place on hover: Stop, or a web preview's Show output (its row opens the page). */
const rowAction = (previewId: string, label: string) => {
  const button = row(previewId)?.parentElement?.querySelector<HTMLButtonElement>(`[data-slot="summary-row-action"] button[aria-label^="${label}"]`);
  button?.focus();
  button?.click();
};
const stopRow = (previewId: string) => rowAction(previewId, "Stop");

/** The output tab in front, as the session's tab bar would show it. */
function FrontTab() {
  const tabs = useSessionTabsOf(id);
  const front = tabs.tabs.find((candidate) => candidate.id === tabs.active);
  return front?.kind === "panel" && front.view.type === "output" ? <OutputTab conversationId={id} view={front.view} /> : null;
}

function Fixture() {
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  const [result, setResult] = useState("");
  useEffect(() => {
    const seen: Record<string, unknown> = {};
    const timers: ReturnType<typeof setTimeout>[] = [];
    const at = (ms: number, action: () => void) => timers.push(setTimeout(action, ms));
    at(500, () => {
      seen.section = document.querySelector("[data-activities] > header button[aria-controls]")?.textContent;
      seen.order = [...document.querySelectorAll("[data-activities] [data-activity]")].map((button) => button.getAttribute("data-activity"));
      seen.chip = row("preview-5")?.textContent;
      seen.pausedIcon = !!row("preview-3")?.querySelector("svg");
      rowAction("preview-4", "Show output");
    });
    if (params.has("drive")) {
      at(1000, () => {
        seen.opened = sessionTabs(id).active;
        seen.tabText = tab()?.textContent;
        seen.initial = labels();
        press("Pause", tab()?.querySelector("header") ?? null);
      });
      at(1300, () => { seen.paused = labels(); press("Resume", tab()?.querySelector("header") ?? null); });
      at(1600, () => {
        seen.resumed = labels();
        const info = useApp.getState().info!;
        useApp.setState({ info: { ...info, platform: "windows" } });
      });
      at(1900, () => { seen.windows = labels(); useApp.setState({ info: { ...useApp.getState().info!, platform: "macos" } }); });
      at(2200, () => { failPause = true; press("Pause", tab()?.querySelector("header") ?? null); });
      at(2500, () => {
        seen.error = tab()?.querySelector('[role="alert"]')?.textContent;
        failPause = false;
        press("Open", tab()?.querySelector("header") ?? null);
        const tabs = sessionTabs(id);
        const browser = tabs.tabs.find((candidate) => candidate.id === tabs.active);
        seen.browser = browser?.kind === "browser" ? browser.url : undefined;
        rowAction("preview-4", "Show output");
      });
      at(2800, () => { press("Stop", tab()?.querySelector("header") ?? null); });
      at(3100, () => {
        seen.stopped = !row("preview-4");
        seen.ended = tab()?.textContent;
        stopRow("preview-3");
      });
      at(3400, () => {
        seen.focusAfterStop = document.activeElement?.getAttribute("data-slot");
        seen.calls = calls.filter((call) => ["pausePreview", "resumePreview", "stopPreview"].includes(call.method)).map((call) => call.method);
        // Reactivate after a long idle period without waiting a minute in the fixture.
        const stopped = { ...rows[0]!, state: { type: "stopped" as const, reason: "stopped by the user" }, endedAtMs: now - 60000 };
        useBoard.setState({ board: { ...useBoard.getState().board!, previews: { [stopped.id]: stopped } } });
      });
      const realNow = Date.now;
      // Every text the row ever holds from here on, even one replaced before the next paint.
      const chipTexts: string[] = [];
      const watch = new MutationObserver((records) => {
        for (const record of records) {
          // The section comes back with its row: the row's text as it first reads.
          for (const node of record.addedNodes) {
            if (node instanceof Element) for (const added of node.querySelectorAll("[data-activity]")) chipTexts.push(added.textContent ?? "");
          }
          if (!(record.target instanceof Node) || !record.target.parentElement?.closest("[data-activity]") && !(record.target instanceof Element && record.target.closest("[data-activity]"))) continue;
          if (record.oldValue !== null) chipTexts.push(record.oldValue);
          if (record.type === "characterData") chipTexts.push(record.target.textContent ?? "");
          for (const node of record.addedNodes) chipTexts.push(node.textContent ?? "");
        }
      });
      at(3600, () => {
        seen.endedLeft = !document.querySelector("[data-activities]");
        watch.observe(document.body, { subtree: true, childList: true, characterData: true, characterDataOldValue: true });
        Date.now = () => now + 120000;
        const started = { ...rows[0]!, startedAtMs: now + 60000 };
        useBoard.setState({ board: { ...useBoard.getState().board!, previews: { [started.id]: started } } });
      });
      at(3750, () => {
        seen.reactivated = row("preview-5")?.textContent;
        watch.disconnect();
        seen.reactivatedTexts = chipTexts;
        Date.now = realNow;
        stopRow("preview-5");
      });
      at(4050, () => {
        seen.lastStopFocus = document.activeElement?.getAttribute("data-slot");
        seen.lastStopRemoved = !document.querySelector("[data-activities]");
        setResult(JSON.stringify(seen));
      });
    }
    return () => timers.forEach(clearTimeout);
  }, []);
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <main className="bg-background text-foreground relative flex h-screen w-full min-w-0">
        <div className="flex min-w-0 flex-1 flex-col">
          <header className="h-titlebar flex items-center justify-between gap-3 px-5">
            <span className="text-sm font-medium">Build the web app</span>
          </header>
          <FrontTab />
          <PinnedSummary conversation={conversation} />
        </div>
      </main>
      {result && <pre id="running-result">{result}</pre>}
    </AssistantRuntimeProvider>
  );
}
createRoot(document.getElementById("root")!).render(<TooltipProvider><SidebarProvider defaultOpen={false}><Fixture /></SidebarProvider></TooltipProvider>);
