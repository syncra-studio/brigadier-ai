// Replays an arm's recorded daemon events through the desktop app's own state code and renders the
// live request block with the real RequestBlock, once per second of the request. See README.md.
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { renderToStaticMarkup } from "react-dom/server";

import { type BoardDigest, buildThread } from "@/app/conversation/blocks";
import * as phaseView from "@/app/conversation/phaseView";
import { type BlockMeta, RequestBlock } from "@/app/conversation/RequestBlock";
import { ViewContext } from "@/app/conversation/viewContext";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { EventEnvelope } from "@/ipc/generated";
import { applyBoardEvents, emptyBoard, useBoard } from "@/state/board";
import { applyEvents, emptyThread, useApp } from "@/state/store";

import { setMessage } from "./auiMock";

type Recorded = EventEnvelope & { recv_ms: number };
type Node = { nodeName: string; value?: string; attrs?: { name: string; value: string }[]; childNodes?: Node[] };

declare global {
  // eslint-disable-next-line no-var
  var __replayNow: number | null;
}

function attr(node: Node, name: string): string | undefined {
  return node.attrs?.find((a) => a.name === name)?.value;
}

function textOf(node: Node): string {
  if (node.nodeName === "#text") return node.value ?? "";
  // Elements laid out side by side (flex gaps) read as separate words.
  return (node.childNodes ?? []).map(textOf).join(" ");
}

function findAll(node: Node, pred: (n: Node) => boolean, out: Node[] = []): Node[] {
  if (pred(node)) out.push(node);
  for (const child of node.childNodes ?? []) findAll(child, pred, out);
  return out;
}

const slot = (name: string) => (n: Node) => attr(n, "data-slot") === name;
const clean = (s: string) => s.replace(/\s+/g, " ").trim();

type Frame = {
  t: number;
  state: string | null;
  header: string | null;
  orchestratorNotes: string[];
  actionRows: string[];
  workerRows: string[];
  workerActivity: string[];
  cards: number;
  activity: string | null;
  answer: string | null;
  onlyThinking: boolean;
  /** Live with no note, step, worker row or card, whatever the live line says ("Delegating…"). */
  noContent: boolean;
};

/** What the rendered block shows, read from its markup by the slots the components set. */
function readBlock(html: string, parseFragment: (html: string) => Node, t: number, state: string): Frame {
  const root = parseFragment(html);
  const work = findAll(root, slot("request-work"))[0] ?? null;
  const header = findAll(root, slot("request-work-header"))[0];
  const inWork = (name: string) => (work ? findAll(work, slot(name)).map((n) => clean(textOf(n))) : []);
  const notes = inWork("aui_assistant-message-content");
  // A run of finished thread tool steps folds into one `work-group` row ("Searched code, ran a
  // command"); closed, it renders only its summary line, so that line is the row on screen.
  const actionRows = inWork("orchestrator-step").concat(inWork("work-group"), inWork("compaction"));
  const workerRows = work ? findAll(work, slot("task-row")).map((n) => clean(textOf(n))) : [];
  const workerActivity = inWork("task-activity");
  const cards = work ? findAll(work, slot("request-card")).length + findAll(work, slot("request-steer")).length : 0;
  const activityNode = findAll(root, slot("request-activity"))[0];
  const activity = activityNode ? clean(textOf(activityNode)) : null;
  const answerNodes = work ? [] : findAll(root, slot("aui_assistant-message-content"));
  const answer = answerNodes.length ? clean(textOf(answerNodes.at(-1) as Node)).slice(0, 120) : null;
  const live = state === "working" || state === "waiting";
  const contentless = notes.length + actionRows.length + workerRows.length + cards === 0;
  return {
    t,
    state,
    header: header ? clean(textOf(header)) : null,
    orchestratorNotes: notes.map((s) => s.slice(0, 160)),
    actionRows,
    workerRows,
    workerActivity,
    cards,
    activity,
    answer,
    onlyThinking: live && work !== null && contentless && (activity === null || activity === "Thinking"),
    noContent: live && work !== null && contentless,
  };
}

export async function run(arm: string, parseFragment: (html: string) => Node): Promise<void> {
  const start = JSON.parse(readFileSync(join(arm, "start.json"), "utf8"));
  const conversationId: string = start.conversation;
  const t0: number = start.t0_ms;
  const sent = start.send?.value?.outcome?.message;
  const events: Recorded[] = readFileSync(join(arm, "rec/events.jsonl"), "utf8")
    .split("\n")
    .filter((line) => line.trim())
    .flatMap((line) => {
      try {
        return [JSON.parse(line) as Recorded];
      } catch {
        return []; // a line still being written
      }
    })
    .toSorted((a, b) => a.seq - b.seq);

  // The conversation as getConversation would read it when the user opens it before sending: empty.
  useApp.setState({
    selection: { type: "conversation", id: conversationId },
    threads: { [conversationId]: { ...emptyThread } },
  });
  useBoard.setState({ board: { ...emptyBoard(conversationId), loaded: true } });

  let next = 0;
  let clock = 0; // monotone event clock: the latest atMs applied so far
  const applyUntil = (t: number) => {
    const batch: Recorded[] = [];
    while (next < events.length && Math.max(clock, (events[next] as Recorded).atMs) <= t) {
      const event = events[next++] as Recorded;
      clock = Math.max(clock, event.atMs);
      batch.push(event);
    }
    if (batch.length) {
      applyEvents(batch);
      applyBoardEvents(batch);
    }
  };

  let requestId: string | null = sent?.requestId ?? sent?.id ?? null;
  const frames: Frame[] = [];
  let endT: number | null = null;
  const lastAt = events.reduce((max, e) => Math.max(max, e.atMs), 0);

  for (let t = t0; t <= lastAt + 1000; t += 1000) {
    applyUntil(t);
    globalThis.__replayNow = t;
    const board = useBoard.getState().board!;
    const app = useApp.getState();
    const thread = app.threads[conversationId] ?? emptyThread;
    const conversation = app.conversations[conversationId] ?? null;
    if (!requestId) requestId = thread.items.find((m) => m.role === "user")?.requestId ?? null;
    // As ConversationView selects it.
    const digest: BoardDigest & { head: string | null } = {
      tasks: board.tasks,
      approvals: board.approvals,
      questions: board.questions,
      plans: board.plans,
      requests: board.requests,
      orchestratorSteps: board.orchestratorSteps,
      machineSteps: board.machineSteps,
      decisions: board.decisions,
      compactions: board.compactions,
      runRequest: board.runRequest,
      streaming: board.streaming,
      head: board.head,
    };
    const session = conversation?.kind === "session";
    // Before phase 5's record cleanup, the app swapped stored overnight reports' texts in
    // (`shownTexts`/`reportTexts`); since then it passes the stored texts as they are.
    const legacy = phaseView as unknown as {
      shownTexts?: (t: typeof thread.fullText, r: unknown) => typeof thread.fullText;
      reportTexts?: (o: unknown) => unknown;
    };
    const texts =
      legacy.shownTexts && legacy.reportTexts
        ? legacy.shownTexts(thread.fullText, legacy.reportTexts(board.overnight))
        : thread.fullText;
    const tree = buildThread(thread.items, texts, thread.hasMore, digest, [], session ? "edits" : "all");
    const node = tree.nodes.find((n) => n.kind === "block" && requestId !== null && n.block.requestIds.includes(requestId));
    if (!node) {
      // The block isn't in the thread yet (the user message not stored yet): the app shows the
      // pending message's block, which works with nothing in it.
      frames.push({ t: (t - t0) / 1000, state: null, header: null, orchestratorNotes: [], actionRows: [], workerRows: [], workerActivity: [], cards: 0, activity: null, answer: null, onlyThinking: true, noContent: true });
      continue;
    }
    const { block } = node;
    const setup = conversation?.setup;
    const picked = setup?.type === "chat" ? setup.model : setup?.type === "session" ? setup.orchestrator : null;
    const answerId = block.texts.at(-1)?.messageId ?? null;
    // As ConversationView's useItems builds it (rework only decides "Try again": not measured).
    // Fields newer app versions added are passed when the block has them, so one harness
    // replays through older and newer app code alike.
    const newer = block as Partial<{ thinking: unknown; worked: unknown; quotaWait: boolean }>;
    const meta: BlockMeta = {
      texts: block.texts.map((text) => ({ position: text.position, model: text.model })),
      cards: block.cards,
      rows: block.rows,
      orchestratorSteps: block.orchestratorSteps,
      ...(newer.thinking !== undefined ? { thinking: newer.thinking } : {}),
      compactions: block.compactions,
      steers: block.steers.map((steer) => ({
        position: steer.position,
        text: steer.text,
        atMs: steer.message.createdAtMs,
        attachments: steer.message.attachments,
      })),
      state: block.state,
      startedAtMs: block.startedAtMs,
      endedAtMs: block.endedAtMs,
      ...(newer.worked !== undefined ? { worked: newer.worked } : {}),
      ...(newer.quotaWait !== undefined ? { quotaWait: newer.quotaWait } : {}),
      picked,
      session,
      rework: false,
      requestId: block.key,
      requestIds: block.requestIds,
      answerId,
    };
    setMessage({
      id: node.id,
      metadata: { custom: { block: meta } },
      status: block.state === "failed" ? { type: "incomplete", reason: "error", error: block.error } : { type: "running" },
      parts: block.texts.map((text) => ({ type: "text", text: text.text })),
      isLast: true,
    });
    const html = renderToStaticMarkup(
      <TooltipProvider>
        <ViewContext.Provider value={{ selection: { type: "conversation", id: conversationId }, conversation, embedded: false }}>
          <RequestBlock />
        </ViewContext.Provider>
      </TooltipProvider>,
    );
    const frame = readBlock(html, parseFragment, (t - t0) / 1000, block.state);
    frames.push(frame);
    if (process.env.REPLAY_HTML) writeFileSync(join(arm, `ui-replay-${frame.t}.html`), html);
    // Over once the request itself is (a block whose request isn't on the board yet reads "done").
    const stored = requestId ? board.requests[requestId] : undefined;
    if (stored && block.state !== "working" && block.state !== "waiting") {
      endT = frame.t;
      break;
    }
  }

  // Stretches of nothing but "Thinking" (and, looser, of no content whatever the live line says).
  const stretchesOf = (flag: (f: Frame) => boolean) => {
    const out: { start: number; length: number }[] = [];
    let from: number | null = null;
    for (const frame of frames) {
      if (flag(frame) && from === null) from = frame.t;
      if (!flag(frame) && from !== null) {
        out.push({ start: from, length: frame.t - from });
        from = null;
      }
    }
    if (from !== null) out.push({ start: from, length: (frames.at(-1)?.t ?? from) + 1 - from });
    return out;
  };
  const longestOf = (list: { start: number; length: number }[]) =>
    list.reduce<{ start: number; length: number } | null>((best, s) => (!best || s.length > best.length ? s : best), null);
  const stretches = stretchesOf((f) => f.onlyThinking);
  const longest = longestOf(stretches);
  const noContent = stretchesOf((f) => f.noContent);
  const first = (pred: (f: Frame) => boolean) => frames.find(pred)?.t ?? null;
  const request = requestId ? useBoard.getState().board?.requests[requestId] : undefined;
  const summary = {
    arm,
    requestEndedS: request?.endedAtMs ? (request.endedAtMs - t0) / 1000 : null,
    conversationId,
    requestId,
    finished: endT !== null,
    durationS: endT ?? frames.at(-1)?.t ?? null,
    finalState: frames.at(-1)?.state ?? null,
    longestOnlyThinking: longest,
    onlyThinkingOver30s: stretches.filter((s) => s.length > 30),
    onlyThinkingTotalS: frames.filter((f) => f.onlyThinking).length,
    longestNoContent: longestOf(noContent),
    firstWorkerRowS: first((f) => f.workerRows.length > 0),
    firstActionRowS: first((f) => f.actionRows.length > 0),
    firstOrchestratorNoteS: first((f) => f.orchestratorNotes.length > 0),
    firstHeaderS: first((f) => f.header !== null),
  };
  writeFileSync(join(arm, "ui-replay.json"), JSON.stringify({ summary, stretches, noContentStretches: noContent, frames }, null, 1));

  // Short timeline: only the seconds where what shows changed (counts, worker words, live line).
  let previous = "";
  console.log(`\n${arm}: seconds where the block changed (t = s after send)`);
  for (const f of frames) {
    const words = f.workerRows.map((r) => r.replace(/·.*$/, "").trim());
    const shape = JSON.stringify([f.state, f.onlyThinking, f.header !== null, f.orchestratorNotes.length, f.actionRows.length, words, f.activity, f.cards, f.answer !== null]);
    if (shape === previous) continue;
    previous = shape;
    const bits = [
      f.header && `header="${f.header}"`,
      f.orchestratorNotes.length && `notes=${f.orchestratorNotes.length} last="${f.orchestratorNotes.at(-1)?.slice(0, 60)}"`,
      f.actionRows.length && `actions=${f.actionRows.length} last="${f.actionRows.at(-1)?.slice(0, 60)}"`,
      words.length && `workers=${JSON.stringify(words)}`,
      f.activity && `live="${f.activity}"`,
      f.answer && `answer="${f.answer.slice(0, 50)}"`,
    ].filter(Boolean);
    console.log(`  ${String(f.t).padStart(5)}s [${f.state ?? "pending"}]${f.onlyThinking ? " ONLY-THINKING" : ""} ${bits.join(" ")}`);
  }
  console.log("\nsummary:", JSON.stringify(summary, null, 1));
  console.log(`wrote ${join(arm, "ui-replay.json")}`);
}
