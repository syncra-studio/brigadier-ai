/**
 * A standalone Vite entry, excluded from the app entry and production build: the thread's
 * scrolling (thread-scroll.ts) in the app's own conversation view, on synthetic conversations.
 * Requests are answered here and never reach a daemon; each conversation's history comes back
 * `LOAD_MS` late, and a long reply's full text later still, like a daemon's.
 * - `long`: a session of 36 turns (the older ones behind "N earlier messages"), each answer a
 *   report with its Details folded, one long user message clipped behind "Show more".
 * - `other`: a Chat of 8 turns. `working`: a Chat whose one turn still runs, never opened.
 *
 * `?drive=1` runs the checks the scroll tests read and writes them to
 * `<pre id="thread-scroll-result">` (`&only=<step>` runs one of them); `window.threadScroll`
 * holds the steps for a browser probe.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/ConversationView";
import planned from "@/fixtures/boards/thread-plan-2026-10-10.events.json";
import { roomFor } from "@/components/assistant-ui/thread-scroll";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, ConversationView as View, EventEnvelope, Message, Request, UserRequest } from "@/ipc/generated";
import { select, send } from "@/state/actions";
import { applyToBoard, emptyBoard, useBoard } from "@/state/board";
import { applyEvents, emptyThread, useApp } from "@/state/store";

const LOAD_MS = 250;
/** How far the window shrinks in the resize checks. */
const SHRINK_PX = 120;
const BLOB_MS = 400;
const start = Date.now() - 86_400_000;

const paragraph = (turn: number, index: number) =>
  `Paragraph ${index + 1} of answer ${turn + 1}. The thread keeps its place while the rows around it change size, and it follows new content only from the bottom, so a reply streaming in below what the user reads never moves the view.`;

function conversation(id: string, kind: "chat" | "session", title: string): Conversation {
  return {
    id,
    kind,
    projectId: null,
    title,
    pinnedAtMs: null,
    createdAtMs: start,
    updatedAtMs: start,
    lifecycle: "active",
    forkedFrom: null,
    sideOf: null,
    fallback: null,
    quotaWait: null,
    setup:
      kind === "chat"
        ? { type: "chat", model: { provider: "claude", model: null, effort: null, fast: false } }
        : {
            type: "session",
            repo: "/tmp/thread-scroll-fixture",
            environment: { type: "newWorktree", branch: "brigadier/scroll/session", base: "main" },
            permission: "fullAccess",
            planMode: false,
            workersSeeUncommitted: null,
            orchestrator: { provider: "claude", model: "opus", effort: "high" },
          },
  } as unknown as Conversation;
}

function messages(id: string, turns: number, report: boolean): Message[] {
  const list: Message[] = [];
  for (let turn = 0; turn < turns; turn++) {
    const request = `${id}-r${turn}`;
    const at = start + turn * 60_000;
    const asked =
      turn === turns - 3
        ? Array.from({ length: 24 }, (_, line) => `Line ${line + 1} of a long message the user pasted.`).join("\n")
        : `Question ${turn + 1}: how does the thread scroll here?`;
    list.push({
      id: `${id}-u${turn}`, conversationId: id, seq: list.length + 1, role: "user", text: asked, blob: null,
      createdAtMs: at, attachments: [], mentions: [], model: null, requestId: request, parentId: list.at(-1)?.id ?? null,
    });
    const body = Array.from({ length: 2 + (turn % 4) }, (_, index) => paragraph(turn, index)).join("\n\n");
    const details = report ? `\n\n### Details\n\n${Array.from({ length: 6 }, (_, index) => `- Check ${index + 1} passed.`).join("\n")}` : "";
    // Every fifth answer is too long to keep inline: its full text loads later.
    const blob = turn % 5 === 4;
    list.push({
      id: `${id}-a${turn}`, conversationId: id, seq: list.length + 1, role: "assistant",
      text: blob ? paragraph(turn, 0) : body + details, blob: blob ? `${id}-blob-${turn}` : null,
      createdAtMs: at + 30_000, attachments: [], mentions: [], model: null, requestId: request, parentId: list.at(-1)?.id ?? null,
    });
  }
  return list;
}

// The plan session of 2026-10-10 while phase 1 builds: its capsule shows over the composer.
const planId = planned.conversationId;
let planBoard = emptyBoard(planId);
const planMessages: Message[] = [];
for (const envelope of (planned.events as unknown as EventEnvelope[]).filter((event) => event.atMs <= 1791603900000)) {
  planBoard = applyToBoard(planBoard, envelope);
  if (envelope.event.type === "messageAppended") planMessages.push({ ...envelope.event.message, seq: envelope.streamSeq });
}

const conversations: Record<string, Conversation> = {
  long: conversation("long", "session", "A long session"),
  other: conversation("other", "chat", "Another chat"),
  working: conversation("working", "chat", "A chat still answering"),
  [planId]: conversation(planId, "session", "Times and lang flags"),
};
const stored: Record<string, Message[]> = {
  long: messages("long", 36, true),
  other: messages("other", 8, false),
  working: messages("working", 1, false).slice(0, 1),
  [planId]: planMessages,
};
const blobs = new Map<string, string>();
for (const list of Object.values(stored)) {
  for (const message of list) {
    if (message.blob) blobs.set(message.blob, Array.from({ length: 8 }, (_, index) => paragraph(message.seq, index)).join("\n\n"));
  }
}

/** A report's request changed code, so its answer keeps its Details (Q79). */
function requests(id: string): UserRequest[] {
  if (id !== "long") return [];
  return (stored[id] ?? []).filter((message) => message.role === "user").map((message) => ({
    id: message.requestId!, conversationId: id, preview: "", state: { type: "done" }, startedAtMs: message.createdAtMs,
    endedAtMs: message.createdAtMs + 30_000, steeredInto: null, steeredAfter: null, undo: null,
    worked: [{ fromMs: message.createdAtMs, toMs: message.createdAtMs + 30_000 }], quotaWait: false,
    changes: { stat: { files: [{ path: "src/scroll.ts", insertions: 1, deletions: 0, binary: false }], insertions: 1, deletions: 0 }, settled: true },
  }) as UserRequest);
}

function view(id: string): View {
  const running = id === "working";
  return {
    conversation: conversations[id]!,
    context: null,
    messages: { messages: stored[id] ?? [], hasMore: false },
    tasks: [], approvals: [], questions: [], plans: [], requests: requests(id), workerSteps: [], orchestratorSteps: [],
    machineSteps: [], thinking: [], compactions: [], decisions: [], waiting: [],
    queue: { items: [], paused: false },
    run: running ? "running" : "idle",
    runRequest: running ? "working-r0" : null,
    head: stored[id]?.at(-1)?.id ?? null,
    ratings: {},
    streaming: running ? { messageId: "working-answer", text: paragraph(0, 0), requestId: "working-r0" } : null,
    notices: [], memories: [], overnight: [], reviews: [], previews: [],
  } as unknown as View;
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

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
/** After the next frame is laid out (and its resize observers ran). */
const frame = () => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve)));
let sequence = 1000;
/** Requests not answered yet: the page settles only once none is left. */
let pending = 0;

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
    case "readBlobText":
      await wait(BLOB_MS);
      return { method: req.method, text: blobs.get(req.hash) ?? "" };
    case "createConversation": {
      const id = `new-${sequence++}`;
      conversations[id] = conversation(id, "chat", "A new chat");
      stored[id] = [];
      return { method: req.method, conversation: conversations[id] };
    }
    case "sendMessage": {
      await wait(100);
      const list = (stored[req.conversationId] ??= []);
      const message: Message = {
        id: `sent-${sequence++}`, conversationId: req.conversationId, seq: list.length + 1, role: "user",
        text: req.text, blob: null, createdAtMs: Date.now(), attachments: [], mentions: [], model: null,
        requestId: `sent-request-${sequence}`, parentId: list.at(-1)?.id ?? null,
      };
      list.push(message);
      appended(message);
      return { method: req.method, outcome: { type: "sent", message } };
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

useApp.setState({
  conversations,
  connection: { status: "connected", daemon: null, reason: null },
  pinnedSummary: false,
});

/** The app's main column (App.tsx): one conversation view, a new one for each conversation. */
function Page() {
  const selection = useApp((s) => s.selection);
  useEffect(() => {
    if (new URLSearchParams(location.search).get("drive") === "1") void drive();
  }, []);
  return (
    <TooltipProvider>
      <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
        <main id="thread-scroll-main" className="relative flex min-h-0 w-full flex-1 flex-col">
          <ConversationView
            key={selection.type === "conversation" ? selection.id : selection.type}
            selection={selection}
          />
        </main>
      </SidebarProvider>
    </TooltipProvider>
  );
}

/* ----- the checks ----------------------------------------------------------------------- */

const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const viewport = () => $('[data-slot="aui_thread-viewport"]')!;
const rows = () => [...$('[data-slot="aui_message-group"]')!.children] as HTMLElement[];
const max = () => viewport().scrollHeight - viewport().clientHeight;
const distance = () => Math.round(max() - viewport().scrollTop);
const viewTop = () => viewport().getBoundingClientRect().top;
/** The floating footer's top: the composer, and whatever sits above it. */
const stackTop = () => $('[data-slot="aui_thread-footer-column"]')!.getBoundingClientRect().top;
const button = () => $<HTMLButtonElement>(".aui-thread-scroll-to-bottom")!;
const round = (value: number) => Math.round(value * 100) / 100;

/** What the checks measure, as one string: it holds still once the page has settled. */
function layoutNow(): string {
  const box = $('[data-slot="aui_thread-viewport"]');
  const footer = $('[data-slot="aui_thread-footer-column"]')?.getBoundingClientRect().top;
  return box ? `${box.scrollTop} ${box.scrollHeight} ${box.clientHeight} ${footer}` : "";
}

/** A fold opening, a fade: a finite animation or transition still running. */
const animating = () =>
  document.getAnimations().some((animation) => animation.playState === "running" && animation.effect?.getTiming().iterations !== Infinity);

/**
 * Waits `ms`, then until every request is answered, no reply waits on its renderer, nothing
 * animates and the layout holds for 100ms of frames. A slower machine (a CI runner) loads and
 * lays out later than the time a step allows; the step then waits longer, never less.
 */
async function settle(ms: number): Promise<void> {
  await wait(ms);
  const until = performance.now() + 10_000;
  let last = layoutNow();
  let stillSince = performance.now();
  for (let frames = 0; performance.now() < until; ) {
    await frame();
    const now = layoutNow();
    if (pending > 0 || $('[data-slot="reply-loading"]') || animating() || now !== last) {
      stillSince = performance.now();
      frames = 0;
    } else if (++frames >= 3 && performance.now() - stillSince >= 100) return;
    last = now;
  }
}

/** A scroll by the user (a wheel, a key, the scrollbar): the page sets it itself. */
async function userScroll(top: number): Promise<void> {
  viewport().dispatchEvent(new WheelEvent("wheel", { deltaY: top < viewport().scrollTop ? -1 : 1 }));
  viewport().scrollTop = top;
  await settle(50);
}

async function open(id: string): Promise<void> {
  select({ type: "conversation", id });
  await settle(LOAD_MS + BLOB_MS + 400);
}

/** The newest row's bottom to the floating footer's top. */
const lastGap = () => round(stackTop() - rows().at(-1)!.getBoundingClientRect().bottom);

/** The gap kept between the content's end and the floating footer. */
const COMPOSER_GAP = 32;

/** How far down the view a sent turn's end settles: the view's height above the gap, less the room. */
function turnLine(): number {
  const visible = stackTop() - COMPOSER_GAP - viewTop();
  return round(visible - roomFor(visible));
}

/** The share of the way still to go at the last frame before a time, and at the first frame from it on. */
type Around = { before: number; after: number };

/**
 * Samples scrollTop each frame until the returned call: how the view moved, as the share of the
 * way still to go around 100ms and 200ms after it began, how long it took to come within a
 * pixel, and whether it ever went past where it ended. Each place is timed by the frame that
 * put it there: a frame's callbacks run in the order they were asked for, so this one, asked for
 * before the move's, reads where the frame before left the view. A slow machine's frames can
 * come far apart, so a time is read as the frames on either side of it.
 */
function trace(): () => { moved: number; at100: Around; at200: Around; settledMs: number; overshoot: boolean } {
  const samples: { t: number; top: number }[] = [];
  let on = true;
  let previous: number | null = null;
  const tick = (now: number) => {
    if (previous !== null) samples.push({ t: previous, top: viewport().scrollTop });
    previous = now;
    if (on) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
  return () => {
    on = false;
    const end = samples.at(-1)!.top;
    const at = samples.findIndex((sample, index) => index > 0 && sample.top !== samples[index - 1]!.top);
    const from = samples[Math.max(0, at - 1)]!;
    const span = end - from.top;
    const left = (sample: { top: number }) => round((end - sample.top) / span);
    const around = (ms: number): Around => ({
      before: left(samples.findLast((sample) => sample.t - from.t < ms) ?? from),
      after: left(samples.find((sample) => sample.t - from.t >= ms) ?? samples.at(-1)!),
    });
    const settled = samples.find((sample) => sample.t > from.t && Math.abs(end - sample.top) < 1)!;
    return {
      moved: round(span),
      at100: around(100),
      at200: around(200),
      settledMs: Math.round(settled.t - from.t),
      overshoot: samples.some((sample) => (sample.top - end) * Math.sign(span) > 0.5),
    };
  };
}

function buttonState() {
  const box = button().getBoundingClientRect();
  const column = $('[data-slot="aui_thread-footer-column"]')!.getBoundingClientRect();
  const style = getComputedStyle(button());
  return {
    shown: style.opacity === "1" && style.pointerEvents !== "none",
    tabbable: button().tabIndex >= 0,
    size: [round(box.width), round(box.height)],
    aboveStack: round(stackTop() - box.bottom),
    offCentre: round(box.left + box.width / 2 - (column.left + column.width / 2)),
    dots: button().querySelector(".wave-dot") !== null,
  };
}

/** A toggle's top on screen, before and after its fold opens or closes. */
async function toggleStays(toggle: HTMLElement): Promise<{ before: number; after: number; distance: number }> {
  const before = toggle.getBoundingClientRect().top;
  toggle.click();
  await settle(450);
  return { before: round(before), after: round(toggle.getBoundingClientRect().top), distance: distance() };
}

/**
 * Streams `words` more words of an answer, noting the view's scrollTop and distance each time
 * the answer's growth is laid out, until the answer stops growing. The reply reveals its text on
 * its own timers, so a reading between frames can see text the thread hasn't laid out yet: this
 * observer, made after the thread's own, reads each frame right after the thread handled it.
 */
async function stream(messageId: string, requestId: string, words: number): Promise<{ tops: number[]; distances: number[] }> {
  const tops: number[] = [];
  const distances: number[] = [];
  const laidOut = new ResizeObserver(() => {
    tops.push(Math.round(viewport().scrollTop));
    distances.push(distance());
  });
  laidOut.observe($('[data-slot="aui_message-group"]')!);
  const streaming = useBoard.getState().board?.streaming;
  let text = streaming?.messageId === messageId ? streaming.text : "";
  for (let index = 0; index < words; index++) {
    text += `${index % 12 === 11 ? "\n\n" : " "}word${index}`;
    useBoard.setState(({ board }) =>
      board ? { board: { ...board, run: "running", runRequest: requestId, streaming: { messageId, text, requestId, startedAtMs: Date.now() } } } : {},
    );
    await wait(20);
    await frame();
  }
  await settle(0);
  laidOut.disconnect();
  return { tops, distances };
}

function stopStreaming(conversationId: string, messageId: string, text: string, requestId: string): void {
  const list = (stored[conversationId] ??= []);
  const message: Message = {
    id: messageId, conversationId, seq: list.length + 1, role: "assistant", text, blob: null,
    createdAtMs: Date.now(), attachments: [], mentions: [], model: null, requestId, parentId: list.at(-1)?.id ?? null,
  };
  list.push(message);
  appended(message);
  useBoard.setState(({ board }) => (board ? { board: { ...board, run: "idle", runRequest: null, streaming: null } } : {}));
}

/** The steps, each returning what it measured. */
const steps = {
  /** First open: the bottom, and the gap from the last content to the composer. */
  async firstOpen() {
    await open("long");
    const fade = (selector: string) => round($(selector)!.getBoundingClientRect().height);
    const footer = $("[data-thread-scroll-footer]")!.getBoundingClientRect().height;
    return {
      distance: distance(),
      lastGap: lastGap(),
      button: buttonState(),
      // How far the fades reach: under the top bar, and above the floating footer.
      topFade: fade(".thread-top-fade"),
      // Over the view, not in what scrolls: an overscroll leaves it at the view's top.
      topFadeScrolls: viewport().contains($(".thread-top-fade")),
      topFadeAt: round($(".thread-top-fade")!.getBoundingClientRect().top - viewTop()),
      bottomFade: round(fade(".thread-bottom-fade") - footer),
      // How far the scroll button reaches above the fade's top, over the text.
      buttonOverText: round($(".thread-bottom-fade")!.getBoundingClientRect().top - $(".aui-thread-scroll-to-bottom")!.getBoundingClientRect().top),
    };
  },
  /** The button's threshold and place: the last row 7px (within the 8px allowed, clear of sub-pixel rounding), then 9px, under the composer's top. */
  async button() {
    await userScroll(max() - COMPOSER_GAP - 7);
    await settle(250);
    const at8 = buttonState();
    await userScroll(max() - COMPOSER_GAP - 9);
    await settle(250);
    const at9 = buttonState();
    return { at8, at9 };
  },
  /** Mid-scroll, a session switch and back: the same place, with history and long text loading late. */
  async switchBack() {
    await userScroll(Math.round(max() / 2));
    await settle(100);
    const top = viewport().scrollTop;
    const anchor = rows().find((row) => row.getBoundingClientRect().bottom > viewTop())!;
    const id = anchor.dataset["messageId"];
    const offset = round(anchor.getBoundingClientRect().top - viewTop());
    await open("other");
    // Gone from memory: it loads again, its long replies' full text after it.
    useApp.setState((state) => ({ threads: { ...state.threads, long: emptyThread } }));
    select({ type: "conversation", id: "long" });
    const early: number[] = [];
    // While its history loads the column is empty: a screen reader's "Loading conversation", no placeholder lines.
    let loading = false;
    let placeholders = 0;
    for (let at = 0; at < 12; at++) {
      await wait(100);
      early.push(Math.round(viewport().scrollTop));
      if (rows().length === 0) {
        loading ||= $('[data-slot="aui_thread-history-loading"]')?.textContent === "Loading conversation";
        placeholders += document.querySelectorAll('[data-slot="aui_thread-column"] [data-slot="skeleton"]').length;
      }
    }
    // A slower machine loads it later: the last sample is once it has.
    await settle(0);
    early.push(Math.round(viewport().scrollTop));
    const back = rows().find((row) => row.dataset["messageId"] === id);
    return { top: Math.round(top), restored: Math.round(viewport().scrollTop), offset, restoredOffset: back ? round(back.getBoundingClientRect().top - viewTop()) : null, early, loading, placeholders };
  },
  /** Earlier turns shown, then a switch and back: the same older row at the same place. */
  async revealedBack() {
    $<HTMLButtonElement>('[data-slot="earlier-turns"]')?.click();
    await settle(200);
    const older = rows().find((row) => row.dataset["messageId"] === "long-u2")!;
    await userScroll(viewport().scrollTop + older.getBoundingClientRect().top - viewTop() - 40);
    const offset = round(older.getBoundingClientRect().top - viewTop());
    await open("other");
    await open("long");
    const back = rows().find((row) => row.dataset["messageId"] === "long-u2");
    return { offset, restoredOffset: back ? round(back.getBoundingClientRect().top - viewTop()) : null };
  },
  /** A side panel in fullscreen hides the thread (`display: none`) and back: the same place, mid-scroll and at the bottom. */
  async fullscreen() {
    // The conversation's column, which ConversationView hides under a fullscreen panel.
    const column = $('[data-slot="pane-workspace"] > div > div')!;
    const hidden = async () => {
      column.classList.add("hidden");
      await settle(200);
      const height = viewport().clientHeight;
      column.classList.remove("hidden");
      await settle(200);
      return height;
    };
    await userScroll(Math.round(max() / 3));
    await settle(100);
    const top = Math.round(viewport().scrollTop);
    const anchor = rows().find((row) => row.getBoundingClientRect().bottom > viewTop())!;
    const offset = round(anchor.getBoundingClientRect().top - viewTop());
    const hiddenHeight = await hidden();
    const middle = { top, restored: Math.round(viewport().scrollTop), offset, restoredOffset: round(anchor.getBoundingClientRect().top - viewTop()), button: buttonState().shown };
    await userScroll(max());
    await settle(100);
    await hidden();
    return { hiddenHeight, middle, bottom: distance() };
  },
  /** A report's Details, a disclosure, opened and closed mid-scroll and at the bottom. */
  async expand() {
    // Mid-thread, away from the bottom (within 24px of it the bottom would stay instead).
    await userScroll(Math.round(max() / 2));
    const summaries = [...document.querySelectorAll<HTMLElement>('[data-slot="report-details"] > summary')];
    const middle = summaries.find((summary) => {
      const top = summary.getBoundingClientRect().top - viewTop();
      return top > 200 && top < viewport().clientHeight - 300;
    }) ?? summaries[Math.floor(summaries.length / 2)]!;
    await userScroll(viewport().scrollTop + middle.getBoundingClientRect().top - viewTop() - 300);
    const midOpen = await toggleStays(middle);
    const midClose = await toggleStays(middle);
    const showMore = [...document.querySelectorAll<HTMLButtonElement>("button")].find((element) => element.textContent === "Show more")!;
    await userScroll(viewport().scrollTop + showMore.getBoundingClientRect().top - viewTop() - 300);
    const userOpen = await toggleStays(showMore);
    // At the bottom, following: the bottom stays, what opens shows above the composer.
    await userScroll(max());
    const last = summaries.at(-1)!;
    const bottomOpen = await toggleStays(last);
    const gapOpen = lastGap();
    const buttonAfter = buttonState();
    const bottomClose = await toggleStays(last);
    return { midOpen, midClose, userOpen, bottomOpen, gapOpen, buttonAfter, bottomClose };
  },
  /** A notice above the composer: the view doesn't move; at the bottom the gaps are kept. */
  async composerGrows() {
    await userScroll(max());
    await settle(100);
    const before = { top: Math.round(viewport().scrollTop), stackTop: round(stackTop()) };
    const notice = { level: "warning", text: "A notice above the composer, two lines long to make the footer taller than one row would.\nIts second line.", atMs: Date.now() };
    useBoard.setState(({ board }) => (board ? { board: { ...board, notices: [notice as never] } } : {}));
    await settle(300);
    const grown = { top: Math.round(viewport().scrollTop), stackTop: round(stackTop()), distance: distance(), lastGap: lastGap(), button: buttonState() };
    // Scrolled up, the view stays still while it shrinks and grows again.
    await userScroll(max() - 300);
    const above = Math.round(viewport().scrollTop);
    useBoard.setState(({ board }) => (board ? { board: { ...board, notices: [] } } : {}));
    await settle(300);
    useBoard.setState(({ board }) => (board ? { board: { ...board, notices: [notice as never] } } : {}));
    await settle(300);
    const scrolled = { before: above, after: Math.round(viewport().scrollTop) };
    await userScroll(max());
    await settle(250);
    const atBottom = { lastGap: lastGap(), button: buttonState() };
    useBoard.setState(({ board }) => (board ? { board: { ...board, notices: [] } } : {}));
    await settle(300);
    const shrunk = { distance: distance(), lastGap: lastGap() };
    return { before, grown, scrolled, atBottom, shrunk };
  },
  /**
   * A send from the bottom of a conversation: the spring to its place, its answer filling the
   * room and then followed; scrolled away it isn't, until the user is back at the bottom.
   */
  async send() {
    await open("other");
    await userScroll(0);
    const topPadding = round(rows()[0]!.getBoundingClientRect().top - viewTop());
    await userScroll(max());
    const moving = trace();
    await send({ text: "A new question, sent from the bottom.", attachments: [], mentions: [] });
    await settle(600);
    const spring = moving();
    const user = rows().findLast((row) => row.dataset["role"] === "user")!;
    const placed = {
      turnEnd: round(rows().at(-1)!.getBoundingClientRect().bottom - viewTop()),
      line: turnLine(),
      distance: distance(),
      room: round($('[data-slot="aui_thread-spacer"]')!.getBoundingClientRect().height),
      userTop: round(user.getBoundingClientRect().top - viewTop()),
      viewHeight: viewport().clientHeight,
    };
    const requestId = stored["other"]!.at(-1)!.requestId!;
    const top = Math.round(viewport().scrollTop);
    const streamed = await stream("other-streaming", requestId, 260);
    // It filled the room without moving, then was followed: the view only ever went down.
    const overflowed = {
      top,
      grew: streamed.tops.at(-1)! - top,
      distances: [...new Set(streamed.distances)],
      lastDistance: streamed.distances.at(-1)!,
      lastGap: lastGap(),
      button: buttonState(),
      room: round($('[data-slot="aui_thread-spacer"]')!.getBoundingClientRect().height),
    };
    await userScroll(max() - 200);
    const awayTop = Math.round(viewport().scrollTop);
    const away = await stream("other-streaming", requestId, 120);
    const unfollowed = { top: awayTop, tops: [...new Set(away.tops)], distance: distance(), button: buttonState() };
    await userScroll(max());
    const followed = await stream("other-streaming", requestId, 120);
    // At the bottom, a resize of the window keeps it there.
    $("#thread-scroll-main")!.style.paddingBottom = `${SHRINK_PX}px`;
    await settle(200);
    const shrunkWindow = distance();
    $("#thread-scroll-main")!.style.paddingBottom = "";
    await settle(200);
    const grownWindow = distance();
    stopStreaming("other", "other-answer", "Done.", requestId);
    await settle(200);
    return {
      topPadding,
      spring,
      placed,
      overflowed,
      unfollowed,
      followed: { distances: [...new Set(followed.distances)], grew: followed.tops.at(-1)! - followed.tops[0]! },
      shrunkWindow,
      grownWindow,
    };
  },
  /** At the bottom with the sent turn's room (not following), a resize keeps it at the bottom. */
  async resizeAtBottom() {
    await send({ text: "Another question.", attachments: [], mentions: [] });
    await settle(600);
    const placed = distance();
    $("#thread-scroll-main")!.style.paddingBottom = `${SHRINK_PX * 2}px`;
    await settle(200);
    const shrunk = distance();
    $("#thread-scroll-main")!.style.paddingBottom = "";
    await settle(200);
    return { placed, shrunk, grown: distance() };
  },
  /** A send, then away to another conversation while it answers and back: at the bottom, without the room. */
  async returnAfterSend() {
    await open("other");
    await userScroll(max());
    await send({ text: "A question left while it answers.", attachments: [], mentions: [] });
    await settle(600);
    const room = round($('[data-slot="aui_thread-spacer"]')!.getBoundingClientRect().height);
    await open("long");
    await open("other");
    return {
      room,
      distance: distance(),
      roomBack: round($('[data-slot="aui_thread-spacer"]')!.getBoundingClientRect().height),
      lastGap: lastGap(),
      button: buttonState().shown,
    };
  },
  /** Scrolled up, a send leaves the view where it is: the turn is added below. */
  async sendScrolledUp() {
    await open("other");
    await userScroll(Math.round(max() / 2));
    await settle(100);
    const top = Math.round(viewport().scrollTop);
    await send({ text: "A question sent while reading above.", attachments: [], mentions: [] });
    await settle(600);
    return { top, after: Math.round(viewport().scrollTop), button: buttonState().shown };
  },
  /** The first message of a new chat, sent from the new-chat view. */
  async newChat() {
    select({ type: "draft", kind: "chat" });
    await settle(200);
    await send({ text: "The first message of a new chat.", attachments: [], mentions: [] }, { kind: "chat", setup: null });
    await settle(LOAD_MS + 600);
    const user = rows().find((row) => row.dataset["role"] === "user");
    return { userTop: user ? round(user.getBoundingClientRect().top - viewTop()) : null, top: viewport().scrollTop };
  },
  /** With the capsule above the composer: the gaps are to the capsule's top. */
  async capsule() {
    // A shorter window, so the plan's thread runs past the view and its bottom is a real one.
    $("#thread-scroll-main")!.style.paddingBottom = `${SHRINK_PX}px`;
    await open(planId);
    useBoard.setState({ board: { ...planBoard, loaded: true, head: planMessages.at(-1)?.id ?? null } });
    await settle(400);
    await userScroll(max());
    await settle(250);
    const capsule = $('[data-slot="composer-capsule"]');
    const atBottom = { capsule: capsule !== null, capsuleInStack: capsule ? round(capsule.getBoundingClientRect().top - stackTop()) : null, lastGap: lastGap(), distance: distance() };
    await userScroll(max() - 200);
    await settle(250);
    const above = buttonState();
    $("#thread-scroll-main")!.style.paddingBottom = "";
    await settle(200);
    return { atBottom, button: above };
  },
  /**
   * A sent turn that messages another session and waits for its reply, its events in the order
   * the daemon sends them: the user's message holds still while the live rows change.
   */
  async peerExchange() {
    await open("other");
    await userScroll(max());
    await send({ text: "Ask the other chat what 6 + 7 is and tell me.", attachments: [], mentions: [] });
    await settle(600);
    const conversationId = "other";
    const requestId = stored[conversationId]!.at(-1)!.requestId!;
    const user = rows().findLast((row) => row.dataset["role"] === "user")!;
    const userTop = () => round(user.getBoundingClientRect().top - viewTop());
    const tops: number[] = [];
    let on = true;
    const tick = () => {
      if (!on) return;
      tops.push(userTop());
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
    const stages: { stage: string; userTop: number; status: string | null; rows: string[] }[] = [];
    let seq = 0;
    const emit = async (stage: string, ...events: object[]) => {
      for (const event of events) {
        const envelope = { seq: sequence++, stream: conversationId, streamSeq: 5000 + seq++, atMs: Date.now(), event } as unknown as EventEnvelope;
        useBoard.setState(({ board }) => (board ? { board: applyToBoard(board, envelope) } : {}));
      }
      await frame();
      await frame();
      stages.push({
        stage,
        userTop: userTop(),
        status: [...document.querySelectorAll('[data-slot="thread-status"]')].at(-1)?.textContent ?? null,
        rows: [...document.querySelectorAll("[data-slot=peer-words]")].map((row) => row.textContent ?? ""),
      });
    };
    const logged = (name: string, itemId: string, status: string) => ({
      type: "orchestratorLogged", conversationId,
      entry: { type: "provider", provider: "claude", event: { type: "toolCall", itemId, name, input: null, status, output: null } },
    });
    const stepped = (name: string, itemId: string, status: string, request = requestId) => ({
      type: "orchestratorStepped",
      step: { requestId: request, atMs: Date.now(), position: 0, kind: { type: "tool", itemId, name, detail: null, status, throughPosition: 0 } },
    });
    const asked = { id: requestId, conversationId, preview: "", state: { type: "working" }, startedAtMs: Date.now(), endedAtMs: null,
      steeredInto: null, steeredAfter: null, undo: null, worked: [{ fromMs: Date.now(), toMs: null }], quotaWait: false };
    await emit("starting", { type: "requestUpdated", request: asked }, { type: "runStateChanged", conversationId, state: "starting", error: null, requestId });
    await emit("running", { type: "runStateChanged", conversationId, state: "running", error: null, requestId });
    await emit("call", logged("mcp__brigadier__message_session", "send-1", "inProgress"), stepped("mcp__brigadier__message_session", "send-1", "inProgress"));
    await emit("recorded", {
      type: "peerMessageSent", messageId: "peer-1", to: "long", text: "What is 6 + 7?", blob: null, replyTo: null, chainId: "chain-1", hop: 1,
      requestId, turnInputId: null, state: { type: "pending" }, sentAtMs: Date.now(), automatic: false, answerId: null,
    });
    await emit("delivered", { type: "peerMessageState", messageId: "peer-1", state: { type: "delivered" }, sentAtMs: null });
    await emit("sent", logged("mcp__brigadier__message_session", "send-1", "completed"), stepped("mcp__brigadier__message_session", "send-1", "completed"));
    await emit("waiting", logged("mcp__brigadier__wait_for_replies", "wait-1", "inProgress"), stepped("mcp__brigadier__wait_for_replies", "wait-1", "inProgress"));
    await emit("waited", logged("mcp__brigadier__wait_for_replies", "wait-1", "completed"), stepped("mcp__brigadier__wait_for_replies", "wait-1", "completed"));
    on = false;
    stopStreaming(conversationId, "other-peer-answer", "6 + 7 is 13.", requestId);
    await emit("answered", { type: "requestUpdated", request: { ...asked, state: { type: "done" }, endedAtMs: Date.now() } });
    stages.pop();
    await settle(200);
    // A new turn sends, then the provider retries before anything follows: the retry still shows.
    await send({ text: "Ask it 7 + 8 too.", attachments: [], mentions: [] });
    await settle(600);
    const again = stored[conversationId]!.at(-1)!.requestId!;
    await emit("again", { type: "requestUpdated", request: { ...asked, id: again } },
      { type: "runStateChanged", conversationId, state: "running", error: null, requestId: again },
      logged("mcp__brigadier__message_session", "send-2", "inProgress"),
      stepped("mcp__brigadier__message_session", "send-2", "inProgress", again),
      { type: "peerMessageSent", messageId: "peer-2", to: "long", text: "What is 7 + 8?", blob: null, replyTo: null, chainId: "chain-2", hop: 1,
        requestId: again, turnInputId: null, state: { type: "delivered" }, sentAtMs: Date.now(), automatic: false, answerId: null },
      stepped("mcp__brigadier__message_session", "send-2", "completed", again),
      logged("mcp__brigadier__message_session", "send-2", "completed"));
    await emit("retrying", {
      type: "orchestratorLogged", conversationId,
      entry: { type: "provider", provider: "claude", event: { type: "error", error: { kind: "network", message: "reset", willRetry: true, limit: null, code: null } } },
    });
    stopStreaming(conversationId, "other-peer-again", "7 + 8 is 15.", again);
    await settle(200);
    return { stages, tops: [...new Set(tops)] };
  },
  /** An existing conversation never opened, whose one turn still runs: the bottom. */
  async working() {
    await open("working");
    return { distance: distance(), rows: rows().length };
  },
};

async function drive(): Promise<void> {
  const result: Record<string, unknown> = {};
  try {
    const only = new URLSearchParams(location.search).get("only");
    for (const [name, step] of Object.entries(steps)) {
      if (only !== null && name !== only) continue;
      // Where a run that ran out of time stopped.
      document.body.dataset["scrollStep"] = name;
      result[name] = await step();
    }
  } catch (error) {
    result["error"] = String(error instanceof Error ? error.stack : error);
  }
  const pre = document.createElement("pre");
  pre.id = "thread-scroll-result";
  pre.textContent = JSON.stringify(result);
  document.body.append(pre);
}

Object.assign(window, { threadScroll: { steps, open, send, userScroll, distance, lastGap, buttonState } });
useBoard.setState({ board: emptyBoard("long") });
createRoot(document.getElementById("root")!).render(<Page />);
