/**
 * A standalone Vite entry, excluded from the app entry and production build: the rich composer
 * (issue #158) in the app's own conversation view, IPC mocked. A draft, a queued message and an
 * earlier prompt hold Markdown; it checks each comes back as formatting, then types with the
 * real keys (`fixtureInput`) and checks what each send carries. What it saw goes as JSON into
 * `<pre id="rich-composer-result">`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import type { AttachmentRef, Conversation, ConversationView as View, Message, QueuedMessage, Request, UserRequest } from "@/ipc/generated";
import { select } from "@/state/actions";
import { setRichComposer } from "@/state/composerMode";
import { useApp } from "@/state/store";

setRichComposer(true);

const id = "rich-chat";
const start = Date.now() - 600_000;
const DRAFT = "## Draft *title*\nAsk @Release *draft* about `x`";
const QUEUED = "1. first\n2. `second`";
const EARLIER = "- one\n- **two**";

const conversation = {
  id, kind: "chat", projectId: null, title: "Rich composer", pinnedAtMs: null, createdAtMs: start,
  updatedAtMs: start, lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null,
  setup: { type: "chat", model: { provider: "claude", model: null, effort: null, fast: false }, permission: "acceptEdits" },
} as unknown as Conversation;
const messages: Message[] = [
  { id: "u1", conversationId: id, seq: 1, role: "user", text: EARLIER, blob: null, createdAtMs: start,
    attachments: [], mentions: [], model: null, requestId: "r1", parentId: null },
  { id: "a1", conversationId: id, seq: 2, role: "assistant", text: "Done.", blob: null, createdAtMs: start + 30_000,
    attachments: [], mentions: [], model: null, requestId: "r1", parentId: "u1" },
];
const request = {
  id: "r1", conversationId: id, preview: "one", state: { type: "done" }, startedAtMs: start,
  endedAtMs: start + 30_000, steeredInto: null, steeredAfter: null, undo: null, worked: [], quotaWait: false,
} as unknown as UserRequest;
const queued: QueuedMessage = { id: "q1", text: QUEUED, attachments: [], mentions: [], queuedAtMs: start, editedAtMs: null } as QueuedMessage;

localStorage.setItem("brigadier.drafts", JSON.stringify({
  [id]: {
    text: DRAFT, attachments: [], atMs: Date.now(),
    mentions: [{ at: DRAFT.indexOf("@"), name: "Release *draft*", mention: { type: "chat", id: "c1", title: "Release *draft*" } }],
  },
}));

const sends: { text: string; mentions: unknown[]; attachments: AttachmentRef[] }[] = [];
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
          requests: [request], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [],
          compactions: [], decisions: [], queue: { items: [queued], paused: false }, run: "idle",
          runRequest: null, head: "a1", ratings: {}, streaming: null, notices: [],
          previews: [],
        } as unknown as View,
      };
    case "addAttachment":
      return {
        method: req.method,
        attachment: { id: `blob-${sequence++}`, name: req.name, mime: req.mime, bytes: atob(req.data).length, pasted: req.pasted, inline: null },
      };
    case "deleteQueued":
      return { method: req.method, queue: { items: [], paused: false } };
    case "sendMessage": {
      sends.push({ text: req.text, mentions: req.mentions, attachments: req.attachments });
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
const has = (selector: string, text?: string) =>
  [...(field()?.querySelectorAll(selector) ?? [])].some((element) => text === undefined || element.textContent === text);

/** Selects the field's text, once the editor has seen the selection change. */
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

/** Sends what is typed with Enter, once its attachments are stored, and returns the text sent. */
async function send(): Promise<string> {
  const before = sends.length;
  // Enter is refused while an image is still being stored (Send says Files uploading…).
  await until("the attachments stored", () => document.querySelector(".aui-composer-send")?.hasAttribute("disabled") === false);
  await key("Enter");
  await until("the send", () => sends.length > before);
  await until("the field emptied", () => typed() === "");
  return sends.at(-1)!.text;
}

/** The Plain text composer switch, as Settings › General sets it. */
const plainOn = (on: boolean) =>
  useApp.setState((state) => ({ settings: { ...state.settings, plainTextComposer: on } }));
/** The field in either mode. */
const anyField = () => document.querySelector<HTMLElement>("[data-slot=composer-input] [contenteditable=true]");

const imageChip = () => document.querySelector("[aria-label='Preview Image #1']");

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    await until("the field", () => field() !== null);
    // A draft comes back formatted, its mention (with Markdown in its name) a chip.
    await until("the draft", () => has("h2") && field()!.querySelector("[data-slot=composer-chip]") !== null);
    seen.draft = {
      heading: has("h2", "Draft title"),
      italic: has("h2 .italic", "title"),
      chip: [...field()!.querySelectorAll("[data-slot=composer-chip]")].map((chip) => chip.getAttribute("data-chip")),
      code: has(".font-mono", "x"),
    };
    await fixtureClick(field()!);
    seen.draftSent = await send();
    seen.draftMentions = sends.at(-1)!.mentions;

    // ↑ in the empty field pulls the queued message back, formatted.
    await key("ArrowUp");
    await until("the queued message", () => has("ol"));
    seen.queued = { items: field()!.querySelectorAll("ol > li").length, code: has(".font-mono", "second") };
    await clear();
    // ↑ again walks back to the earlier prompts.
    for (let tries = 0; tries < 3 && !has("ul"); tries++) {
      await key("ArrowUp");
      await sleep(100);
    }
    await until("the earlier prompt", () => has("ul"));
    seen.recalled = { items: field()!.querySelectorAll("ul > li").length, bold: has(".font-semibold", "two") };
    await clear();

    // Markers turn into formatting as they are typed, and go as Markdown.
    await type("**bold** and *it* ");
    await until("bold and italic", () => has(".font-semibold", "bold") && has(".italic", "it"));
    seen.marks = await send();

    // Shift+Enter: the next list item; Enter sends, inside a list too.
    await type("- a");
    await key("Enter", ["Shift"]);
    await type("b");
    seen.list = await send();
    await type("a");
    await key("Enter", ["Shift"]);
    await type("b");
    seen.lines = await send();

    // A code block: coloured, Enter sends from inside it.
    await type("```ts ");
    await until("a code block", () => has("code"));
    await type("let x = 1");
    await until("colours", () => [...field()!.querySelectorAll("code span")].some((span) => (span as HTMLElement).style.color !== ""));
    seen.coloured = true;
    seen.codeSent = await send();
    // Shift+Enter is a new line in it; on an empty last line it leaves the block.
    await type("``` ");
    await until("a code block again", () => has("code"));
    await type("x");
    await key("Enter", ["Shift"]);
    await type("y");
    await key("Enter", ["Shift"]);
    await key("Enter", ["Shift"]);
    await type("after");
    seen.codeLeft = await send();
    // A closing ``` line leaves it too.
    await type("``` ");
    await until("a third code block", () => has("code"));
    await type("z");
    await key("Enter", ["Shift"]);
    await type("```");
    await type("w");
    seen.fenceLeft = await send();
    // And ↓ on its last line.
    await type("``` ");
    await until("a fourth code block", () => has("code"));
    await type("q");
    await key("ArrowDown");
    await type("r");
    seen.downLeft = await send();

    // No key formats text: ⌘B (Ctrl+B) does nothing in the field.
    await type("plain");
    await selectAll();
    await key("b", ["Meta"]);
    await key("b", ["Control"]);
    await key("ArrowRight");
    await type("x");
    seen.noBold = { bold: has(".font-semibold"), sent: await send() };

    // The @ menu opens in a list item and after formatting: the caret's place in the Markdown.
    await type("- see @");
    await until("the @ menu in a list", () => document.querySelector("[data-slot=composer-mentions]") !== null);
    await key("Escape");
    await clear();
    await type("**b** @");
    await until("the @ menu after bold", () => document.querySelector("[data-slot=composer-mentions]") !== null);
    seen.menus = true;
    await key("Escape");
    await clear();

    // A task list: its box ticks with a click, and goes as Markdown.
    await type("- [ ] task");
    await until("a task item", () => has("li.composer-check", "task"));
    const box = field()!.querySelector("li.composer-check")!.getBoundingClientRect();
    await fixtureInput({ type: "click", x: box.x + 6, y: box.y + box.height / 2 });
    await until("the box ticked", () => has("li.composer-check-on", "task"));
    await fixtureClick(field()!.querySelector("li.composer-check")!);
    seen.task = await send();

    // What is typed is kept as a draft, as Markdown.
    await type("**kept** ");
    await until("the draft kept", () => (JSON.parse(localStorage.getItem("brigadier.drafts") ?? "{}") as Record<string, { text?: string }>)[id]?.text === "**kept** ");
    seen.draftKept = true;
    await clear();

    // A pasted image is an [Image #n] chip, as in the plain field: deleted with its image, back with undo.
    const canvas = document.createElement("canvas");
    canvas.width = 8;
    canvas.height = 8;
    const png = await new Promise<Blob>((resolve) => canvas.toBlob((blob) => resolve(blob!), "image/png"));
    const image = new DataTransfer();
    image.items.add(new File([png], "image.png", { type: "image/png" }));
    field()!.dispatchEvent(new ClipboardEvent("paste", { clipboardData: image, bubbles: true, cancelable: true }));
    await until("the image chip", () => imageChip() !== null);
    await key("Backspace");
    await key("Backspace");
    await until("the chip deleted", () => imageChip() === null);
    await key("z", [navigator.platform.startsWith("Mac") ? "Meta" : "Control"]);
    await until("the chip back", () => imageChip() !== null);
    await key("ArrowRight");
    await type(" look");
    const sent = await send();
    seen.image = { sent, inline: sends.at(-1)!.attachments.map((ref) => ref.inline) };

    // Plain text pasted stays literal: its markers go escaped.
    const clipboardData = new DataTransfer();
    clipboardData.setData("text/plain", "**x** and snake_case");
    field()!.dispatchEvent(new ClipboardEvent("paste", { clipboardData, bubbles: true, cancelable: true }));
    await until("the paste", () => typed() === "**x** and snake_case");
    seen.literal = { bold: has(".font-semibold"), sent: await send() };

    // The Plain text composer switch turns the open draft into its Markdown as text, and back.
    setRichComposer(null);
    await type("**b** and `c` see [l](https://x.y) ");
    await until("formatting", () => has(".font-semibold", "b") && has("a", "l"));
    plainOn(true);
    await until("the plain field", () => field() === null && anyField() !== null);
    await until("the draft as text", () => anyField()!.textContent === "**b** and `c` see [l](https://x.y) ");
    seen.plain = {
      text: anyField()!.textContent,
      // Code and links stay literal text there, as Markdown does.
      chips: anyField()!.querySelectorAll("[data-slot=composer-chip]").length,
      bold: anyField()!.querySelector(".font-semibold") !== null,
    };
    plainOn(false);
    await until("the rich field again", () => field() !== null);
    await until("formatting again", () => has(".font-semibold", "b") && has(".font-mono", "c") && has("a", "l"));
    // Switched in Settings, the field takes the keyboard again with a click.
    await fixtureClick(field()!);
    seen.richAgain = await send();
  } catch (error) {
    seen.error = String(error);
  }
  const result = document.createElement("pre");
  result.id = "rich-composer-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

select({ type: "conversation", id });
// Shown, as the app shows it, once the conversation is selected: once its view is in.
await until("the conversation shown", () => useApp.getState().selection.type === "conversation");
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
