/**
 * A standalone Vite entry, excluded from the app entry and production build: the conversation's guess at
 * the user's next message (`UserRequest.suggestion`, G18) in the app's own conversation view.
 * A Chat whose one request is over carries a guess; IPC is mocked and records every
 * request (Enter takes the guess, never sends). It drives the
 * real keys (`fixtureInput`): Tab, → and Enter each take it; no key hint shows with it. Then it
 * checks the field keeps typed
 * text as typed (no text checking, no double-space period), and writes what it saw as JSON
 * into `<pre id="composer-suggestion-result">`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import type { Conversation, ConversationView as View, Message, Request, UserRequest } from "@/ipc/generated";
import { select } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

const GUESS = "Run the tests once more.";
const id = "suggestion-chat";
const start = Date.now() - 600_000;

const conversation = {
  id, kind: "chat", projectId: null, title: "A chat with a guess", pinnedAtMs: null, createdAtMs: start,
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
  suggestion: GUESS,
} as unknown as UserRequest;

/** Every request, in order. */
const asked: string[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  asked.push(req.method);
  switch (req.method) {
    case "getConversation":
      return {
        method: req.method,
        view: {
          conversation, context: null, messages: { messages, hasMore: false }, tasks: [], approvals: [], questions: [],
          plans: [], requests: [request], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [],
          compactions: [], decisions: [], waiting: [], queue: { items: [], paused: false }, run: "idle",
          runRequest: null, head: "a1", ratings: {}, streaming: null, notices: [], memories: [], overnight: [],
          reviews: [], previews: [],
        } as unknown as View,
      };
    default:
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
const ghost = () => document.querySelector<HTMLElement>("[data-slot=composer-suggestion]");
const field = () => document.querySelector<HTMLElement>("[data-slot=aui_composer-shell] [contenteditable=true]");
const typed = () => field()?.textContent?.trim() ?? "";
const key = (name: string) => fixtureInput({ type: "key", key: name });
/** The latest request's state, as the daemon would publish it. */
const setState = (state: UserRequest["state"]) =>
  useBoard.setState(({ board }) =>
    board ? { board: { ...board, requests: { ...board.requests, r1: { ...board.requests["r1"]!, state } } } } : {},
  );

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    await until("the guess", () => ghost() !== null && field() !== null);
    seen.ghost = ghost()?.textContent;
    seen.placeholder = document.querySelector("[data-slot=aui_composer-shell] [contenteditable=true]")?.getAttribute("aria-placeholder") ?? null;
    // Tab in the empty field takes it, unsent.
    await fixtureClick(field()!);
    await key("Tab");
    await until("the guess in the field", () => typed() === GUESS);
    seen.taken = typed();
    seen.ghostAfterTab = ghost() !== null;
    // Emptied again, it comes back.
    for (let index = 0; index < GUESS.length + 2; index++) await key("Backspace");
    await until("the field empty", () => typed() === "");
    await until("the guess back", () => ghost() !== null);
    seen.backWhenEmpty = true;
    seen.ghostAfterTaken = ghost()?.textContent;
    // → and Enter take it too; Enter sends nothing.
    const takes: Record<string, boolean> = {};
    for (const name of ["ArrowRight", "Enter"]) {
      const before = asked.length;
      await key(name);
      await until(`the guess in the field by ${name}`, () => typed() === GUESS);
      await sleep(200);
      takes[name] = asked.slice(before).length === 0;
      for (let index = 0; index < GUESS.length + 2; index++) await key("Backspace");
      await until("the field empty", () => typed() === "");
      await until("the guess back", () => ghost() !== null);
    }
    seen.takesUnsent = takes;
    // Typing hides it.
    await key("x");
    await until("the letter", () => typed() === "x");
    seen.ghostWhileTyping = ghost() !== null;
    await key("Backspace");
    await until("the guess back after typing", () => ghost() !== null);
    // While the request works again, no guess.
    setState({ type: "working" });
    await until("no guess while working", () => ghost() === null);
    seen.ghostWhileWorking = ghost() !== null;
    setState({ type: "done" });
    await until("the guess after the request ends", () => ghost() !== null);
    seen.plainTyping = ["spellcheck", "autocorrect", "autocapitalize"].map((name) => field()?.getAttribute(name));
    // macOS's double-space period: the input method replaces the space before the caret with ". ".
    await fixtureClick(field()!);
    document.execCommand("insertText", false, "a ");
    await until("the typed space", () => typed() === "a");
    const space = field()!.querySelector("span")!.firstChild!;
    field()!.dispatchEvent(new InputEvent("beforeinput", {
      inputType: "insertReplacementText", data: ". ", bubbles: true, cancelable: true,
      targetRanges: [new StaticRange({ startContainer: space, startOffset: 1, endContainer: space, endOffset: 2 })],
    }));
    await sleep(100);
    seen.doubleSpace = field()?.textContent;
  } catch (error) {
    seen.error = String(error);
  }
  const result = document.createElement("pre");
  result.id = "composer-suggestion-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

select({ type: "conversation", id });
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
