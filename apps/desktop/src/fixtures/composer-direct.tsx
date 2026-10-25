/**
 * A standalone Vite entry, excluded from the app entry and production build: the composer's
 * To: (one message straight to one of the session's workers), IPC mocked. The `@` menu offers
 * "Message directly" for live workers only; picking one sets a To: chip and leaves no mention;
 * a message then goes to the worker alone, past the session's paused queue, with an ordinary
 * `@task-N` as context. A refused send puts the whole message and its To: back; one at a time
 * goes; a taken one takes the To: away and opens the worker's thread. While the To: shows, no
 * queued message is pulled into the composer; the worker's panel sets the To: again, and leaving
 * the session takes it away. What it
 * saw goes as JSON into `<pre id="composer-direct-result">`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import { rowOf, threadOf } from "@/fixtures/workerRows";
import type { Conversation, ConversationView as View, Project, Request, Task } from "@/ipc/generated";
import { select } from "@/state/actions";
import { useApp } from "@/state/store";

const id = "direct-session";
const other = "other-session";
const start = Date.now() - 600_000;
const sessionOf = (conversationId: string, title: string) => ({
  id: conversationId, kind: "session", projectId: "p1", title, pinnedAtMs: null, createdAtMs: start, updatedAtMs: start,
  lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null, setup: null,
}) as unknown as Conversation;
const project = { id: "p1", name: "app", createdAtMs: start, repos: [{ path: "/tmp/app", name: "app" }], prefs: {}, trust: [] } as unknown as Project;
const taskOf = (taskId: string, number: number, title: string, state: Task["state"]) => ({
  id: taskId, conversationId: id, number, title, state, kind: "implement", role: "worker", attempts: [], messages: [],
  route: { choice: { provider: "claude", model: null, effort: null }, reason: "", explanation: null },
  requestId: null, position: number, createdAtMs: start + number, updatedAtMs: start + number,
}) as unknown as Task;
const live = taskOf("task-live", 1, "Build the importer", "running");
const ended = taskOf("task-ended", 2, "Old spike", "stopped");
const docs = taskOf("task-docs", 3, "Write the docs", "running");
// The session's kept draft: an image and a pasted text, both #1.
const PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
const image = { id: "img-1", name: "image.png", mime: "image/png", bytes: 68, pasted: false, inline: 1 };
const paste = { id: "paste-1", name: "Pasted text.md", mime: "text/markdown", bytes: 12, pasted: true, inline: 1 };
localStorage.setItem("brigadier.drafts", JSON.stringify({
  [id]: { text: "[Image #1] see [Pasted text #1]", attachments: [image, paste], atMs: Date.now(), mentions: [] },
}));
const queued = { id: "q1", text: "queued for the session", attachments: [], mentions: [], delivered: false, deciding: false };

/** What each request carried: `session …` for the session's own, `worker <task> …` for a direct one. */
const sent: string[] = [];
const calls: Request[] = [];
/** How the next direct send ends; one held ends as `release` says. */
let next: "fail" | "hold" | "ok" = "ok";
let release: ((taken: boolean) => void) | null = null;
let sequence = 10;
mockIPC(async (command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  calls.push(req);
  switch (req.method) {
    case "getConversation":
      return {
        method: req.method,
        view: {
          conversation: useApp.getState().conversations[req.id] ?? sessionOf(req.id, "Other"), context: null,
          messages: { messages: [], hasMore: false }, tasks: req.id === id ? [live, ended, docs] : [], approvals: [], questions: [],
          requests: [], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [],
          compactions: [], decisions: [], queue: { items: req.id === id ? [queued] : [], paused: true }, run: "idle",
          runRequest: null, head: null, ratings: {}, streaming: null, notices: [],
          previews: [],
        } as unknown as View,
      };
    case "threadView":
      return { method: req.method, view: { entries: [], turns: [], workers: req.conversationId === id ? [rowOf(live), rowOf(ended), rowOf(docs)] : [] } };
    case "workerThread":
      return { method: req.method, view: threadOf([]) };
    case "readAttachment":
      return { method: req.method, data: req.id === "img-1" ? PNG : btoa("# Notes") };
    case "listFiles":
      return { method: req.method, files: [], truncated: false };
    case "sendToWorker": {
      sent.push(`worker ${req.taskId} ${req.text} ${JSON.stringify(req.mentions)}`);
      const how = next;
      next = "ok";
      if (how === "fail") throw new Error("worker 1 has ended");
      if (how === "hold" && !(await new Promise<boolean>((resolve) => { release = resolve; }))) throw new Error("worker 1 has ended");
      return { method: req.method, queued: false };
    }
    case "sendMessage": {
      sent.push(`session ${req.text}`);
      const message = {
        id: `sent-${sequence}`, conversationId: req.conversationId, seq: sequence++, role: "user", text: req.text, blob: null,
        attachments: req.attachments, mentions: req.mentions, createdAtMs: Date.now(), model: null,
        requestId: `sent-request-${sequence}`, parentId: null,
      };
      return { method: req.method, outcome: { type: "queued", item: { id: `q-${sequence}`, text: req.text, attachments: [], mentions: [] } }, message };
    }
    default:
      return { method: req.method };
  }
});
const state = useApp.getState();
useApp.setState({
  conversations: { [id]: sessionOf(id, "Direct"), [other]: sessionOf(other, "Other") },
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
const placeholder = () => document.querySelector("[data-slot=composer-input] .aui-lexical-placeholder")?.textContent ?? "";
const key = (name: string, modifiers?: ("Alt" | "Control" | "Meta" | "Shift")[]) =>
  fixtureInput({ type: "key", key: name, ...(modifiers ? { modifiers } : {}) });
const type = (text: string) => fixtureInput({ type: "type", text });
const chip = () => document.querySelector<HTMLButtonElement>("[data-slot=to-worker-chip]");
const menu = () => document.querySelector<HTMLElement>("[data-slot=composer-mentions]");
/** The `@` menu's rows under `heading`, by name. */
function rowsUnder(heading: string): string[] {
  const rows: string[] = [];
  let under = false;
  for (const node of menu()?.querySelectorAll("p, button, [role=option]") ?? []) {
    if (node.tagName === "P") under = node.textContent === heading;
    else if (under) rows.push(node.textContent ?? "");
  }
  return rows;
}
const directCalls = () => calls.filter((call): call is Extract<Request, { method: "sendToWorker" }> => call.method === "sendToWorker");
const directSends = () => directCalls().length;
const sentRefs = () => directCalls().at(-1)!.attachments.map((ref) => ref.id).toSorted();
const heldRows = () => [...document.querySelectorAll("[data-slot=held-direct]")].map((row) => row.querySelector("span")?.textContent ?? "");
const heldButton = (label: string) => [...document.querySelectorAll<HTMLButtonElement>("[data-slot=held-direct] button")].find((button) => button.textContent === label)!;
async function clearComposer() {
  field()!.focus();
  await key("a", [navigator.platform.startsWith("Mac") ? "Meta" : "Control"]);
  await key("Backspace");
  await until("the composer cleared", () => typed() === "");
}
/** Starts a held direct send of what the composer holds. */
async function holdSend() {
  next = "hold";
  const before = directSends();
  await key("Enter");
  await until("the send under way", () => directSends() === before + 1);
}
/** Leaves the session and comes back. */
async function revisit() {
  show(other);
  await until("the other session", () => field() !== null && chip() === null);
  show(id);
  await until("back", () => field() !== null && document.querySelector("[data-slot=composer]") !== null);
  await sleep(300);
}

/** Picks the "Message directly" row of the worker named `name`. */
async function pickDirect(name: string) {
  field()!.focus();
  // The menu opens on an `@` that starts a word.
  await type(typed() === "" || /\s$/.test(typed()) ? "@" : " @");
  await until("the @ menu", () => rowsUnder("Message directly").length > 0);
  const rows = [...menu()!.querySelectorAll("p, button, [role=option]")];
  const at = rows.findIndex((node) => node.tagName === "P" && node.textContent === "Message directly");
  const row = rows.slice(at + 1).find((node) => node.textContent?.includes(name));
  if (!row) throw new Error(`no direct row for ${name}`);
  await fixtureClick(row);
  await until("the To: chip", () => chip() !== null);
}

/** The view shown, switched from the outside (leaving the session and coming back). */
let show: (conversationId: string) => void = () => {};
function Shown() {
  const [shown, setShown] = useState(id);
  useEffect(() => {
    show = setShown;
  }, []);
  return <ConversationView key={shown} selection={{ type: "conversation", id: shown }} />;
}

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    await until("the field", () => field() !== null);
    // Its chips drawn too: before they are, the text ends in a space.
    await until("the kept draft", () => typed() === "Image #1 see Pasted text #1");

    // An image and a pasted text, both #1, go to the worker; the user types on meanwhile,
    // leaves and comes back, and then the worker refuses it: it is kept whole apart, never
    // mixed into the composer, and Restore brings it back, both attachments, with its To:.
    await pickDirect("Build the importer");
    await holdSend();
    seen.firstRefs = sentRefs();
    await type("newer words");
    await revisit();
    release?.(false);
    await until("the message kept apart", () => heldRows().length === 1);
    seen.keptApart = { text: typed(), chip: chip() !== null, held: heldRows() };
    await clearComposer();
    heldButton("Restore").click();
    await until("the message restored", () => typed().includes("see") && chip() !== null);
    seen.restored = { text: typed(), chip: chip()!.textContent, held: heldRows().length };
    next = "ok";
    await key("Enter");
    await until("the retry", () => chip() === null);
    seen.retryRefs = sentRefs();

    // Refused after another To: was picked: the new To: and its words stay as they are.
    await pickDirect("Build the importer");
    await type("for one");
    await holdSend();
    await pickDirect("Write the docs");
    await type("for two");
    release?.(false);
    await until("the refused one kept apart", () => heldRows().length === 1);
    seen.newRecipient = { text: typed(), chip: chip()!.textContent, held: heldRows() };
    heldButton("Discard").click();
    await until("discarded", () => heldRows().length === 0);
    chip()!.click();
    await clearComposer();

    // Taken late, after the user came back and picked the same worker again: that To: stays.
    await pickDirect("Build the importer");
    await type("late");
    await holdSend();
    await revisit();
    await pickDirect("Build the importer");
    release?.(true);
    await sleep(300);
    seen.revisitChip = chip() !== null;
    // So does one picked again in the same visit while the message was on its way.
    await type("again");
    await holdSend();
    await pickDirect("Build the importer");
    release?.(true);
    await sleep(300);
    seen.reselectChip = chip() !== null;
    chip()!.click();
    await until("the To: taken away", () => chip() === null);
    await clearComposer();

    // The menu: every worker to mention, only the live ones to message directly.
    field()!.focus();
    await type("@");
    await until("the @ menu", () => rowsUnder("Message directly").length > 0);
    seen.workers = rowsUnder("Workers").length;
    seen.direct = rowsUnder("Message directly");
    await key("Escape");
    await key("Backspace");
    await until("the field emptied", () => typed() === "");

    // Picking it sets the To: and leaves no mention in the text.
    await pickDirect("Build the importer");
    seen.chip = chip()!.textContent;
    seen.textAfterPick = typed();
    seen.placeholder = placeholder();

    // While the To: shows, ↑ on the empty composer pulls no queued message.
    await key("ArrowUp");
    await sleep(150);
    seen.pulled = calls.some((call) => call.method === "deleteQueued");

    const sentBefore = sent.length;
    // Refused: the whole message comes back, with its To:.
    next = "fail";
    await type("Use blue @task-1 ");
    await key("Enter");
    await until("the refusal", () => document.body.textContent?.includes("worker 1 has ended") ?? false);
    await until("the message back", () => typed().includes("Use blue"));
    seen.afterFailure = { text: typed(), chip: chip() !== null };

    // One at a time: a second Enter while the first is on its way sends nothing.
    next = "hold";
    const before = directSends();
    await key("Enter");
    await until("the send under way", () => directSends() === before + 1);
    await type("second");
    await key("Enter");
    await sleep(200);
    seen.whileSending = { sends: directSends() - before, text: typed() };
    release?.(true);

    // Taken: the To: goes, the worker's thread opens, nothing went to the session (its queue is
    // paused, and no question about it showed).
    await until("the To: gone", () => chip() === null);
    await until("the worker's thread", () => document.querySelector('section[aria-label^="Worker:"]') !== null);
    seen.worker = document.querySelector('section[aria-label^="Worker:"]')!.getAttribute("aria-label");
    seen.pausedQuestion = document.querySelector("[role=dialog]") !== null;
    seen.sent = sent.slice(sentBefore);

    // The worker's Message sets the To: again; leaving the session takes it away.
    for (let i = 0; i < 20 && typed() !== ""; i++) await key("Backspace");
    // This page has no window shell to make room for the panel: it opens past the window's edge.
    document.querySelector<HTMLButtonElement>("[data-slot=worker-message]")!.click();
    await until("the To: from the worker's panel", () => chip() !== null);
    seen.focusedAfterMessage = document.activeElement === field();
    show(other);
    await until("the other session", () => field() !== null && chip() === null);
    show(id);
    await until("back", () => field() !== null);
    await sleep(200);
    seen.chipAfterSwitch = chip() !== null;
  } catch (error) {
    seen.error = String(error);
  }
  const result = document.createElement("pre");
  result.id = "composer-direct-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

await select({ type: "conversation", id });
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
      <main className="relative flex min-h-0 w-full flex-1 flex-col">
        <Shown />
      </main>
    </SidebarProvider>
  </TooltipProvider>,
);
void drive();
