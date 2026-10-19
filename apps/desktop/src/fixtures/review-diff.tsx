/**
 * A standalone Vite entry, excluded from the app entry and production build: the Review tab's
 * diff of one file loaded whole, measured through each of its toggles. IPC is mocked, no daemon.
 * The result goes to `<pre id="review-diff-result">`; `stop=1` holds the page for a screenshot.
 *
 * - Unified: the file's numbered lines, its unmodified stretches folded with a count, a fold
 *   opened in place.
 * - Word diffs: the words a replaced line changed, marked.
 * - Wrap: a long line scrolls, then wraps.
 * - Split: old and new side by side.
 * - Add to chat: the "+" by a hovered line's number puts the line, quoted, into the composer.
 * - `warm=1`: what the app warms at idle loaded first; the diff then draws in its colours at
 *   once, never as plain lines first.
 */
import { AssistantRuntimeProvider, type ThreadMessage, useAui, useExternalStoreRuntime } from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { ReviewTab } from "@/app/conversation/ReviewTab";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, Request, ReviewDiff } from "@/ipc/generated";
import { setReviewOption } from "@/state/review";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const CHAT = "review-diff";
const ROOT = "/repo";

/** A 30-line file whose 3rd and 25th lines change, loaded whole (one hunk over all of it). */
const before = Array.from({ length: 30 }, (_, index) => `export const value${index + 1} = ${index + 1};`);
const after = before.map((line, index) =>
  index === 2 ? "export const value3 = Math.round(3 * 100) / 100;" : index === 24 ? `export const value25 = 25; // ${"a very long comment ".repeat(12)}` : line,
);
const patch =
  "diff --git a/src/values.ts b/src/values.ts\n--- a/src/values.ts\n+++ b/src/values.ts\n@@ -1,30 +1,30 @@\n" +
  before.map((line, index) => (line === after[index] ? ` ${line}\n` : `-${line}\n+${after[index]}\n`)).join("");
const review: ReviewDiff = {
  scope: { type: "uncommitted" },
  files: [{ path: "src/values.ts", from: null, status: "modified", insertions: 2, deletions: 2, binary: false }],
  insertions: 2,
  deletions: 2,
  patch,
  fullFiles: true,
  commits: [],
  branch: "main",
  base: null,
} as ReviewDiff;

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "getReviewDiff") return { method: req.method, review };
  if (req.method === "listFiles") return { method: req.method, files: ["src/values.ts"], truncated: false };
  return { method: req.method };
});
useApp.setState((state) => ({
  conversations: {
    [CHAT]: {
      id: CHAT, kind: "session", title: "Review", projectId: null, pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, lifecycle: "active",
      forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
      setup: { type: "session", repo: ROOT, environment: { type: "local" }, permission: "fullAccess", planMode: false, workersSeeUncommitted: null, orchestrator: { provider: "codex", model: null, effort: null } },
    } as unknown as Conversation,
  },
  selection: { type: "conversation", id: CHAT },
  settings: state.settings,
}));

const composer = { text: () => "" };
function ComposerProbe() {
  const aui = useAui();
  useEffect(() => {
    composer.text = () => aui.composer().getState().text;
  }, [aui]);
  return null;
}

function Fixture() {
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <ComposerProbe />
      <ReviewTab conversationId={CHAT} target={{ type: "changes", staged: false }} />
    </AssistantRuntimeProvider>
  );
}

const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
const wait = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));
const roots = () => [...document.querySelectorAll("diffs-container")].flatMap((host) => (host.shadowRoot ? [host.shadowRoot] : []));
const inside = <T extends Element = HTMLElement>(selector: string) => roots().flatMap((root) => [...root.querySelectorAll<T>(selector)]);
async function until(check: () => boolean, ms = 8000) {
  for (const end = performance.now() + ms; performance.now() < end && !check(); ) await frame();
  await wait(150);
}

function look() {
  const pre = inside("pre")[0];
  return {
    lines: inside("[data-content] > [data-line]").length,
    numbers: inside("[data-gutter] [data-line-number-content]").map((cell) => cell.textContent?.trim()),
    folds: inside("[data-gutter] [data-unmodified-lines]").map((cell) => cell.textContent?.trim()),
    marks: inside("[data-diff-span]").map((span) => span.textContent),
    split: inside("[data-code]").length,
    overflow: pre?.getAttribute("data-overflow") ?? null,
    coloured: inside("[data-content] span[style]").length > 0,
  };
}

async function measure() {
  const out: Record<string, unknown> = {};
  await until(() => look().coloured);
  out.unified = look();
  // A folded stretch opens in place.
  inside("[data-expand-button]")[0]?.dispatchEvent(new MouseEvent("click", { bubbles: true, composed: true }));
  await until(() => look().lines > (out.unified as { lines: number }).lines, 3000);
  out.expanded = look();
  setReviewOption("wrap", true);
  await until(() => look().overflow === "wrap", 3000);
  out.wrapped = look();
  setReviewOption("wrap", false);
  setReviewOption("split", true);
  await until(() => look().split === 2, 3000);
  out.split = look();
  setReviewOption("split", false);
  await until(() => look().split === 1, 3000);
  // Add the changed line 3 to the chat: hover it, press the "+" by its number.
  const line = inside("[data-content] > [data-line='3'][data-line-type=change-addition]")[0];
  for (const type of ["pointerover", "pointerenter", "pointermove"]) line?.dispatchEvent(new PointerEvent(type, { bubbles: true, composed: true, pointerType: "mouse" }));
  await until(() => inside("[data-utility-button]").length > 0, 2000);
  const plus = inside("[data-utility-button]")[0];
  out.plus = !!plus;
  for (const type of ["pointerdown", "pointerup", "click"]) plus?.dispatchEvent(new PointerEvent(type, { bubbles: true, composed: true, pointerType: "mouse", button: 0 }));
  await until(() => composer.text() !== "", 2000);
  out.composer = composer.text();
  return out;
}

/** Warmed first: whether the plain lines ever showed, and whether the first lines drawn were coloured. */
async function warmed() {
  let plain = false;
  const watch = new MutationObserver(() => {
    plain ||= document.querySelector('[data-slot="diff-plain"]') !== null;
  });
  watch.observe(document.body, { childList: true, subtree: true });
  render();
  let first: boolean | null = null;
  for (const end = performance.now() + 8000; performance.now() < end && first === null; ) {
    // Checked on each task, not each frame: the lines arrive within one.
    if (inside("[data-content] > [data-line]").length > 0) first = look().coloured;
    else await wait(1);
  }
  watch.disconnect();
  return { plain, firstColoured: first };
}

function render() {
  createRoot(document.getElementById("root")!).render(
    <TooltipProvider>
      <Fixture />
    </TooltipProvider>,
  );
}

const run = query.get("warm") === "1" ? import("@/lib/warmCode").then((module) => module.warmCode()).then(warmed) : (render(), measure());

void run
  .catch((error: unknown) => ({ error: String(error) }))
  .then(async (result) => {
    if (query.get("stop") === "1") await new Promise(() => {});
    const out = document.createElement("pre");
    out.id = "review-diff-result";
    out.textContent = JSON.stringify(result);
    document.body.append(out);
  });
