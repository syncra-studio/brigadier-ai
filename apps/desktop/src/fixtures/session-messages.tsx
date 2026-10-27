/**
 * A standalone Vite entry, excluded from the app entry and production build: two sessions that
 * message each other (S30), each thread as its CLI's session file holds it. "Release prep" ran
 * `brigadier send` to "API work"; "API work" got the message under Brigadier's header and replied.
 * The page shows the receiver, jumps from its caption to the sender's row, then both threads take
 * a new message live. IPC is mocked, no daemon. What each shows goes to
 * `<pre id="session-messages-result">`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, ConversationView as View, FileEntry, ProviderEvent, Request, ThreadTurn, ThreadView } from "@/ipc/generated";
import { openConversation } from "@/state/actions";
import { applyThreadNews } from "@/state/cliThread";
import { useApp } from "@/state/store";

document.documentElement.dataset.platform = "macos";

const SENDER = "0192aaaa-bbbb-7ccc-8ddd-0000a11ce5e5";
const RECEIVER = "0192aaaa-bbbb-7ccc-8ddd-00004f2a9c1e";
const FIRST = "sm-0123456789ab";
const SECOND = "sm-ba9876543210";
const ASK = "Can you check the orders schema before I cut the release? The migration in 0042 renames customer_id, and the export job still reads the old column on every nightly run.";
const MORE = "Also the docs.";

const conversationOf = (id: string, title: string) =>
  ({
    id, kind: "session", projectId: null, title, pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, lifecycle: "active",
    forkedFrom: null, sideOf: null, quotaWait: null,
    setup: {
      type: "session", repo: "/tmp/proj", environment: { type: "local" }, permission: "full", planMode: false,
      workersSeeUncommitted: null, orchestrator: { provider: "claude", model: "claude-opus-5-5", effort: "low" },
    },
  }) as unknown as Conversation;

const viewOf = (conversation: Conversation) =>
  ({
    conversation, context: null, messages: { messages: [], hasMore: false },
    tasks: [], approvals: [], questions: [], requests: [], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [], compactions: [], decisions: [],
    queue: { items: [], paused: false }, run: "idle", runRequest: null, head: null, ratings: {}, streaming: null, notices: [], previews: [],
  }) as unknown as View;

const at = (event: ProviderEvent, atMs: number, more: Partial<FileEntry> = {}): FileEntry => ({ event, atMs, ...more });
const user = (itemId: string, text: string, atMs: number, more: Partial<FileEntry> = {}) => at({ type: "message", itemId, role: "user", text }, atMs, more);
const reply = (itemId: string, text: string, atMs: number) => at({ type: "message", itemId, role: "assistant", text }, atMs);
const turn = (userMessage: string, startedAtMs: number, endedAtMs: number | null, requests: string[]): ThreadTurn =>
  ({ userMessage, startedAtMs, endedAtMs, end: endedAtMs === null ? null : "done", error: null, requests }) as ThreadTurn;
const header = (id: string, text: string) => `[message from session "Release prep" (${SENDER}) · ${id}]\n${text}`;
const send = (itemId: string, text: string, status: "inProgress" | "completed", output: string | null): ProviderEvent => ({
  type: "toolCall", itemId, name: "Bash", input: JSON.stringify({ command: `brigadier send 4f2a9c1e "${text}"` }), status, output,
});

const T = Date.now() - 600_000;
const views: Record<string, ThreadView> = {
  [SENDER]: {
    entries: [
      user("u1", "Ask API work to check the schema", T),
      at(send("t1", ASK, "completed", `Sent ${FIRST} to session 4f2a9c1e: it works on it.\n`), T + 2000),
      reply("a1", "Asked API work to check the schema.", T + 4000),
    ],
    turns: [turn("u1", T, T + 5000, ["r1"])],
  },
  [RECEIVER]: {
    entries: [
      user("p1", header(FIRST, ASK), T + 3000, { said: { by: "brigadier" } }),
      user("notice", "Automatic engine wake.", T + 3100, { said: { by: "brigadier" }, steer: true }),
      reply("b1", "The export job reads customer_id; I'll point it at the new column.", T + 9000),
    ],
    // Brigadier's own turn: the message carries no request of the user's.
    turns: [turn("p1", T + 3000, T + 10000, [])],
  },
};

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  const id = req.method === "threadView" ? req.conversationId : req.method === "getConversation" ? req.id : SENDER;
  switch (req.method) {
    case "getConversation": return { method: req.method, view: viewOf(useApp.getState().conversations[id]!) };
    case "threadView": return { method: req.method, view: views[id] };
    case "listFiles": return { method: req.method, files: [], truncated: false };
    default: return { method: req.method };
  }
});

useApp.setState({
  conversations: { [SENDER]: conversationOf(SENDER, "Release prep"), [RECEIVER]: conversationOf(RECEIVER, "API work") },
  connection: { status: "connected", daemon: null, reason: null },
  summaryShown: { [SENDER]: false, [RECEIVER]: false },
});

function Page() {
  const selection = useApp((s) => s.selection);
  return (
    <TooltipProvider>
      <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
        <main className="relative flex min-h-0 w-full flex-1 flex-col">
          <ConversationView selection={selection} />
        </main>
      </SidebarProvider>
    </TooltipProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Page />);

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, " ").trim() ?? null;
const all = (selector: string) => [...document.querySelectorAll<HTMLElement>(selector)];
async function until(ready: () => unknown, tries = 100): Promise<boolean> {
  for (let tried = 0; tried < tries; tried++) {
    if (ready()) return true;
    await wait(50);
  }
  return false;
}

/** The thread's messages from other sessions as it shows them. */
const received = () =>
  all("[data-slot=peer-message]").map((message) => ({
    id: message.dataset.peerMessage ?? null,
    caption: text(message.querySelector("[data-slot=peer-caption]")),
    text: text(message.querySelector("[data-flash]")),
    right: message.getBoundingClientRect().right > document.querySelector("main")!.getBoundingClientRect().width / 2,
  }));

/** The thread's rows of messages it sent other sessions. */
const sentRows = () =>
  all("[data-slot=orchestrator-step][data-kind=message]").map((row) => {
    const message = row.querySelector<HTMLElement>("[data-slot=peer-sent-text]");
    return {
      text: text(row),
      state: row.dataset.state ?? null,
      id: row.dataset.peerMessage ?? null,
      link: text(row.querySelector("[data-slot=peer-link]")),
      message: text(message),
      cut: !!message && message.scrollWidth > message.clientWidth,
      shimmer: !!row.querySelector(".shimmer"),
      // The row itself opens to nothing.
      opens: row.getAttribute("role") === "button" || !!row.querySelector("[role=button]"),
    };
  });

/** Every bubble's text: Brigadier's header never shows. */
const headers = () => all("main *").some((element) => element.childElementCount === 0 && (element.textContent ?? "").includes("[message from session"));

const seen: Record<string, unknown> = {};
try {
  // The receiver, read back: the message stands where the user's would, captioned with its sender.
  await openConversation(RECEIVER);
  await until(() => received().length > 0 && document.querySelector("[data-role=assistant]"));
  await wait(300);
  seen.receiver = { received: received(), headers: headers(), answer: text(document.querySelector("[data-slot=aui_assistant-message-content][data-answer]")), automaticNotice: document.querySelector("main")!.textContent!.includes("Automatic engine wake.") };

  // Its caption opens the sender at the row of that message, its work unfolded.
  all("[data-slot=peer-caption]")[0]?.click();
  await until(() => useApp.getState().selection?.type === "conversation" && sentRows().some((row) => row.id === FIRST));
  await wait(400);
  const selection = useApp.getState().selection;
  seen.jump = { opened: selection?.type === "conversation" ? selection.id : null, rows: sentRows() };

  // The whole message in a tooltip, as the pointer rests on it.
  const message = document.querySelector<HTMLElement>("[data-slot=peer-sent-text]");
  message?.dispatchEvent(new PointerEvent("pointermove", { bubbles: true, pointerType: "mouse" }));
  await until(() => document.querySelector("[data-slot=tooltip-content]"), 40);
  seen.tooltip = text(document.querySelector("[data-slot=tooltip-content]"));
  message?.dispatchEvent(new PointerEvent("pointerleave", { bubbles: true, pointerType: "mouse" }));

  // Live, in the sender: a new turn's send runs, then went.
  const L = Date.now();
  // The CLI's session file holds what the stream said: a read after it has the same.
  const live = (id: string, entries: FileEntry[], opened?: ThreadTurn) => {
    const view = views[id]!;
    const ids = new Set(entries.map((entry) => ("itemId" in entry.event ? entry.event.itemId : "")));
    views[id] = {
      entries: [...view.entries.filter((entry) => !("itemId" in entry.event) || !ids.has(entry.event.itemId)), ...entries],
      turns: opened ? [...view.turns, opened] : view.turns,
    };
    applyThreadNews({ conversationId: id, entries, ...(opened && { opened: true }) });
  };
  live(SENDER, [user("u2", "Tell it about the docs too", L), at(send("t2", MORE, "inProgress", null), L + 100)], turn("u2", L, null, ["r2"]));
  await until(() => sentRows().some((row) => row.state === "sending"));
  seen.sending = sentRows().find((row) => row.state === "sending") ?? null;
  live(SENDER, [at(send("t2", MORE, "completed", `Sent ${SECOND} to session 4f2a9c1e: queued into its turn.\n`), L + 100)]);
  await until(() => sentRows().some((row) => row.id === SECOND));
  seen.sentLive = sentRows().find((row) => row.id === SECOND) ?? null;

  // Live, in the receiver: the message steered into its running turn shows there, captioned.
  await openConversation(RECEIVER);
  await until(() => received().length > 0);
  live(RECEIVER, [user("p2", "Fix the export job", L + 200), user("s2", header(SECOND, MORE), L + 300, { steer: true, said: { by: "brigadier" } })], turn("p2", L + 200, null, ["r3"]));
  await until(() => received().some((each) => each.id === SECOND));
  await wait(200);
  seen.receivedLive = { received: received(), headers: headers() };
} catch (error) {
  seen.failed = String(error);
}
seen.renderError = text(document.querySelector("[data-slot=turn-render-error]"));
const result = document.createElement("pre");
result.id = "session-messages-result";
result.textContent = JSON.stringify(seen);
document.body.append(result);
