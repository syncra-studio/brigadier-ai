/**
 * The merge message in a production ConversationView (the finished phase flow): the first
 * request merged its pull request on its own, and its reply carries the thread's proof, a
 * screenshot and a recording (`attach_proof`), and Revert. ?drive=1 writes what the message
 * shows to #merge-message-result, then clicks Revert twice and records what was sent
 * (MergeMessageRender.test.ts reads it).
 */
import type { OrchestratorStep } from "@/ipc/generated";
import { useBoard } from "@/state/board";

const params = new URLSearchParams(location.search);
history.replaceState(null, "", `${location.pathname}?view=done&summary=0&approvals=0`);
await import("@/fixtures/flow");

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const SVG = btoa(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 48"><rect width="64" height="48" fill="teal"/></svg>`);
const read: string[] = [];
const sent: string[] = [];
// The flow fixture's daemon answers requests; the proofs' bytes are answered here.
const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args: unknown, options?: unknown) => Promise<unknown> } })["__TAURI_INTERNALS__"];
const invoke = internals.invoke.bind(internals);
internals.invoke = (command, args, options) => {
  const req = (args as { request?: { method: string; itemId?: string } } | undefined)?.request;
  if (command === "ipc_request" && req?.method === "readThreadImage") {
    read.push(req.itemId ?? "");
    const video = req.itemId === "proof-video";
    return Promise.resolve({ method: req.method, data: video ? btoa("not a real recording") : SVG, mime: video ? "video/webm" : "image/svg+xml" });
  }
  if (command === "ipc_request" && req?.method === "sendMessage") {
    const { conversationId, text } = req as unknown as { conversationId: string; text: string };
    sent.push(text);
    const message = { id: `sent-${sent.length}`, conversationId, seq: 100 + sent.length, role: "user", text, blob: null, createdAtMs: Date.now(), attachments: [], mentions: [], model: null, requestId: null, parentId: null };
    return wait(200).then(() => ({ method: req.method, outcome: { type: "sent", message } }));
  }
  return invoke(command, args, options);
};

const step = (position: number, kind: OrchestratorStep["kind"]): OrchestratorStep => ({ requestId: "r1", atMs: Date.now(), position, kind });
useBoard.setState(({ board }) =>
  board
    ? {
        board: {
          ...board,
          orchestratorSteps: [
            ...board.orchestratorSteps,
            step(7.2, { type: "proof", id: "proof-shot", name: "readme.png", mime: "image/png", caption: "The install section, fixed" }),
            step(7.3, { type: "proof", id: "proof-video", name: "install.webm", mime: "video/webm" }),
            step(7.4, { type: "merged", branch: "brigadier/flow/session", base: "main", commits: 1, askedIn: "auto:1", head: "a1b2c3d", mergeCommit: "f00dfeed", pr: 7, auto: true, requests: ["r1"] }),
          ],
        },
      }
    : {},
);

const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, " ").trim() ?? "";

const message = () => document.querySelector<HTMLElement>('[data-slot="merge-message"]');

async function drive() {
  for (let tries = 0; tries < 100 && !message()?.querySelector("img, video"); tries++) await wait(50);
  await wait(300);
  const root = message();
  const answer = [...document.querySelectorAll<HTMLElement>('[data-slot="aui_message-group"] *')].find((each) => text(each).startsWith("Fixed the typo"));
  return {
    proofs: [...(root?.querySelectorAll<HTMLElement>('[data-slot="proof"]') ?? [])].map((proof) => ({
      kind: proof.dataset.kind,
      image: proof.querySelector("img")?.getAttribute("alt") ?? null,
      video: proof.querySelector("video")?.getAttribute("src")?.slice(0, 16) ?? null,
      caption: text(proof.querySelector("figcaption")) || null,
    })),
    // Under the first request's answer, after it.
    underAnswer: !!answer && !!root && (answer.compareDocumentPosition(root) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0,
    // No row of its own in the work.
    rows: [...document.querySelectorAll('[data-slot="orchestrator-step"]')].filter((row) => /readme\.png|install\.webm|attach/i.test(text(row))).length,
    read: [...new Set(read)].toSorted(),
    revert: await revert(),
  };
}

/** Revert, clicked twice: one request sent, then it reads as asked. */
async function revert() {
  const button = () => message()?.querySelector<HTMLButtonElement>('button[data-slot="merge-revert"]');
  const shown = text(button());
  button()?.click();
  button()?.click();
  for (let tries = 0; tries < 60 && !message()?.querySelector('p[data-slot="merge-revert"]'); tries++) await wait(50);
  await wait(400);
  return { shown, sent, after: text(message()?.querySelector('[data-slot="merge-revert"]')) };
}

if (params.has("drive")) {
  void drive().then((seen) => {
    const result = document.createElement("pre");
    result.id = "merge-message-result";
    result.textContent = JSON.stringify(seen);
    document.body.append(result);
  });
}
