/**
 * A standalone Vite entry, excluded from the app entry and production build: the composer's
 * small behaviours in the app's own conversation view, one case per page load (`?case=`). A Chat
 * whose one request is over; IPC is mocked and records what the composer sends. It drives the
 * real keys (`fixtureInput`) and writes what it saw as JSON into `<pre id="composer-polish-result">`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/ConversationView";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, ConversationView as View, Message, Request, UserRequest } from "@/ipc/generated";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import { select } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

const id = "polish-chat";
const start = Date.now() - 600_000;
const which = new URLSearchParams(location.search).get("case");
// A draft for the cases that start with one (the editor then has text to put its caret in).
localStorage.clear();
if (which === "never") {
  localStorage.setItem("brigadier.drafts", JSON.stringify({ [id]: { text: "half a thought", attachments: [], mentions: [], atMs: Date.now() } }));
}

const conversation = {
  id, kind: "chat", projectId: null, title: "A chat", pinnedAtMs: null, createdAtMs: start,
  updatedAtMs: start, lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
  setup: { type: "chat", model: { provider: "claude", model: null, effort: null, fast: false } },
} as unknown as Conversation;
const messages: Message[] = [
  { id: "u1", conversationId: id, seq: 1, role: "user", text: "Fix the failing test.", blob: null, createdAtMs: start,
    attachments: [], mentions: [], model: null, requestId: "r1", parentId: null },
  { id: "a1", conversationId: id, seq: 2, role: "assistant", text: "Fixed: the date was parsed in the wrong zone.",
    blob: null, createdAtMs: start + 30_000, attachments: [], mentions: [], model: null, requestId: "r1", parentId: "u1" },
];
const request = {
  id: "r1", conversationId: id, preview: "Fix the failing test.", state: { type: "done" }, startedAtMs: start,
  endedAtMs: start + 30_000, steeredInto: null, steeredAfter: null, undo: null, worked: [], quotaWait: false,
  suggestion: null,
} as unknown as UserRequest;

/** The message queue: paused after an interrupt, with one message waiting, for the `paused` case. */
const queue = which === "paused"
  ? { items: [{ id: "q1", text: "Then update the docs.", attachments: [], mentions: [], queuedAtMs: start, editedAtMs: null }], paused: true }
  : { items: [], paused: false };

/** What the composer sent, by method. */
const sent: Request[] = [];
/** Stores the file being attached, once the case lets it (`null`: none waits). */
let finishUpload: (() => void) | null = null;
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "addAttachment": {
      const { name, mime } = req;
      const attachment = { id: `blob-${name}`, name, mime, bytes: 4, pasted: false, inline: null };
      return new Promise((resolve) => {
        finishUpload = () => resolve({ method: req.method, attachment });
      });
    }
    case "getConversation":
      return {
        method: req.method,
        view: {
          conversation, context: null, messages: { messages, hasMore: false }, tasks: [], approvals: [], questions: [],
          plans: [], requests: [request], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [],
          compactions: [], decisions: [], waiting: [], queue, run: "idle",
          runRequest: null, head: "a1", ratings: {}, streaming: null, notices: [], memories: [], overnight: [],
          reviews: [], previews: [],
        } as unknown as View,
      };
    default:
      sent.push(req);
      return { method: req.method };
  }
});
useApp.setState({ conversations: { [id]: conversation }, connection: { status: "connected", daemon: null, reason: null } });

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
/** Waits for a condition (never a guess at timing), failing with what it waited for. */
async function until(what: string, check: () => boolean, ms = 10_000) {
  const end = performance.now() + ms;
  while (!check()) {
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(16);
  }
}
const field = () => document.querySelector<HTMLElement>("[data-slot=aui_composer-shell] [contenteditable=true]");

/** The latest request's state, as the daemon would publish it. */
const setState = (state: UserRequest["state"]) =>
  useBoard.setState(({ board }) =>
    board ? { board: { ...board, requests: { ...board.requests, r1: { ...board.requests["r1"]!, state } } } } : {},
  );

/** What has the keyboard, by its label. */
const focused = () => document.activeElement?.getAttribute("aria-label") ?? document.activeElement?.tagName ?? null;

/**
 * #304: a field the user went to (Find in the chat) keeps the keyboard when the composer would
 * focus itself, as it does when its editor loads and when a run starts.
 */
async function focusCase(seen: Record<string, unknown>) {
  await until("the editor", () => field() !== null);
  await fixtureClick(field()!);
  await fixtureInput({ type: "type", text: "half" });
  await until("the typed text", () => field()?.textContent === "half");
  const find = document.createElement("input");
  find.setAttribute("aria-label", "Find in chat");
  document.body.append(find);
  find.focus();
  setState({ type: "working" });
  await sleep(300);
  seen.runStart = focused();
  setState({ type: "done" });
  // With nothing else focused, a run's start still brings the keyboard back to the composer.
  find.remove();
  await sleep(100);
  setState({ type: "working" });
  await until("the composer focused", () => focused() === "Message input", 3000).catch(() => {});
  seen.runStartFree = focused();
  setState({ type: "done" });
}

/** The composer's send button. */
const sendButton = () => document.querySelector<HTMLButtonElement>(".aui-composer-send");

/** #302: while an attached file is still being stored, Send waits and says why; Enter waits too. */
async function uploadingCase(seen: Record<string, unknown>) {
  await until("the editor", () => field() !== null);
  await fixtureClick(field()!);
  await fixtureInput({ type: "type", text: "see notes" });
  await until("the typed text", () => field()?.textContent === "see notes");
  const files = new DataTransfer();
  files.items.add(new File(["notes"], "notes.txt", { type: "text/plain" }));
  field()!.dispatchEvent(new ClipboardEvent("paste", { clipboardData: files, bubbles: true, cancelable: true }));
  await until("the upload", () => finishUpload !== null);
  await sleep(100);
  seen.whileUploading = { disabled: sendButton()?.disabled, tip: sendButton()?.textContent };
  await fixtureInput({ type: "key", key: "Enter" });
  await sleep(200);
  seen.enterWhileUploading = sent.some((req) => req.method === "sendMessage");
  finishUpload!();
  await until("the upload done", () => sendButton()?.disabled === false);
  seen.afterUpload = { disabled: sendButton()?.disabled, tip: sendButton()?.textContent };
}

/** The open dialog, if any. */
const dialog = () => document.querySelector<HTMLElement>("[role=dialog]");
/** Whether the dialog is open and done moving in (a click mid-animation can miss it). */
const dialogSettled = () =>
  dialog() !== null && dialog()!.getAnimations({ subtree: true }).every((animation) => animation.playState === "finished");
const dialogButton = (name: string) =>
  [...(dialog()?.querySelectorAll("button") ?? [])].find((button) => button.textContent?.trim() === name) ?? null;
/** The messages sent, with the slot each asked for. */
const sends = () =>
  sent.flatMap((req) => (req.method === "sendMessage" ? [{ text: req.text, queueIndex: req.queueIndex }] : []));

/**
 * #303: sending while the queue is paused asks first ("Send message?"): Send now sends it ahead
 * of the paused queue, Keep paused puts it in the queue, which stays paused; Esc sends nothing.
 */
async function pausedCase(seen: Record<string, unknown>) {
  await until("the editor", () => field() !== null);
  await fixtureClick(field()!);
  await fixtureInput({ type: "type", text: "first" });
  await until("the typed text", () => field()?.textContent === "first");
  await fixtureInput({ type: "key", key: "Enter" });
  await until("the dialog", () => dialog() !== null, 3000).catch(() => {});
  seen.asked = {
    title: dialog()?.querySelector("h2")?.textContent ?? null,
    buttons: [...(dialog()?.querySelectorAll("button") ?? [])].map((button) => button.textContent?.trim()).filter(Boolean),
    sent: sends().length,
  };
  await fixtureInput({ type: "key", key: "Escape" });
  await until("the dialog closed", () => dialog() === null, 3000).catch(() => {});
  seen.escaped = { sent: sends().length, kept: field()?.textContent };
  await fixtureInput({ type: "key", key: "Enter" });
  await until("the dialog again", () => dialogSettled() && dialogButton("Keep paused") !== null, 3000);
  await fixtureClick(dialogButton("Keep paused")!);
  await until("the message queued", () => sends().length === 1, 3000).catch(() => {});
  seen.keptPaused = sends();
  await until("the field empty, the dialog gone", () => field()?.textContent === "" && dialog() === null, 3000);
  await fixtureClick(field()!);
  await fixtureInput({ type: "type", text: "second" });
  await until("the second text", () => field()?.textContent === "second");
  await fixtureClick(sendButton()!);
  await until("the dialog from Send", () => dialogSettled() && dialogButton("Send now") !== null, 3000);
  await fixtureClick(dialogButton("Send now")!);
  await until("the message sent", () => sends().length === 2, 3000).catch(() => {});
  seen.sentNow = sends().slice(1);
}

/** The composer's placeholder, as the editor shows it. */
const placeholder = () => field()?.getAttribute("aria-placeholder") ?? null;

/**
 * #297: while a task runs, the empty composer's placeholder says for how long ("Working for 2m
 * 5s"), from a second in, ticking; typed text hides it, and the request's end takes it away.
 */
async function workingCase(seen: Record<string, unknown>) {
  await until("the editor", () => field() !== null);
  seen.idle = placeholder();
  // A request that has worked for a while.
  useBoard.setState(({ board }) =>
    board ? { board: { ...board, requests: { ...board.requests, r1: { ...board.requests["r1"]!, state: { type: "working" }, startedAtMs: Date.now() - 125_000, endedAtMs: null } } } } : {},
  );
  await until("the working placeholder", () => placeholder()?.startsWith("Working for") ?? false, 3000).catch(() => {});
  const first = placeholder();
  seen.working = first;
  await until("the next tick", () => placeholder() !== first, 3000).catch(() => {});
  seen.ticked = placeholder();
  setState({ type: "done" });
  await until("the placeholder back", () => placeholder() === seen.idle, 3000).catch(() => {});
  seen.done = placeholder();
  // One that just started: the usual words for its first second.
  useBoard.setState(({ board }) =>
    board ? { board: { ...board, requests: { ...board.requests, r1: { ...board.requests["r1"]!, state: { type: "working" }, startedAtMs: Date.now(), endedAtMs: null } } } } : {},
  );
  await sleep(100);
  seen.justStarted = placeholder();
  await until("a second in", () => placeholder()?.startsWith("Working for") ?? false, 3000).catch(() => {});
  seen.secondIn = placeholder();
}

const CASES: Record<string, (seen: Record<string, unknown>) => Promise<void>> = {
  focus: focusCase,
  uploading: uploadingCase,
  paused: pausedCase,
  working: workingCase,
};

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    const run = CASES[which ?? ""];
    if (!run) throw new Error(`no case ${String(which)}`);
    await run(seen);
    seen.sent = sent.map((req) => req.method);
  } catch (error) {
    seen.error = String(error);
  }
  const result = document.createElement("pre");
  result.id = "composer-polish-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

select({ type: "conversation", id });
void drive();
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
      <main className="relative flex min-h-0 w-full flex-1 flex-col">
        <ConversationView key={id} selection={{ type: "conversation", id }} />
      </main>
    </SidebarProvider>
  </TooltipProvider>,
);
