/**
 * A standalone Vite entry, excluded from the app entry and production build: starting a message
 * as a new session in the background from a project session's composer (issue #158), IPC
 * mocked. The Send tooltip names ⌘⏎, the arrow turns while ⌘ is held, ⌘-click and ⌘⏎ start a
 * session, and `/` › Start as a new session sets a New session chip for the next send. What it
 * saw goes as JSON into `<pre id="composer-background-result">`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { fixtureInput } from "@/fixtures/input";
import type { Conversation, ConversationView as View, Project, Request } from "@/ipc/generated";
import { select } from "@/state/actions";
import { markBackgroundSend } from "@/state/backgroundSend";
import { useApp } from "@/state/store";

const id = "background-session";
const start = Date.now() - 600_000;
const session = {
  id, kind: "session", projectId: "p1", title: "Background", pinnedAtMs: null, createdAtMs: start, updatedAtMs: start,
  lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null, setup: null,
} as unknown as Conversation;
const project = { id: "p1", name: "app", createdAtMs: start, repos: [{ path: "/tmp/app", name: "app" }], prefs: {}, trust: [] } as unknown as Project;

/** Where each message went: `here`, or `new` for a new background session. */
const sent: string[] = [];
let sequence = 10;
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getConversation":
      return {
        method: req.method,
        view: {
          conversation: useApp.getState().conversations[req.id] ?? session, context: null,
          messages: { messages: [], hasMore: false }, tasks: [], approvals: [], questions: [],
          requests: [], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [],
          compactions: [], decisions: [], queue: { items: [], paused: false }, run: "idle",
          runRequest: null, head: null, ratings: {}, streaming: null, notices: [],
          previews: [],
        } as unknown as View,
      };
    case "listFiles":
      return { method: req.method, files: [], truncated: false };
    case "startSession": {
      sent.push(`new ${req.text}`);
      return { method: req.method, conversation: { ...session, id: `started-${sequence++}`, title: req.text } };
    }
    case "sendMessage": {
      sent.push(`here ${req.text}`);
      const message = {
        id: `sent-${sequence}`, conversationId: req.conversationId, seq: sequence++, role: "user", text: req.text, blob: null,
        attachments: req.attachments, mentions: req.mentions, createdAtMs: Date.now(), model: null,
        requestId: `sent-request-${sequence}`, parentId: null,
      };
      return { method: req.method, outcome: { type: "sent", message } };
    }
    default:
      return { method: req.method };
  }
});
const state = useApp.getState();
useApp.setState({
  conversations: { [id]: session },
  projects: { p1: project },
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { status: "connected", daemon: null, reason: null },
});

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(what: string, check: () => boolean, ms = 10_000) {
  const end = performance.now() + ms;
  while (!check()) {
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}: ${document.querySelector("[data-slot=composer]")?.textContent ?? "no composer"}`);
    await sleep(16);
  }
}
const field = () => document.querySelector<HTMLElement>("[data-slot=composer-input] [contenteditable=true]");
const typed = () => field()?.textContent ?? "";
const key = (name: string, modifiers?: ("Alt" | "Control" | "Meta" | "Shift")[]) =>
  fixtureInput({ type: "key", key: name, ...(modifiers ? { modifiers } : {}) });
const type = (text: string) => fixtureInput({ type: "type", text });
const sendButton = () => document.querySelector<HTMLButtonElement>(".aui-composer-send")!;
const newArrow = () => sendButton().querySelector("[data-slot=send-new-session]") !== null;
const chip = () => document.querySelector<HTMLButtonElement>("[data-slot=new-session-chip]");
/** ⌘ pressed or let go, as the window hears it. */
const command = (down: boolean) =>
  window.dispatchEvent(new KeyboardEvent(down ? "keydown" : "keyup", { key: "Meta", metaKey: down, bubbles: true }));

async function tooltip(): Promise<string> {
  const box = sendButton().getBoundingClientRect();
  await fixtureInput({ type: "pointer", x: box.x + box.width / 2, y: box.y + box.height / 2 });
  await until("the tooltip", () => document.querySelector("[data-slot=tooltip-content]") !== null);
  const text = document.querySelector("[data-slot=tooltip-content]")!.textContent ?? "";
  await fixtureInput({ type: "pointer", x: 5, y: 5 });
  await until("the tooltip gone", () => document.querySelector("[data-slot=tooltip-content]") === null);
  return text;
}

/** Sends with `how`, and returns where the message went. */
async function sendWith(how: () => Promise<void> | void): Promise<string> {
  const before = sent.length;
  await how();
  await until("the send", () => sent.length > before);
  await until("the field emptied", () => typed() === "");
  return sent.at(-1)!;
}

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    await until("the field", () => field() !== null);
    field()!.focus();
    await type("one");
    seen.tooltip = await tooltip();
    // Holding ⌘ turns the arrow, letting go turns it back.
    seen.arrowBefore = newArrow();
    command(true);
    await until("the arrow turned", newArrow);
    seen.heldTooltip = await tooltip();
    command(false);
    await until("the arrow back", () => !newArrow());
    // ⌘-click starts a new session; a plain click sends here.
    seen.commandClick = await sendWith(() => {
      sendButton().dispatchEvent(new MouseEvent("click", { bubbles: true, metaKey: true }));
    });
    // A ⌘-click whose send never came leaves no mark for the next plain click.
    markBackgroundSend(id);
    await type("two");
    seen.click = await sendWith(() => sendButton().click());
    await type("three");
    seen.commandEnter = await sendWith(() => key("Enter", ["Meta"]));

    // `/` › Start as a new session: a New session chip, and the next send goes to a new session.
    await type("/backgr");
    await until("the / menu", () => document.querySelector("[data-slot=composer-commands]")?.textContent?.includes("Start as a new session") ?? false);
    await key("Enter");
    await until("the chip", () => chip() !== null);
    await type("four");
    await until("the arrow turned by the chip", newArrow);
    seen.chipSend = await sendWith(() => key("Enter"));
    seen.chipAfter = chip() !== null;
    // A click on the chip takes it back unsent.
    await type("/backgr");
    await until("the / menu again", () => document.querySelector("[data-slot=composer-commands]") !== null);
    await key("Enter");
    await until("the chip again", () => chip() !== null);
    chip()!.click();
    await until("the chip gone", () => chip() === null);
    field()!.focus();
    await type("five");
    seen.afterChip = await sendWith(() => key("Enter"));
  } catch (error) {
    seen.error = String(error);
  }
  const result = document.createElement("pre");
  result.id = "composer-background-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

await select({ type: "conversation", id });
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
      <main className="relative flex min-h-0 w-full flex-1 flex-col">
        <ConversationView key={id} selection={{ type: "conversation", id }} />
      </main>
    </SidebarProvider>
  </TooltipProvider>,
);
void drive();
