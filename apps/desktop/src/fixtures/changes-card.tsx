/**
 * A standalone Vite entry, excluded from the app entry and production build: the Changes card
 * in each of its states, as the user's design has them (`Of3wE`). IPC is mocked, no daemon.
 *
 * - `merged-finding`: merged into main, six files (three shown, "Show 3 more files"), an open
 *   review finding in the footer row (R4.1 and its title), Revert and View changes.
 * - `collapsed`: the same, folded: the finding moves into the header's line.
 * - `all`: merged, no finding, every file shown ("Show less"); no footer.
 * - `confirm`: Revert asks in the footer: what happens to main, Cancel and "Revert 6 files".
 * - `reverting`: the revert sent: "Reverting…" in place of the footer's buttons.
 * - `reverted`: the session reads Reverted and git shows the revert: one line, folded,
 *   "Undone on main · 4:02 PM", Redo.
 * - `branch`: not merged yet, on its branch two commits ahead: Discard and Merge into main.
 * - `merge-only`: a turn that merged with nothing of its own counted: the merge's card.
 *
 * `?case=<id>` shows one case alone (for a screenshot); `?drive=1` writes what each case shows,
 * then clicks through it, to `<pre id="changes-card-result">` (ChangesCardRender.test.ts);
 * `?case=all&drive=fold` folds and opens the card from its header with real clicks (cardFold.ts).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";
import type { ReactNode } from "react";

import type { TurnMerge } from "@/app/conversation/activity/MergeMessage";
import type { Merged } from "@/app/conversation/activity/revert";
import { ChangesCard } from "@/app/conversation/ChangesCard";
import { ViewContext } from "@/app/conversation/viewContext";
import { driveFold } from "@/fixtures/cardFold";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, FileStat, MergeRecord, OrchestratorStep, Request, ReviewRun, UserRequest } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { sessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { useSettledTabs } from "@/state/tabStates";

const query = new URLSearchParams(location.search);
const CHAT = "changes-card";
const REVERTED = "changes-card-reverted";

const FILES: FileStat[] = [
  { path: "public/index.html", insertions: 11, deletions: 2, binary: false },
  { path: "src/server.js", insertions: 22, deletions: 5, binary: false },
  { path: "src/store.js", insertions: 5, deletions: 0, binary: false },
  { path: "src/dates.js", insertions: 30, deletions: 0, binary: false },
  { path: "test/dates.test.js", insertions: 18, deletions: 0, binary: false },
  { path: "README.md", insertions: 2, deletions: 0, binary: false },
];

const request = (id: string, conversationId = CHAT): UserRequest =>
  ({
    id, conversationId, preview: "Due dates", state: { type: "done" }, startedAtMs: 0, endedAtMs: 1, steeredInto: null, steeredAfter: null,
    undo: null, worked: [], quotaWait: false,
    changes: { settled: true, stat: { files: FILES, insertions: 88, deletions: 7 } },
  }) as unknown as UserRequest;

const record = (open: number): MergeRecord => ({
  head: "a1b2c3d4",
  checks: [{ command: "npm test", status: "exit 0", onHead: true }],
  reviews: [{ reviewer: "codex", model: null, from: "0000aaaa", to: "9999bbbb", state: { type: "findings", count: 2 }, fixed: 1, dismissed: 1 - open, open, onHead: true }],
  ci: null,
  proofs: 0,
  verifier: null,
  firstAuto: false,
});

const merged = (commit: string, open = 0): Merged => ({
  type: "merged", branch: "brigadier/due-dates", base: "main", commits: 2, askedIn: null, head: "a1b2c3d4", mergeCommit: commit, auto: false,
  requests: [], record: record(open),
});

/** When the reverted merge's revert landed: 4:02 PM. */
const REVERTED_AT = new Date(2026, 9, 19, 16, 2).getTime();

const MERGES = {
  finding: merged("f1f1f1f1", 1),
  clean: merged("c1c1c1c1"),
  // The session's latest merge: the one its Reverted state names.
  reverted: merged("e1e1e1e1"),
};

const review = {
  id: "review-4", conversationId: CHAT, tip: "9999bbbb", number: 4, state: { type: "findings", count: 2 }, findings: null, notify: { type: "thread" },
  startedAtMs: 0, endedAtMs: 1,
  items: [
    { id: "R4.1", severity: "P1", title: "A malformed request path can crash the server", location: "src/server.js:41", status: { type: "open", why: null }, high: true },
    { id: "R4.2", severity: "P3", title: "A stray console.log", location: null, status: { type: "fixed", task: null, why: null, checked: null }, high: false },
  ],
} as unknown as ReviewRun;

const step = (position: number, kind: OrchestratorStep["kind"]): OrchestratorStep => ({ requestId: null, atMs: 0, position, kind });
useBoard.setState({
  board: {
    ...emptyBoard(CHAT),
    loaded: true,
    requests: Object.fromEntries(["finding", "collapsed", "all", "confirm", "reverting", "reverted", "branch"].map((id) => [id, request(id)])),
    reviews: { [review.id]: review },
    orchestratorSteps: [step(1, MERGES.finding), step(2, MERGES.clean), step(3, MERGES.reverted)],
  },
});
useSettledTabs.setState({ byTab: { [CHAT]: { state: "merged", ahead: 2 }, [REVERTED]: { state: "reverted" } } });
useApp.setState({ selection: { type: "conversation", id: CHAT } });

const sent: string[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "sendMessage") {
    sent.push(req.text);
    const message = { id: `sent-${sent.length}`, conversationId: req.conversationId, seq: 100 + sent.length, role: "user", text: req.text, blob: null, createdAtMs: Date.now(), attachments: [], mentions: [], model: null, requestId: null, parentId: null };
    return { method: req.method, outcome: { type: "sent", message } };
  }
  // The reverted merge's revert, as git shows it; the other merges stand.
  if (req.method === "getMergeUndo") {
    return { method: req.method, undo: req.mergeCommit === MERGES.reverted.mergeCommit ? { revertedBy: "d0d0d0d0d0", revertedAtMs: REVERTED_AT } : null };
  }
  return { method: req.method };
});

const session = (id: string) =>
  ({
    id, kind: "session", title: "Due dates",
    setup: { type: "session", repo: "/w", environment: { type: "newWorktree", base: "main", branch: "three-small-improvements", path: null, start: null }, permission: "default", orchestrator: { provider: "claude", model: null }, workersSeeUncommitted: null, planMode: false },
  }) as unknown as Conversation;

const own = (merge: Merged): TurnMerge => ({ merge, own: true });

const CASES: { id: string; conversation: string; node: ReactNode }[] = [
  { id: "merged-finding", conversation: CHAT, node: <ChangesCard requestIds={["finding"]} merge={own(MERGES.finding)} /> },
  { id: "collapsed", conversation: CHAT, node: <ChangesCard requestIds={["collapsed"]} merge={own(MERGES.finding)} /> },
  { id: "all", conversation: CHAT, node: <ChangesCard requestIds={["all"]} merge={own(MERGES.clean)} /> },
  { id: "confirm", conversation: CHAT, node: <ChangesCard requestIds={["confirm"]} merge={own(MERGES.clean)} /> },
  { id: "reverting", conversation: CHAT, node: <ChangesCard requestIds={["reverting"]} merge={own(MERGES.clean)} /> },
  { id: "reverted", conversation: REVERTED, node: <ChangesCard requestIds={["reverted"]} merge={own(MERGES.reverted)} /> },
  { id: "branch", conversation: CHAT, node: <ChangesCard requestIds={["branch"]} /> },
  { id: "merge-only", conversation: CHAT, node: <ChangesCard requestIds={[]} merge={own(MERGES.clean)} /> },
];

const only = query.get("case");
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <div className="flex w-190 flex-col gap-6 p-6">
      {CASES.filter((each) => !only || each.id === only).map((each) => (
        <ViewContext.Provider key={each.id} value={{ selection: { type: "conversation", id: each.conversation }, conversation: session(each.conversation), embedded: false }}>
          <div data-case={each.id}>{each.node}</div>
        </ViewContext.Provider>
      ))}
    </div>
  </TooltipProvider>,
);

const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
const wait = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));
async function until(check: () => boolean, ms = 8000) {
  for (const end = performance.now() + ms; performance.now() < end && !check(); ) await frame();
  await wait(50);
}
const words = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, " ").trim() ?? null;
const at = (id: string) => document.querySelector<HTMLElement>(`[data-case="${id}"]`);
const button = (id: string, label: string) =>
  [...(at(id)?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find((each) => (each.getAttribute("aria-label") ?? words(each)) === label);

function look(id: string) {
  const card = at(id)?.querySelector<HTMLElement>("[data-slot=turn-diff]");
  return {
    state: card?.dataset.state ?? null,
    title: words(card?.querySelector("[data-slot=turn-diff-title]")),
    counts: words(card?.querySelector("[data-slot=turn-diff-counts]")),
    status: words(card?.querySelector("[data-slot=turn-diff-status]")),
    problem: words(card?.querySelector("[data-slot=turn-diff-problem]")),
    buttons: [...(card?.querySelectorAll("button") ?? [])]
      .filter((each) => !each.closest("[data-slot=turn-diff-file],[data-slot=merge-warning],[data-slot=merge-revert-confirm]") && each.dataset.slot !== "turn-diff-more")
      .map((each) => each.getAttribute("aria-label") ?? words(each)),
    files: [...(card?.querySelectorAll("[data-slot=turn-diff-file]") ?? [])].map((row) => [words(row.querySelector("[data-slot=turn-diff-dir]")), words(row.querySelector("[data-slot=turn-diff-name]")), words(row.querySelector("[data-slot=turn-diff-file-counts]"))]),
    more: words(card?.querySelector("[data-slot=turn-diff-more]")),
    warning: card?.querySelector("[data-slot=merge-warning]")
      ? [words(card.querySelector("[data-slot=merge-warning-title]")), words(card.querySelector("[data-slot=merge-warning-finding]")), words(card.querySelector("[data-slot=merge-warning-open]"))]
      : null,
    confirm: card?.querySelector("[data-slot=merge-revert-confirm]") ? words(card.querySelector("[data-slot=merge-revert-confirm]")) : null,
  };
}

const activeTab = () => {
  const tabs = sessionTabs(CHAT);
  const tab = tabs.tabs.find((each) => each.id === tabs.active);
  return tab?.kind === "review" ? { id: tab.id, target: tab.target } : tab ? { id: tab.id } : null;
};

/** The states that need a click to reach: folded, all files, Revert's question, the revert sent. */
async function reach() {
  await until(() => !!document.querySelector("[data-slot=turn-diff-title]"));
  button("collapsed", "Fold the files")?.click();
  at("all")?.querySelector<HTMLButtonElement>("[data-slot=turn-diff-more]")?.click();
  button("confirm", "Revert")?.click();
  button("reverting", "Revert")?.click();
  await wait(50);
  at("reverting")?.querySelector<HTMLButtonElement>("[data-slot=merge-revert-go]")?.click();
  await until(() => (!!only && only !== "reverting") || !!at("reverting")?.querySelector("[role=status]"));
  // The folded card's body closes as it animates.
  await until(() => !at("collapsed") || look("collapsed").files.length === 0);
  document.body.dataset.reached = "1";
}

async function drive() {
  const out: Record<string, unknown> = {};
  out.cases = Object.fromEntries(CASES.map((each) => [each.id, look(each.id)]));
  // A file row opens that file's part of the merge, in a tab.
  at("merged-finding")?.querySelector<HTMLButtonElement>("[data-slot=turn-diff-file] button")?.click();
  await wait(50);
  out.row = activeTab();
  button("merged-finding", "View changes")?.click();
  await wait(50);
  out.viewChanges = activeTab();
  at("merged-finding")?.querySelector<HTMLButtonElement>("[data-slot=merge-warning-open]")?.click();
  await wait(50);
  out.finding = activeTab()?.id ?? null;
  // Not merged: a row opens its part of the turn; Merge into main asks the thread.
  at("branch")?.querySelector<HTMLButtonElement>("[data-slot=turn-diff-file] button")?.click();
  await wait(50);
  out.branchRow = activeTab();
  at("branch")?.querySelector<HTMLButtonElement>("[data-slot=turn-diff-merge]")?.click();
  await until(() => sent.length >= 2);
  // Confirm's Cancel puts Revert back.
  button("confirm", "Cancel")?.click();
  await wait(50);
  out.cancelled = look("confirm").buttons;
  // The reverted card opens to its files.
  button("reverted", "Show the files")?.click();
  await wait(50);
  out.revertedOpen = look("reverted").files.length;
  // Redo asks the thread to revert the revert.
  button("reverted", "Redo")?.click();
  await until(() => sent.length >= 3);
  out.sent = sent;
  return out;
}

void reach()
  .then(() =>
    query.get("drive") === "1"
      ? drive()
      : query.get("drive") === "fold"
        ? driveFold(at("all")!.querySelector("[data-slot=turn-diff]")!, button("all", "View changes")!, () => look("all").files.length)
        : new Promise<never>(() => {}),
  )
  .catch((error: unknown) => ({ error: String(error) }))
  .then((result) => {
    const out = document.createElement("pre");
    out.id = "changes-card-result";
    out.textContent = JSON.stringify(result);
    document.body.append(out);
  });
