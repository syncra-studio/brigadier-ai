/**
 * A standalone Vite entry, excluded from the app entry and production build: a session's workers
 * @-mentioned (`@task-N`) wherever the user reads them, each by its name, in the app's own
 * conversation view, IPC mocked: the kept draft's chip, the `@` menu, the queued
 * messages above the composer and one pulled back into it, a notice of one the session sent on to
 * a worker, the sent bubble and the rail's preview of it. What it saw goes as JSON into
 * `<pre id="worker-names-result">`; `WorkerNamesRender.test.ts` checks it.
 */
import "@/fixtures/cliView";
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { fixtureInput } from "@/fixtures/input";
import type { Conversation, ConversationView as View, Message, Project, QueuedMessage, Request, SentToWorker, Task } from "@/ipc/generated";
import { select } from "@/state/actions";
import { useApp } from "@/state/store";

const id = "names-session";
const start = Date.now() - 600_000;
const conversation = {
  id, kind: "session", projectId: "p1", title: "Names", pinnedAtMs: null, createdAtMs: start, updatedAtMs: start,
  lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null, setup: null,
} as unknown as Conversation;
const project = { id: "p1", name: "app", createdAtMs: start, repos: [{ path: "/tmp/app", name: "app" }], prefs: {}, trust: [] } as unknown as Project;
const taskOf = (taskId: string, number: number, title: string, state: Task["state"]) => ({
  id: taskId, conversationId: id, number, title, state, kind: "implement", role: "worker", attempts: [], messages: [],
  route: { choice: { provider: "claude", model: null, effort: null }, reason: "", explanation: null },
  requestId: null, position: number, createdAtMs: start + number, updatedAtMs: start + number,
}) as unknown as Task;
const tasks = [taskOf("task-a", 1, "Build the importer", "running"), taskOf("task-b", 3, "Write the docs", "running")];

/** Four turns, so the rail shows; the first asks a worker by its `@task-N`. */
const messages: Message[] = [];
for (let turn = 0; turn < 4; turn++) {
  const text = turn === 0 ? "@task-1 look at the parser" : `Question ${turn + 1}, nothing mentioned.`;
  messages.push({
    id: `u${turn}`, conversationId: id, seq: messages.length + 1, role: "user", text, blob: null, createdAtMs: start + turn * 60_000,
    attachments: [], mentions: turn === 0 ? [{ type: "task", id: "task-a" }] : [], model: null, requestId: `r${turn}`, parentId: messages.at(-1)?.id ?? null,
  });
  messages.push({
    id: `a${turn}`, conversationId: id, seq: messages.length + 1, role: "assistant", text: `Answer ${turn + 1}.`, blob: null,
    createdAtMs: start + turn * 60_000 + 30_000, attachments: [], mentions: [], model: null, requestId: `r${turn}`, parentId: messages.at(-1)?.id ?? null,
  });
}
const queued: QueuedMessage = { id: "q1", text: "then @task-3 checks it", attachments: [], mentions: [{ type: "task", id: "task-b" }], queuedAtMs: start, editedAtMs: null };
const sent: SentToWorker = { id: "s1", text: "@task-1 rebase first", worker: "task-a", workerNumber: 1, workerTitle: "Build the importer", sentAtMs: start };

localStorage.setItem("brigadier.drafts", JSON.stringify({
  [id]: { text: "ask @task-1 now", attachments: [], atMs: Date.now(), mentions: [{ type: "task", id: "task-a" }] },
}));

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getConversation":
      return {
        method: req.method,
        view: {
          conversation, context: null, messages: { messages, hasMore: false }, tasks, approvals: [], questions: [],
          requests: [], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [],
          compactions: [], decisions: [], queue: { items: [queued], paused: true, sent: [sent] }, run: "idle",
          runRequest: null, head: "a3", ratings: {}, streaming: null, notices: [], previews: [],
        } as unknown as View,
      };
    case "listFiles":
      return { method: req.method, files: [], truncated: false };
    case "deleteQueued":
      return { method: req.method, queue: { items: [], paused: true, sent: [sent] } };
    default:
      return { method: req.method };
  }
});
const state = useApp.getState();
useApp.setState({
  conversations: { [id]: conversation },
  projects: { p1: project },
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { status: "connected", daemon: null, reason: null },
});

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(what: string, check: () => boolean, ms = 10_000) {
  const end = performance.now() + ms;
  while (!check()) {
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}: ${typed()}`);
    await sleep(16);
  }
}
const field = () => document.querySelector<HTMLElement>("[data-slot=composer-input] [contenteditable=true]");
const typed = () => field()?.textContent ?? "";
const key = (name: string, modifiers?: ("Alt" | "Control" | "Meta" | "Shift")[]) =>
  fixtureInput({ type: "key", key: name, ...(modifiers ? { modifiers } : {}) });
const type = (text: string) => fixtureInput({ type: "type", text });
const menu = () => document.querySelector<HTMLElement>("[data-slot=composer-mentions]");
async function clearComposer() {
  field()!.focus();
  await key("a", ["Meta"]);
  await key("Backspace");
  await until("the composer cleared", () => typed() === "");
}

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    await until("the field", () => field() !== null);
    await until("the bubble", () => document.querySelector("[data-message-id=u0]") !== null);
    // The kept draft read back with the session's workers not yet known to the field.
    await until("the kept draft", () => typed().startsWith("ask"));
    await sleep(500);
    seen.draft = typed();

    const row = document.querySelector<HTMLElement>("[data-slot=queue-item-text]");
    seen.queued = { text: row?.textContent ?? null, title: row?.getAttribute("title") ?? null };
    seen.sentNotice = document.querySelector("[data-slot=message-queue-sent] span")?.textContent ?? null;

    const bubble = document.querySelector<HTMLElement>("[data-message-id=u0]")!;
    seen.bubble = bubble.querySelector("[data-slot=worker-chip]")?.textContent ?? bubble.innerText;

    const marker = document.querySelector<HTMLElement>("[data-rail-marker=u0]");
    marker?.focus();
    await until("the rail's preview", () => document.querySelector("[data-slot=rail-preview]") !== null, 3_000).catch(() => {});
    seen.railPreview = document.querySelector("[data-slot=rail-preview] p")?.textContent ?? null;
    seen.railLabel = marker?.getAttribute("aria-label") ?? null;

    // The `@` menu names each worker.
    await clearComposer();
    await type("@");
    await until("the @ menu", () => menu() !== null && menu()!.textContent!.includes("Workers"));
    seen.menu = [...menu()!.querySelectorAll("[role=option]")].map((node) => node.textContent ?? "").filter((text) => text.includes("Build") || text.includes("Write") || text.includes("task-"));
    await key("Escape");
    await clearComposer();

    // ↑ in the empty field takes the queued message back into it.
    await key("ArrowUp");
    await until("the queued message pulled", () => typed().startsWith("then"));
    await sleep(200);
    seen.pulled = typed();

    // Nothing the user reads anywhere says `task-N`.
    seen.anywhere = document.body.innerText.match(/[^\n]*task-\d+[^\n]*/g) ?? [];
  } catch (error) {
    seen.error = String(error);
  }
  const result = document.createElement("pre");
  result.id = "worker-names-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

await select({ type: "conversation", id });
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
      <main className="relative flex min-h-0 w-full flex-1 flex-col">
        <ConversationView selection={{ type: "conversation", id }} />
      </main>
    </SidebarProvider>
  </TooltipProvider>,
);
void drive();
