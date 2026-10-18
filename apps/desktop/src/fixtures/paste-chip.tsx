/**
 * A standalone Vite entry, excluded from the app entry and production build: text pasted as a
 * chip (issue #158) in the rich composer of the app's own conversation view, IPC mocked. A
 * draft, a queued message and an earlier prompt hold an image and a pasted text of the same
 * number; it checks each comes back with both, then pastes rich and long text, and checks what
 * each send carries, the editor the chip opens and the chip in the sent message. What it saw
 * goes as JSON into `<pre id="paste-chip-result">`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/ConversationView";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import type { AttachmentRef, Conversation, ConversationView as View, Message, QueuedMessage, Request, UserRequest } from "@/ipc/generated";
import { select } from "@/state/actions";
import { setRichComposer } from "@/state/composerMode";
import { useApp } from "@/state/store";

setRichComposer(true);

const id = "paste-chat";
const start = Date.now() - 600_000;

/** The mocked blob store: each blob's bytes, base64. */
const blobs = new Map<string, string>();
const store = (blob: string, text: string) => blobs.set(blob, btoa(text));
const PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
blobs.set("img-1", PNG);
store("paste-1", "# Notes\n\n- a");
const image: AttachmentRef = { id: "img-1", name: "image.png", mime: "image/png", bytes: 68, pasted: false, inline: 1 };
const paste: AttachmentRef = { id: "paste-1", name: "Pasted text.md", mime: "text/markdown", bytes: 12, pasted: true, inline: 1 };

const DRAFT = "[Image #1] see [Pasted text #1]";
const QUEUED = "queued [Pasted text #1]";
const EARLIER = "earlier [Image #1] [Pasted text #1]";

const conversation = {
  id, kind: "chat", projectId: null, title: "Paste chip", pinnedAtMs: null, createdAtMs: start,
  updatedAtMs: start, lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
  setup: { type: "chat", model: { provider: "claude", model: null, effort: null, fast: false } },
} as unknown as Conversation;
const messages: Message[] = [
  { id: "u1", conversationId: id, seq: 1, role: "user", text: EARLIER, blob: null, createdAtMs: start,
    attachments: [image, paste], mentions: [], model: null, requestId: "r1", parentId: null },
  { id: "a1", conversationId: id, seq: 2, role: "assistant", text: "Done.", blob: null, createdAtMs: start + 30_000,
    attachments: [], mentions: [], model: null, requestId: "r1", parentId: "u1" },
];
const request = {
  id: "r1", conversationId: id, preview: "earlier", state: { type: "done" }, startedAtMs: start,
  endedAtMs: start + 30_000, steeredInto: null, steeredAfter: null, undo: null, worked: [], quotaWait: false,
} as unknown as UserRequest;
const queued = { id: "q1", text: QUEUED, attachments: [paste], mentions: [], queuedAtMs: start, editedAtMs: null, deciding: false } as QueuedMessage;

localStorage.setItem("brigadier.drafts", JSON.stringify({
  [id]: { text: DRAFT, attachments: [image, paste], atMs: Date.now(), mentions: [] },
}));

const sends: { text: string; attachments: AttachmentRef[] }[] = [];
let sequence = 10;
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getConversation":
      return {
        method: req.method,
        view: {
          conversation, context: null, messages: { messages, hasMore: false }, tasks: [], approvals: [], questions: [],
          plans: [], requests: [request], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [],
          compactions: [], decisions: [], waiting: [], queue: { items: [queued], paused: false }, run: "idle",
          runRequest: null, head: "a1", ratings: {}, streaming: null, notices: [], memories: [], overnight: [],
          reviews: [], previews: [],
        } as unknown as View,
      };
    case "addAttachment": {
      const blob = `blob-${sequence++}`;
      blobs.set(blob, req.data);
      return {
        method: req.method,
        attachment: { id: blob, name: req.name, mime: req.mime, bytes: atob(req.data).length, pasted: req.pasted, inline: null },
      };
    }
    case "readAttachment":
      return { method: req.method, data: blobs.get(req.id) ?? "" };
    case "deleteQueued":
      return { method: req.method, queue: { items: [], paused: false } };
    case "sendMessage": {
      sends.push({ text: req.text, attachments: req.attachments });
      const message: Message = {
        id: `sent-${sequence}`, conversationId: id, seq: sequence++, role: "user", text: req.text, blob: null,
        attachments: req.attachments, mentions: req.mentions, createdAtMs: Date.now(), model: null,
        requestId: `sent-request-${sequence}`, parentId: null,
      };
      return { method: req.method, outcome: { type: "sent", message } };
    }
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
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}: ${field()?.innerHTML ?? "no field"}`);
    await sleep(16);
  }
}
const field = () => document.querySelector<HTMLElement>("[data-rich-composer] [contenteditable=true]");
const typed = () => field()?.textContent ?? "";
const key = (name: string, modifiers?: ("Alt" | "Control" | "Meta" | "Shift")[]) =>
  fixtureInput({ type: "key", key: name, ...(modifiers ? { modifiers } : {}) });
const type = (text: string) => fixtureInput({ type: "type", text });
const imageChip = () => field()?.querySelector("[aria-label='Preview Image #1']") ?? null;
const pasteChip = (n = 1) => field()?.querySelector(`[aria-label='Open Pasted text #${n}']`) ?? null;
/** The composer's attachments in the attachments row (cards). */
const cards = () => document.querySelectorAll("[data-slot=composer-attachment]").length;
/** What a sent attachment's blob holds. */
const content = (ref: AttachmentRef | undefined) => (ref ? atob(blobs.get(ref.id) ?? "") : null);
const kinds = (refs: readonly AttachmentRef[]) =>
  refs.map((ref) => ({ pasted: ref.pasted, inline: ref.inline, mime: ref.mime })).toSorted((a, b) => Number(a.pasted) - Number(b.pasted));

async function selectAll() {
  document.execCommand("selectAll");
  await sleep(60);
}

async function clear() {
  field()!.focus();
  await selectAll();
  await key("Backspace");
  await until("an empty field", () => typed() === "");
}

/** Sends what is typed with Enter, once its attachments are stored, and returns the send. */
async function send(): Promise<(typeof sends)[number]> {
  const before = sends.length;
  // Stored: no attachment is still uploading.
  await sleep(150);
  await key("Enter");
  await until("the send", () => sends.length > before);
  await until("the field emptied", () => typed() === "");
  return sends.at(-1)!;
}

/** The open pasted text's editor: editable, or read only. */
const editor = () => document.querySelector<HTMLElement>("[data-slot=pasted-text-editor] [contenteditable=true]");
const shown = () => document.querySelector("[data-slot=pasted-text-editor] [contenteditable=false]");

function pasteData(data: Record<string, string>) {
  const clipboardData = new DataTransfer();
  for (const [format, value] of Object.entries(data)) clipboardData.setData(format, value);
  field()!.dispatchEvent(new ClipboardEvent("paste", { clipboardData, bubbles: true, cancelable: true }));
}

const LIST_HTML = "<meta charset='utf-8'><h2>Title</h2><ul><li>a</li><li>b</li></ul>";

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    await until("the field", () => field() !== null);
    // A draft with image #1 and pasted text #1 comes back with both chips, and sends both.
    await until("the draft's chips", () => imageChip() !== null && pasteChip() !== null);
    await fixtureClick(field()!);
    const draft = await send();
    seen.draft = { text: draft.text, kinds: kinds(draft.attachments) };

    // ↑ pulls the queued message back with its pasted text.
    await key("ArrowUp");
    await until("the queued message", () => pasteChip() !== null);
    seen.queued = { text: typed(), image: imageChip() !== null };
    await clear();
    // ↑ again walks back to the earlier prompt, its image and pasted text with it.
    for (let tries = 0; tries < 3 && !typed().startsWith("earlier"); tries++) {
      await key("ArrowUp");
      await sleep(100);
    }
    await until("the earlier prompt", () => imageChip() !== null && pasteChip() !== null);
    const recalled = await send();
    seen.recalled = { text: recalled.text, kinds: kinds(recalled.attachments) };

    // Rich text copied from a page, pasted into text: a chip, sent as Markdown in its place.
    await type("see ");
    pasteData({ "text/html": LIST_HTML, "text/plain": "Title\na\nb" });
    await until("the paste chip", () => pasteChip() !== null);
    await type("ok");
    const rich = await send();
    seen.rich = { text: rich.text, kinds: kinds(rich.attachments), content: content(rich.attachments[0]) };

    // One bold phrase pastes as formatted text, no chip.
    await type("a ");
    pasteData({ "text/html": "<meta charset='utf-8'><b>bold</b> words", "text/plain": "bold words" });
    await until("the bold phrase", () => typed() === "a bold words");
    seen.phrase = { bold: field()!.querySelector(".font-semibold")?.textContent ?? null, chip: pasteChip() !== null, sent: (await send()).text };

    // Long plain text: a chip in text; into an empty field, a card.
    const long = "word ".repeat(420).trim();
    await type("x ");
    pasteData({ "text/plain": long });
    await until("the long paste's chip", () => pasteChip() !== null);
    const longChip = await send();
    seen.longChip = { text: longChip.text, kinds: kinds(longChip.attachments), whole: content(longChip.attachments[0]) === long };
    field()!.focus();
    pasteData({ "text/plain": long });
    await until("the long paste's card", () => cards() === 1);
    seen.card = { chip: pasteChip() !== null, typed: typed() };
    const card = await send();
    seen.cardSent = kinds(card.attachments);

    // A chip copied and pasted again brings its text along, numbered on where its number is taken.
    await type("c ");
    pasteData({ "text/html": LIST_HTML, "text/plain": "Title\na\nb" });
    await until("the chip to copy", () => pasteChip() !== null);
    await sleep(150);
    await selectAll();
    const clipboard = new DataTransfer();
    field()!.dispatchEvent(new ClipboardEvent("copy", { clipboardData: clipboard, bubbles: true, cancelable: true }));
    await key("ArrowRight");
    // The editor takes the caret's move on selectionchange.
    await sleep(60);
    field()!.dispatchEvent(new ClipboardEvent("paste", { clipboardData: clipboard, bubbles: true, cancelable: true }));
    await until("the copied chip", () => pasteChip(2) !== null);
    const copiedSend = await send();
    seen.copied = {
      text: copiedSend.text,
      kinds: kinds(copiedSend.attachments),
      same: copiedSend.attachments.map(content).every((text) => text === "## Title\n\n- a\n- b"),
    };
    // Into a composer without it (sent since), it comes with its text.
    field()!.focus();
    await type("again ");
    field()!.dispatchEvent(new ClipboardEvent("paste", { clipboardData: clipboard, bubbles: true, cancelable: true }));
    await until("the copied chip again", () => pasteChip() !== null);
    const againSend = await send();
    seen.copiedAgain = { text: againSend.text, kinds: kinds(againSend.attachments), content: content(againSend.attachments[0]) };

    // Text too long to go whole says so on its chip.
    await type("big ");
    pasteData({ "text/plain": "y".repeat(210_000) });
    await until("the big paste's chip", () => pasteChip() !== null);
    await until("the big paste stored", () => field()!.querySelector("[data-slot=inline-paste]")?.textContent?.includes("Start and end are sent") === true);
    seen.big = field()!.querySelector("[data-slot=inline-paste]")?.textContent;
    await send();

    // An image and a pasted text together: two chips of the same number, two attachments.
    const canvas = document.createElement("canvas");
    canvas.width = 8;
    canvas.height = 8;
    const png = await new Promise<Blob>((resolve) => canvas.toBlob((blob) => resolve(blob!), "image/png"));
    const files = new DataTransfer();
    files.items.add(new File([png], "image.png", { type: "image/png" }));
    field()!.dispatchEvent(new ClipboardEvent("paste", { clipboardData: files, bubbles: true, cancelable: true }));
    await until("the image chip", () => imageChip() !== null);
    pasteData({ "text/html": LIST_HTML, "text/plain": "Title\na\nb" });
    await until("the paste chip by the image", () => pasteChip() !== null);
    // Deleted with Backspace, its text goes; undo brings it back.
    await key("Backspace");
    await key("Backspace");
    await until("the paste chip deleted", () => pasteChip() === null);
    await key("z", [navigator.platform.startsWith("Mac") ? "Meta" : "Control"]);
    await until("the paste chip back", () => pasteChip() !== null);
    // The chip opens the pasted text in the Pages editor; Done keeps an edit.
    await fixtureClick(pasteChip()!);
    await until("the editor", () => editor()?.querySelector("h2")?.textContent === "Title");
    const item = [...editor()!.querySelectorAll("li")].at(-1)!;
    await fixtureClick(item);
    await key("ArrowRight");
    await key("ArrowRight");
    await type(" more");
    await until("the edit", () => item.textContent === "b more");
    const done = [...document.querySelectorAll("button")].find((button) => button.textContent === "Done")!;
    await fixtureClick(done);
    await until("the editor closed", () => editor() === null);
    await until("the edited text stored", () => document.querySelectorAll("[data-slot=composer-attachment]").length === 0 && pasteChip() !== null);
    field()!.focus();
    const both = await send();
    seen.both = {
      text: both.text,
      kinds: kinds(both.attachments),
      edited: content(both.attachments.find((ref) => ref.pasted)),
    };

    // The earlier message shows its pasted text's chip where it was pasted; it opens read only.
    const bubble = () => [...document.querySelectorAll("[data-slot=inline-paste]")].find((chip) => !field()!.contains(chip));
    await until("the chip in the sent message", () => bubble() !== undefined);
    await fixtureClick(bubble()!.querySelector("button")!);
    await until("the read-only editor", () => shown()?.querySelector("h1")?.textContent === "Notes");
    seen.bubble = {
      done: [...document.querySelectorAll("button")].some((button) => button.textContent === "Done"),
      text: shown()!.querySelector("li:last-child")?.textContent,
      handles: document.querySelectorAll("[data-slot=pasted-text-editor] [data-slot=page-drag-handle]").length,
    };
  } catch (error) {
    seen.error = String(error);
  }
  const result = document.createElement("pre");
  result.id = "paste-chip-result";
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
