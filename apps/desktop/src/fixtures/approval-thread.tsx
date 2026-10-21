/**
 * A standalone Vite entry, excluded from the app entry and production build: the thread's decided
 * approval cards (ApprovalCardView) of every kind, as the user's design has them (app.pen #352
 * ApprovalCard, #381 ApprovalCardView): a command outside the sandbox with its cwd, an outward
 * command, a landing, an action, an outline, a peer message, a browser action, and a card with no
 * subtitle and no body. IPC is mocked, no daemon.
 *
 * `?case=<id>` shows one card alone (for a screenshot); `?drive=1` writes each card's header, body
 * and footer measures to `<pre id="approval-thread-result">` (ApprovalThreadRender.test.ts).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ApprovalCardView } from "@/app/conversation/cards/ApprovalCardView";
import { TooltipProvider } from "@/components/overlays";
import type { ApprovalRequest, Task } from "@/ipc/generated";
import { emptyBoard, type ShownApproval, useBoard } from "@/state/board";

const conversationId = "approval-thread";
const query = new URLSearchParams(location.search);

const request = (id: string, change: Partial<ApprovalRequest>): ApprovalRequest => ({
  id, kind: "command", tool: "Bash", command: null, cwd: null, paths: [], reason: null, escalation: false, input: null, grant: null, ...change,
});

const STAT = {
  files: [
    { path: "src/importer/writer.ts", insertions: 12, deletions: 3, binary: false },
    { path: "src/importer/reader.ts", insertions: 6, deletions: 1, binary: false },
    { path: "docs/importer/diagram.png", insertions: 0, deletions: 0, binary: true },
  ],
  insertions: 18,
  deletions: 4,
};

/** The cards, in the design's order: what each asked and how it was decided. */
const CARDS: [string, ShownApproval["subject"], ShownApproval["state"], string | null][] = [
  [
    "command",
    { type: "cli", request: request("command", { command: "pnpm --filter importer run sync --target staging", paths: ["/tmp/importer-cache"], cwd: "/Users/me/importer-staging", reason: "Runs the importer against the staging copy", escalation: true }) },
    { type: "allowed", by: "user", similar: false },
    "t1",
  ],
  ["outward", { type: "outwardCommand", argv: ["git", "push", "origin", "fix/importer-retries"], cwd: "/Users/me/importer" }, { type: "denied", by: "user", message: null }, "t1"],
  ["landing", { type: "landing", taskId: "t1", branch: "main", diffStat: STAT }, { type: "allowed", by: "user", similar: false }, null],
  ["action", { type: "action", action: "Clear the preview cache?", details: "They take 3.2 GB in ~/Library/Caches/brigadier/previews and are made again when needed." }, { type: "allowed", by: "user", similar: false }, "t1"],
  ["outline", { type: "outline", taskId: "t1", title: "Fix importer retries", outline: "1. Split the reader and the writer.\n2. Move the retry loop into the writer.\n3. Test a dropped connection." }, { type: "expired", reason: "the session ended" }, "t1"],
  ["peer", { type: "peerMessage", messageId: "m-1", to: "c-2", title: "Math helper", project: "Alpha", text: "What is 9 + 10?" }, { type: "allowed", by: "user", similar: false }, null],
  [
    "browser",
    { type: "browser", tab: "browser:agent-1", url: "https://shop.example/checkout", generation: 3, sensitive: "pay", question: "Pay on shop.example?", details: "Clicks button “Pay $5”.\nOn https://shop.example/checkout", fingerprint: "click\u001fe4" },
    { type: "allowed", by: "user", similar: true },
    null,
  ],
  ["bare", { type: "cli", request: request("bare", { tool: "Bash" }) }, { type: "allowed", by: "policy", similar: false }, null],
];

const approvals = Object.fromEntries(
  CARDS.map(([id, subject, state, taskId]): [string, ShownApproval] => [
    id,
    { id, conversationId, taskId, requestId: null, position: 0, state, createdAtMs: 0, resolvedAtMs: 1, subject },
  ]),
);
const tasks = {
  t1: { id: "t1", conversationId, number: 1, title: "Fix importer retries", state: "running", requestId: null, position: 1, kind: "implement", role: "worker", attempts: [], messages: [], createdAtMs: 0, updatedAtMs: 0 } as unknown as Task,
};
useBoard.setState({ board: { ...emptyBoard(conversationId), loaded: true, approvals, tasks } });
mockIPC((command, payload) => (command === "ipc_request" ? { method: (payload as { request: { method: string } }).request.method } : null));

const only = query.get("case");
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <div className="flex w-190 flex-col gap-6 p-6">
      {CARDS.filter(([id]) => !only || id === only).map(([id]) => (
        <div key={id} data-case={id}>
          <ApprovalCardView cardId={id} />
        </div>
      ))}
    </div>
  </TooltipProvider>,
);

const wait = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));
const px = (value: string) => Number.parseFloat(value);

/** A card's sections as the design measures them: padding, the body's gap and each part's inset from the card's edge. */
function measure(id: string) {
  const card = document.querySelector<HTMLElement>(`[data-case="${id}"] [data-card=approval]`);
  if (!card) return null;
  const left = card.getBoundingClientRect().left + card.clientLeft;
  const pad = (element: Element | null) => {
    if (!element) return null;
    const style = getComputedStyle(element);
    return [px(style.paddingTop), px(style.paddingRight), px(style.paddingBottom), px(style.paddingLeft)];
  };
  const header = card.querySelector("[data-slot=approval-card-header]");
  const body = card.querySelector<HTMLElement>("[data-slot=approval-card-body]");
  const footer = card.querySelector<HTMLElement>("[data-slot=approval-card-footer]");
  const footerStyle = footer ? getComputedStyle(footer) : null;
  return {
    header: pad(header),
    body: pad(body),
    gap: body ? px(getComputedStyle(body).rowGap) : null,
    // Every part the body holds starts where the body's padding ends.
    insets: [...new Set([...(body?.children ?? [])].map((child) => Math.round(child.getBoundingClientRect().left - left)))],
    footer: pad(footer),
    // The footer is as tall as its words: no height of its own.
    footerFits: !!footer && !!footerStyle && Math.round(footer.getBoundingClientRect().height - px(footerStyle.paddingTop) - px(footerStyle.paddingBottom)) === Math.round(px(footerStyle.lineHeight)),
    bodyHeight: body ? Math.round(body.getBoundingClientRect().height) : null,
  };
}

async function drive() {
  await wait(100);
  return Object.fromEntries(CARDS.map(([id]) => [id, measure(id)]));
}

if (query.get("drive") === "1") {
  void drive()
    .catch((error: unknown) => ({ error: String(error) }))
    .then((result) => {
      const out = document.createElement("pre");
      out.id = "approval-thread-result";
      out.textContent = JSON.stringify(result);
      document.body.append(out);
    });
}
