/**
 * The sidebar's workspace rows over a faked daemon (WORKSPACES-PLAN.md "States"): one row per
 * workspace under its project, with the strongest state of its tabs and how many share it, the
 * workspace's title when it has several tabs and the tab's own when it has one, archived tabs
 * left out, and a click that opens the tab had open last. Writes what it saw to
 * #workspaces-result (WorkspacesRender.test.ts reads it).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { AppSidebar, AppStrip } from "@/app/AppSidebar";
import { SidebarPanel, SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, EventEnvelope, Project, Request, Workspace } from "@/ipc/generated";
import { applyActivityEvents } from "@/state/activity";
import { useLastOpened } from "@/state/lastOpened";
import { useApp } from "@/state/store";
import { applySettledTabEvents, useSettledTabs } from "@/state/tabStates";

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  return { method: (payload as { request: Request }).request.method };
});

const project = "p-1";
const tab = (id: string, title: string, workspaceId: string | undefined, updatedAtMs: number, extra: Partial<Conversation> = {}): Conversation => ({
  id, kind: "session", projectId: project, title, pinnedAtMs: null, createdAtMs: updatedAtMs, updatedAtMs, setup: null,
  lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
  ...(workspaceId ? { workspaceId } : {}), ...extra,
});
const workspace = (id: string, title: string): Workspace => ({
  id, projectId: project, title, repos: [], base: "main", createdAtMs: 0, updatedAtMs: 0, pinnedAtMs: null, lifecycle: "active",
});
const conversations = [
  // Two tabs working: "2 working", under the workspace's title; an archived third is left out.
  tab("w1-a", "Header", "ws-1", 10),
  tab("w1-b", "Footer", "ws-1", 11),
  tab("w1-gone", "Gone", "ws-1", 12, { lifecycle: "archived" }),
  // One tab, ready: the tab's own title.
  tab("w2-a", "Fix the login", "ws-2", 9),
  // One tab, merged.
  tab("w3-a", "Tidy the docs", "ws-3", 8),
  // A quota wait beats a ready sibling.
  tab("w4-a", "Waiting one", "ws-4", 7, { quotaWait: { reason: "limit", resetsAtMs: null, rule: null, ranking: null, sinceMs: 0 } }),
  tab("w4-b", "Ready one", "ws-4", 6),
  // A lone tab working shows as its row did.
  tab("w5-a", "Lone worker", "ws-5", 5),
];
useApp.setState({
  catalogLoaded: true,
  connection: { status: "connected", daemon: null, reason: null },
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  projects: { [project]: { id: project, name: "brigadier", repos: [], createdAtMs: 0 } as unknown as Project },
  workspaces: Object.fromEntries(
    [workspace("ws-1", "Redesigning the header"), workspace("ws-2", "Login"), workspace("ws-3", "Docs"), workspace("ws-4", "Waiting on a limit"), workspace("ws-5", "Lone")]
      .map((each) => [each.id, each]),
  ),
  conversations: Object.fromEntries(conversations.map((each) => [each.id, each])),
  selection: { type: "none" },
});
// Footer was open last: the row opens it.
useLastOpened.setState({ at: { "w1-a": 1, "w1-b": 2 } });
let seq = 0;
const envelope = (stream: string, event: EventEnvelope["event"]): EventEnvelope => ({ seq: ++seq, stream, streamSeq: seq, atMs: seq, event });
applyActivityEvents(["w1-a", "w1-b", "w5-a"].map((id) =>
  envelope(`conversation:${id}`, { type: "runStateChanged", conversationId: id, state: "running", error: null, requestId: null })));
useSettledTabs.setState({ byTab: { "w4-b": "ready" } });
applySettledTabEvents([
  envelope("conversation:w2-a", { type: "requestReady", conversationId: "w2-a", requestId: "r-1", head: "abc" }),
  envelope("conversation:w3-a", { type: "requestReady", conversationId: "w3-a", requestId: "r-2", head: "def" }),
  envelope("conversation:w3-a", {
    type: "orchestratorStepped",
    step: { requestId: null, atMs: 1, position: 1, kind: { type: "merged", branch: "b", base: "main", commits: 1, askedIn: null } },
  }),
]);

const rows = () => [...document.querySelectorAll<HTMLElement>('[aria-expanded] ~ * li, li li')]
  .filter((row, index, all) => all.indexOf(row) === index)
  .map((row) => {
    const button = row.querySelector<HTMLButtonElement>("button")!;
    const status = row.querySelector<HTMLElement>('[data-slot="row-status"]');
    return {
      title: button.querySelector("span")?.textContent?.trim(),
      status: status ? `${status.dataset.status}:${status.textContent?.trim() ?? ""}` : null,
      active: button.dataset.active === "true",
    };
  });

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    const timer = setTimeout(() => {
      const seen: Record<string, unknown> = { rows: rows() };
      const header = [...document.querySelectorAll<HTMLButtonElement>("li li button")].find((button) => button.textContent?.includes("Redesigning the header"))!;
      header.click();
      setTimeout(() => {
        seen.opened = useApp.getState().selection;
        seen.activeAfter = rows().filter((row) => row.active).map((row) => row.title);
        setResult(JSON.stringify(seen));
      }, 100);
    }, 300);
    return () => clearTimeout(timer);
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-chrome flex h-screen flex-col">
        <SidebarProvider className="relative min-h-0 flex-1 flex-col" defaultOpen>
          <div className="relative flex min-h-0 min-w-0 flex-1">
            <SidebarPanel strip={<AppStrip />}>
              <AppSidebar />
            </SidebarPanel>
            <main className="bg-background flex-1" />
          </div>
        </SidebarProvider>
      </div>
      {result && <pre id="workspaces-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
