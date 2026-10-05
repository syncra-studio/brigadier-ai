// oxlint-disable react/refs, brigadier/no-raw-design-values -- Browser fixtures emulate external page styling; the probe exposes pane callbacks for automation.
/** Production pane components with synthetic shell, webview and worker data. No daemon. */
import {
  AssistantRuntimeProvider,
  type ThreadMessage,
  useExternalStoreRuntime,
} from "@assistant-ui/react";
import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import {
  PanelButtons,
  PanelButtonsRoom,
  SidePanel,
  SidePanelContext,
  useSidePanel,
} from "@/app/conversation/SidePanel";
import {
  ComposerPlacement,
  FloatingComposerSlot,
} from "@/app/conversation/PaneComposer";
import { TerminalPane } from "@/app/conversation/TerminalTab";
import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import night from "@/fixtures/boards/overnight-2026-10-03.json";
import type { BrowserBounds, Request, Task } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { useApp } from "@/state/store";
import { useBrowsers } from "@/state/browsers";
import { useTerminalSessions } from "@/state/terminalSessions";
import { emitTerminalOutput } from "@/state/terminals";

const id = "pane-fixture";
const base = Object.values(night.tasks)[0] as unknown as Task;
const task: Task = {
  ...base,
  id: "worker-1",
  conversationId: id,
  title: "Build the pane controls",
  number: 1,
  state: "running",
  attempts: [],
  run: null,
  candidate: null,
  report: null,
  requestId: null,
  gateLink: null,
  quotaWait: null,
  blockedReason: null,
  createdAtMs: Date.now() - 68000,
  updatedAtMs: Date.now(),
};
useBoard.setState({
  board: {
    ...emptyBoard(id),
    loaded: true,
    tasks: { [task.id]: task },
    summaries: {
      [task.id]: {
        text: "Checking keyboard shortcuts and remembered sizes",
        atMs: Date.now(),
      },
    },
    transcripts: {
      [task.id]: {
        loading: false,
        hasMore: false,
        entries: [
          {
            streamSeq: 1,
            atMs: Date.now() - 20000,
            event: {
              type: "message",
              itemId: "fixture-reply",
              role: "assistant",
              text: "The browser and terminal now keep independent sessions. I’m checking resizing and switching between panes.",
            },
          },
        ],
      },
    },
  },
});
useApp.setState({
  info: {
    platform: "macos",
    version: "fixture",
    processStartMs: Date.now(),
    smoke: false,
    firstLaunch: false,
    budgetTolerance: 1,
  },
  connection: { status: "connected", daemon: null, reason: null },
});
const shells = new Map<string, string>();
const nativePages = new Map<string, HTMLDivElement>();
const calls: unknown[] = [];
function place(pageId: string, bounds: BrowserBounds | null) {
  const page = nativePages.get(pageId);
  if (!page) return;
  page.hidden = bounds === null;
  if (bounds)
    Object.assign(page.style, {
      left: `${bounds.x}px`,
      top: `${bounds.y}px`,
      width: `${bounds.width}px`,
      height: `${bounds.height}px`,
    });
}
mockIPC(
  (command, args) => {
    calls.push({ command, args });
    const values = args as Record<string, unknown>;
    if (command === "browser_open" || command === "browser_navigate") {
      const pageId = values.id as string;
      let page = nativePages.get(pageId);
      if (!page) {
        page = document.createElement("div");
        page.dataset.fixtureWebview = pageId;
        Object.assign(page.style, {
          position: "fixed",
          zIndex: "2",
          overflow: "auto",
          background: "#fafafa",
          color: "#202020",
          padding: "40px",
          font: "15px system-ui",
        });
        document.body.append(page);
        nativePages.set(pageId, page);
      }
      page.innerHTML = `<p style="font-size:12px;color:#666">Illustrative browser fixture</p><h1 style="font-size:28px;margin:18px 0">${String(values.url).includes("guide") ? "Project guide" : "Development preview"}</h1><p style="line-height:1.6;max-width:40ch">A local page running beside the conversation. Each browser tab keeps its own page and navigation history.</p>`;
      place(pageId, values.bounds as BrowserBounds);
      setTimeout(
        () =>
          useBrowsers.setState(({ pages }) => ({
            pages: {
              ...pages,
              [pageId]: {
                ...pages[pageId]!,
                url: values.url as string,
                title: String(values.url).includes("guide")
                  ? "Project guide"
                  : "Development preview",
                loading: false,
                blocked: null,
              },
            },
          })),
        30,
      );
    }
    if (command === "browser_place")
      place(values.id as string, values.bounds as BrowserBounds | null);
    if (command === "browser_close") {
      nativePages.get(values.id as string)?.remove();
      nativePages.delete(values.id as string);
    }
    if (command !== "ipc_request") return null;
    const req = values.request as Request;
    if (req.method === "openTerminal") {
      const key = req.sessionId ?? "default";
      const terminalId = shells.get(key) ?? `fixture-${key}`;
      shells.set(key, terminalId);
      return {
        method: req.method,
        terminal: {
          id: terminalId,
          shell: "/bin/zsh",
          cwd: "/tmp/brigadier/panes",
          scrollback:
            "stephen@workstation panes % pnpm dev\r\n\r\n  VITE v8.3.0  ready in 142 ms\r\n\r\n  ➜  Local:   http://localhost:3000/\r\n\r\nstephen@workstation panes % ",
        },
      };
    }
    if (req.method === "writeTerminal")
      emitTerminalOutput({
        type: "data",
        terminalId: req.terminalId,
        data: req.data,
      });
    if (req.method === "closeTerminal")
      for (const [key, terminalId] of shells)
        if (terminalId === req.terminalId) shells.delete(key);
    if (req.method === "listWorkerEvents")
      return { method: req.method, page: { entries: [], hasMore: false } };
    return { method: req.method };
  },
  { shouldMockEvents: true },
);

function Fixture() {
  const { panel, agents } = useSidePanel(id, "session");
  const runtime = useExternalStoreRuntime<ThreadMessage>({
    messages: [],
    onNew: async () => {},
  });
  useEffect(() => {
    Object.assign(window, {
      panes: {
        panel,
        agents,
        calls,
        shells,
        useBrowsers,
        useTerminalSessions,
        emitPaneShortcut: (shortcut: string) => emit("pane-shortcut", shortcut),
      },
    });
  }, [panel, agents]);
  const full = panel.visible && panel.state.fullscreen;
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <SidePanelContext.Provider value={panel}>
        <AgentsPanelContext.Provider value={agents}>
          <div className="flex h-screen w-full min-w-0">
            <aside className="border-border flex w-80 shrink-0 flex-col gap-5 border-r px-6 py-8">
              <h1 className="text-lg font-medium">Brigadier</h1>
              <p className="text-muted-foreground text-sm">Projects</p>
              <p className="text-sm">brigadier-ai</p>
              <p className="rounded-control bg-muted px-3 py-2 text-sm">
                Independent panes
              </p>
            </aside>
            <main
              ref={panel.workspace}
              data-slot="pane-workspace"
              className="relative flex min-w-0 flex-1 flex-col"
            >
              <div className="relative flex min-h-0 flex-1">
                <div
                  className={full ? "hidden" : "flex min-w-0 flex-1 flex-col"}
                >
                  <header className="h-titlebar flex shrink-0 items-center px-4">
                    <h2 className="min-w-0 flex-1 truncate text-sm font-medium">
                      Independent panes
                    </h2>
                    <PanelButtonsRoom besidePanel />
                  </header>
                  <div className="flex min-h-0 flex-1 flex-col gap-5 overflow-auto px-5 py-8 text-sm">
                    <p className="self-end rounded-control bg-muted px-4 py-2">
                      Give each tool its own pane.
                    </p>
                    <p>
                      The browser, workers and files share one right-side slot.
                      The terminal stays at the bottom.
                    </p>
                    <button
                      onClick={() => agents.setPanel("worker-1")}
                      className="self-start underline"
                    >
                      Build the pane controls
                    </button>
                    <button
                      onClick={() => agents.setPanel(null)}
                      className="border-border self-start rounded-control border px-3 py-2"
                    >
                      Context · 1 worker
                    </button>
                    <div className="flex-1" />
                    <ComposerPlacement>
                      <div className="bg-background border-border rounded-capsule border p-3 shadow-menu">
                        <textarea
                          aria-label="Message"
                          placeholder="Do anything"
                          className="block w-full resize-none bg-transparent outline-none"
                          rows={1}
                        />
                        <div className="text-muted-foreground mt-2 flex justify-between text-xs">
                          <span>＋　 Model　 ◇</span>
                          <button aria-label="Send message">↑</button>
                        </div>
                      </div>
                    </ComposerPlacement>
                  </div>
                </div>
                <SidePanel conversationId={id} />
                <PanelButtons />
                <FloatingComposerSlot capsule={false} />
              </div>
              <TerminalPane conversationId={id} />
            </main>
          </div>
        </AgentsPanelContext.Provider>
      </SidePanelContext.Provider>
    </AssistantRuntimeProvider>
  );
}
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <SidebarProvider defaultOpen={false}>
      <Fixture />
    </SidebarProvider>
  </TooltipProvider>,
);
