/**
 * The sidebar's session rows over a faked daemon: one row per session under its project, each
 * with its own title and state, archived sessions left out, and a click that opens the session.
 * The sessions of what was once a workspace are plain sessions too. `close=1` adds the open
 * session's tab bar: closing a working session's tab stops nothing and a click on its row opens
 * it again. Writes what it saw to #sessions-result (SessionsRender.test.ts reads it).
 */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { AppSidebar, AppStrip } from "@/app/AppSidebar";
import { SessionTabBar } from "@/app/conversation/SessionTabBar";
import { SidebarPanel, SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, EventEnvelope, Project, Request } from "@/ipc/generated";
import { applyActivityEvents } from "@/state/activity";
import { useApp } from "@/state/store";
import { applySettledTabEvents } from "@/state/tabStates";

const params = new URLSearchParams(location.search);
const requests: string[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const { method } = (payload as { request: Request }).request;
  requests.push(method);
  return { method };
});

const project = "p-1";
const session = (id: string, title: string, updatedAtMs: number, extra: Partial<Conversation> = {}): Conversation => ({
  id, kind: "session", projectId: project, title, pinnedAtMs: null, createdAtMs: updatedAtMs, updatedAtMs, setup: null,
  lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null, ...extra,
});
const conversations = [
  // Two sessions working that once shared a workspace: a row each; an archived third is left out.
  session("w1-a", "Header", 10),
  session("w1-b", "Footer", 11),
  session("w1-gone", "Gone", 12, { lifecycle: "archived" }),
  session("w2-a", "Fix the login", 9),
  session("w3-a", "Tidy the docs", 8),
  session("w5-a", "Lone worker", 5),
];
useApp.setState({
  catalogLoaded: true,
  connection: { status: "connected", daemon: null, reason: null },
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  projects: { [project]: { id: project, name: "brigadier", repos: [], createdAtMs: 0 } as unknown as Project },
  conversations: Object.fromEntries(conversations.map((each) => [each.id, each])),
  selection: { type: "none" },
});
let seq = 0;
const envelope = (stream: string, event: EventEnvelope["event"]): EventEnvelope => ({ seq: ++seq, stream, streamSeq: seq, atMs: seq, event });
applyActivityEvents(["w1-a", "w1-b", "w5-a"].map((id) =>
  envelope(`conversation:${id}`, { type: "runStateChanged", conversationId: id, state: "running", error: null, requestId: null })));
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

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (check: () => boolean) => { for (let n = 0; n < 200 && !check(); n++) await wait(20); };
const rowButton = (title: string) => [...document.querySelectorAll<HTMLButtonElement>("li li button")].find((button) => button.textContent?.includes(title));
const rowStatus = (title: string) => rows().find((row) => row.title === title)?.status ?? null;

const close = () => document.querySelector<HTMLButtonElement>('[data-slot="session-tabs"] button[aria-label="Close Lone worker tab"]');

/** Opens the lone working session, closes its tab with ×, then opens it again from its row. */
async function closeAndReopen() {
  const seen: Record<string, unknown> = {};
  await until(() => rowButton("Lone worker") !== undefined);
  rowButton("Lone worker")!.click();
  await until(() => close() !== null);
  seen.opened = useApp.getState().selection;
  seen.statusBefore = rowStatus("Lone worker");
  requests.length = 0;
  close()!.click();
  await until(() => useApp.getState().selection.type !== "conversation");
  await wait(100);
  seen.afterClose = useApp.getState().selection;
  seen.requestsOnClose = [...requests];
  seen.statusAfter = rowStatus("Lone worker");
  seen.barAfter = document.querySelector('[data-slot="session-tabs"]') !== null;
  rowButton("Lone worker")!.click();
  await until(() => close() !== null);
  seen.reopened = useApp.getState().selection;
  seen.chatTab = document.querySelector('[data-slot="session-tabs"] [data-tab-id="chat"] [role="tab"]')?.textContent?.trim();
  return seen;
}

function OpenBar() {
  const selection = useApp((s) => s.selection);
  const conversation = useApp((s) => (selection.type === "conversation" ? s.conversations[selection.id] : undefined));
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  if (!conversation) return null;
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <SessionTabBar conversation={conversation} onRename={() => {}} />
    </AssistantRuntimeProvider>
  );
}

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (params.has("close")) {
      let disposed = false;
      void closeAndReopen().then((seen) => { if (!disposed) setResult(JSON.stringify(seen)); });
      return () => { disposed = true; };
    }
    const timer = setTimeout(() => {
      const seen: Record<string, unknown> = { rows: rows() };
      rowButton("Header")!.click();
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
            <main className="bg-background flex-1">{params.has("close") && <OpenBar />}</main>
          </div>
        </SidebarProvider>
      </div>
      {result && <pre id="sessions-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
