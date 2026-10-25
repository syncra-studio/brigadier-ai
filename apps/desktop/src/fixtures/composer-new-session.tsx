/**
 * A standalone Vite entry, excluded from the app entry and production build: a project's New
 * session opened from a conversation, as the sidebar's button does, IPC mocked, the repository
 * read taking a while. It opens once with nothing read of the repository yet and once again
 * after; then another project's from the new chat, and one whose checkout isn't on a branch, a
 * real problem the composer still tells. Each records the composer card's top, and what shows
 * under the card, on every frame from the click until a while after the read is in. What it saw
 * goes as JSON into `<pre id="composer-new-session-result">`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { model, overview } from "@/fixtures/agents";
import type { Conversation, ConversationView as View, Project, RepoInfo, Request } from "@/ipc/generated";
import { select } from "@/state/actions";
import { type Selection, useApp } from "@/state/store";

/** How long the repository read takes. */
const READ_MS = 600;
localStorage.clear();
const start = Date.now() - 600_000;
const projectOf = (id: string, name: string) =>
  ({ id, name, createdAtMs: start, repos: [{ path: `/work/${name}`, name }], prefs: {}, trust: [] }) as unknown as Project;
/** The repository at `path`: on main, but the detached one isn't on a branch. */
const repoAt = (path: string): RepoInfo => ({
  path, name: path.split("/").at(-1)!, currentBranch: path.endsWith("detached") ? null : "main", dirty: false,
  branches: [{ name: "main", commit: "a1", checkedOutAt: path.endsWith("detached") ? null : path }],
});
const conversation = {
  id: "c1", kind: "session", projectId: "p1", title: "Earlier work", pinnedAtMs: null, createdAtMs: start, updatedAtMs: start,
  lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null, setup: null,
} as unknown as Conversation;
const message = {
  id: "m1", conversationId: "c1", seq: 1, role: "user", text: "Fix the login form", blob: null, attachments: [], mentions: [],
  createdAtMs: start, model: null, requestId: "r1", parentId: null,
};

mockIPC(async (command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getRepoInfo":
      await new Promise((resolve) => setTimeout(resolve, READ_MS));
      return { method: req.method, repo: repoAt(req.path) };
    case "getConversation":
      return {
        method: req.method,
        view: {
          conversation, context: null, messages: { messages: [message], hasMore: false }, tasks: [], approvals: [], questions: [],
          requests: [], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [], compactions: [], decisions: [],
          queue: { items: [], paused: false }, run: "idle", runRequest: null, head: null, ratings: {}, streaming: null,
          notices: [], previews: [],
        } as unknown as View,
      };
    case "threadView":
      return { method: req.method, view: { entries: [], turns: [], workers: [] } };
    case "listFiles":
      return { method: req.method, files: [], truncated: false };
    default:
      return { method: req.method };
  }
});
const state = useApp.getState();
useApp.setState({
  conversations: { c1: conversation },
  projects: { p1: projectOf("p1", "todo-app"), p2: projectOf("p2", "api"), p3: projectOf("p3", "detached") },
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { status: "connected", daemon: null, reason: null },
  providers: { ...state.providers, view: { providers: [overview("claude", [model("opus", "Opus 5.5", { isDefault: true })])], sessions: [], fixtures: [] } },
});

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(what: string, check: () => boolean, ms = 10_000) {
  const end = performance.now() + ms;
  while (!check()) {
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(16);
  }
}
const card = () => document.querySelector<HTMLElement>("[data-slot=aui_composer-shell]");
const editor = () => card()?.querySelector("[contenteditable=true]") ?? null;
const bar = () => document.querySelector("[aria-label='Composer utility bar']");

/** A frame: the card's top in px, and what shows under the card. */
type Frame = { top: number; under: string };

/**
 * Opens `projectId`'s New session from what is shown, as the sidebar's button does, and
 * records the composer on each frame for `ms`.
 */
async function openNewSession(projectId: string, ms = READ_MS + 600): Promise<{ before: number; frames: Frame[] }> {
  const before = Math.round(card()!.getBoundingClientRect().top);
  const frames: Frame[] = [];
  let recording = true;
  const record = () => {
    const shell = card();
    if (shell) frames.push({ top: Math.round(shell.getBoundingClientRect().top), under: shell.nextElementSibling?.textContent ?? "" });
    if (recording) requestAnimationFrame(record);
  };
  void select({ type: "draft", kind: "session", projectId });
  requestAnimationFrame(record);
  await sleep(ms);
  recording = false;
  await sleep(50);
  return { before, frames };
}

/** The conversation, shown and settled. */
async function showConversation() {
  await select({ type: "conversation", id: "c1" });
  await until("the conversation's composer", () => editor() !== null && bar() === null);
  await sleep(300);
}

async function run() {
  const seen: Record<string, unknown> = {};
  await showConversation();
  // Nothing of the repository read yet: the first open this run.
  seen.cold = await openNewSession("p1");
  await showConversation();
  // Read once already: it is read again as the draft opens.
  seen.repeat = await openNewSession("p1");
  // From the new chat, the same view drawn on: another project, nothing of it read yet.
  await select({ type: "draft", kind: "chat" });
  await until("the new chat's composer", () => editor() !== null && bar() !== null);
  await sleep(300);
  seen.fromChat = await openNewSession("p2");
  // A checkout on no branch: a real problem, told once the read says so.
  await showConversation();
  seen.detached = await openNewSession("p3");
  return seen;
}

function Shown() {
  const selection = useApp((s) => s.selection);
  return <ConversationView key={viewKey(selection)} selection={selection} />;
}

/** As the app keys its view: one per conversation, one for every draft. */
function viewKey(selection: Selection): string {
  return selection.type === "conversation" ? selection.id : selection.type;
}

createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
      <main className="relative flex min-h-0 w-full flex-1 flex-col">
        <Shown />
      </main>
    </SidebarProvider>
  </TooltipProvider>,
);

void run()
  .catch((error: unknown) => ({ error: String(error) }))
  .then((seen) => {
    const out = document.createElement("pre");
    out.id = "composer-new-session-result";
    out.textContent = JSON.stringify(seen);
    document.body.append(out);
  });
