/**
 * A finished turn's end in a production ConversationView (the finished phase flow): the first
 * request merged its pull request, and its reply carries the thread's proof, a
 * screenshot and a recording, as thumbnails (?edited=1: the turn also edited files; ?later=1: the
 * next request merged them). ?drive=1 writes what the turn shows to #merge-message-result
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
  return invoke(command, args, options);
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
            step(later ? 99 : 7.4, { type: "merged", branch: "brigadier/flow/session", base: "main", commits: 1, askedIn: later ? null : "auto:1", head: "a1b2c3d4", mergeCommit: "f00dfeed", pr: 7, requests: ["r1"] }, later ? "r2" : "r1"),
          ],
        },
      }
    : {},
);

const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, " ").trim() ?? "";
const all = (selector: string) => [...document.querySelectorAll<HTMLElement>(selector)];

async function drive() {
  for (let tries = 0; tries < 100 && !document.querySelector('[data-slot="turn-proofs"] img, [data-slot="turn-proofs"] video'); tries++) await wait(50);
  await wait(300);
  const proofs = document.querySelector<HTMLElement>('[data-slot="turn-proofs"]');
  const answer = all('[data-slot="aui_message-group"] *').find((each) => text(each).startsWith("Fixed the typo"));
  // The thumbnail's frame, its border in.
  const image = proofs?.querySelector("[data-slot=proof] > *")?.getBoundingClientRect();
  return {
    proofs: [...(proofs?.querySelectorAll<HTMLElement>('[data-slot="proof"]') ?? [])].map((proof) => ({
      kind: proof.dataset.kind,
      image: proof.querySelector("img")?.getAttribute("alt") ?? null,
      video: proof.querySelector("video")?.getAttribute("src")?.slice(0, 16) ?? null,
      // Under each thumbnail, "1. What it shows" (its file name when the thread gave no caption).
      caption: text(proof.querySelector("figcaption")),
    })),
    // The proofs are the Preview card's.
    inCard: !!proofs?.closest('[data-slot="preview-card"]'),
    title: text(document.querySelector('[data-slot="preview-title"]')),
    thumbnail: image ? [Math.round(image.width), Math.round(image.height)] : null,
    // Under the first request's answer, after it.
    underAnswer: !!answer && !!proofs && (answer.compareDocumentPosition(proofs) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0,
    // No row of its own in the work.
    rows: all('[data-slot="orchestrator-step"]').filter((row) => /readme\.png|install\.webm|attach/i.test(text(row))).length,
    read: [...new Set(read)].toSorted(),
    // What was checked is in the Progress tab, never listed in the thread.
    checkedInThread: !!document.querySelector('[data-slot="merge-checked"]') || /screenshots? or recordings?|Merged on its own/.test(text(document.body)),
  };
}

if (params.has("drive")) {
  void drive().then((seen) => {
    const result = document.createElement("pre");
    result.id = "merge-message-result";
    result.textContent = JSON.stringify(seen);
    document.body.append(result);
  });
}
