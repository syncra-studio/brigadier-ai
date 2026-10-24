/** Drives follow-up events through the real ConversationView, with no daemon or app data. */
// oxlint-disable-next-line import/no-unassigned-import
import "@/fixtures/flow";
import { mockIPC } from "@tauri-apps/api/mocks";

import type { EventEnvelope, Message, QueuedMessage, Request, SendOutcome, SentToWorker } from "@/ipc/generated";
import { send } from "@/state/actions";
import { applyActivityNews } from "@/state/activity";
import { applyBoardEvents, emptyBoard, useBoard } from "@/state/board";
import { applyEvents, emptyThread, useApp } from "@/state/store";

const id = "flow-session";
const text = "Follow-up awaiting admission";
const item: QueuedMessage = { id: "follow-up", text, attachments: [], mentions: [], queuedAtMs: Date.now(), editedAtMs: null };
const message: Message = { id: "admitted", conversationId: id, seq: 4, role: "user", text, attachments: [], mentions: [], createdAtMs: Date.now() + 100, blob: null, model: null, requestId: "admitted", parentId: null };
let respond: (outcome: SendOutcome) => void;
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "sendMessage") return new Promise((resolve) => { respond = (outcome) => resolve({ method: req.method, outcome }); });
  if (req.method === "getPullRequest") return { method: req.method, pullRequest: null };
  if (req.method === "listFiles") return { method: req.method, files: [], truncated: false };
  return { method: req.method };
});
useApp.setState({ threads: { [id]: emptyThread }, pending: [], summaryShown: { [id]: false } });
useBoard.setState({ board: emptyBoard(id) });
// The session rests: its CLI's turn is over.
applyActivityNews({ conversationId: id, entries: [], ended: "done" });

let seq = 0;
function apply(events: EventEnvelope["event"][]) {
  const batch = events.map((event) => ({ event, stream: `conversation:${id}`, streamSeq: ++seq, seq, atMs: Date.now() } as EventEnvelope));
  applyEvents(batch);
  applyBoardEvents(batch);
}
function queued(items: QueuedMessage[], sent: SentToWorker[] = []): EventEnvelope["event"] {
  return { type: "queueChanged", conversationId: id, queue: { items, paused: false, sent } };
}
const pause = () => new Promise((resolve) => setTimeout(resolve, 150));
/** The row's state as the rail shows it (run2-D40). */
function railState() {
  const row = document.querySelector('[data-slot="message-queue-item"]');
  const notice = document.querySelector('[data-slot="message-queue-sent"]');
  return {
    deciding: row?.getAttribute("data-deciding") === "true" && row.getAttribute("aria-busy") === "true",
    note: row?.querySelector(".shimmer, [data-slot=held-note]")?.textContent?.split(" · ")[0] ?? null,
    buttons: [...(row?.querySelectorAll("button") ?? [])].map((button) => button.textContent ?? "").filter((label) => /new session|Run here/.test(label)),
    notice: notice?.querySelector("span")?.textContent ?? null,
  };
}
function view() {
  return {
    rail: [...document.querySelectorAll('[data-slot="message-queue-item"]')].filter((node) => node.textContent?.includes(text)).length,
    chat: [...document.querySelectorAll('[data-role="user"]')].filter((node) => node.textContent?.includes(text)).length,
  };
}
async function drive() {
  await pause();
  const sending = send({ text, attachments: [], mentions: [] });
  await pause();
  const idle = view();
  apply([queued([item])]);
  await pause();
  const queuedView = view();
  respond({ type: "queued", item });
  await sending;
  await pause();
  const lateResponse = view();
  apply([queued([]), { type: "messageAppended", message }]);
  await pause();
  const joined = view();
  useApp.setState({ threads: { [id]: emptyThread } });
  apply([queued([item])]);
  await pause();
  apply([queued([])]);
  await pause();
  const removed = view();
  // The session judges it with `brigadier queue`; the rail shows each state the daemon writes.
  apply([queued([{ ...item, deciding: true, delivered: true }])]);
  await pause();
  const deciding = railState();
  apply([queued([{ ...item, deciding: true, delivered: true, heldBy: { command: "cargo test", sinceMs: Date.now() } }])]);
  await pause();
  const held = railState();
  apply([queued([{ ...item, delivered: true, offerSession: true }])]);
  await pause();
  const offered = railState();
  apply([queued([], [{ id: item.id, text, worker: "task-2", workerNumber: 2, workerTitle: "Docs", sentAtMs: Date.now() }])]);
  await pause();
  const sentToWorker = { ...railState(), ...view() };
  const result = document.createElement("pre");
  result.id = "follow-ups-result";
  result.textContent = JSON.stringify({ idle, queued: queuedView, lateResponse, joined, removed, deciding, held, offered, sentToWorker });
  document.body.append(result);
}
void drive();
