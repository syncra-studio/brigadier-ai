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
 * `<pre id="thread-scroll-result">`; `window.threadScroll` holds the steps for a browser probe.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/ConversationView";
import planned from "@/fixtures/boards/thread-plan-2026-10-10.events.json";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, ConversationView as View, EventEnvelope, Message, Request } from "@/ipc/generated";
import { select, send } from "@/state/actions";
import { applyToBoard, emptyBoard, useBoard } from "@/state/board";
import { applyEvents, emptyThread, useApp } from "@/state/store";

const LOAD_MS = 250;
/** How far the window shrinks in the resize checks. */
const SHRINK_PX = 120;
const BLOB_MS = 400;
const start = Date.now() - 86_400_000;

const paragraph = (turn: number, index: number) =>
  `Paragraph ${index + 1} of answer ${turn + 1}. The thread keeps its place while the rows around it change size, and it follows new content only from the bottom, the way ChatGPT's conversation does when a reply streams in below what the user reads.`;

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

function view(id: string): View {
  const running = id === "working";
  return {
    conversation: conversations[id]!,
    context: null,
    messages: { messages: stored[id] ?? [], hasMore: false },
    tasks: [], approvals: [], questions: [], plans: [], requests: [], workerSteps: [], orchestratorSteps: [],
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

mockIPC(async (command, payload) => {
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
});

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

/** A scroll by the user (a wheel, a key, the scrollbar): the page sets it itself. */
async function userScroll(top: number): Promise<void> {
  viewport().dispatchEvent(new WheelEvent("wheel", { deltaY: top < viewport().scrollTop ? -1 : 1 }));
  viewport().scrollTop = top;
  await wait(50);
}

async function open(id: string): Promise<void> {
  select({ type: "conversation", id });
  await wait(LOAD_MS + BLOB_MS + 400);
}

/** The newest row's bottom to the floating footer's top. */
const lastGap = () => round(stackTop() - rows().at(-1)!.getBoundingClientRect().bottom);

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
  await wait(450);
  return { before: round(before), after: round(toggle.getBoundingClientRect().top), distance: distance() };
}

/** Streams `words` more words of an answer, noting the view's scrollTop and distance after each. */
async function stream(messageId: string, requestId: string, words: number): Promise<{ tops: number[]; distances: number[] }> {
  const tops: number[] = [];
  const distances: number[] = [];
  const streaming = useBoard.getState().board?.streaming;
  let text = streaming?.messageId === messageId ? streaming.text : "";
  for (let index = 0; index < words; index++) {
    text += `${index % 12 === 11 ? "\n\n" : " "}word${index}`;
    useBoard.setState(({ board }) =>
      board ? { board: { ...board, run: "running", runRequest: requestId, streaming: { messageId, text, requestId } } } : {},
    );
    await wait(20);
    await frame();
    tops.push(Math.round(viewport().scrollTop));
    distances.push(distance());
  }
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
      bottomFade: round(fade(".thread-bottom-fade") - footer),
    };
  },
  /** The button's threshold and place. */
  async button() {
    await userScroll(max() - 8);
    await wait(250);
    const at8 = buttonState();
    await userScroll(max() - 9);
    await wait(250);
    const at9 = buttonState();
    return { at8, at9 };
  },
  /** Mid-scroll, a session switch and back: the same place, with history and long text loading late. */
  async switchBack() {
    await userScroll(Math.round(max() / 2));
    await wait(100);
    const top = viewport().scrollTop;
    const anchor = rows().find((row) => row.getBoundingClientRect().bottom > viewTop())!;
    const id = anchor.dataset["messageId"];
    const offset = round(anchor.getBoundingClientRect().top - viewTop());
    await open("other");
    // Gone from memory: it loads again, its long replies' full text after it.
    useApp.setState((state) => ({ threads: { ...state.threads, long: emptyThread } }));
    select({ type: "conversation", id: "long" });
    const early: number[] = [];
    for (let at = 0; at < 12; at++) {
      await wait(100);
      early.push(Math.round(viewport().scrollTop));
    }
    const back = rows().find((row) => row.dataset["messageId"] === id);
    return { top: Math.round(top), restored: Math.round(viewport().scrollTop), offset, restoredOffset: back ? round(back.getBoundingClientRect().top - viewTop()) : null, early };
  },
  /** Earlier turns shown, then a switch and back: the same older row at the same place. */
  async revealedBack() {
    $<HTMLButtonElement>('[data-slot="earlier-turns"]')?.click();
    await wait(200);
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
      await wait(200);
      const height = viewport().clientHeight;
      column.classList.remove("hidden");
      await wait(200);
      return height;
    };
    await userScroll(Math.round(max() / 3));
    await wait(100);
    const top = Math.round(viewport().scrollTop);
    const anchor = rows().find((row) => row.getBoundingClientRect().bottom > viewTop())!;
    const offset = round(anchor.getBoundingClientRect().top - viewTop());
    const hiddenHeight = await hidden();
    const middle = { top, restored: Math.round(viewport().scrollTop), offset, restoredOffset: round(anchor.getBoundingClientRect().top - viewTop()), button: buttonState().shown };
    await userScroll(max());
    await wait(100);
    await hidden();
    return { hiddenHeight, middle, bottom: distance() };
  },
  /** A report's Details, a disclosure, opened and closed mid-scroll and at the bottom. */
  async expand() {
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
    await userScroll(max());
    const last = summaries.at(-1)!;
    const bottomOpen = await toggleStays(last);
    const buttonAfter = buttonState();
    const bottomClose = await toggleStays(last);
    return { midOpen, midClose, userOpen, bottomOpen, buttonAfter, bottomClose };
  },
  /** A notice above the composer: the view doesn't move; at the bottom the gaps are kept. */
  async composerGrows() {
    await userScroll(max());
    await wait(100);
    const before = { top: Math.round(viewport().scrollTop), stackTop: round(stackTop()) };
    const notice = { level: "warning", text: "A notice above the composer, two lines long to make the footer taller than one row would.\nIts second line.", atMs: Date.now() };
    useBoard.setState(({ board }) => (board ? { board: { ...board, notices: [notice as never] } } : {}));
    await wait(300);
    const grown = { top: Math.round(viewport().scrollTop), stackTop: round(stackTop()), distance: distance(), button: buttonState() };
    await userScroll(max());
    await wait(250);
    const atBottom = { lastGap: lastGap(), button: buttonState() };
    useBoard.setState(({ board }) => (board ? { board: { ...board, notices: [] } } : {}));
    await wait(300);
    const shrunk = { distance: distance(), lastGap: lastGap() };
    return { before, grown, atBottom, shrunk };
  },
  /** A send from the bottom of a conversation, its answer streaming, then followed from the bottom. */
  async send() {
    await open("other");
    await userScroll(0);
    const topPadding = round(rows()[0]!.getBoundingClientRect().top - viewTop());
    await userScroll(max());
    await send({ text: "A new question, sent from the bottom.", attachments: [], mentions: [] });
    await wait(600);
    const user = rows().findLast((row) => row.dataset["role"] === "user")!;
    const placed = {
      userTop: round(user.getBoundingClientRect().top - viewTop()),
      distance: distance(),
      room: round($('[data-slot="aui_thread-spacer"]')!.getBoundingClientRect().height),
      belowUser: round(stackTop() - user.getBoundingClientRect().bottom),
      viewHeight: viewport().clientHeight,
    };
    const requestId = stored["other"]!.at(-1)!.requestId!;
    const top = Math.round(viewport().scrollTop);
    const streamed = await stream("other-streaming", requestId, 260);
    const unfollowed = { top, tops: [...new Set(streamed.tops)], distance: distance(), button: buttonState() };
    await userScroll(max());
    const followed = await stream("other-streaming", requestId, 120);
    // At the bottom, a resize of the window keeps it there.
    $("#thread-scroll-main")!.style.paddingBottom = `${SHRINK_PX}px`;
    await wait(200);
    const shrunkWindow = distance();
    $("#thread-scroll-main")!.style.paddingBottom = "";
    await wait(200);
    const grownWindow = distance();
    stopStreaming("other", "other-answer", "Done.", requestId);
    await wait(200);
    return {
      topPadding,
      placed,
      unfollowed,
      followed: { distances: [...new Set(followed.distances)], grew: followed.tops.at(-1)! - followed.tops[0]! },
      shrunkWindow,
      grownWindow,
    };
  },
  /** At the bottom with the sent turn's room (not following), a resize keeps it at the bottom. */
  async resizeAtBottom() {
    await send({ text: "Another question.", attachments: [], mentions: [] });
    await wait(600);
    const placed = distance();
    $("#thread-scroll-main")!.style.paddingBottom = `${SHRINK_PX * 2}px`;
    await wait(200);
    const shrunk = distance();
    $("#thread-scroll-main")!.style.paddingBottom = "";
    await wait(200);
    return { placed, shrunk, grown: distance() };
  },
  /** The first message of a new chat, sent from the new-chat view. */
  async newChat() {
    select({ type: "draft", kind: "chat" });
    await wait(200);
    await send({ text: "The first message of a new chat.", attachments: [], mentions: [] }, { kind: "chat", setup: null });
    await wait(LOAD_MS + 600);
    const user = rows().find((row) => row.dataset["role"] === "user");
    return { userTop: user ? round(user.getBoundingClientRect().top - viewTop()) : null, top: viewport().scrollTop };
  },
  /** With the capsule above the composer: the gaps are to the capsule's top. */
  async capsule() {
    await open(planId);
    useBoard.setState({ board: { ...planBoard, loaded: true, head: planMessages.at(-1)?.id ?? null } });
    await wait(400);
    await userScroll(max());
    await wait(250);
    const capsule = $('[data-slot="composer-capsule"]');
    const atBottom = { capsule: capsule !== null, capsuleInStack: capsule ? round(capsule.getBoundingClientRect().top - stackTop()) : null, lastGap: lastGap(), distance: distance() };
    await userScroll(max() - 200);
    await wait(250);
    return { atBottom, button: buttonState() };
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
    for (const [name, step] of Object.entries(steps)) {
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
