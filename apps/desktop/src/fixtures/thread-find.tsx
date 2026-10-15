/**
 * A standalone Vite entry, excluded from the app entry and production build: Find in the chat
 * (ThreadFind.tsx) in the app's own conversation view, on a synthetic chat of 60 turns. Opening
 * it brings the newest 40 (the older 10 of them folded behind "N earlier messages"); the oldest
 * 20 stay in the database. Requests are answered here and never reach a daemon. The words found:
 * - "solitary" once, "needle" in seven answers, "tick" seven times in every loaded answer (280);
 * - "zephyr" only in a folded turn, "quokka" only in a turn never loaded;
 * - "marmalade" at the end of a long message of the user's, clipped behind "Show more".
 *
 * `?drive=1` runs the checks the find test reads and writes them to `<pre id="thread-find-result">`;
 * `window.threadFind` holds the steps for a browser probe.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/ConversationView";
import { ACTIVE_MATCH, MATCHES } from "@/app/conversation/ThreadFind";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, ConversationView as View, EventEnvelope, Message, Request } from "@/ipc/generated";
import { select } from "@/state/actions";
import { applyToBoard, emptyBoard, useBoard } from "@/state/board";
import { applyEvents, useApp } from "@/state/store";

const LOAD_MS = 200;
const TURNS = 60;
/** Turns the first load brings. */
const FIRST_TURNS = 40;
const NEEDLE_TURNS = new Set([31, 35, 40, 44, 50, 55, 58]);
const LONG_TURN = 57;
const start = Date.now() - 86_400_000;

const conversation = {
  id: "long", kind: "chat", projectId: null, title: "A long chat", pinnedAtMs: null, createdAtMs: start, updatedAtMs: start, lifecycle: "active",
  forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
  setup: { type: "chat", model: { provider: "claude", model: null, effort: null, fast: false } },
} as unknown as Conversation;

function userText(turn: number): string {
  if (turn !== LONG_TURN) return `Question ${turn + 1}: where does this turn sit in the thread?`;
  const lines = Array.from({ length: 30 }, (_, index) => `Line ${index + 1} of a long message, one paragraph each.`);
  return [...lines, "The last line asks for marmalade."].join("\n\n");
}

function answerText(turn: number): string {
  const words = [`Answer ${turn + 1}. The clock goes tick tick tick tick tick tick tick all day.`];
  if (NEEDLE_TURNS.has(turn)) words.push("A needle in this answer.");
  if (turn === 45) words.push("A solitary word.");
  if (turn === 22) words.push("A zephyr in a folded turn.");
  if (turn === 3) words.push("A quokka in a page never loaded.");
  words.push(`Second paragraph of answer ${turn + 1}.`);
  return words.join("\n\n");
}

function messages(): Message[] {
  const list: Message[] = [];
  for (let turn = 0; turn < TURNS; turn++) {
    const request = `long-r${turn}`;
    const at = start + turn * 60_000;
    list.push({
      id: `long-u${turn}`, conversationId: "long", seq: list.length + 1, role: "user", text: userText(turn),
      blob: null, createdAtMs: at, attachments: [], mentions: [], model: null, requestId: request, parentId: list.at(-1)?.id ?? null,
    });
    list.push({
      id: `long-a${turn}`, conversationId: "long", seq: list.length + 1, role: "assistant", text: answerText(turn), blob: null,
      createdAtMs: at + 30_000, attachments: [], mentions: [], model: null, requestId: request, parentId: list.at(-1)?.id ?? null,
    });
  }
  return list;
}

const stored = messages();

function view(): View {
  const first = Math.max(0, stored.length - FIRST_TURNS * 2);
  return {
    conversation,
    context: null,
    messages: { messages: stored.slice(first), hasMore: first > 0 },
    tasks: [], approvals: [], questions: [], plans: [], requests: [], workerSteps: [], orchestratorSteps: [],
    machineSteps: [], thinking: [], compactions: [], decisions: [], waiting: [],
    queue: { items: [], paused: false },
    run: "idle", runRequest: null, head: stored.at(-1)?.id ?? null, ratings: {}, streaming: null,
    notices: [], memories: [], overnight: [], reviews: [], previews: [],
  } as unknown as View;
}

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const frame = () => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve)));
let pending = 0;
let sequence = 1000;

mockIPC(async (command, payload) => {
  pending++;
  try {
    return await answer(command, payload);
  } finally {
    pending--;
  }
});

async function answer(command: string, payload: unknown): Promise<unknown> {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getConversation":
      await wait(LOAD_MS);
      return { method: req.method, view: view() };
    case "listMessages":
      // Older pages never come: what the database holds beyond the first load stays there.
      throw new Error("Not loaded in this fixture.");
    case "getPullRequest":
      return { method: req.method, pullRequest: null };
    case "getRunDiff":
      return { method: req.method, diff: null };
    case "listFiles":
      return { method: req.method, files: [], truncated: false };
    default:
      return { method: req.method };
  }
}

/** The daemon's event for a new message, to the app's store and the open conversation's board. */
function appended(message: Message): void {
  const envelope = {
    seq: sequence++, stream: message.conversationId, streamSeq: message.seq, atMs: Date.now(),
    event: { type: "messageAppended", message },
  } as unknown as EventEnvelope;
  applyEvents([envelope]);
  useBoard.setState(({ board }) => (board?.conversationId === message.conversationId ? { board: applyToBoard(board, envelope) } : {}));
}

useApp.setState((state) => ({
  conversations: { long: conversation },
  connection: { status: "connected", daemon: null, reason: null },
  pinnedSummary: false,
  info: { ...state.info, platform: "macos" } as typeof state.info,
}));

function Page() {
  const selection = useApp((s) => s.selection);
  useEffect(() => {
    if (new URLSearchParams(location.search).get("drive") === "1") void drive();
  }, []);
  return (
    <TooltipProvider>
      <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
        <main className="relative flex min-h-0 w-full flex-1 flex-col">
          <ConversationView key={selection.type === "conversation" ? selection.id : selection.type} selection={selection} />
        </main>
      </SidebarProvider>
    </TooltipProvider>
  );
}

/* ----- the checks ----------------------------------------------------------------------- */

const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const viewport = () => $('[data-slot="aui_thread-viewport"]')!;
const bar = () => $('[data-slot="thread-find"]');
const field = () => $<HTMLInputElement>('input[aria-label="Find in chat"]');
const label = () => $('[data-slot="thread-find-count"]')?.textContent ?? null;
const button = (name: string) => $<HTMLButtonElement>(`[data-slot="thread-find"] [data-find="${name}"]`);
const round = (value: number) => Math.round(value * 100) / 100;

function layoutNow(): string {
  const box = viewport();
  return `${box.scrollTop} ${box.scrollHeight} ${box.clientHeight} ${label() ?? ""}`;
}

/** A finite, time-driven animation still running (not a spinner). */
const animating = () =>
  document.getAnimations().some(
    (animation) =>
      animation.playState === "running" && animation.timeline instanceof DocumentTimeline && animation.effect?.getTiming().iterations !== Infinity,
  );

/** Waits `ms`, then until every request is answered, nothing animates and the layout holds for 100ms of frames. */
async function settle(ms: number): Promise<void> {
  await wait(ms);
  const deadline = performance.now() + 10_000;
  let last = layoutNow();
  let stillSince = performance.now();
  for (let frames = 0; performance.now() < deadline; ) {
    await frame();
    const now = layoutNow();
    if (pending > 0 || $('[data-slot="reply-loading"]') || animating() || now !== last) {
      stillSince = performance.now();
      frames = 0;
    } else if (++frames >= 3 && performance.now() - stillSince >= 200) return;
    last = now;
  }
}

/** Waits a frame at a time until `check` holds, up to 10s; whether it did. */
async function until(check: () => boolean): Promise<boolean> {
  const end = performance.now() + 10_000;
  while (!check()) {
    if (performance.now() > end) return false;
    await frame();
  }
  return true;
}

/** A key press as the keyboard (or the macOS menu) sends it to `target`. */
function press(target: EventTarget, code: string, mods: { meta?: boolean; shift?: boolean } = {}): void {
  const key = code === "Enter" || code === "Escape" ? code : code.replace(/^Key/, "").toLowerCase();
  target.dispatchEvent(
    new KeyboardEvent("keydown", { key, code, metaKey: mods.meta ?? false, shiftKey: mods.shift ?? false, bubbles: true, cancelable: true }),
  );
}

/** Types `text` into the find field, as the user would, replacing what it held. */
function typeQuery(text: string): void {
  const input = field()!;
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, text);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

/** The text node in the thread holding `word`, and where. */
function textHolding(word: string): { node: Text; at: number } | null {
  const walker = document.createTreeWalker(viewport(), NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const at = (node as Text).data.indexOf(word);
    if (at >= 0) return { node: node as Text, at };
  }
  return null;
}

function selectRange(range: Range): void {
  const selection = getSelection()!;
  selection.removeAllRanges();
  selection.addRange(range);
}

const matches = () => CSS.highlights.get(MATCHES);
const activeMatch = (): Range | null => {
  const highlight = CSS.highlights.get(ACTIVE_MATCH);
  const first = highlight ? [...highlight][0] : undefined;
  return first instanceof Range ? first : null;
};

/** Where the active match is in the view: inside its band above the composer, and off its centre by how much. */
function activePlace() {
  const range = activeMatch();
  if (!range) return null;
  const rect = range.getBoundingClientRect();
  const box = viewport().getBoundingClientRect();
  const footer = $("[data-thread-scroll-footer]")!.getBoundingClientRect().height;
  const barBottom = bar()?.getBoundingClientRect().bottom ?? box.top;
  const top = barBottom;
  const bottom = box.bottom - footer;
  return {
    text: range.toString(),
    inView: rect.top >= top && rect.bottom <= bottom,
    centreOff: round(rect.top + rect.height / 2 - (top + 16 + bottom - 16) / 2),
  };
}

/** Types `text` and waits for the count to show and the view to settle. */
async function find(text: string) {
  typeQuery(text);
  await until(() => label() !== null);
  await settle(100);
  return { label: label(), size: matches()?.size ?? 0, active: activeMatch()?.toString() ?? null, place: activePlace() };
}

/** Presses a key on the find field and waits for the count to change and the view to settle. */
async function stepWith(code: string, mods: { meta?: boolean; shift?: boolean } = {}) {
  const before = label();
  press(field()!, code, mods);
  await until(() => label() !== before);
  await settle(50);
  return label();
}

/** A colour as the canvas paints it: its red, green, blue and alpha, 0–255. */
function channels(variable: string): number[] {
  const probe = document.createElement("div");
  probe.style.backgroundColor = `var(${variable})`;
  document.body.append(probe);
  const colour = getComputedStyle(probe).backgroundColor;
  probe.remove();
  const canvas = document.createElement("canvas");
  canvas.width = 1;
  canvas.height = 1;
  const context = canvas.getContext("2d")!;
  context.fillStyle = colour;
  context.fillRect(0, 0, 1, 1);
  return [...context.getImageData(0, 0, 1, 1).data];
}

/** The "Show more" toggle of the long message: whether it is open. */
function toggle(): string | null {
  const word = textHolding("marmalade");
  const bubble = word?.node.parentElement?.closest(".aui-user-message-content");
  return bubble?.querySelector(":scope > button[aria-expanded]")?.getAttribute("aria-expanded") ?? null;
}

const steps = {
  /** A single line selected in the thread, then ⌘F: the bar opens at the top right, with it as its text, focused. */
  async opened() {
    select({ type: "conversation", id: "long" });
    await settle(LOAD_MS + 400);
    const foldedBefore = $('[data-slot="earlier-turns"]')?.textContent ?? null;
    const word = textHolding("solitary")!;
    const range = document.createRange();
    range.setStart(word.node, word.at);
    range.setEnd(word.node, word.at + "solitary".length);
    selectRange(range);
    press(document.body, "KeyF", { meta: true });
    await until(() => label() !== null);
    await settle(100);
    const root = $(".aui-thread-root")!.getBoundingClientRect();
    const box = bar()?.getBoundingClientRect();
    const input = field();
    return {
      foldedBefore,
      foldedAfter: $('[data-slot="earlier-turns"]') !== null,
      bar: box ? { top: round(box.top - root.top), right: round(root.right - box.right) } : null,
      maxWidth: bar() ? getComputedStyle(bar()!).maxWidth : null,
      placeholder: input?.placeholder ?? null,
      value: input?.value ?? null,
      focused: document.activeElement === input,
      selected: input ? [input.selectionStart, input.selectionEnd] : null,
      label: label(),
      size: matches()?.size ?? 0,
      active: activeMatch()?.toString() ?? null,
      buttons: ["previous", "next", "close"].map((name) => button(name)?.textContent ?? null),
      matchColour: channels("--find-match"),
      activeColour: channels("--find-match-active"),
    };
  },
  /** The same word typed into the composer is not counted; ⌘F there takes the focus back to the bar's text, selected. */
  async composer() {
    const editable = $<HTMLElement>("[data-slot=composer-input] .aui-lexical-input")!;
    editable.focus();
    document.execCommand("insertText", false, "solitary");
    await settle(300);
    press(editable, "KeyF", { meta: true });
    await until(() => document.activeElement === field());
    await settle(300);
    const input = field();
    return {
      typed: editable.textContent,
      label: label(),
      size: matches()?.size ?? 0,
      focused: document.activeElement === input,
      selected: input ? [input.selectionStart, input.selectionEnd] : null,
    };
  },
  /** Seven matches: Enter goes on round to the first, ⇧Enter back, ⌘G and ⇧⌘G the same. */
  async keys() {
    const found = await find("needle");
    const enters: (string | null)[] = [];
    for (let index = 0; index < 7; index++) enters.push(await stepWith("Enter"));
    const shiftEnter = await stepWith("Enter", { shift: true });
    const commandG = await stepWith("KeyG", { meta: true });
    const shiftCommandG = await stepWith("KeyG", { meta: true, shift: true });
    return { found, enters, shiftEnter, commandG, shiftCommandG };
  },
  /** The oldest needle, far up: it scrolls into view, centred, and the view stops following the bottom. */
  async revealed() {
    let shown = label();
    for (let index = 0; index < 7 && !shown?.startsWith("1 /"); index++) shown = await stepWith("Enter");
    const place = activePlace();
    const scrollTop = viewport().scrollTop;
    const message: Message = {
      id: "long-late", conversationId: "long", seq: stored.length + 1, role: "assistant",
      text: "A late note under the newest answer, long enough to grow the thread below the view.\n\nIt has a second paragraph too.",
      blob: null, createdAtMs: Date.now(), attachments: [], mentions: [], model: null, requestId: `long-r${TURNS - 1}`, parentId: stored.at(-1)?.id ?? null,
    };
    stored.push(message);
    appended(message);
    await settle(400);
    return { label: shown, place, moved: viewport().scrollTop - scrollTop, after: label() };
  },
  /** ⌘F again with the focus elsewhere: the field takes it back, its text selected. */
  async again() {
    viewport().focus();
    getSelection()?.removeAllRanges();
    press(viewport(), "KeyF", { meta: true });
    await until(() => document.activeElement === field());
    const input = field();
    return { focused: document.activeElement === input, selected: input ? [input.selectionStart, input.selectionEnd] : null, value: input?.value ?? null };
  },
  /** Counting stops at 250. */
  async many() {
    return find("tick");
  },
  /** Nothing found: "0 results", the buttons off, no marks. */
  async none() {
    const found = await find("quokka");
    return { ...found, previous: button("previous")?.disabled ?? null, next: button("next")?.disabled ?? null };
  },
  /** A word only in a turn that was folded away: the bar unfolded them, so it is found. */
  async folded() {
    return find("zephyr");
  },
  /** A word in a long message's clipped tail: going to it opens the message. */
  async clipped() {
    const before = toggle();
    const found = await find("marmalade");
    await until(() => toggle() === "true");
    await settle(100);
    return { before, ...found, after: toggle(), place: activePlace() };
  },
  /** Esc closes the bar and clears the marks. */
  async closed() {
    press(field()!, "Escape");
    await until(() => bar() === null);
    await settle(50);
    return { bar: bar() !== null, matches: CSS.highlights.has(MATCHES), active: CSS.highlights.has(ACTIVE_MATCH) };
  },
  /** A selection over two lines gives an empty text. */
  async multiline() {
    const first = textHolding("Second paragraph of answer 50")!;
    const second = textHolding("Question 51")!;
    const range = document.createRange();
    range.setStart(first.node, first.at);
    range.setEnd(second.node, second.at + 5);
    selectRange(range);
    press(document.body, "KeyF", { meta: true });
    await until(() => field() !== null);
    await settle(50);
    const result = { value: field()?.value ?? null, label: label(), size: matches()?.size ?? 0 };
    button("close")?.click();
    await until(() => bar() === null);
    return { ...result, closedByButton: bar() === null };
  },
};

async function drive(): Promise<void> {
  const result: Record<string, unknown> = {};
  try {
    for (const [name, step] of Object.entries(steps)) {
      document.body.dataset["findStep"] = name;
      result[name] = await step();
    }
  } catch (error) {
    result["error"] = String(error instanceof Error ? error.stack : error);
  }
  const pre = document.createElement("pre");
  pre.id = "thread-find-result";
  pre.textContent = JSON.stringify(result);
  document.body.append(pre);
}

Object.assign(window, { threadFind: { steps, settle } });
useBoard.setState({ board: emptyBoard("long") });
createRoot(document.getElementById("root")!).render(<Page />);
