/**
 * A standalone Vite entry, excluded from the app entry and production build: Find in the chat
 * (ThreadFind.tsx) in the app's own conversation view, on a synthetic chat of 60 turns. Opening
 * it brings the newest 40 (the older 10 of them folded behind "N earlier messages"); the oldest
 * 20 stay in the database. Requests are answered here and never reach a daemon. The words found:
 * - "solitary" once, "needle" in seven answers, "tick" seven times in every loaded answer (280);
 * - "zephyr" only in a folded turn, "quokka" only in a turn never loaded, "narwhal" nowhere;
 * - "marmalade" at the end of a long message of the user's, clipped behind "Show more".
 * With `?drive=folds`, turns 51 and 53 did work, folded away: "wombat" only in turn 51's.
 * With `?drive=groups`, turn 51 did three steps, one group: "ocelot" only in its third.
 * With `?drive=older`, the older pages load, five turns a page: "kiwi" in turn 18 (a page not
 * loaded) and turn 46; "quokka" in turn 4, the oldest page. The engine's search is answered here.
 *
 * `?drive=1` runs the checks the find test reads and writes them to `<pre id="thread-find-result">`;
 * `window.threadFind` holds the steps for a browser probe.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { COMPOSER_EDITABLE } from "@/app/conversation/composerTarget";
import { ACTIVE_BUTTON, ACTIVE_MATCH, FINDING, MATCHES } from "@/app/conversation/ThreadFind";
import { ChangesCard } from "@/app/conversation/ChangesCard";
import { ViewContext } from "@/app/conversation/viewContext";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, ConversationView as View, EventEnvelope, Message, MessageMatch, OrchestratorStep, Request, UserRequest } from "@/ipc/generated";
import { revealApp } from "@/lib/splash";
import { select } from "@/state/actions";
import { applyToBoard, emptyBoard, useBoard } from "@/state/board";
import { applyEvents, useApp } from "@/state/store";

const LOAD_MS = 200;
const SHORT = new URLSearchParams(location.search).get("drive") === "short";
/** `?drive=late-editor`: startup isn't over, so the composer's editor waits to load (whenRevealed). */
const LATE_EDITOR = new URLSearchParams(location.search).get("drive") === "late-editor";
/** `?drive=folds`: two turns did work, folded away when they ended. */
const FOLDS = new URLSearchParams(location.search).get("drive") === "folds";
/** `?drive=groups`: a turn did three steps, grouped in one row. */
const GROUPS = new URLSearchParams(location.search).get("drive") === "groups";
/** `?drive=older`: older pages load, `OLDER_PAGE` messages each. */
const CAPPED = new URLSearchParams(location.search).get("drive") === "older-capped";
const OLDER = CAPPED || new URLSearchParams(location.search).get("drive") === "older";
const OLDER_PAGE = 10;
type Work = readonly (readonly [string, string])[];
const WORK_TURNS: ReadonlyMap<number, Work> = GROUPS
  ? new Map([[50, [["Read", "/repo/src/alpha.ts"], ["Read", "/repo/src/beta.ts"], ["Grep", "ocelot"]]]])
  : new Map([[50, [["Grep", "wombat"]]], [52, [["Read", "/repo/src/badger.ts"]]]]);
if (LATE_EDITOR) document.documentElement.dataset["splash"] = "";
const TURNS = SHORT ? 1 : 60;
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
  if (CAPPED) {
    // Keep the first loaded match away from the top's automatic history loader.
    const lead = turn === TURNS - FIRST_TURNS ? "A paragraph before the first loaded match.\n\n".repeat(30) : "";
    return lead + `Answer ${turn + 1}. ` + Array.from({ length: turn < TURNS - FIRST_TURNS ? 60 : 6 }, (_, index) => `${index + 1}:pipit`).join(" ");
  }
  const words = [`Answer ${turn + 1}. The clock goes tick tick tick tick tick tick tick all day.`];
  if (NEEDLE_TURNS.has(turn)) words.push("A needle in this answer.");
  if (turn === 45) words.push("A solitary word.");
  if (turn === 22) words.push("A zephyr in a folded turn.");
  if (turn === 3) words.push("A quokka in a page never loaded.");
  if (turn === 17 || turn === 45) words.push("A kiwi here.");
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

/** The work of `WORK_TURNS`: one tool call each, before the answer. */
function work(): { requests: UserRequest[]; steps: OrchestratorStep[] } {
  if (!FOLDS && !GROUPS) return { requests: [], steps: [] };
  const requests: UserRequest[] = [];
  const steps: OrchestratorStep[] = [];
  for (const [turn, calls] of WORK_TURNS) {
    const id = `long-r${turn}`;
    const at = start + turn * 60_000;
    requests.push({
      id, conversationId: "long", preview: `Question ${turn + 1}`, state: { type: "done" }, startedAtMs: at, endedAtMs: at + 30_000,
      steeredInto: null, steeredAfter: null, undo: null, worked: [{ fromMs: at, toMs: at + 30_000 }], quotaWait: false,
    } as unknown as UserRequest);
    calls.forEach(([name, detail], index) => {
      steps.push({
        requestId: id, atMs: at + 5000 + index * 3000, position: turn * 2 + 1,
        kind: { type: "tool", itemId: `call-${turn}-${index}`, name, detail, status: "completed", throughPosition: turn * 2 + 1, endedAtMs: at + 7000 + index * 3000 },
      } as unknown as OrchestratorStep);
    });
  }
  return { requests, steps };
}

function view(): View {
  const first = Math.max(0, stored.length - FIRST_TURNS * 2);
  const { requests, steps } = work();
  return {
    conversation,
    context: null,
    messages: { messages: stored.slice(first), hasMore: first > 0 },
    tasks: [], approvals: [], questions: [], plans: [], requests, workerSteps: [], orchestratorSteps: steps,
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
/** Older pages asked for. */
let loads = 0;

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
    case "listMessages": {
      // Older pages never come, but with `?drive=older`: what the database holds beyond the first load stays there.
      if (!OLDER) throw new Error("Not loaded in this fixture.");
      loads++;
      await wait(LOAD_MS);
      const older = stored.filter((message) => req.before === null || message.seq < req.before);
      return { method: req.method, page: { messages: older.slice(-OLDER_PAGE), hasMore: older.length > OLDER_PAGE } };
    }
    case "searchMessages": {
      // The engine's search: the messages before the leaf, their text case aside, newest first up to the limit.
      const leaf = stored.find((message) => message.id === req.leaf);
      const needle = req.query.toLowerCase();
      const matches: MessageMatch[] = [];
      let left = req.limit;
      let more = false;
      for (const message of stored.filter((each) => leaf && each.seq < leaf.seq).toReversed()) {
        const count = needle === "" ? 0 : message.text.toLowerCase().split(needle).length - 1;
        if (count === 0) continue;
        if (count > left) more = true;
        if (left === 0) break;
        matches.unshift({ id: message.id, seq: message.seq, count: Math.min(count, left) });
        left -= Math.min(count, left);
        if (more) break;
      }
      return { method: req.method, found: { matches, more } };
    }
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
  summaryShown: { long: false },
  info: { ...state.info, platform: "macos" } as typeof state.info,
}));

function Page() {
  const selection = useApp((s) => s.selection);
  useEffect(() => {
    if (new URLSearchParams(location.search).has("drive")) void drive();
  }, []);
  return (
    <TooltipProvider>
      <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
        <main className="relative flex min-h-0 w-full flex-1 flex-col">
          <ConversationView key={selection.type === "conversation" ? selection.id : selection.type} selection={selection} embedded={new URLSearchParams(location.search).get("drive") === "embedded"} />
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

/** The turn holding answer `turn + 1`. */
const blockOf = (turn: number) => [...document.querySelectorAll('[data-role="assistant"]')].find((each) => each.textContent?.includes(`Answer ${turn + 1}.`));

/** Turn `turn`'s work fold: "open", "closed" (kept for Find to search), or null while it holds nothing. */
const foldOf = (turn: number) => blockOf(turn)?.querySelector('[data-slot="request-fold"]')?.getAttribute("data-fold") ?? null;

/** Whether turn `turn`'s header says its work is open. */
const headerOf = (turn: number) => blockOf(turn)?.querySelector('button[data-slot="request-work-header"]')?.getAttribute("aria-expanded") ?? null;

/** Turn `turn`'s work group: whether it is open ("true", "false"), or null with none. */
const groupOf = (turn: number) => blockOf(turn)?.querySelector('[data-slot="work-group"] > [data-slot="collapsible-trigger"]')?.getAttribute("aria-expanded") ?? null;

/** The rows turn `turn`'s work group shows. */
const groupRows = (turn: number) => blockOf(turn)?.querySelectorAll('[data-slot="work-group"] > [data-slot="collapsible-content"] [data-kind]').length ?? 0;

/** The turn (counted from 1) whose answer holds the match gone to. */
const activeTurn = () => Number(/Answer (\d+)\./.exec(activeMatch()?.startContainer.parentElement?.closest('[data-role="assistant"]')?.textContent ?? "")?.[1] ?? Number.NaN);

/** The answers the thread shows: not those folded into a turn's closed work, kept hidden for Find. */
const shownAnswers = () => [...document.querySelectorAll('[data-slot="aui_assistant-message-content"]')].filter((each) => !each.closest('[data-fold="closed"]'));

/** The `::highlight()` rules on the page: none until marks are shown (they slow every style pass). */
function highlightRules(): string[] {
  return [...document.styleSheets].flatMap((sheet) => [...sheet.cssRules]).flatMap((rule) => /::highlight\(([\w-]+)\)/.exec(rule.cssText)?.[1] ?? []);
}

const steps = {
  /** A single line selected in the thread, then ⌘F: the bar opens at the top right, with it as its text, focused. */
  async opened() {
    await select({ type: "conversation", id: "long" });
    await settle(LOAD_MS + 400);
    const rulesBefore = highlightRules();
    const foldedBefore = $('[data-slot="earlier-turns"]')?.textContent ?? null;
    const word = textHolding("solitary")!;
    const range = document.createRange();
    range.setStart(word.node, word.at);
    range.setEnd(word.node, word.at + "solitary".length);
    selectRange(range);
    const row = viewport().firstElementChild!;
    const selectableBefore = getComputedStyle(row).userSelect;
    press(document.body, "KeyF", { meta: true });
    await until(() => label() !== null);
    await settle(100);
    const root = $(".aui-thread-root")!.getBoundingClientRect();
    const box = bar()?.getBoundingClientRect();
    const input = field();
    return {
      rulesBefore,
      rulesOpen: highlightRules(),
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
      // WebKit marks only selectable text: the thread's text is, while it holds marks.
      selectable: [selectableBefore, viewport().hasAttribute(FINDING) ? getComputedStyle(row).userSelect : null],
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
  /** Counting stops at 250. A new query changes the marks in place: the same sets, so WebKit repaints what they lose. */
  async many() {
    const kept = [matches(), CSS.highlights.get(ACTIVE_MATCH)];
    const found = await find("tick");
    return { ...found, sameMarks: kept[0] === matches() && kept[1] === CSS.highlights.get(ACTIVE_MATCH) };
  },
  /** Nothing found: "0 results", the buttons off, no marks. */
  async none() {
    const found = await find("narwhal");
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
  /** A match at the top of a box that scrolls on its own, scrolled to its end: the box scrolls to it, then the thread. */
  async inner() {
    const answers = shownAnswers();
    const box = document.createElement("div");
    box.className = "max-h-shell-output overflow-y-auto";
    const lines = document.createElement("pre");
    lines.textContent = ["A walrus on the first line.", ...Array.from({ length: 40 }, (_, index) => `filler ${index + 1}`)].join("\n");
    box.append(lines);
    answers.at(-2)?.append(box);
    box.scrollTop = box.scrollHeight;
    await settle(300);
    const found = await find("walrus");
    const range = activeMatch();
    const rect = range?.getBoundingClientRect();
    const sides = box.getBoundingClientRect();
    const inBox = !!rect && rect.top >= sides.top && rect.bottom <= sides.bottom;
    box.remove();
    await settle(300);
    return { label: found.label, inBox, inView: found.place?.inView ?? false };
  },
  /** A match in a button's text, which WebKit never marks: the button gets a ring, gone when another match is gone to. */
  async inButton() {
    const answers = shownAnswers();
    const pangolin = document.createElement("button");
    pangolin.textContent = "Ran the pangolin check";
    answers.at(-2)?.append(pangolin);
    await settle(300);
    const found = await find("pangolin");
    const ringed = pangolin.hasAttribute(ACTIVE_BUTTON);
    await find("walrus pangolin quokka");
    const left = pangolin.hasAttribute(ACTIVE_BUTTON);
    pangolin.remove();
    await settle(300);
    return { label: found.label, ringed, left };
  },
  /** A report's Details opened and closed while the bar is open: what it holds is counted only while it shows. */
  async details() {
    const answers = shownAnswers();
    const details = document.createElement("details");
    details.innerHTML = "<summary>Details</summary><p>The okapi check passed.</p>";
    answers.at(-2)?.append(details);
    await settle(300);
    const closed = (await find("okapi")).label;
    details.open = true;
    await until(() => label() !== closed);
    await settle(50);
    const opened = label();
    details.open = false;
    await until(() => label() !== opened);
    await settle(50);
    const shut = label();
    details.remove();
    await settle(300);
    return { closed, opened, shut };
  },
  /** Esc closes the bar and clears the marks: the sets it drops are emptied first, which is what repaints them. */
  async closed() {
    const kept = [matches(), CSS.highlights.get(ACTIVE_MATCH)];
    press(field()!, "Escape");
    await until(() => bar() === null);
    await settle(50);
    return {
      bar: bar() !== null,
      finding: viewport().hasAttribute(FINDING),
      matches: CSS.highlights.has(MATCHES),
      active: CSS.highlights.has(ACTIVE_MATCH),
      emptied: kept.map((highlight) => highlight?.size ?? null),
      rules: highlightRules(),
    };
  },
  /** Esc gives the keyboard back to what had it, out of view, without scrolling the thread to it. */
  async keepsPlace() {
    const away = viewport().querySelector<HTMLElement>("button")!;
    viewport().dispatchEvent(new WheelEvent("wheel", { deltaY: 1 }));
    viewport().scrollTop = viewport().scrollHeight;
    away.focus({ preventScroll: true });
    press(away, "KeyF", { meta: true });
    await until(() => label() !== null);
    await settle(600);
    const top = viewport().scrollTop;
    press(field()!, "Escape");
    await until(() => bar() === null);
    await settle(300);
    return { focusBack: document.activeElement === away, moved: Math.round(viewport().scrollTop - top) };
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
    const mode = new URLSearchParams(location.search).get("drive");
    if (mode === "welcome" || mode === "embedded") {
      select(mode === "welcome" ? { type: "draft", kind: "chat" } : { type: "conversation", id: "long" });
      await settle(LOAD_MS + 400);
      result["padding"] = getComputedStyle(viewport().querySelector('[data-slot="aui_thread-column"]')!).paddingTop;
    } else if (mode === "glitches") {
      await steps.opened();
      const host = document.createElement("div");
      viewport().querySelector('[data-slot="aui_thread-column"]')!.append(host);
      const changed: UserRequest = {
        id: "find-files", conversationId: "long", preview: "Files", state: { type: "done" },
        startedAtMs: start, endedAtMs: start + 1, steeredInto: null, steeredAfter: null,
        undo: null, worked: [], quotaWait: false,
        changes: { settled: true, stat: { insertions: 2, deletions: 0, files: ["otter.ts", "badger.ts"].map((path) => ({ path, insertions: 1, deletions: 0, binary: false })) } },
      };
      useBoard.setState(({ board }) => ({ board: board && { ...board, requests: { ...board.requests, [changed.id]: changed } } }));
      const root = createRoot(host);
      root.render(<ViewContext.Provider value={{ selection: { type: "conversation", id: "long" }, conversation, embedded: false }}><ChangesCard requestIds={[changed.id]} /></ViewContext.Provider>);
      await settle(300);
      await find("otter.ts");
      const row = host.querySelector('[data-slot="turn-diff-file"] > button')!;
      const style = getComputedStyle(row);
      result["ring"] = { onRow: row.hasAttribute(ACTIVE_BUTTON), inset: parseFloat(style.outlineWidth) + parseFloat(style.outlineOffset) <= 0 };
      // WebKit can paint button ranges on neighbouring selectable text. They must never enter the registry.
      result["buttonMarks"] = matches()?.size ?? 0;
      const prose = document.createElement("p");
      prose.innerHTML = 'before <code class="aui-md-inline-code bg-muted text-code-inline rounded-md px-1.5 py-0.5 font-mono">formatted</code> after';
      host.append(prose);
      await find("before formatted after");
      result["inline"] = { singleTextNodes: [...(matches() ?? [])].every((range) => range.startContainer === range.endContainer), pieces: matches()?.size };
      await find("solitary");
      result["previousGone"] = [...(matches() ?? [])].every((range) => range.toString() === "solitary");
      await find("marmalade");
      const bubble = textHolding("marmalade")!.node.parentElement!.closest(".aui-user-message-content")!;
      result["expanded"] = bubble.querySelector(":scope > button")?.getAttribute("aria-expanded");
      const anchor = document.createRange();
      anchor.selectNodeContents(textHolding("Line 1 of a long message")!.node);
      viewport().dispatchEvent(new WheelEvent("wheel", { deltaY: -1 }));
      viewport().scrollTop += anchor.getBoundingClientRect().top - viewport().getBoundingClientRect().top - 100;
      await settle(100);
      const beforeCollapse = anchor.getBoundingClientRect().top;
      press(field()!, "Escape");
      await until(() => bar() === null);
      await settle(100);
      result["collapseMoved"] = round(anchor.getBoundingClientRect().top - beforeCollapse);
      result["collapsed"] = bubble.querySelector(":scope > button")?.getAttribute("aria-expanded");
      press(document.body, "KeyF", { meta: true });
      await until(() => bar() !== null);
      await find("marmalade");
      bubble.querySelector<HTMLButtonElement>(":scope > button")!.click();
      await settle(100);
      bubble.querySelector<HTMLButtonElement>(":scope > button")!.click();
      await settle(100);
      press(field()!, "Escape");
      await until(() => bar() === null);
      await settle(100);
      result["manualExpanded"] = bubble.querySelector(":scope > button")?.getAttribute("aria-expanded");
      root.unmount();
      host.remove();
    } else if (FOLDS) {
      // A word only in a closed work fold: counted, its fold left closed while typing; Enter opens it and goes there.
      await select({ type: "conversation", id: "long" });
      await settle(LOAD_MS + 400);
      result["before"] = { wombat: headerOf(50), badger: headerOf(52), mounted: foldOf(50) };
      press(document.body, "KeyF", { meta: true });
      await until(() => bar() !== null);
      const typed = await find("wombat");
      const kept = blockOf(50)?.querySelector('[data-slot="request-fold"]');
      result["typed"] = { label: typed.label, size: typed.size, wombat: headerOf(50), badger: headerOf(52), kept: foldOf(50), height: kept?.getBoundingClientRect().height ?? null };
      press(field()!, "Enter");
      await until(() => headerOf(50) === "true");
      await settle(100);
      result["entered"] = { label: label(), active: activeMatch()?.toString() ?? null, place: activePlace(), wombat: headerOf(50), badger: headerOf(52) };
      // Closed: the fold gone to stays open, the other closed and holding nothing.
      press(field()!, "Escape");
      await until(() => bar() === null);
      await settle(100);
      result["closed"] = { wombat: headerOf(50), badger: headerOf(52), badgerFold: foldOf(52) };
      // Closed by hand: it animates down from its full height, then holds nothing.
      const fold = blockOf(50)!.querySelector('[data-slot="request-fold"]')!;
      const full = fold.getBoundingClientRect().height;
      blockOf(50)!.querySelector<HTMLButtonElement>('button[data-slot="request-work-header"]')!.click();
      await frame();
      const midway = fold.getBoundingClientRect().height;
      await settle(100);
      result["shut"] = { full: full > 0, midway: midway > 0, after: foldOf(50) };
    } else if (GROUPS) {
      // A word only in a closed group inside opened work: counted, the group left closed while typing; Enter opens it and goes there.
      await select({ type: "conversation", id: "long" });
      await settle(LOAD_MS + 400);
      blockOf(50)!.querySelector<HTMLButtonElement>('button[data-slot="request-work-header"]')!.click();
      await settle(100);
      result["before"] = { work: headerOf(50), group: groupOf(50), rows: groupRows(50) };
      press(document.body, "KeyF", { meta: true });
      await until(() => bar() !== null);
      const typed = await find("ocelot");
      const kept = blockOf(50)?.querySelector('[data-slot="work-group-kept"]');
      result["typed"] = { label: typed.label, active: typed.active, group: groupOf(50), height: kept?.getBoundingClientRect().height ?? null };
      press(field()!, "Enter");
      await until(() => groupOf(50) === "true" && activeMatch()?.toString() === "ocelot");
      await settle(100);
      result["entered"] = { label: label(), active: activeMatch()?.toString() ?? null, place: activePlace(), group: groupOf(50), work: headerOf(50) };
      press(field()!, "Escape");
      await until(() => bar() === null);
      await settle(100);
      result["closed"] = { group: groupOf(50), kept: blockOf(50)?.querySelector('[data-slot="work-group-kept"]') !== null };
    } else if (CAPPED) {
      // 240 loaded matches; the next older page adds 300, passing the cap inside that page.
      await select({ type: "conversation", id: "long" });
      await settle(LOAD_MS + 400);
      press(document.body, "KeyF", { meta: true });
      await until(() => bar() !== null);
      await settle(100);
      await find("pipit");
      await until(() => label()?.endsWith("/ 250+ results") === true);
      await settle(100);
      for (let step = 0; step < 250 && label() !== "11 / 250+ results"; step++) {
        const before = label();
        press(field()!, "Enter", { shift: true });
        await until(() => label() !== before);
      }
      await settle(100);
      const current = () => {
        const range = activeMatch();
        const prefix = document.createRange();
        if (range) {
          prefix.selectNodeContents(range.startContainer.parentElement!.closest('[data-role="assistant"]')!);
          prefix.setEnd(range.startContainer, range.startOffset);
        }
        return { label: label(), turn: activeTurn(), occurrence: prefix.toString().split("pipit").length, loads, size: matches()?.size };
      };
      result["before"] = current();
      press(field()!, "Enter", { shift: true });
      await until(() => activeTurn() === 20);
      await settle(100);
      result["back"] = current();
      await stepWith("Enter", { shift: true });
      result["previous"] = current();
      await stepWith("Enter");
      result["next"] = current();
    } else if (OLDER) {
      // Words in pages not loaded: counted with the thread's own without loading; a step to one loads down to it.
      await select({ type: "conversation", id: "long" });
      await settle(LOAD_MS + 400);
      press(document.body, "KeyF", { meta: true });
      await until(() => bar() !== null);
      const kiwi = await find("kiwi");
      await until(() => label() === "2 / 2 results");
      await settle(100);
      result["kiwi"] = { label: label(), active: activeMatch()?.toString() ?? null, turn: activeTurn(), loads };
      press(field()!, "Enter", { shift: true });
      await until(() => activeTurn() === 18);
      await settle(100);
      result["back"] = { first: kiwi.label, label: label(), active: activeMatch()?.toString() ?? null, turn: activeTurn(), place: activePlace(), loads };
      const quokka = await find("quokka");
      await until(() => label() === "1 / 1 result");
      await settle(100);
      result["quokka"] = { first: quokka.label, label: label(), active: activeMatch()?.toString() ?? null, shown: textHolding("quokka") !== null, loads };
      press(field()!, "Enter");
      await until(() => activeMatch()?.toString() === "quokka");
      await settle(100);
      result["entered"] = { label: label(), active: activeMatch()?.toString() ?? null, turn: activeTurn(), place: activePlace(), loads };
    } else if (LATE_EDITOR) {
      // ⌘F while the plain field stands in for the editor; the editor then loads and mounts.
      await select({ type: "conversation", id: "long" });
      await settle(LOAD_MS + 400);
      const editable = () => $(COMPOSER_EDITABLE);
      result["standIn"] = editable() === null && $('textarea[aria-label="Message input"]') !== null;
      const word = textHolding("solitary")!;
      const range = document.createRange();
      range.setStart(word.node, word.at);
      range.setEnd(word.node, word.at + "solitary".length);
      selectRange(range);
      press(document.body, "KeyF", { meta: true });
      await until(() => label() !== null);
      await settle(100);
      result["focusedBefore"] = document.activeElement === field();
      void revealApp();
      result["editorLoaded"] = await until(() => editable() !== null);
      await settle(100);
      result["focusedAfter"] = document.activeElement === field();
    } else if (SHORT || new URLSearchParams(location.search).get("drive") === "top") {
      await select({ type: "conversation", id: "long" });
      await settle(LOAD_MS + 400);
      $('[data-slot="earlier-turns"]')?.click();
      await settle(100);
      viewport().dispatchEvent(new WheelEvent("wheel", { deltaY: -1 }));
      viewport().scrollTop = 0;
      await settle(100);
      const anchorWord = SHORT ? "Question 1" : "Question 21";
      const anchorTop = () => {
        const anchor = document.createRange();
        anchor.selectNodeContents(textHolding(anchorWord)!.node);
        return anchor.getBoundingClientRect().top;
      };
      const beforeOpen = anchorTop();
      press(document.body, "KeyF", { meta: true });
      await until(() => bar() !== null);
      await settle(100);
      result["openingMoved"] = round(anchorTop() - beforeOpen);
      viewport().scrollTop = 0;
      await frame();
      const first = document.createTreeWalker(viewport().querySelector('[data-slot="aui_thread-column"]')!, NodeFilter.SHOW_TEXT).nextNode()!;
      const range = document.createRange();
      range.selectNodeContents(first);
      const beforeClose = range.getBoundingClientRect().top;
      result["clear"] = beforeClose >= bar()!.getBoundingClientRect().bottom;
      press(field()!, "Escape");
      await until(() => bar() === null);
      await settle(100);
      result["closingMoved"] = round(range.getBoundingClientRect().top - beforeClose);
    } else for (const [name, step] of Object.entries(steps)) {
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
