/** Drives a message sent while the session works through the real ConversationView, with no daemon or app data. */
// oxlint-disable-next-line import/no-unassigned-import
import "@/fixtures/flow";
import { mockIPC } from "@tauri-apps/api/mocks";

import type { EventEnvelope, Message, MovedMessage, QueuedMessage, Request, SendOutcome } from "@/ipc/generated";
import { send } from "@/state/actions";
import { applyBoardEvents, emptyBoard, useBoard } from "@/state/board";
import { applyEvents, emptyThread, useApp } from "@/state/store";

const id = "flow-session";
const text = "Also check the formatting";
const item: QueuedMessage = { id: "follow-up", text, attachments: [], mentions: [], queuedAtMs: Date.now(), editedAtMs: null };
const message: Message = { id: "admitted", conversationId: id, seq: 4, role: "user", text, attachments: [], mentions: [], createdAtMs: Date.now() + 100, blob: null, model: null, requestId: "admitted", parentId: null };
let respond: (outcome: SendOutcome) => void;
const settled: unknown[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "settleMoved") settled.push(req);
  if (req.method === "sendMessage") return new Promise((resolve) => { respond = (outcome) => resolve({ method: req.method, outcome }); });
  if (req.method === "getRunDiff") return { method: req.method, diff: null };
  if (req.method === "getPullRequest") return { method: req.method, pullRequest: null };
  if (req.method === "listFiles") return { method: req.method, files: [], truncated: false };
  return { method: req.method };
});
useApp.setState({ threads: { [id]: emptyThread }, pending: [], pinnedSummary: false });
useBoard.setState({ board: emptyBoard(id) });

let seq = 0;
function apply(events: EventEnvelope["event"][]) {
  const batch = events.map((event) => ({ event, stream: `conversation:${id}`, streamSeq: ++seq, seq, atMs: Date.now() } as EventEnvelope));
  applyEvents(batch);
  applyBoardEvents(batch);
}
function queued(items: QueuedMessage[], moved?: MovedMessage[]): EventEnvelope["event"] {
  return { type: "queueChanged", conversationId: id, queue: { items, paused: false, ...(moved ? { moved } : {}) } };
}
const pause = () => new Promise((resolve) => setTimeout(resolve, 150));
function view() {
  return {
    rail: [...document.querySelectorAll('[data-slot="message-queue-item"]')].filter((node) => node.textContent?.includes(text)).length,
    chat: [...document.querySelectorAll('[data-role="user"]')].filter((node) => node.textContent?.includes(text)).length,
  };
}
/** Buttons on the queued rows other than the row's own Steer and actions. */
const offers = () => [...document.querySelectorAll('[data-slot="message-queue-item"] button')].map((node) => node.textContent ?? "").filter((label) => /new session|Run here/.test(label));
const decidingShown = () => /Deciding/.test(document.body.textContent ?? "");
async function drive() {
  await pause();
  // Sent while the turn runs: it shows in the chat at once and joins the running turn.
  const sending = send({ text, attachments: [], mentions: [] });
  await pause();
  const pending = view();
  respond({ type: "sent", message });
  await sending;
  apply([{ type: "messageAppended", message }]);
  await pause();
  const sent = { ...view(), deciding: decidingShown() };
  // A queued row an older build left marked as being judged or offered: a plain queued row.
  apply([queued([{ ...item, deciding: true, offerTab: true } as QueuedMessage])]);
  await pause();
  const legacy = { rail: view().rail, deciding: decidingShown(), offers: offers() };
  apply([queued([])]);
  await pause();
  const removed = view();
  // The thread sent it on to another session: a notice with Undo, which asks the daemon to take it back.
  const sentOn: MovedMessage = { id: "moved", text: "Rename the login file.", attachments: [], mentions: [], to: "other-tab", toTitle: "Login page", movedAtMs: Date.now(), sent: { type: "sent", messageId: "m-1" } };
  apply([queued([], [sentOn])]);
  await pause();
  const notice = document.querySelector('[data-slot="message-queue-moved"]');
  const noticeText = notice?.textContent ?? "";
  [...(notice?.querySelectorAll("button") ?? [])].find((node) => node.textContent === "Undo")?.click();
  await pause();
  const result = document.createElement("pre");
  result.id = "follow-ups-result";
  result.textContent = JSON.stringify({ pending, sent, legacy, removed, noticeText, settled });
  document.body.append(result);
}
void drive();
