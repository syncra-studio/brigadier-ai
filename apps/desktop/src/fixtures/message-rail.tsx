/**
 * A standalone Vite entry, excluded from the app entry and production build: the rail of the
 * user's messages beside the thread (message-rail.tsx), ⌥↑ / ⌥↓, and older pages loading at the
 * top, in the app's own conversation view on synthetic conversations. Requests are answered here
 * and never reach a daemon.
 * - `long`: a chat of 80 turns. Opening it brings the newest 40 (the older 10 of them folded
 *   behind "N earlier messages"); older pages come 20 turns at a time, `LOAD_MS` late, and the
 *   next one fails while `failNext` is set.
 * - `short`: a chat of 3 turns.
 *
 * `?drive=1` runs the checks the rail test reads and writes them to `<pre id="message-rail-result">`;
 * `window.messageRail` holds the steps for a browser probe.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/ConversationView";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, ConversationView as View, EventEnvelope, Message, Request } from "@/ipc/generated";
import { select } from "@/state/actions";
import { applyToBoard, emptyBoard, useBoard } from "@/state/board";
import { applyEvents, useApp } from "@/state/store";

const LOAD_MS = 300;
const TURNS = 80;
/** Turns the first load brings, and each older page. */
const FIRST_TURNS = 40;
const PAGE_TURNS = 20;
/** Narrow enough that the room beside the column is under 48px. */
const NARROW_PX = 780;
const start = Date.now() - 86_400_000;

function conversation(id: string, title: string): Conversation {
  return {
    id, kind: "chat", projectId: null, title, pinnedAtMs: null, createdAtMs: start, updatedAtMs: start, lifecycle: "active",
    forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
    setup: { type: "chat", model: { provider: "claude", model: null, effort: null, fast: false } },
  } as unknown as Conversation;
}

function messages(id: string, turns: number): Message[] {
  const list: Message[] = [];
  for (let turn = 0; turn < turns; turn++) {
    const request = `${id}-r${turn}`;
    const at = start + turn * 60_000;
    list.push({
      id: `${id}-u${turn}`, conversationId: id, seq: list.length + 1, role: "user", text: `Question ${turn + 1}: where does this turn sit in the thread?`,
      blob: null, createdAtMs: at, attachments: [], mentions: [], model: null, requestId: request, parentId: list.at(-1)?.id ?? null,
    });
    const body = Array.from({ length: 1 + (turn % 3) }, (_, index) => `Answer ${turn + 1}, paragraph ${index + 1}. The rail marks each message the user sent, and a jump lands its turn at the top.`).join("\n\n");
    list.push({
      id: `${id}-a${turn}`, conversationId: id, seq: list.length + 1, role: "assistant", text: body, blob: null,
      createdAtMs: at + 30_000, attachments: [], mentions: [], model: null, requestId: request, parentId: list.at(-1)?.id ?? null,
    });
  }
  return list;
}

const conversations: Record<string, Conversation> = {
  long: conversation("long", "A long chat"),
  short: conversation("short", "A short chat"),
};
const stored: Record<string, Message[]> = { long: messages("long", TURNS), short: messages("short", 3) };

function view(id: string): View {
  const list = stored[id] ?? [];
  const first = Math.max(0, list.length - FIRST_TURNS * 2);
  return {
    conversation: conversations[id]!,
    context: null,
    messages: { messages: list.slice(first), hasMore: first > 0 },
    tasks: [], approvals: [], questions: [], plans: [], requests: [], workerSteps: [], orchestratorSteps: [],
    machineSteps: [], thinking: [], compactions: [], decisions: [], waiting: [],
    queue: { items: [], paused: false },
    run: "idle", runRequest: null, head: list.at(-1)?.id ?? null, ratings: {}, streaming: null,
    notices: [], memories: [], overnight: [], reviews: [], previews: [],
  } as unknown as View;
}

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const frame = () => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve)));
let pending = 0;
let failNext = false;
let pages = 0;
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
      return { method: req.method, view: view(req.id) };
    case "listMessages": {
      await wait(LOAD_MS);
      if (failNext) {
        failNext = false;
        throw new Error("The daemon could not read the conversation.");
      }
      pages++;
      const before = req.before ?? Number.POSITIVE_INFINITY;
      const older = (stored[req.conversationId] ?? []).filter((message) => message.seq < before);
      const page = older.slice(Math.max(0, older.length - PAGE_TURNS * 2));
      return { method: req.method, page: { messages: page, hasMore: page.length < older.length } };
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

useApp.setState({ conversations, connection: { status: "connected", daemon: null, reason: null }, pinnedSummary: false });

function Page() {
  const selection = useApp((s) => s.selection);
  useEffect(() => {
    if (new URLSearchParams(location.search).get("drive") === "1") void drive();
  }, []);
  return (
    <TooltipProvider>
      <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
        <main id="message-rail-main" className="relative flex min-h-0 w-full flex-1 flex-col">
          <ConversationView key={selection.type === "conversation" ? selection.id : selection.type} selection={selection} />
        </main>
      </SidebarProvider>
    </TooltipProvider>
  );
}

/* ----- the checks ----------------------------------------------------------------------- */

const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const viewport = () => $('[data-slot="aui_thread-viewport"]')!;
const rail = () => $('nav[aria-label="User messages"]');
const markers = () => [...document.querySelectorAll<HTMLElement>("[data-rail-marker]")];
const markerIds = () => markers().map((marker) => marker.dataset["railMarker"] ?? "");
const userRows = () => [...document.querySelectorAll<HTMLElement>('[data-slot="aui_message-group"] > [data-role="user"][data-message-id]')];
const row = (id: string) => userRows().find((element) => element.dataset["messageId"] === id) ?? null;
const round = (value: number) => Math.round(value * 100) / 100;
/** Where row `id`'s top sits under the view's top. */
const offsetOf = (id: string) => {
  const element = row(id);
  return element ? round(element.getBoundingClientRect().top - viewport().getBoundingClientRect().top) : null;
};
const landings = (id: string) => Number(row(id)?.dataset["railLanded"] ?? 0);

function layoutNow(): string {
  const box = $('[data-slot="aui_thread-viewport"]');
  return box ? `${box.scrollTop} ${box.scrollHeight} ${box.clientHeight} ${markers().length}` : "";
}

/** A finite, time-driven animation still running (not a list's scroll-driven edge fade, not a spinner). */
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
    } else if (++frames >= 3 && performance.now() - stillSince >= 100) return;
    last = now;
  }
}

/** Waits a frame at a time until `check` holds, up to 10s; whether it did. */
/**
 * Watches for `selector` from now on, however briefly it shows (a spinner while a page loads can
 * come and go between two frames on a runner that draws few): `seen` holds its first sighting.
 */
function watchFor(selector: string): { seen: () => Element | null; stop: () => void } {
  const sighting = () => (document.querySelector(selector)?.cloneNode(true) as Element | undefined) ?? null;
  let first = sighting();
  const observer = new MutationObserver(() => {
    first ??= sighting();
  });
  observer.observe(document.body, { childList: true, subtree: true });
  return { seen: () => first, stop: () => observer.disconnect() };
}

async function until(check: () => boolean): Promise<boolean> {
  const end = performance.now() + 10_000;
  while (!check()) {
    if (performance.now() > end) return false;
    await frame();
  }
  return true;
}

async function open(id: string): Promise<void> {
  select({ type: "conversation", id });
  await settle(LOAD_MS + 400);
}

/** ⌥ and an arrow, pressed on `target`. */
function altArrow(target: Element, key: "ArrowUp" | "ArrowDown"): void {
  target.dispatchEvent(new KeyboardEvent("keydown", { key, code: key, altKey: true, bubbles: true, cancelable: true }));
}

/** Presses ⌥ and an arrow in the thread and waits for row `id` to land. */
async function stepTo(key: "ArrowUp" | "ArrowDown", id: string) {
  const before = landings(id);
  viewport().focus();
  altArrow(viewport(), key);
  const landed = await until(() => landings(id) > before);
  await settle(50);
  return { landed, offset: offsetOf(id) };
}

/** Clicks the rail's marker for `id` and waits for its row to land. */
async function clickTo(id: string) {
  const before = landings(id);
  markers().find((marker) => marker.dataset["railMarker"] === id)!.click();
  const landed = await until(() => landings(id) > before);
  await settle(50);
  return { landed, offset: offsetOf(id) };
}

/** The rows in view, as the rail's markers should mark them: inside the view's edges by 16px. */
/** The user messages whose turns show: each from its message to the next (the last to the thread's end). */
function turnsInView(): string[] {
  const box = viewport().getBoundingClientRect();
  const rows = userRows();
  const end = $('[data-slot="aui_message-group"]')!.getBoundingClientRect().bottom;
  return rows
    .filter((element, index) => element.getBoundingClientRect().top < box.bottom - 16 && (rows[index + 1]?.getBoundingClientRect().top ?? end) > box.top + 16)
    .map((element) => element.dataset["messageId"] ?? "");
}
const currentMarkers = () => markers().filter((marker) => marker.getAttribute("aria-current") === "true").map((marker) => marker.dataset["railMarker"] ?? "");

/** A point inside `marker`, for a pointer event. */
const pointAt = (marker: HTMLElement) => ({ clientX: marker.getBoundingClientRect().left + 4, clientY: marker.getBoundingClientRect().top + 5 });
const lineWidth = (index: number) => round(markers()[index]!.querySelector<HTMLElement>("[data-slot=rail-line]")!.getBoundingClientRect().width);

const steps = {
  /** First open, at the bottom: the rail beside the column, its markers, the ones in view. */
  async opened() {
    await open("long");
    const nav = rail();
    const box = nav?.getBoundingClientRect();
    const shown = viewport().getBoundingClientRect();
    const column = $('[data-slot="aui_thread-column"]')!.getBoundingClientRect();
    const list = nav?.querySelector("ol");
    const first = markers()[0]?.getBoundingClientRect();
    const line = markers()[0]?.querySelector("[data-slot=rail-line]")?.getBoundingClientRect();
    return {
      rail: nav !== null,
      gutter: round(column.left - shown.left),
      left: box ? round(box.left - shown.left) : null,
      centreOff: box ? round(box.top + box.height / 2 - (shown.top + shown.height / 2)) : null,
      listMax: list ? getComputedStyle(list).maxHeight : null,
      windowHeight: innerHeight,
      listScrolls: list ? list.scrollHeight > list.clientHeight : null,
      faded: list?.classList.contains("scroll-edge-fade") ?? null,
      marker: first ? [round(first.width), round(first.height)] : null,
      line: line ? [round(line.width), round(line.height)] : null,
      markers: markerIds(),
      folded: $('[data-slot="earlier-turns"]')?.textContent ?? null,
      current: currentMarkers(),
      inView: turnsInView(),
      // A current marker's line and a resting one's.
      lines: [markers().at(-1), markers()[0]].map((marker) => (marker ? getComputedStyle(marker.querySelector("[data-slot=rail-line]")!).backgroundColor : null)),
    };
  },
  /** Hovering a marker: it and its neighbours grow, and the preview shows the message and its answer. */
  async hover() {
    const target = markers()[20]!;
    target.dispatchEvent(new PointerEvent("pointerover", { bubbles: true, pointerType: "mouse" }));
    target.dispatchEvent(new PointerEvent("pointerenter", { bubbles: false, pointerType: "mouse" }));
    await settle(250);
    const preview = $('[data-slot="rail-preview"]');
    const lines = [17, 18, 19, 20, 21, 22, 23, 24].map(lineWidth);
    const previewText = [...(preview?.querySelectorAll("p") ?? [])].map((p) => p.textContent ?? "");
    const previewBox = preview?.getBoundingClientRect();
    target.dispatchEvent(new PointerEvent("pointerout", { bubbles: true, pointerType: "mouse", relatedTarget: document.body }));
    rail()?.dispatchEvent(new PointerEvent("pointerleave", { bubbles: false, pointerType: "mouse", relatedTarget: document.body }));
    await settle(250);
    return {
      lines,
      previewText,
      previewWidth: previewBox ? round(previewBox.width) : null,
      previewRightOfRail: previewBox ? round(previewBox.left - rail()!.getBoundingClientRect().right) : null,
      afterLeave: { preview: $('[data-slot="rail-preview"]') !== null, line: lineWidth(20) },
    };
  },
  /** ⌥↑ from the bottom: the message above the view's top lands, and new content below no longer moves the view. */
  async keysFromBottom() {
    const top = viewport().getBoundingClientRect().top;
    const expected = userRows().findLast((element) => element.getBoundingClientRect().top < top)!.dataset["messageId"]!;
    const step = await stepTo("ArrowUp", expected);
    const scrollTop = viewport().scrollTop;
    const list = stored["long"]!;
    const message: Message = {
      id: "long-late", conversationId: "long", seq: list.length + 1, role: "assistant",
      text: "A late note under the newest answer, long enough to grow the thread below the view.\n\nIt has a second paragraph too.",
      blob: null, createdAtMs: Date.now(), attachments: [], mentions: [], model: null, requestId: `long-r${TURNS - 1}`, parentId: list.at(-1)?.id ?? null,
    };
    list.push(message);
    appended(message);
    await settle(200);
    return { expected, ...step, moved: viewport().scrollTop - scrollTop };
  },
  /** A click lands the turn at the top and flashes it; ⌥↓ and ⌥↑ step from there; in the composer they do nothing. */
  async clickAndKeys() {
    const ids = markerIds();
    const clicked = await clickTo(ids[15]!);
    const down = await stepTo("ArrowDown", ids[16]!);
    const up = await stepTo("ArrowUp", ids[15]!);
    const scrollTop = viewport().scrollTop;
    const input = $<HTMLTextAreaElement>('[aria-label="Message input"]')!;
    input.focus();
    altArrow(input, "ArrowUp");
    await settle(300);
    return { clicked, down, up, composerMoved: viewport().scrollTop - scrollTop, current: currentMarkers(), inView: turnsInView() };
  },
  /** A press-and-drag along the rail scrubs: each marker passed lands its turn at once, with no flash. */
  async scrub() {
    const ids = markerIds();
    const from = markers()[25]!;
    const to = markers()[28]!;
    const flashes = landings(ids[28]!);
    const pointer = { bubbles: true, pointerId: 1, pointerType: "mouse", button: 0, buttons: 1 };
    from.dispatchEvent(new PointerEvent("pointerdown", { ...pointer, ...pointAt(from) }));
    to.dispatchEvent(new PointerEvent("pointermove", { ...pointer, ...pointAt(to) }));
    // At once: no glide.
    const offset = offsetOf(ids[28]!);
    to.dispatchEvent(new PointerEvent("pointerup", { ...pointer, buttons: 0, ...pointAt(to) }));
    await settle(100);
    return { offset, flashed: landings(ids[28]!) > flashes };
  },
  /** Narrow, the rail goes; wide again, it comes back. */
  async narrow() {
    const main = $("#message-rail-main")!;
    main.style.width = `${NARROW_PX}px`;
    await settle(200);
    const narrow = { rail: rail() !== null, gutter: round($('[data-slot="aui_thread-column"]')!.getBoundingClientRect().left - viewport().getBoundingClientRect().left) };
    main.style.width = "";
    await settle(200);
    return { narrow, wide: rail() !== null };
  },
  /** ⌥↑ from the oldest turn shown: the folded turns open and the one before lands. */
  async intoFolded() {
    const ids = markerIds();
    const first = row(userRows()[0]!.dataset["messageId"]!)!.dataset["messageId"]!;
    const at = ids.indexOf(first);
    await clickTo(first);
    const step = await stepTo("ArrowUp", ids[at - 1]!);
    return { first, at, target: ids[at - 1], ...step, folded: $('[data-slot="earlier-turns"]') !== null, markers: markers().length };
  },
  /**
   * ⌥↑ from the oldest message loaded: the next older page loads (a spinner meanwhile) and its
   * newest message lands. Landing there puts the top in view, which loads the page by itself:
   * that load fails here, so the page ⌥↑ asks for is its own.
   */
  async intoUnloaded() {
    const oldest = markerIds()[0]!;
    failNext = true;
    await clickTo(oldest);
    const autoFailed = await until(() => $('[data-slot="earlier-failed"]') !== null);
    await settle(50);
    const pagesBefore = pages;
    viewport().focus();
    const watch = watchFor('[data-slot="earlier-loading"]');
    altArrow(viewport(), "ArrowUp");
    const spinner = await until(() => watch.seen() !== null);
    watch.stop();
    const loading = watch.seen();
    const spinnerSeen = loading
      ? { role: loading.getAttribute("role"), text: loading.textContent, classes: loading.className, svg: loading.querySelector("svg") !== null }
      : null;
    const target = `long-u${Number(oldest.replace("long-u", "")) - 1}`;
    const landed = await until(() => landings(target) > 0);
    await settle(100);
    return { oldest, target, autoFailed, spinner, spinnerSeen, landed, offset: offsetOf(target), pages: pages - pagesBefore, markers: markers().length };
  },
  /** Scrolled to the top, the next page loads by itself; when it fails, Try again loads it, the view staying put. */
  async atTop() {
    failNext = true;
    const pagesBefore = pages;
    const watch = watchFor('[data-slot="earlier-loading"]');
    viewport().dispatchEvent(new WheelEvent("wheel", { deltaY: -1 }));
    viewport().scrollTop = 0;
    const spinner = await until(() => watch.seen() !== null);
    watch.stop();
    const failed = await until(() => $('[data-slot="earlier-failed"]') !== null);
    await settle(100);
    const failure = { text: $('[data-slot="earlier-failed"]')?.textContent ?? null, pages: pages - pagesBefore };
    const anchor = userRows()[0]!.dataset["messageId"]!;
    const before = offsetOf(anchor);
    const retry = [...document.querySelectorAll<HTMLButtonElement>('[data-slot="earlier-failed"] button')].find((button) => button.textContent === "Try again")!;
    retry.click();
    const loaded = await until(() => userRows()[0]!.dataset["messageId"] !== anchor);
    await settle(200);
    return {
      spinner, failed, failure, anchor, before, after: offsetOf(anchor), loaded, pages: pages - pagesBefore,
      first: userRows()[0]!.dataset["messageId"], more: $('[data-slot="earlier-load"], [data-slot="earlier-loading"], [data-slot="earlier-failed"]') !== null,
      markers: markers().length,
      listScrolls: (() => {
        const list = rail()?.querySelector("ol");
        return list ? list.scrollHeight > list.clientHeight : null;
      })(),
      faded: rail()?.querySelector("ol")?.classList.contains("scroll-edge-fade") ?? null,
    };
  },
  /** Three messages: no rail. */
  /** A new turn whose answer is taller than the view, scrolled to its end: its message is the current one. */
  async longAnswer() {
    const list = stored["long"]!;
    const at = Date.now();
    const asked: Message = {
      id: "long-tall-u", conversationId: "long", seq: list.length + 1, role: "user", text: "Explain it all at length.",
      blob: null, createdAtMs: at, attachments: [], mentions: [], model: null, requestId: "long-tall", parentId: list.at(-1)?.id ?? null,
    };
    const said: Message = {
      id: "long-tall-a", conversationId: "long", seq: list.length + 2, role: "assistant",
      text: Array.from({ length: 30 }, (_, index) => `Paragraph ${index + 1} of a long answer, so the message it answers scrolls away above the view.`).join("\n\n"),
      blob: null, createdAtMs: at + 1000, attachments: [], mentions: [], model: null, requestId: "long-tall", parentId: asked.id,
    };
    for (const message of [asked, said]) {
      list.push(message);
      appended(message);
    }
    await until(() => !!$('[data-message-id="long-tall-u"]') && markerIds().includes("long-tall-u"));
    await settle(200);
    viewport().dispatchEvent(new WheelEvent("wheel", { deltaY: 1 }));
    viewport().scrollTop = viewport().scrollHeight;
    await settle(300);
    const shown = viewport().getBoundingClientRect();
    return {
      messagesShown: userRows().filter((element) => {
        const rect = element.getBoundingClientRect();
        return rect.bottom > shown.top && rect.top < shown.bottom;
      }).length,
      current: currentMarkers(),
      inView: turnsInView(),
    };
  },
  async short() {
    await open("short");
    return { rail: rail() !== null, rows: userRows().length };
  },
};

async function drive(): Promise<void> {
  const result: Record<string, unknown> = {};
  try {
    for (const [name, step] of Object.entries(steps)) {
      document.body.dataset["railStep"] = name;
      result[name] = await step();
    }
  } catch (error) {
    result["error"] = String(error instanceof Error ? error.stack : error);
  }
  const pre = document.createElement("pre");
  pre.id = "message-rail-result";
  pre.textContent = JSON.stringify(result);
  document.body.append(pre);
}

Object.assign(window, { messageRail: { steps, open, settle } });
useBoard.setState({ board: emptyBoard("long") });
createRoot(document.getElementById("root")!).render(<Page />);
