/**
 * A finished turn's end in a production ConversationView (the finished phase flow): the first
 * request merged its pull request on its own, and its reply carries the thread's proof, a
 * screenshot and a recording (`attach_proof`), as thumbnails, then the merge in its Changes card
 * with Revert and View changes: the merge's own (the turn edited nothing it counted), its files'
 * (?edited=1), or, merged by the next request (?later=1), the first one's files' while the second
 * shows the merge's own. ?warn=1 records an open finding and a failed CI run: the card's footer
 * row, which opens the Progress tab. ?drive=1 writes what the turn shows to #merge-message-result,
 * then clicks Revert, confirms twice and records what was sent (MergeMessageRender.test.ts reads it).
 */
import type { MergeRecord, OrchestratorStep } from "@/ipc/generated";
import { useBoard } from "@/state/board";
import { openPanelTab, sessionTabs } from "@/state/sessionTabs";

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

// Tests ran on the merged commit; the review stopped one commit short of it; CI passed on it.
const warn = params.has("warn");
const RECORD: MergeRecord = {
  head: "a1b2c3d4",
  checks: [{ command: "pnpm test", status: "exit 0", onHead: true }],
  reviews: [{ reviewer: "codex", model: "gpt-6", from: "0000aaaa", to: "9999bbbb", state: { type: "findings", count: 2 }, fixed: 1, dismissed: warn ? 0 : 1, open: warn ? 1 : 0, onHead: false }],
  ci: { sha: "a1b2c3d4", result: warn ? "failed" : "passed", onHead: true },
  proofs: 2,
  verifier: null,
  firstAuto: true,
};

const later = params.has("later");
const edited = later || params.has("edited");
const step = (position: number, kind: OrchestratorStep["kind"], requestId = "r1"): OrchestratorStep => ({ requestId, atMs: Date.now(), position, kind });
useBoard.setState(({ board }) =>
  board
    ? {
        board: {
          ...board,
          requests: edited
            ? {
                ...board.requests,
                r1: {
                  ...board.requests["r1"]!,
                  changes: {
                    stat: { files: [{ path: "README.md", insertions: 1, deletions: 1, binary: false }, { path: "docs/install.md", insertions: 4, deletions: 0, binary: false }], insertions: 5, deletions: 1 },
                    settled: true,
                  },
                },
              }
            : board.requests,
          orchestratorSteps: [
            ...board.orchestratorSteps,
            step(7.2, { type: "proof", id: "proof-shot", name: "readme.png", mime: "image/png", caption: "The install section, fixed" }),
            step(7.3, { type: "proof", id: "proof-video", name: "install.webm", mime: "video/webm" }),
            step(later ? 99 : 7.4, { type: "merged", branch: "brigadier/flow/session", base: "main", commits: 1, askedIn: later ? null : "auto:1", head: "a1b2c3d4", mergeCommit: "f00dfeed", pr: 7, auto: !later, requests: ["r1"], record: RECORD }, later ? "r2" : "r1"),
          ],
        },
      }
    : {},
);

// ?open=reviews: the Progress tab in front, for a look.
if (params.get("open") === "reviews") {
  const conversationId = useBoard.getState().board?.conversationId;
  if (conversationId) openPanelTab(conversationId, { type: "reviews" }, "Progress");
}

const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, " ").trim() ?? "";
const all = (selector: string) => [...document.querySelectorAll<HTMLElement>(selector)];

async function drive() {
  for (let tries = 0; tries < 100 && !document.querySelector('[data-slot="turn-proofs"] img, [data-slot="turn-proofs"] video'); tries++) await wait(50);
  for (let tries = 0; tries < 60 && !document.querySelector('[data-slot="merge-revert"]'); tries++) await wait(50);
  await wait(300);
  const cards = all('[data-slot="turn-diff"]');
  const proofs = document.querySelector<HTMLElement>('[data-slot="turn-proofs"]');
  const answer = all('[data-slot="aui_message-group"] *').find((each) => text(each).startsWith("Fixed the typo"));
  const image = proofs?.querySelector("img")?.getBoundingClientRect();
  // The card with the turn's own files; a merge's own card has none.
  const card = cards.find((each) => each.querySelector('[data-slot="turn-diff-file"]'));
  const seen = {
    proofs: [...(proofs?.querySelectorAll<HTMLElement>('[data-slot="proof"]') ?? [])].map((proof) => ({
      kind: proof.dataset.kind,
      image: proof.querySelector("img")?.getAttribute("alt") ?? null,
      video: proof.querySelector("video")?.getAttribute("src")?.slice(0, 16) ?? null,
      // Nothing under a thumbnail: its caption is its name and tooltip.
      captionShown: !!proof.querySelector("figcaption") || text(proof).includes("install section"),
    })),
    thumbnail: image ? [Math.round(image.width), Math.round(image.height)] : null,
    // Under the first request's answer, after it.
    underAnswer: !!answer && !!proofs && (answer.compareDocumentPosition(proofs) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0,
    // No row of its own in the work.
    rows: all('[data-slot="orchestrator-step"]').filter((row) => /readme\.png|install\.webm|attach/i.test(text(row))).length,
    read: [...new Set(read)].toSorted(),
    // What was checked is in the Progress tab, never listed in the thread.
    checkedInThread: !!document.querySelector('[data-slot="merge-checked"]') || /screenshots? or recordings?|Merged on its own/.test(text(document.body)),
    headers: cards.filter((each) => !each.querySelector('[data-slot="turn-diff-file"]')).map((each) => text(each.querySelector('[data-slot="turn-diff-title"]'))),
    card: card ? { title: text(card.querySelector('[data-slot="turn-diff-title"]')), merged: text(card.querySelector('[data-slot="turn-diff-status"]')), buttons: [...card.querySelectorAll("button:not([aria-label])")].map(text).filter((each) => /Revert|Discard|View changes/.test(each)), rowWords: all('[data-slot="turn-diff-reviewed"]').map(text) } : null,
    warning: all('[data-slot="merge-warning-title"]').map(text),
    tab: null as string | null,
    reviews: [] as string[],
    revert: null as unknown,
  };
  const warning = document.querySelector<HTMLButtonElement>('[data-slot="merge-warning-open"]');
  if (warning) {
    warning.click();
    await wait(200);
    const conversationId = useBoard.getState().board?.conversationId ?? "";
    seen.tab = sessionTabs(conversationId).active;
    for (let tries = 0; tries < 60 && !document.querySelector('[data-slot="job-merge-checked"]'); tries++) await wait(50);
    seen.reviews = all('[data-slot="job-merge-checked"] li').map((line) => `${line.dataset.warn ? "! " : ""}${text(line)}`);
  }
  seen.revert = await revert();
  return seen;
}

const revertButton = () => document.querySelector<HTMLButtonElement>('button[data-slot="merge-revert"]');

/** The first Revert: it asks in the card's footer; confirmed twice, one request is sent and it reads Reverting…. */
async function revert() {
  const shown = text(revertButton());
  revertButton()?.click();
  for (let tries = 0; tries < 60 && !document.querySelector('[data-slot="merge-revert-go"]'); tries++) await wait(50);
  const go = document.querySelector<HTMLButtonElement>('[data-slot="merge-revert-go"]');
  go?.click();
  go?.click();
  for (let tries = 0; tries < 60 && sent.length === 0; tries++) await wait(50);
  await wait(400);
  return { shown, sent, after: all('[data-slot="merge-revert-confirm"] [role=status]').map(text) };
}

if (params.has("drive")) {
  void drive().then((seen) => {
    const result = document.createElement("pre");
    result.id = "merge-message-result";
    result.textContent = JSON.stringify(seen);
    document.body.append(result);
  });
}
