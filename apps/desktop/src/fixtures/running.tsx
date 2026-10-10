// oxlint-disable react/refs -- Like the panes fixture, this uses the production panel API, whose workspace member is a ref callback.
/** Production Running panel with four preview states and synthetic IPC; no daemon. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { PreviewChip } from "@/app/conversation/PreviewChip";
import { SidePanel, SidePanelContext, useSidePanel } from "@/app/conversation/SidePanel";
import { previewActive } from "@/app/conversation/previewStatus";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Preview, Request } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { useBrowserTabs } from "@/state/browsers";
import { useApp } from "@/state/store";

const id = "running-fixture";
const params = new URLSearchParams(location.search);
const now = Date.now();
const rows: Preview[] = [
  { id: "preview-4", name: "Web app", command: "pnpm dev --host 127.0.0.1", state: { type: "running" }, url: "http://localhost:5173/", startedAtMs: now - 154000, endedAtMs: null },
  { id: "preview-3", name: "Component explorer", command: "pnpm storybook --port 6006", state: { type: "paused" }, url: "http://127.0.0.1:6006/", startedAtMs: now - 302000, endedAtMs: null },
  { id: "preview-2", name: "Static preview", command: "pnpm preview", state: { type: "exited", code: 1, status: "exit 1" }, startedAtMs: now - 500000, endedAtMs: now - 488000 },
  { id: "preview-1", name: "Desktop app", command: "pnpm tauri dev --config src-tauri/tauri.dev.conf.json", state: { type: "stopped", reason: "stopped by the user" }, startedAtMs: now - 600000, endedAtMs: now - 350000 },
].map((row) => ({ conversationId: id, pid: 42, workdir: "/work/session", workspace: "/work/session", log: null, ...row }) as Preview);
useBoard.setState({ board: { ...emptyBoard(id), loaded: true, previews: Object.fromEntries(rows.map((row) => [row.id, row])) } });
useApp.setState({
  info: { platform: params.get("platform") === "windows" ? "windows" : "macos", version: "fixture", processStartMs: now, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { status: "connected", daemon: null, reason: null },
});
const calls: Request[] = [];
let failPause = false;
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  calls.push(req);
  if (req.method === "previewLog") return {
    method: req.method,
    tail: req.previewId === "preview-4" ? "VITE ready in 286 ms\nLocal: http://localhost:5173/\nWaiting for file changes…" : req.previewId === "preview-2" ? "Build output not found. Run pnpm build first." : "Preview output is ready.",
    url: rows.find((row) => row.id === req.previewId)?.url ?? null,
  };
  if (req.method === "pausePreview" && failPause) throw new Error("The preview has ended.");
  const board = useBoard.getState().board!;
  let previews = { ...board.previews };
  if (req.method === "clearPreviews") previews = Object.fromEntries(Object.entries(previews).filter(([, row]) => previewActive(row.state)));
  if (req.method === "pausePreview" || req.method === "resumePreview" || req.method === "stopPreview") {
    for (const [key, row] of Object.entries(previews)) {
      if ((req.previewId === null || key === req.previewId) && previewActive(row.state)) {
        previews[key] = { ...row, state: req.method === "pausePreview" ? { type: "paused" } : req.method === "resumePreview" ? { type: "running" } : { type: "stopped", reason: "stopped by the user" }, endedAtMs: req.method === "stopPreview" ? Date.now() : null };
      }
    }
  }
  useBoard.setState({ board: { ...board, previews } });
  return { method: req.method };
});
const row = (name: string) => document.querySelector(`article[aria-label="${name}"]`);
const press = (label: string, within: ParentNode = document) => [...within.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.trim() === label)?.click();
const labels = (name: string) => [...(row(name)?.querySelectorAll("button") ?? [])].map((button) => button.textContent?.trim());

function Fixture() {
  const { panel } = useSidePanel(id, "session");
  const [result, setResult] = useState("");
  const { openTab } = panel;
  useEffect(() => { openTab("running"); }, [openTab]);
  useEffect(() => {
    const seen: Record<string, unknown> = {};
    const timers: ReturnType<typeof setTimeout>[] = [];
    const at = (ms: number, action: () => void) => timers.push(setTimeout(action, ms));
    at(1000, () => {
      press("Show log", row("Web app")!);
      if (!params.has("drive")) return;
      seen.initial = rows.map((preview) => ({ name: preview.name, text: row(preview.name)?.textContent, actions: labels(preview.name) }));
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
        const tabs = useBrowserTabs.getState().conversations[id]!;
        seen.browser = tabs.restoredUrls?.[tabs.active];
        document.querySelector<HTMLButtonElement>('[data-slot="preview-chip"]')?.click();
      });
      at(3000, () => { press("Clear finished"); });
      at(3300, () => { seen.cleared = Object.keys(useBoard.getState().board!.previews); press("Stop", row("Web app")!); });
      at(3600, () => { seen.stopped = labels("Web app"); press("Stop all"); });
      at(3900, () => { seen.allStopped = Object.values(useBoard.getState().board!.previews).every((p) => p.state.type === "stopped"); press("Clear finished"); });
      at(4200, () => {
        seen.empty = document.querySelector('[aria-label="Session previews"]')?.textContent;
        seen.calls = calls.filter((call) => call.method !== "previewLog").map((call) => call.method);
        setResult(JSON.stringify(seen));
      });
    }
    return () => timers.forEach(clearTimeout);
  }, []);
  return (
    <SidePanelContext.Provider value={panel}>
      <main ref={panel.workspace} className="bg-background text-foreground flex h-screen w-full min-w-0">
        <div className="flex min-w-0 flex-1 flex-col">
          <header className="h-titlebar flex items-center justify-between gap-3 px-5">
            <span className="text-sm font-medium">Build the web app</span><PreviewChip conversationId={id} />
          </header>
          <div className="mx-auto max-w-xl space-y-4 px-6 py-12 text-sm">
            <p>The web app is ready to preview.</p>
            <p className="text-muted-foreground">Open its local address from Running. Pause a preview to hold it, or stop it when you are finished.</p>
          </div>
        </div>
        <SidePanel conversationId={id} />
      </main>
      {result && <pre id="running-result">{result}</pre>}
    </SidePanelContext.Provider>
  );
}
createRoot(document.getElementById("root")!).render(<TooltipProvider><SidebarProvider defaultOpen={false}><Fixture /></SidebarProvider></TooltipProvider>);
