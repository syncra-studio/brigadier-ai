/** Real question card and composer; only IPC storage and answers are confined to this page. */
import { mockIPC } from "@/ipc/nativeMocks";
import { fixtureInput } from "@/fixtures/input";
import { setAsideCards } from "@/app/conversation/ActionCards";
import type { AttachmentRef, Question, Request } from "@/ipc/generated";
import { useBoard } from "@/state/board";

const show = new URLSearchParams(location.search).has("show");
history.replaceState(null, "", `${location.pathname}?view=done&approvals=0&summary=0&sidebar=0${show ? "&show" : ""}`);
localStorage.removeItem("brigadier.drafts");
await import("@/fixtures/flow");
const bytes = new Map<string, string>();
const calls: Request[] = [];
let failed = false;
mockIPC(async (command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  calls.push(req);
  switch (req.method) {
    case "addAttachment": {
      if (failed) throw new Error("Upload unavailable");
      const id = String(bytes.size + 1).padStart(64, "0");
      bytes.set(id, req.data);
      const attachment: AttachmentRef = { id, name: req.name, mime: req.mime, bytes: atob(req.data).length, pasted: req.pasted, inline: null };
      return { method: req.method, attachment };
    }
    case "readAttachment": return { method: req.method, data: bytes.get(req.id) ?? "" };
    case "prepareCardAnswer": {
      const text = req.text.replace(/\[(Image|Pasted text) #(\d+)\]/g, (marker, kind: string, n: string) => {
        const ref = req.attachments.find((entry) => entry.inline === Number(n) && entry.pasted === (kind === "Pasted text"));
        if (!ref) throw new Error(`Missing attachment for ${marker}`);
        return ref.pasted ? new TextDecoder().decode(Uint8Array.from(atob(bytes.get(ref.id)!), (c) => c.charCodeAt(0))) : `/tmp/brigadier-card-images/${ref.id}-image.png`;
      });
      return { method: req.method, text };
    }
    case "listFiles": return { method: req.method, files: ["src/main.ts"], truncated: false };
    case "getPullRequest": return { method: req.method, pullRequest: null };
    case "workerThread": return { method: req.method, view: { entries: [], turns: [] } };
    default: return { method: req.method };
  }
});
const pause = (ms = 25) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(what: string, check: () => boolean) {
  const deadline = Date.now() + 12000;
  while (!check()) { if (Date.now() > deadline) throw new Error(`Waiting for ${what}`); await pause(); }
}
const field = () => document.querySelector<HTMLElement>('[data-slot=action-card] [contenteditable=true]')!;
const composer = () => document.querySelector<HTMLElement>('[data-slot=aui_composer-shell] [contenteditable=true]')!;
const chips = () => ({ image: field()?.querySelector('[data-slot=inline-image]') != null, paste: field()?.querySelector('[data-slot=inline-paste]') != null });
function paste(data: DataTransfer) { field().dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true })); }
function focusEditor(element: HTMLElement) {
  element.focus();
  const range = document.createRange();
  range.selectNodeContents(element);
  range.collapse(false);
  const selection = window.getSelection()!;
  selection.removeAllRanges();
  selection.addRange(range);
}
async function type(text: string) {
  if (show) document.execCommand("insertText", false, text);
  else await fixtureInput({ type: "type", text });
}
const question: Question = {
  id: "card-paste", conversationId: "flow-session", taskId: null, requestId: null, position: 999,
  kind: { type: "orchestrator" }, text: "", options: [], recommended: null,
  items: [{ text: "What should Brigadier change in this screen?", options: [{ label: "Keep the current layout", description: null }], recommended: 0 }],
  answer: null, answers: [], createdAtMs: Date.now(), answeredAtMs: null,
};
const longText = "Keep the screenshot and these review notes together.\n".repeat(55);
const seen: Record<string, unknown> = {};
try {
  await until("the board", () => !!useBoard.getState().board?.loaded);
  useBoard.setState(({ board }) => ({ board: board && { ...board, tasks: {}, requests: {}, approvals: {}, questions: { [question.id]: question }, orchestratorSteps: [], run: "idle", runRequest: null } }));
  await until("the editors", () => !!field() && !!composer());
  focusEditor(composer());
  await type("Main composer draft");
  focusEditor(field());
  const image = new DataTransfer();
  // A valid tiny PNG, the same format a screenshot puts on the clipboard.
  const png = Uint8Array.from(atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg=="), (c) => c.charCodeAt(0));
  image.items.add(new File([png], "image.png", { type: "image/png" }));
  paste(image);
  await until("the image chip", () => chips().image);
  await type(" and ");
  const text = new DataTransfer();
  text.setData("text/plain", longText);
  paste(text);
  await until("both stored chips", () => chips().paste && calls.filter((call) => call.method === "addAttachment").length === 2);
  await pause(700);
  seen.chips = chips();
  seen.mainDraft = composer().innerText;
  seen.labels = field().innerText;
  if (!show) {
    await type(" in @src/main");
    await until("the mentions menu", () => document.querySelector("[data-slot=composer-mentions]") !== null);
    await fixtureInput({ type: "key", key: "Enter" });
    await until("the file mention", () => field().querySelector('[data-chip=file]') !== null);
    seen.mentionAnswered = calls.some((call) => call.method === "answerQuestion");
    // Putting a question aside unmounts it, as a visit to Settings does.
    setAsideCards("flow-session", [question.id]);
    await until("the card to unmount", () => !field());
    setAsideCards("flow-session", []);
    await until("restored chips and mention", () => chips().image && chips().paste && field().querySelector('[data-chip=file]') !== null);
    seen.restored = chips();
    focusEditor(field());
    await fixtureInput({ type: "key", key: "Enter", modifiers: ["Shift"] });
    await type("Use these details");
    seen.multiline = field().innerText.includes("\nUse these details");
    seen.beforeEnter = calls.filter((call) => call.method === "answerQuestion").length;
    await fixtureInput({ type: "key", key: "Enter" });
    await until("the answer", () => calls.some((call) => call.method === "answerQuestion"));
    seen.answer = calls.find((call) => call.method === "answerQuestion");
    seen.prepared = calls.find((call) => call.method === "prepareCardAnswer");
    // Long text pasted into an empty card must also be a chip, not an invisible attachment row.
    useBoard.setState(({ board }) => ({ board: board && { ...board, questions: { empty: { ...question, id: "empty", items: [{ ...question.items[0]!, options: [] }] } } } }));
    await until("the empty card", () => !!field() && !field().innerText.trim());
    focusEditor(field());
    paste(text);
    await until("the empty field's text chip", () => chips().paste);
    seen.emptyChip = chips().paste;
    field().dispatchEvent(new DragEvent("drop", { dataTransfer: image, bubbles: true, cancelable: true }));
    await pause(100);
    // The thread retains its normal drop handling; the card itself accepts no dropped chip.
    seen.dropIgnored = !chips().image;
    const afterDrop = calls.filter((call) => call.method === "addAttachment").length;
    failed = true;
    paste(image);
    await until("the failed upload", () => calls.filter((call) => call.method === "addAttachment").length > afterDrop);
    await pause(100);
    await fixtureInput({ type: "key", key: "Enter" });
    await until("the upload error", () => [...document.querySelectorAll('[data-slot=action-card] [role=alert]')].some((alert) => alert.textContent?.includes("could not be stored")));
    await until("the answer to be editable after the error", () => !!field());
    seen.failedKept = chips();
    seen.answersAfterFailure = calls.filter((call) => call.method === "answerQuestion").length;
    // Arrow navigation out of a single-line field and Escape still return to the card.
    focusEditor(field());
    await fixtureInput({ type: "key", key: "Escape" });
    seen.escapeFocus = document.activeElement?.matches("[data-slot=action-card]");
  }
} catch (error) { seen.error = error instanceof Error ? error.stack : String(error); }
const result = document.createElement("pre");
result.id = "question-card-paste-result";
result.className = "hidden";
result.textContent = JSON.stringify(seen);
document.body.append(result);
