// oxlint-disable react/refs -- The fixture exposes the real panel controller to browser checks.
/** Production terminal and panel controller, synthetic IPC; never contacts a daemon. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { SidePanelContext, useSidePanel } from "@/app/conversation/SidePanel";
import { TerminalPane } from "@/app/conversation/TerminalTab";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, Project, Request } from "@/ipc/generated";
import { useApp } from "@/state/store";
import { useTerminalSessions } from "@/state/terminalSessions";
import { emitTerminalOutput } from "@/state/terminals";

const id = "terminal-fixture";
const name = new URLSearchParams(location.search).get("name") ?? "brigadier-ai";
useApp.setState({
  info: {
    platform: "macos",
    version: "fixture",
    processStartMs: 0,
    smoke: false,
    firstLaunch: false,
    budgetTolerance: 1,
  },
  connection: { status: "connected", daemon: null, reason: null },
  conversations: { [id]: { id, projectId: "project" } as Conversation },
  projects: {
    project: {
      id: "project",
      repos: [{ path: `/code/${name}`, name }],
    } as Project,
  },
});
const shells = new Map<string, string>();
const calls: Request[] = [];
mockIPC((command, args) => {
  if (command !== "ipc_request") return null;
  const req = (args as { request: Request }).request;
  calls.push(req);
  if (req.method === "openTerminal") {
    const key = req.sessionId ?? "default";
    const terminalId = shells.get(key) ?? `shell-${key}`;
    shells.set(key, terminalId);
    return {
      method: req.method,
      terminal: {
        id: terminalId,
        shell: "/bin/zsh",
        cwd: "/tmp/session-4f2d486d",
        scrollback: `stephen@Stephens-MacBook-Pro ${name} % \r\n`,
      },
    };
  }
  if (req.method === "closeTerminal") {
    for (const [key, value] of shells)
      if (value === req.terminalId) shells.delete(key);
  }
  if (req.method === "writeTerminal")
    emitTerminalOutput({
      type: "data",
      terminalId: req.terminalId,
      data: req.data,
    });
  return { method: req.method };
});

function Fixture() {
  const { panel } = useSidePanel(id, "session");
  useEffect(() => {
    Object.assign(window, {
      terminalFixture: { panel, shells, calls, useTerminalSessions },
    });
  }, [panel]);
  return (
    <SidePanelContext.Provider value={panel}>
      <main
        ref={panel.workspace}
        className="flex h-screen w-full min-w-0 flex-col"
      >
        <div className="flex min-h-0 flex-1 flex-col items-start gap-4 p-4">
          <button type="button" onClick={() => panel.openTab("terminal")}>
            Open terminal
          </button>
          <div data-slot="composer">
            <textarea aria-label="Message" />
          </div>
        </div>
        <TerminalPane conversationId={id} />
      </main>
    </SidePanelContext.Provider>
  );
}
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <SidebarProvider defaultOpen={false}>
      <Fixture />
    </SidebarProvider>
  </TooltipProvider>,
);
