/**
 * The sidebar's session rows over a faked daemon: one row per session under its project, each
 * with its own title and state, archived sessions left out, and a click that opens the session.
 * The sessions of what was once a workspace are plain sessions too. `close=1` adds the open
 * session's tab bar: closing a working session's tab stops nothing and a click on its row opens
 * it again. The sessions that need the user come first; one that
 * starts needing the user moves up. Writes what it saw to #sessions-result (SessionsRender.test.ts reads it).
 */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { AppSidebar, AppStrip } from "@/app/AppSidebar";
import { SessionTabBar } from "@/app/conversation/SessionTabBar";
import { SidebarPanel, SidebarProvider } from "@/components/layout/sidebar";
import { TooltipProvider } from "@/components/overlays/tooltip";
import type { Conversation, ConversationActivity, EventEnvelope, Project, Request } from "@/ipc/generated";
import { fromSnapshot, useActivity } from "@/state/activity";
import { useApp } from "@/state/store";
import { Toaster } from "@/components/overlays/toast";
import { useToasts } from "@/state/toasts";
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
  session("w10-a", "Ready to review", 1),
  session("w11-a", "Paused task", 0),
];
if (params.has("unread")) for (const conversation of conversations) conversation.finishedAtMs = 1;
if (params.has("unreadOnly")) conversations.push(session("w12-a", "Unread reply", -1, { finishedAtMs: 1 }));
if (params.has("chats")) for (const conversation of conversations) { conversation.kind = "chat"; conversation.projectId = null; }
useApp.setState({
  catalogLoaded: true,
  connection: { status: "connected", daemon: null, reason: null },
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  projects: { [project]: { id: project, name: "brigadier", repos: [], createdAtMs: 0 } as unknown as Project },
  conversations: Object.fromEntries(conversations.map((each) => [each.id, each])),
  selection: params.has("manual") ? { type: "conversation", id: "w1-a" } : { type: "none" },
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
    activity("w11-a", { stopped: true }),
    activity("w9-a", { run: "running", phases: [["tool:fin-1", "merging"]] }),
  ]),
});
let seq = 0;
const envelope = (stream: string, event: EventEnvelope["event"]): EventEnvelope => ({ seq: ++seq, stream, streamSeq: seq, atMs: seq, event });
useSettledTabs.setState({ byTab: { "w8-a": { state: "reverted" }, "w10-a": { state: "ready" } } });
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

const plainStatus = (status: HTMLElement) => {
  const blank = document.body.appendChild(document.createElement("span"));
  const defaults = getComputedStyle(blank);
  const style = getComputedStyle(status);
  const plain = style.backgroundColor === defaults.backgroundColor && style.borderRadius === defaults.borderRadius;
  blank.remove();
  return plain;
};

const rows = () => [...document.querySelectorAll<HTMLElement>('[aria-expanded] ~ * li, li li')]
  .filter((row, index, all) => all.indexOf(row) === index)
  .map((row) => {
    const button = row.querySelector<HTMLButtonElement>("button")!;
    const status = row.querySelector<HTMLElement>('[data-slot="row-status"]');
    return {
      title: button.querySelector('[data-slot="row-title"]')?.textContent?.trim(),
      status: status ? `${status.dataset.status}:${status.textContent?.trim() ?? ""}` : null,
      tip: status?.title ?? null,
      active: button.dataset.active === "true",
      mark: status?.querySelector('[data-slot="status-spinner"]') ? "spinner" : status?.querySelector('[data-slot="status-unread"]') ? "unread" : null,
      error: !!button.querySelector('[data-slot="status-error"]'),
      plain: status ? plainStatus(status) : true,
      words: status?.querySelector('[data-slot="status-word"]')?.textContent ?? null,
    };
  });

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (check: () => boolean) => { for (let n = 0; n < 200 && !check(); n++) await wait(20); };
const rowButton = (title: string) => [...document.querySelectorAll<HTMLButtonElement>("li li button")].find((button) => button.textContent?.includes(title));
const projectHeader = () => {
  const button = document.querySelector<HTMLButtonElement>('li > div > button[aria-expanded]')!;
  return { text: button.textContent?.trim(), icons: button.querySelectorAll("svg").length };
};
/** Anything worker-shaped in the sidebar: a session's workers show in the session, never under its row. */
const workerRows = () => [...document.querySelectorAll<HTMLElement>('[data-slot^="session-worker"], [data-task]')].map((row) => row.textContent?.trim());
/**
 * How each row with a state shares its width: the title's share of the row's inside in percent, and
 * whether the state's own words ("Needs you") show whole, the reason being cut first.
 */
const rowWidths = () => [...document.querySelectorAll<HTMLElement>('[data-slot="row-status"][data-status]')]
  .filter((status) => status.dataset.status !== "unread")
  .map((status) => {
    const button = status.closest("button")!;
    const title = button.querySelector('[data-slot="row-title"]')!;
    const words = status.querySelector<HTMLElement>('[data-slot="status-word"]')!;
    const style = getComputedStyle(button);
    const inside = button.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
    return {
      title: title.textContent?.trim(),
      titleShare: Math.round((title.getBoundingClientRect().width / inside) * 100),
      stateWhole: words.scrollWidth <= words.clientWidth,
    };
  });
const rowStatus = (title: string) => rows().find((row) => row.title === title)?.status ?? null;

/** The toasts on screen, by their text. */
const toastTexts = () => [...document.querySelectorAll<HTMLElement>("[data-slot=toast]")].map((each) => each.textContent?.trim());
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
  // It says where it went, as a toast on screen, with no dialog.
  await until(() => document.querySelector('[role="dialog"], [role="alertdialog"]') !== null || toastTexts().length > 0);
  seen.notes = toastTexts();
  seen.dialog = document.querySelector('[role="dialog"], [role="alertdialog"]') !== null;
  seen.statusAfter = rowStatus("Lone worker");
  seen.barAfter = document.querySelector('[data-slot="session-tabs"]') !== null;
  rowButton("Lone worker")!.click();
  await until(() => close() !== null);
  seen.reopened = useApp.getState().selection;
  const chatTab = document.querySelector('[data-slot="session-tabs"] [data-tab-id="chat"] [role="tab"]');
  seen.chatTab = chatTab?.querySelector(".session-tab-title")?.textContent?.trim();
  // Beside the title, only the sidebar row's marks: no words.
  seen.chatTabStatus = chatTab?.textContent?.replace(seen.chatTab as string, "").trim();
  const tabStatus = chatTab?.querySelector('[data-slot="status-spinner"]')?.parentElement;
  seen.chatTabTip = chatTab?.getAttribute("title");
  seen.chatTabMark = chatTab?.querySelector('[data-slot="status-spinner"]') ? "spinner" : null;
  seen.chatTabStatusShare = tabStatus && chatTab ? Math.round((tabStatus.getBoundingClientRect().width / chatTab.clientWidth) * 100) : null;
  // An idle session's tab closes quietly: nothing still runs to reopen.
  useToasts.setState({ toasts: [] });
  rowButton("Old fix")!.click();
  await until(() => document.querySelector('[data-slot="session-tabs"] button[aria-label="Close Old fix tab"]') !== null);
  document.querySelector<HTMLButtonElement>('[data-slot="session-tabs"] button[aria-label="Close Old fix tab"]')!.click();
  await until(() => useApp.getState().selection.type !== "conversation");
  await wait(100);
  seen.idleNotes = useToasts.getState().toasts.map((each) => each.text);
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
    if (params.has("states")) {
      void (async () => {
        const states = [];
        await until(() => document.querySelector('[data-slot="row-status"]') !== null);
        for (const conversation of conversations.filter((each) => each.lifecycle === "active")) {
          useApp.setState({ selection: { type: "none" } });
          const row = [...document.querySelectorAll<HTMLElement>('[data-slot="row-status"]')].find((status) => status.closest("button")?.querySelector('[data-slot="row-title"]')?.textContent === conversation.title)!;
          await until(() => row.closest("button")?.dataset.active !== "true");
          const beforeMark = row.querySelector('[data-slot="status-spinner"]') ? "spinner" : row.querySelector('[data-slot="status-unread"]') ? "unread" : null;
          const unread = row.querySelector<HTMLElement>('[data-slot="status-unread"]');
          const spinner = row.querySelector<HTMLElement>('[data-slot="status-spinner"]');
          const error = row.closest("button")?.querySelector<HTMLElement>('[data-slot="status-error"]');
          const info = { beforeMark, error: !!error, plain: plainStatus(row),
            unreadSize: unread?.getBoundingClientRect().width ?? null, unreadSlot: unread?.parentElement?.getBoundingClientRect().width ?? null,
            spinnerSize: spinner ? Number.parseFloat(getComputedStyle(spinner).width) : null, spinnerSlot: spinner?.parentElement?.getBoundingClientRect().width ?? null,
            spinnerDuration: spinner ? getComputedStyle(spinner).animationDuration : null,
            errorSize: error?.querySelector("svg")?.getBoundingClientRect().width ?? null, errorSlot: error?.getBoundingClientRect().width ?? null,
            leadingError: error ? error.compareDocumentPosition(row.closest("button")!.querySelector('[data-slot="row-title"]')!) & Node.DOCUMENT_POSITION_FOLLOWING : null,
          };
          useApp.setState({ selection: { type: "conversation", id: conversation.id } });
          await until(() => document.querySelector('[data-tab-id="chat"] .session-tab-title')?.textContent === conversation.title);
          const tab = document.querySelector<HTMLElement>('[data-tab-id="chat"] [role="tab"]')!;
          // The tab's marks are the sidebar row's own: its spinner or unread dot, its error glyph.
          const mark = tab.querySelector<HTMLElement>('[data-slot="status-spinner"], [data-slot="status-unread"]')?.parentElement;
          states.push({ ...info, title: conversation.title, rowWords: row.querySelector('[data-slot="status-word"]')?.textContent,
            state: row.dataset.status, words: tab.textContent?.replace(conversation.title, "").trim(), tip: tab.title, rowTip: row.title,
            rowMark: row.querySelector('[data-slot="status-spinner"]') ? "spinner" : row.querySelector('[data-slot="status-unread"]') ? "unread" : null,
            mark: tab.querySelector('[data-slot="status-spinner"]') ? "spinner" : tab.querySelector('[data-slot="status-unread"]') ? "unread" : null,
            dot: tab.querySelector('[data-slot="status-dot"]') !== null,
            tabError: tab.querySelector('[data-slot="status-error"]') !== null,
            width: mark?.getBoundingClientRect().width ?? 0, titleFlex: getComputedStyle(tab.querySelector('.session-tab-title')!).flexGrow,
          });
        }
        setResult(JSON.stringify(states));
      })();
      return;
    }
    if (params.has("close")) {
      let disposed = false;
      void closeAndReopen().then((seen) => { if (!disposed) setResult(JSON.stringify(seen)); });
      return () => { disposed = true; };
    }
    if (params.has("flyout")) {
      const timer = setTimeout(async () => {
        document.querySelector<HTMLButtonElement>('button[aria-label="Projects"]')!.click();
        await until(() => Boolean(document.querySelector('button[title="New session in brigadier"]')));
        const header = document.querySelector<HTMLButtonElement>('button[title="New session in brigadier"]')!;
        const list = header.parentElement!;
        setResult(JSON.stringify({
          projectHeader: { text: header.textContent?.trim(), icons: header.querySelectorAll("svg").length },
          sessions: [...list.querySelectorAll('[data-slot="row-title"]')].map((title) => title.textContent?.trim()),
        }));
      }, 300);
      return () => clearTimeout(timer);
    }
    if (params.has("manual")) return;
    const timer = setTimeout(async () => {
      const seen: Record<string, unknown> = { rows: rows(), widths: rowWidths(), projectHeader: projectHeader(), workers: workerRows() };
      rowButton("Header")!.click();
      setTimeout(() => {
        seen.opened = useApp.getState().selection;
        seen.activeAfter = rows().filter((row) => row.active).map((row) => row.title);
        // "Old fix" asks for an approval: it moves up among those that need the user.
        useActivity.setState((state) => ({ byConversation: { ...state.byConversation, ...Object.fromEntries([activity("w8-a", { approvals: ["a-2"] })]) } }));
        setTimeout(() => {
          seen.rowsAfter = rows().map((row) => row.title);
          seen.projectHeaderAfter = projectHeader();
          setResult(JSON.stringify(seen));
        }, 100);
      }, 100);
    }, 300);
    return () => clearTimeout(timer);
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-chrome flex h-screen flex-col">
        <SidebarProvider className="relative min-h-0 flex-1 flex-col" defaultOpen={!params.has("flyout")}>
          <div className="relative flex min-h-0 min-w-0 flex-1">
            <SidebarPanel strip={<AppStrip />}>
              <AppSidebar />
            </SidebarPanel>
            <main className="bg-background flex-1">{(params.has("close") || params.has("manual") || params.has("states")) && <OpenBar />}</main>
          </div>
        </SidebarProvider>
      </div>
      <Toaster className="top-2" />
      {result && <pre id="sessions-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
