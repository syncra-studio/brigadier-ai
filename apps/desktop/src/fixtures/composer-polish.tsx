/**
 * A standalone Vite entry, excluded from the app entry and production build: the composer's
 * small behaviours in the app's own conversation view, one case per page load (`?case=`). A Chat
 * whose one request is over; IPC is mocked and records what the composer sends. It drives the
 * real keys (`fixtureInput`) and writes what it saw as JSON into `<pre id="composer-polish-result">`.
 */
import { emit } from "@tauri-apps/api/event";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { FolderDropZone } from "@/app/shell/FolderDropZone";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, ConversationView as View, Message, Request, RunState, UserRequest } from "@/ipc/generated";
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

let conversation = {
  id, kind: "chat", projectId: null, title: "A chat", pinnedAtMs: null, createdAtMs: start,
  updatedAtMs: start, lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
  setup: { type: "chat", model: { provider: "claude", model: null, effort: null, fast: false } },
} as unknown as Conversation;
// The `plan` case's is a session in Plan mode.
if (which === "plan") {
  conversation = { ...conversation, kind: "session", title: "A session", setup: { type: "session", repo: "/fixture",
    environment: { type: "localCheckout", branch: "fixture" }, permission: "askForApproval", planMode: true,
    workersSeeUncommitted: null, orchestrator: { provider: "claude", model: null, effort: null } } } as unknown as Conversation;
}
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
/** The files attached, by name and type. */
const attached: string[] = [];
/** The window's own drags and drops (macOS), for the `shift` case: Shift held, and the last drop. */
let shiftHeld = false;
let dropped: { shift: boolean; entries: { path: string; folder: boolean }[] } | null = null;
/** Reads a dropped file's bytes, once the `dropSend` case lets it (`null`: none waits). */
let finishRead: (() => void) | null = null;
let holdRead = false;
/** Answers the `stop` case's interrupt, as done or failed (`null`: none waits). */
let finishInterrupt: ((fail: boolean) => void) | null = null;
mockWindows("main");
mockIPC((command, payload) => {
  if (command === "drag_shift") return shiftHeld;
  if (command === "last_drop") return dropped;
  if (command === "read_dropped") {
    const bytes = new TextEncoder().encode("notes").buffer;
    if (!holdRead) return bytes;
    return new Promise((resolve) => {
      finishRead = () => resolve(bytes);
    });
  }
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "checkFolder":
      sent.push(req);
      return { method: req.method, check: { kind: "invalid", reason: "Not a folder" } };
    case "addAttachment": {
      const { name, mime } = req;
      attached.push(`${name} ${mime}`);
      const attachment = { id: `blob-${name}`, name, mime, bytes: 4, pasted: false, inline: null };
      return new Promise((resolve) => {
        finishUpload = () => resolve({ method: req.method, attachment });
      });
    }
    case "interrupt":
      sent.push(req);
      if (which !== "stop") return { method: req.method };
      return new Promise((resolve, reject) => {
        finishInterrupt = (fail) => (fail ? reject(new Error("The turn would not stop.")) : resolve({ method: req.method }));
      });
    case "getDictation":
      // Dictation is on offer, its speech model not yet downloaded (starting it downloads first).
      return { method: req.method, dictation: { available: true, installed: false, downloading: false, modelBytes: 1000 } };
    case "updateSetup":
      sent.push(req);
      conversation = { ...conversation, setup: req.setup };
      return { method: req.method, conversation };
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
}, { shouldMockEvents: true });
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

/** The conversation's model's state, as the daemon would publish it. */
const setRun = (run: RunState) => useBoard.setState(({ board }) => (board ? { board: { ...board, run } } : {}));

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
  // Saved from a web address, its name keeps the address's escapes (issue 429).
  files.items.add(new File(["notes"], "my%20notes.txt", { type: "text/plain" }));
  field()!.dispatchEvent(new ClipboardEvent("paste", { clipboardData: files, bubbles: true, cancelable: true }));
  await until("the upload", () => finishUpload !== null);
  await sleep(100);
  seen.whileUploading = { disabled: sendButton()?.disabled, tip: sendButton()?.textContent };
  const tile = document.querySelector<HTMLElement>("[data-slot=attachment-file]");
  seen.uploadName = { shown: tile?.querySelector(".text-sm")?.textContent, tip: tile?.title, stored: [...attached] };
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
  // The keyboard back in the field before the next Enter.
  await until("the field focused again", () => focused() === "Message input", 3000);
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
  // #366: once it only waits for the answer to its question card, nothing working, no timer.
  const at = Date.now() - 125_000;
  useBoard.setState(({ board }) =>
    board ? { board: { ...board, requests: { ...board.requests, r1: { ...board.requests["r1"]!, startedAtMs: at, worked: [{ fromMs: at, toMs: null }] } } } } : {},
  );
  await until("the timer", () => placeholder()?.startsWith("Working for 2m") ?? false, 3000).catch(() => {});
  // #446: a follow-up joins the running turn as a request of its own, started now: the turn's
  // time still runs from its first message, as its work header says.
  const joinedAt = Date.now();
  useBoard.setState(({ board }) =>
    board
      ? { board: { ...board, requests: { ...board.requests, r2: { ...board.requests["r1"]!, id: "r2", startedAtMs: joinedAt, worked: [{ fromMs: joinedAt, toMs: null }], steeredInto: "r1" } } } }
      : {},
  );
  await sleep(1100);
  seen.joined = placeholder();
  useBoard.setState(({ board }) => {
    if (!board) return {};
    const { r2: _joined, ...requests } = board.requests;
    return { board: { ...board, requests } };
  });
  useBoard.setState(({ board }) =>
    board ? { board: { ...board, requests: { ...board.requests, r1: { ...board.requests["r1"]!, state: { type: "waiting" }, worked: [{ fromMs: at, toMs: at + 60_000 }], waited: [{ fromMs: at + 60_000, toMs: null }] } } } } : {},
  );
  await until("the usual words", () => placeholder() === seen.idle, 3000).catch(() => {});
  // Then a second more, past the next tick.
  await sleep(1100);
  seen.onCard = placeholder();
}

/** The Dictate button's state ("dictate", "downloading", …). */
const dictate = () => document.querySelector<HTMLElement>(".aui-composer-dictate")?.dataset.state ?? null;

/**
 * #298: Esc (no menu open, nothing running) puts the cursor in the composer rather than taking
 * it out; Esc while dictation gets going cancels it.
 */
async function escCase(seen: Record<string, unknown>) {
  await until("the editor", () => field() !== null);
  await fixtureClick(field()!);
  await fixtureInput({ type: "type", text: "hi" });
  await until("the typed text", () => field()?.textContent === "hi");
  await fixtureInput({ type: "key", key: "Escape" });
  await sleep(100);
  seen.inComposer = focused();
  (document.activeElement as HTMLElement | null)?.blur();
  await fixtureInput({ type: "key", key: "Escape" });
  await until("the composer focused", () => focused() === "Message input", 1000).catch(() => {});
  seen.fromNowhere = focused();
  await until("the Dictate button", () => dictate() === "dictate");
  await fixtureInput({ type: "key", key: "d", modifiers: ["Control", "Shift"] });
  await until("dictation starting", () => dictate() === "downloading", 3000).catch(() => {});
  seen.dictating = dictate();
  await fixtureInput({ type: "key", key: "Escape" });
  await until("dictation cancelled", () => dictate() === "dictate", 1000).catch(() => {});
  seen.afterEsc = { dictate: dictate(), cancelled: sent.some((req) => req.method === "cancelDictationDownload") };
}

/** Whether the composer shows Plan mode on (its Plan chip). */
const planOn = () =>
  [...document.querySelectorAll("button[aria-pressed]")].some((button) => button.textContent?.trim() === "Plan");
/** The setups the composer asked for, by whether each had Plan mode on. */
const planSets = () =>
  sent.flatMap((req) => (req.method === "updateSetup" && req.setup.type === "session" ? [req.setup.planMode] : []));

/** Whether the page's caret, with nothing selected, is at the start of its text. */
function caretAtStart() {
  const selection = getSelection();
  return selection?.isCollapsed === true && selection.anchorOffset === 0;
}

/**
 * #299: in Plan mode, Backspace with the caret at the very start of the composer turns Plan mode
 * off (as it would take away a chip there), keeping the text; anywhere else it deletes as usual.
 */
async function planCase(seen: Record<string, unknown>) {
  await until("the editor", () => field() !== null);
  await until("the Plan chip", () => planOn());
  await fixtureClick(field()!);
  await fixtureInput({ type: "type", text: "abc" });
  await until("the typed text", () => field()?.textContent === "abc");
  await fixtureInput({ type: "key", key: "Backspace" });
  await until("a letter deleted", () => field()?.textContent === "ab");
  seen.midText = { text: field()?.textContent, plan: planOn(), sets: planSets() };
  // The caret to the very start, a step at a time.
  for (let step = 0; step < 4 && !caretAtStart(); step++) {
    await fixtureInput({ type: "key", key: "ArrowLeft" });
    await sleep(50);
  }
  await fixtureInput({ type: "key", key: "Backspace" });
  await until("Plan mode off", () => !planOn(), 2000).catch(() => {});
  seen.atStart = { text: field()?.textContent, plan: planOn(), sets: planSets() };
}

/** The drop overlay over the thread. */
const overlay = () => document.querySelector<HTMLElement>("[data-slot=drop-overlay]");

/** A CSS colour as 8-bit red, green, blue and alpha (drawn, so any colour syntax reads). */
function rgba(color: string): [number, number, number, number] {
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 1;
  const context = canvas.getContext("2d")!;
  context.fillStyle = color;
  context.fillRect(0, 0, 1, 1);
  const [red = 0, green = 0, blue = 0, alpha = 0] = context.getImageData(0, 0, 1, 1).data;
  return [red, green, blue, alpha];
}

/**
 * #300: a file dragged over the thread tints it softly in the accent colour, with a small
 * "Drop to attach" pill; no grey veil, no dashed frame.
 */
async function dropCase(seen: Record<string, unknown>) {
  await until("the editor", () => field() !== null);
  const files = new DataTransfer();
  files.items.add(new File(["notes"], "notes.txt", { type: "text/plain" }));
  field()!.dispatchEvent(new DragEvent("dragenter", { dataTransfer: files, bubbles: true, cancelable: true }));
  await until("the overlay", () => overlay() !== null && getComputedStyle(overlay()!).display !== "none", 3000);
  const all = [overlay()!, ...overlay()!.querySelectorAll<HTMLElement>("*")];
  const pill = overlay()!.querySelector<HTMLElement>("[data-slot=drop-pill]");
  const [red, , blue, alpha] = rgba(getComputedStyle(overlay()!).backgroundColor);
  seen.overlay = {
    dashed: all.some((element) => getComputedStyle(element).borderStyle.includes("dashed")),
    blurred: all.some((element) => getComputedStyle(element).backdropFilter !== "none"),
    // The accent's blue, faint: not a grey, and the thread still shows through.
    tinted: blue - red > 20 && alpha > 0 && alpha < 64,
    pill: pill?.textContent?.trim() ?? null,
    pillSmall: pill ? pill.getBoundingClientRect().height <= 40 && pill.getBoundingClientRect().width <= 240 : false,
  };
  field()!.dispatchEvent(new DragEvent("dragleave", { dataTransfer: files, bubbles: true, cancelable: true }));
}

/** The folder veil, while something is dragged over the window off the thread. */
const folderVeil = () => document.body.textContent?.includes("Drop folders to add them as projects") ?? false;
/** Whether "Drop to attach" shows. */
const overlayShown = () => overlay() !== null && getComputedStyle(overlay()!).display !== "none";
/** The middle of an element, where a drag is. */
const middle = (element: Element) => {
  const rect = element.getBoundingClientRect();
  return { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 };
};

/**
 * #301: Shift held while dropping files on the thread types their paths at the composer's caret
 * instead of attaching them, and no overlay shows: by the page's drop (Linux, Windows) and by the
 * window's own (macOS), where a plain drop attaches and one off the thread adds folders.
 */
async function shiftCase(seen: Record<string, unknown>) {
  await until("the editor", () => field() !== null);
  await fixtureClick(field()!);
  await fixtureInput({ type: "type", text: "see" });
  await until("the typed text", () => field()?.textContent === "see");
  // The page's drop, its file URL given (WebKitGTK).
  const files = new DataTransfer();
  files.items.add(new File(["notes"], "notes v2.txt", { type: "text/plain" }));
  files.setData("text/uri-list", "file:///Users/me/notes%20v2.txt");
  const drag = (type: string, shiftKey: boolean) =>
    field()!.dispatchEvent(new DragEvent(type, { dataTransfer: files, bubbles: true, cancelable: true, shiftKey }));
  drag("dragenter", true);
  drag("dragover", true);
  await sleep(100);
  seen.pageOverlay = overlayShown();
  drag("drop", true);
  await until("the path typed", () => field()?.textContent === "see /Users/me/notes v2.txt", 2000).catch(() => {});
  seen.pageText = field()?.textContent;
  // The window's own drag, with Shift held.
  shiftHeld = true;
  const at = middle(field()!);
  await emit("tauri://drag-enter", { paths: ["/Users/me/a.txt"], position: at });
  await emit("tauri://drag-over", { position: at });
  await sleep(150);
  seen.nativeShift = { overlay: overlayShown(), veil: folderVeil() };
  dropped = { shift: true, entries: [{ path: "/Users/me/a.txt", folder: false }] };
  await emit("tauri://drag-drop", { paths: ["/Users/me/a.txt"], position: at });
  await until("the native path typed", () => field()?.textContent?.endsWith(" /Users/me/a.txt") ?? false, 2000).catch(() => {});
  seen.nativeText = field()?.textContent;
  // Without Shift: the overlay, then the file attached.
  shiftHeld = false;
  await emit("tauri://drag-enter", { paths: ["/Users/me/a.png"], position: at });
  await emit("tauri://drag-over", { position: at });
  await until("the overlay", overlayShown, 2000).catch(() => {});
  seen.nativePlain = { overlay: overlayShown(), veil: folderVeil() };
  dropped = { shift: false, entries: [{ path: "/Users/me/a.png", folder: false }] };
  await emit("tauri://drag-drop", { paths: ["/Users/me/a.png"], position: at });
  await until("the file attached", () => attached.length > 0, 2000).catch(() => {});
  seen.attached = [...attached];
  seen.afterDrop = overlayShown();
  // Off the thread: the folder veil, and the drop goes to the folders.
  const off = document.createElement("div");
  off.style.cssText = "position:fixed;left:0;top:0;width:40px;height:40px;z-index:100";
  document.body.append(off);
  const there = middle(off);
  await emit("tauri://drag-enter", { paths: ["/Users/me/repo"], position: there });
  await until("the folder veil", folderVeil, 2000).catch(() => {});
  seen.offThread = { overlay: overlayShown(), veil: folderVeil() };
  dropped = { shift: false, entries: [{ path: "/Users/me/repo", folder: true }] };
  await emit("tauri://drag-drop", { paths: ["/Users/me/repo"], position: there });
  await until("the folder checked", () => sent.some((req) => req.method === "checkFolder"), 2000).catch(() => {});
  seen.folders = sent.flatMap((req) => (req.method === "checkFolder" ? [req.path] : []));
}

/** The second dropped file's remove ×, in the `dropSend` case. */
const remove = () => document.querySelector<HTMLElement>("[aria-label='Remove b.png']");

/**
 * #330: Send while a file dropped on the window (macOS) is still being read waits for it, as for
 * an upload: the file goes with that message, and the next draft starts empty.
 */
async function dropSendCase(seen: Record<string, unknown>) {
  await until("the editor", () => field() !== null);
  await fixtureClick(field()!);
  await fixtureInput({ type: "type", text: "see this" });
  await until("the typed text", () => field()?.textContent === "see this");
  holdRead = true;
  const at = middle(field()!);
  await emit("tauri://drag-enter", { paths: ["/Users/me/a.png"], position: at });
  await emit("tauri://drag-over", { position: at });
  dropped = { shift: false, entries: [{ path: "/Users/me/a.png", folder: false }] };
  await emit("tauri://drag-drop", { paths: ["/Users/me/a.png"], position: at });
  await until("the read", () => finishRead !== null, 3000);
  await fixtureInput({ type: "key", key: "Enter" });
  await sleep(200);
  seen.whileReading = { sent: sends().length, disabled: sendButton()?.disabled, tip: sendButton()?.textContent };
  finishRead!();
  await until("the upload", () => finishUpload !== null, 3000);
  finishUpload!();
  await until("Send ready", () => sendButton()?.disabled === false, 3000);
  await fixtureInput({ type: "key", key: "Enter" });
  await until("the message sent", () => sends().length > 0, 3000);
  await sleep(300);
  seen.messages = sent.flatMap((req) =>
    req.method === "sendMessage" ? [{ text: req.text, attachments: req.attachments.map((ref) => ref.name) }] : [],
  );
  seen.nextDraft = {
    text: field()?.textContent ?? null,
    attachments: document.querySelectorAll("[data-slot=composer-attachment]").length,
  };
  // Another drop, read, its tile removed while it uploads: Send is free again.
  finishRead = null;
  finishUpload = null;
  await fixtureInput({ type: "type", text: "again" });
  dropped = { shift: false, entries: [{ path: "/Users/me/b.png", folder: false }] };
  await emit("tauri://drag-enter", { paths: ["/Users/me/b.png"], position: at });
  await emit("tauri://drag-drop", { paths: ["/Users/me/b.png"], position: at });
  await until("the second read", () => finishRead !== null, 3000);
  finishRead!();
  await until("the second upload", () => finishUpload !== null, 3000);
  await until("its tile", () => remove() !== null, 3000);
  await fixtureClick(remove()!);
  await until("the tile gone", () => remove() === null, 3000).catch(() => {});
  await sleep(200);
  seen.removedWhileUploading = { disabled: sendButton()?.disabled, tip: sendButton()?.textContent };
}

/**
 * #400: a merge stops and starts the conversation's work several times a second. Stop stays Stop
 * throughout, its glyph showing, and the button goes back to Send once the work has stayed over.
 */
async function flipsCase(seen: Record<string, unknown>) {
  await until("the editor", () => field() !== null);
  setState({ type: "working" });
  await until("Stop", () => sendButton()?.dataset.state === "stop", 3000);
  const states = new Set<string>();
  for (let flip = 0; flip < 10; flip++) {
    setState(flip % 2 === 0 ? { type: "done" } : { type: "working" });
    await sleep(80);
    states.add(sendButton()?.dataset.state ?? "none");
  }
  seen.during = [...states];
  const glyph = sendButton()?.querySelector("svg");
  seen.glyph = glyph ? Number(getComputedStyle(glyph.parentElement!).opacity) > 0.9 : false;
  setState({ type: "done" });
  await until("Send back", () => sendButton()?.dataset.state === "send", 3000).catch(() => {});
  seen.after = sendButton()?.dataset.state;
}

/**
 * #422: a session's model starts again for each worker report and landing while the work goes
 * on. Its first start shows the starting spinner; every later one keeps Stop, never dimming.
 */
async function restartsCase(seen: Record<string, unknown>) {
  await until("the editor", () => field() !== null);
  setState({ type: "working" });
  setRun("starting");
  await until("the first start", () => sendButton()?.dataset.state === "starting", 3000);
  seen.first = sendButton()?.dataset.state;
  setRun("running");
  await until("Stop", () => sendButton()?.dataset.state === "stop", 3000);
  const states = new Set<string>();
  for (const run of ["idle", "starting", "running", "idle", "starting", "running"] as const) {
    setRun(run);
    await sleep(80);
    states.add(sendButton()?.dataset.state ?? "none");
  }
  seen.during = [...states];
  setRun("idle");
  setState({ type: "done" });
  await until("Send back", () => sendButton()?.dataset.state === "send", 3000).catch(() => {});
  seen.after = sendButton()?.dataset.state;
  // The next request's first start shows the spinner again.
  setState({ type: "working" });
  setRun("starting");
  await until("the next start", () => sendButton()?.dataset.state === "starting", 3000).catch(() => {});
  seen.next = sendButton()?.dataset.state;
}

/**
 * #498: Stop shows the turn stopped the moment it is pressed, while the daemon still stops it; a
 * stop that fails shows it working again, and says why.
 */
async function stopCase(seen: Record<string, unknown>) {
  await until("the editor", () => field() !== null);
  setState({ type: "working" });
  setRun("running");
  await until("Stop", () => sendButton()?.dataset.state === "stop", 3000);
  await fixtureClick(sendButton()!);
  await until("the interrupt", () => finishInterrupt !== null, 3000);
  // A frame or two, well under the hold Stop keeps through the work's own flips.
  await sleep(100);
  seen.whileStopping = sendButton()?.dataset.state;
  finishInterrupt!(true);
  await until("Stop again", () => sendButton()?.dataset.state === "stop", 3000).catch(() => {});
  seen.afterFailure = sendButton()?.dataset.state;
  seen.said = document.body.textContent?.includes("The turn would not stop.") ?? false;
}

const CASES: Record<string, (seen: Record<string, unknown>) => Promise<void>> = {
  focus: focusCase,
  uploading: uploadingCase,
  paused: pausedCase,
  working: workingCase,
  esc: escCase,
  plan: planCase,
  drop: dropCase,
  shift: shiftCase,
  dropSend: dropSendCase,
  flips: flipsCase,
  restarts: restartsCase,
  stop: stopCase,
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

await select({ type: "conversation", id });
void drive();
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
      <main className="relative flex min-h-0 w-full flex-1 flex-col">
        <ConversationView key={id} selection={{ type: "conversation", id }} />
      </main>
      <FolderDropZone />
    </SidebarProvider>
  </TooltipProvider>,
);
