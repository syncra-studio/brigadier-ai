/**
 * The sidebar's session rows over a faked daemon: one row per session under its project, each
 * with its own title and state, archived sessions left out, and a click that opens the session.
 * The sessions of what was once a workspace are plain sessions too. `close=1` adds the open
 * session's tab bar: closing a working session's tab stops nothing and a click on its row opens
 * it again. The sessions that need the user come first, and the project counts them; one that
 * starts needing the user moves up. Writes what it saw to #sessions-result (SessionsRender.test.ts reads it).
 */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { AppSidebar, AppStrip } from "@/app/AppSidebar";
import { SessionTabBar } from "@/app/conversation/SessionTabBar";
import { SidebarPanel, SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, ConversationActivity, EventEnvelope, Project, Request } from "@/ipc/generated";
import { fromSnapshot, useActivity } from "@/state/activity";
import { useApp } from "@/state/store";
import { useOpenWorker } from "@/state/openWorker";
import { applySettledTabEvents, useSettledTabs } from "@/state/tabStates";

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
// Resets at 14:05 today, as the "waiting for reset" row says.
const RESET = new Date(new Date().setHours(14, 5, 0, 0)).getTime();
const conversations = [
  // Two sessions working that once shared a workspace: a row each; an archived third is left out.
  session("w1-a", "Header", 10),
  session("w1-b", "Footer", 11),
  session("w1-gone", "Gone", 12, { lifecycle: "archived" }),
  session("w2-a", "Fix the login", 9),
  session("w3-a", "Tidy the docs", 8),
  session("w4-a", "Waiting on a limit", 7, { quotaWait: { reason: "limit", resetsAtMs: RESET, rule: null, ranking: null, sinceMs: 0 } }),
  session("w6-a", "Deploy the site", 6),
  session("w7-a", "Broken build", 4),
  session("w8-a", "Old fix", 3),
  session("w9-a", "Ship the release", 2),
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
const activity = (id: string, extra: Partial<ConversationActivity>): [string, ReturnType<typeof fromSnapshot>] =>
  [id, fromSnapshot({ conversationId: id, run: "idle", tasks: [], approvals: [], questions: [], phases: [], ...extra })];
useActivity.setState({
  byConversation: Object.fromEntries([
    activity("w1-a", { run: "running", tasks: [["t-1", "running"], ["t-2", "queued"]], titles: [["t-1", "Header layout"], ["t-2", "Header tests"]] }),
    activity("w1-b", { run: "running", phases: [["ci:o/n main abc", "watchingCi"]] }),
    activity("w5-a", { run: "running" }),
    activity("w6-a", { approvals: ["a-1"] }),
    activity("w7-a", { failed: true }),
    activity("w9-a", { run: "running", phases: [["tool:fin-1", "merging"]] }),
  ]),
});
let seq = 0;
const envelope = (stream: string, event: EventEnvelope["event"]): EventEnvelope => ({ seq: ++seq, stream, streamSeq: seq, atMs: seq, event });
useSettledTabs.setState({ byTab: { "w8-a": { state: "reverted" } } });
applySettledTabEvents([
  envelope("conversation:w2-a", { type: "requestReady", conversationId: "w2-a", requestId: "r-1", head: "abc" }),
  envelope("conversation:w2-a", { type: "pullRequestSeen", conversationId: "w2-a", pullRequest: { number: 7, title: "Login", url: "https://github.com/o/n/pull/7", state: "open" } }),
  envelope("conversation:w2-a", {
    type: "orchestratorStepped",
    step: { requestId: "r-1", atMs: 1, position: 1, kind: { type: "proof", id: "p-1", name: "login.png", mime: "image/png" } },
  }),
  envelope("conversation:w3-a", { type: "requestReady", conversationId: "w3-a", requestId: "r-2", head: "def" }),
  envelope("conversation:w3-a", {
    type: "orchestratorStepped",
    step: { requestId: null, atMs: 1, position: 1, kind: { type: "merged", branch: "b", base: "main", commits: 1, askedIn: null } },
  }),
]);

const rows = () => [...document.querySelectorAll<HTMLElement>('[aria-expanded] ~ * li, li li')]
  .filter((row, index, all) => all.indexOf(row) === index && row.dataset.slot !== "session-workers")
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
const needYou = () => {
  const count = document.querySelector<HTMLElement>('[data-slot="need-you-count"]');
  return count ? { text: count.textContent?.trim(), title: count.title } : null;
};
/** The worker rows under a session's row, and their fold. */
const workerRows = () => [...document.querySelectorAll<HTMLElement>('[data-slot="session-worker"]')].map((row) => row.textContent?.trim());
const workersFold = () => document.querySelector<HTMLButtonElement>('[data-slot="session-workers"] > button[aria-expanded]');
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
  const chatTab = document.querySelector('[data-slot="session-tabs"] [data-tab-id="chat"] [role="tab"]');
  seen.chatTab = chatTab?.querySelector(".session-tab-title")?.textContent?.trim();
  seen.chatTabStatus = chatTab?.querySelector('[data-slot="tab-status"]')?.textContent?.trim();
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
    const timer = setTimeout(async () => {
      const seen: Record<string, unknown> = { rows: rows(), count: needYou(), workers: workerRows(), fold: workersFold()?.textContent?.trim() };
      // A worker's row opens its session with that worker asked for in its Workers panel.
      document.querySelector<HTMLButtonElement>('[data-slot="session-worker"][data-task="t-2"]')!.click();
      seen.workerOpened = { selection: useApp.getState().selection, worker: useOpenWorker.getState().request };
      useApp.setState({ selection: { type: "none" } });
      workersFold()!.click();
      await until(() => workerRows().length === 0);
      seen.workersFolded = workerRows();
      workersFold()!.click();
      await until(() => workerRows().length > 0);
      rowButton("Header")!.click();
      setTimeout(() => {
        seen.opened = useApp.getState().selection;
        seen.activeAfter = rows().filter((row) => row.active).map((row) => row.title);
        // "Old fix" asks for an approval: it moves up among those that need the user.
        useActivity.setState((state) => ({ byConversation: { ...state.byConversation, ...Object.fromEntries([activity("w8-a", { approvals: ["a-2"] })]) } }));
        setTimeout(() => {
          seen.rowsAfter = rows().map((row) => row.title);
          seen.countAfter = needYou();
          setResult(JSON.stringify(seen));
        }, 100);
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
