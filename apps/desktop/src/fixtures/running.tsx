/** Production Running panel with five preview states and synthetic IPC; no daemon. */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { flushSync } from "react-dom";
import { createRoot } from "react-dom/client";

import { AgentStatus, type AgentState } from "@/components/assistant-ui/elements/agent-status";
import { BackgroundInbox } from "@/components/assistant-ui/elements/background-inbox";
import { PinnedSummary } from "@/app/conversation/PinnedSummary";
import { previewActive } from "@/app/conversation/previewStatus";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, Preview, Request } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { sessionTabs } from "@/state/sessionTabs";
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
  { id: "preview-2", name: "Static preview", command: "pnpm preview", state: { type: "exited", code: 1, status: "exit 1" }, startedAtMs: now - 500000, endedAtMs: now - 488000 },
  { id: "preview-1", name: "Desktop app", command: "pnpm tauri dev --config src-tauri/tauri.dev.conf.json", kind: "app", state: { type: "exited", code: 0, status: "exit 0" }, url: "http://localhost:1420/", startedAtMs: now - 600000, endedAtMs: now - 350000 },
].map((row) => ({ conversationId: id, pid: 42, workdir: "/work/session", workspace: "/work/session", log: null, kind: "web", ...row }) as Preview);
useBoard.setState({ board: { ...emptyBoard(id), loaded: true, previews: Object.fromEntries(rows.map((row) => [row.id, row])) } });
useApp.setState({
  pinnedSummary: true, selection: { type: "conversation", id }, conversations: { [id]: conversation },
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
  let previews = { ...board.previews };
  if (req.method === "clearPreviews") previews = Object.fromEntries(Object.entries(previews).filter(([, row]) => previewActive(row.state)));
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
const row = (name: string) => document.querySelector(`article[aria-label="${name}"]`);
const press = (label: string, within: ParentNode = document) => {
  const button = [...within.querySelectorAll<HTMLButtonElement>("button")].find((candidate) => candidate.textContent?.trim() === label);
  button?.focus();
  button?.click();
};
/** The newest preview's Activities row, opened, and its command line, which opens the previews. */
const showPreviews = () => {
  const card = document.querySelector('[data-slot="summary-card"] [data-activity]');
  const header = card?.querySelector<HTMLButtonElement>(":scope > button");
  if (header?.getAttribute("aria-expanded") === "false") flushSync(() => header.click());
  card?.querySelector<HTMLButtonElement>('[data-slot="task-card-transcript"] button')?.click();
};
const labels = (name: string) => [...(row(name)?.querySelectorAll("button") ?? [])].map((button) => button.textContent?.trim());

function Fixture() {
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  const [result, setResult] = useState("");
  useEffect(() => {
    const seen: Record<string, unknown> = {};
    const timers: ReturnType<typeof setTimeout>[] = [];
    const at = (ms: number, action: () => void) => timers.push(setTimeout(action, ms));
    at(500, () => {
      seen.section = document.querySelector('[data-activities] > header')?.textContent;
      seen.chip = document.querySelector('[data-slot="summary-card"] [data-activity] > button')?.textContent;
      showPreviews();
    });
    at(1000, () => {
      press("Show log", row("Web app")!);
      if (!params.has("drive")) return;
      seen.inboxHeader = document.querySelector('[data-slot="background-inbox"] > div')?.textContent;
      seen.initial = rows.map((preview) => ({ name: preview.name, text: row(preview.name)?.textContent, actions: labels(preview.name), indicator: row(preview.name)?.querySelector('[data-slot="background-inbox-indicator"]')?.getAttribute("data-state") }));
      press("Pause", row("Web app")!);
    });
    if (params.has("drive")) {
      at(1300, () => { seen.paused = labels("Web app"); press("Resume", row("Web app")!); });
      at(1600, () => {
        seen.resumed = labels("Web app");
        const info = useApp.getState().info!;
        useApp.setState({ info: { ...info, platform: "windows" } });
      });
      at(1900, () => { seen.windows = labels("Web app"); useApp.setState({ info: { ...useApp.getState().info!, platform: "macos" } }); });
      at(2200, () => { failPause = true; press("Pause", row("Web app")!); });
      at(2500, () => {
        seen.error = row("Web app")?.querySelector('[role="alert"]')?.textContent;
        failPause = false;
        press("http://localhost:5173/", row("Web app")!);
        const tabs = sessionTabs(id);
        const browser = tabs.tabs.find((tab) => tab.id === tabs.active);
        seen.browser = browser?.kind === "browser" ? browser.url : undefined;
      });
      at(2800, () => {
        seen.linkClosed = !document.querySelector('[aria-label="Session previews"]');
        showPreviews();
      });
      at(3000, () => { press("Clear finished"); });
      at(3300, () => { seen.cleared = Object.keys(useBoard.getState().board!.previews); press("Stop", row("Web app")!); });
      at(3600, () => { seen.stopped = !row("Web app"); press("Stop all"); });
      at(3900, () => { seen.allStopped = Object.keys(useBoard.getState().board!.previews).length === 0;
        seen.focusAfterStop = document.activeElement?.getAttribute("data-slot");
        seen.sectionRemoved = !document.querySelector("[data-activities]"); });
      at(4200, () => {
        seen.empty = document.querySelector('[aria-label="Session previews"]')?.textContent ?? "No previews";
        seen.calls = calls.filter((call) => ["pausePreview", "resumePreview", "stopPreview", "clearPreviews"].includes(call.method)).map((call) => call.method);
        // Reactivate after a long idle period without waiting a minute in the fixture.
        const stopped = { ...rows[0]!, state: { type: "exited" as const, code: 0, status: "exit 0" }, endedAtMs: now - 60000 };
        useBoard.setState({ board: { ...useBoard.getState().board!, previews: { [stopped.id]: stopped } } });
      });
      const realNow = Date.now;
      // Every text the row ever holds from here on, even one replaced before the next paint.
      const chipTexts: string[] = [];
      const watch = new MutationObserver((records) => {
        for (const record of records) {
          if (!(record.target instanceof Node) || !record.target.parentElement?.closest("[data-activity]") && !(record.target instanceof Element && record.target.closest("[data-activity]"))) continue;
          if (record.oldValue !== null) chipTexts.push(record.oldValue);
          if (record.type === "characterData") chipTexts.push(record.target.textContent ?? "");
          for (const node of record.addedNodes) chipTexts.push(node.textContent ?? "");
        }
      });
      at(4400, () => {
        watch.observe(document.body, { subtree: true, childList: true, characterData: true, characterDataOldValue: true });
        Date.now = () => now + 120000;
        const started = { ...rows[0]!, startedAtMs: now + 60000 };
        useBoard.setState({ board: { ...useBoard.getState().board!, previews: { [started.id]: started } } });
      });
      at(4550, () => {
        seen.reactivated = document.querySelector("[data-activity] > button")?.textContent;
        watch.disconnect();
        seen.reactivatedTexts = chipTexts;
        Date.now = realNow;
        showPreviews();
      });
      at(4800, () => { press("Stop", row("Brigadier dev app")!); });
      at(5100, () => {
        seen.lastStopFocus = document.activeElement?.getAttribute("data-slot");
        seen.lastStopRemoved = !document.querySelector("[data-activities]") && !document.querySelector('[data-slot="background-inbox"]');
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
          <div className="mx-auto max-w-xl space-y-4 px-6 py-12 text-sm">
            <p>The web app is ready to preview.</p>
            <p className="text-muted-foreground">Open its local address from Activities. Pause a preview to hold it, or stop it when you are finished.</p>
          </div>
          <PinnedSummary conversation={conversation} />
        </div>
      </main>
      {result && <pre id="running-result">{result}</pre>}
    </AssistantRuntimeProvider>
  );
}
function Reference() {
  const kind = params.get("reference");
  return <main className="bg-background text-foreground flex h-screen items-center justify-center">
    {kind === "agent-status" ? <AgentStatus state={(params.get("state") ?? "working") as AgentState}
      label={params.get("label") ?? "Refactoring composer"} elapsed={params.get("elapsed") ?? "0s"} /> :
      <BackgroundInbox runs={[
        { id: "1", title: "Audit the adapters for parity gaps", state: "ready", elapsed: "4m", summary: "6 gaps found" },
        { id: "2", title: "Regenerate the API reference", state: "running", elapsed: "1m" },
        { id: "3", title: "Bump dependencies", state: "failed", elapsed: "2m", summary: "lockfile conflict" },
      ]} />}
  </main>;
}
createRoot(document.getElementById("root")!).render(params.has("reference") ? <Reference /> : <TooltipProvider><SidebarProvider defaultOpen={false}><Fixture /></SidebarProvider></TooltipProvider>);
