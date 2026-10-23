/**
 * A standalone Vite entry, excluded from the app entry and production build: a session's composer
 * in the app's own conversation view, IPC mocked, no daemon. It opens the permission menu from the
 * `/` menu's Permissions command and from ↑ on the focused pill (#95), and the model picker from
 * the Model command, and writes what it saw as
 * JSON into `<pre id="composer-permissions-result">`; `stop=1` holds the page for a screenshot.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, ConversationView as View, Message, Request, UserRequest } from "@/ipc/generated";
import { select } from "@/state/actions";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const id = "composer-permissions";
const start = Date.now() - 20 * 60_000;
const finished = start + 20_000;

const conversation = {
  id, kind: "session", projectId: null, title: "Tax helpers", pinnedAtMs: null, createdAtMs: start, updatedAtMs: finished,
  lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null,
  setup: {
    type: "session", orchestrator: { provider: "claude", model: null, effort: null, fast: false }, permission: "acceptEdits", planMode: false,
    workersSeeUncommitted: null, environment: { type: "localCheckout", repoPath: "/repo", branch: null },
  },
} as unknown as Conversation;

const message = (messageId: string, seq: number, role: "user" | "assistant", text: string, createdAtMs: number, parentId: string | null): Message => ({
  id: messageId, conversationId: id, seq, role, text, blob: null, createdAtMs, attachments: [], mentions: [], model: null, requestId: "r1", parentId,
});
const messages: Message[] = [
  message("u1", 1, "user", "Rename the tax helpers.", start, null),
  message("a1", 200, "assistant", "Renamed `calc` to `taxOf` in the three callers.", finished, "u1"),
];
const request = {
  id: "r1", conversationId: id, preview: "Rename the tax helpers", state: { type: "done" }, startedAtMs: start, endedAtMs: finished,
  steeredInto: null, steeredAfter: null, undo: null, worked: [{ fromMs: start, toMs: finished }], quotaWait: false,
} as unknown as UserRequest;

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "getConversation")
    return {
      method: req.method,
      view: {
        conversation, context: null, messages: { messages, hasMore: false }, tasks: [], approvals: [], questions: [],
        requests: [request], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [],
        compactions: [], decisions: [], queue: { items: [], paused: false }, run: "idle",
        runRequest: null, head: "a1", ratings: {}, streaming: null, notices: [],
        previews: [],
      } as unknown as View,
    };
  return { method: req.method };
});
useApp.setState({ conversations: { [id]: conversation }, connection: { status: "connected", daemon: null, reason: null } });

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
/** Waits for an element (never a guess at timing), failing with what it waited for. */
async function until<T>(what: string, find: () => T | null | undefined, ms = 10_000): Promise<T> {
  const end = performance.now() + ms;
  for (;;) {
    const found = find();
    if (found) return found;
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(16);
  }
}

const menu = () => document.querySelector<HTMLElement>("[data-slot=permission-menu]");
const key = (target: Element, name: string) => target.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true }));

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    // Set up: the editor shows a moment before its plugins wire it to the composer (one of them turns
    // autocorrect off), and a / typed in between is lost.
    const field = await until("the composer", () => document.querySelector<HTMLElement>("[data-slot=aui_composer-shell] [contenteditable=true][autocorrect=off]"));
    field.focus();
    document.execCommand("insertText", false, "/");
    const option = await until("the Permissions command", () =>
      [...document.querySelectorAll<HTMLElement>("[data-slot=composer-commands] [role=option]")].find((each) => each.id.endsWith("-permissions")),
    );
    seen.command = [...option.children].map((part) => part.textContent).filter(Boolean).join(" | ");
    seen.commandIcon = option.querySelector("svg")?.outerHTML === document.querySelector("[data-slot=permission-picker] svg")?.outerHTML;
    // Picked as the user does: typed and run with Enter.
    document.execCommand("insertText", false, "perm");
    await sleep(300);
    key(field, "Enter");
    seen.fromSlash = (await until("the menu from the / command", menu)).dataset.state;
    await sleep(500);
    seen.stillOpen = menu()?.dataset.state;
    key(menu()!, "Escape");
    await until("the menu to close", () => !menu());
    // Closed, a menu hands the focus back to its pill a moment later, which would take it from what has it then.
    await until("the focus back on the permission pill", () => document.activeElement?.matches("[data-slot=permission-picker]"));
    // The Model command opens its picker the same way.
    field.focus();
    document.execCommand("insertText", false, "/model");
    await until("the Model command", () => document.querySelector("[data-slot=composer-commands] [role=option][id$=-model]"));
    key(field, "Enter");
    const models = await until("the model picker", () => document.querySelector<HTMLElement>("[data-slot=model-selector]"));
    await sleep(500);
    seen.model = models.isConnected ? models.dataset.state : "gone";
    key(models, "Escape");
    await until("the model picker to close", () => !document.querySelector("[data-slot=model-selector]"));
    await until("the focus back on the Model pill", () => document.activeElement?.matches("[data-slot=model-selector-trigger]"));
    const pill = document.querySelector<HTMLElement>("[data-slot=permission-picker]")!;
    pill.focus();
    key(pill, "ArrowUp");
    seen.fromArrowUp = (await until("the menu from ↑", menu)).dataset.state;
  } catch (error) {
    seen.error = String(error);
  }
  if (query.get("stop") === "1") return;
  const result = document.createElement("pre");
  result.id = "composer-permissions-result";
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
