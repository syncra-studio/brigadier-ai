/**
 * The session tab's ⋯ "Chat actions" (issue 213): "New session" for a session in a project,
 * "Remove worktree…" only for a session with a worktree of its own, and its dialog's confirm
 * sends `closeWorkspace`. Driven with the browser's real clicks; IPC is mocked and
 * records the calls, no daemon.
 */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import type { ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";

import { ChatActions } from "@/app/conversation/ChatActions";
import { TooltipProvider } from "@/components/overlays/tooltip";
import { fixtureClick } from "@/fixtures/input";
import type { Conversation, Environment, Request, Setup } from "@/ipc/generated";
import { useApp } from "@/state/store";

const base: Conversation = {
  id: "worktree", kind: "session", projectId: "project", title: "Tidy the release notes",
  pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: null, lifecycle: "active",
  forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
};
const setup = (environment: Environment) => ({
  type: "session", repo: "/tmp/session-menu-fixture", environment, permission: "fullAccess", planMode: false,
  workersSeeUncommitted: null, orchestrator: { provider: "claude", model: null, effort: null },
}) as unknown as Setup;
const conversations: Conversation[] = [
  { ...base, setup: setup({ type: "newWorktree", base: "main", branch: "brigadier/session", path: "/tmp/session-menu-fixture/session", start: null }) },
  { ...base, id: "repo", setup: setup({ type: "localCheckout", branch: "main" }) },
  { ...base, id: "chat", kind: "chat", projectId: null },
];
useApp.setState({
  conversations: Object.fromEntries(conversations.map((conversation) => [conversation.id, conversation])),
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
});

const calls: unknown[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "closeWorkspace") {
    calls.push({ method: req.method, conversationId: req.conversationId });
    return { method: req.method, text: "Removed the worktree" };
  }
  return { method: req.method };
});

function Runtime({ children }: { children: ReactNode }) {
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  return <AssistantRuntimeProvider runtime={runtime}>{children}</AssistantRuntimeProvider>;
}

/** One session's ⋯ at a time, in a root of its own: each menu starts from a fresh page state. */
let root: Root | null = null;
function mount(conversation: Conversation) {
  root?.unmount();
  root = createRoot(document.getElementById("root")!);
  root.render(
    <TooltipProvider>
      <Runtime>
        <main className="bg-background text-foreground flex min-h-screen w-96 flex-col gap-4 p-8">
          <div data-fixture={conversation.id}>
            <ChatActions conversation={conversation} onRename={() => {}} compact />
          </div>
        </main>
      </Runtime>
    </TooltipProvider>,
  );
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(what: string, check: () => boolean, ms = 5000) {
  const end = performance.now() + ms;
  while (!check()) {
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(16);
  }
}
const seen: Record<string, unknown> = {};
/** The row's ⋯ button, named by its screen-reader words. */
const dots = (id: string) =>
  [...(document.querySelector(`[data-fixture="${id}"]`)?.querySelectorAll("button") ?? [])].find((button) => button.textContent?.trim() === "Chat actions");
const items = () => [...document.querySelectorAll("[role=menu] [role=menuitem]")];
async function open(id: string) {
  seen.step = `opening ${id}'s menu`;
  await fixtureClick(dots(id)!);
  await until("the menu", () => document.querySelector("[role=menu]") !== null);
}
/** The ⋯ menu's top-level items, opened with a real click. */
async function menu(conversation: Conversation) {
  mount(conversation);
  await until("the menu button", () => dots(conversation.id) !== undefined);
  await open(conversation.id);
  return items().map((item) => item.textContent?.trim());
}

try {
  const menus: Record<string, unknown> = {};
  for (const conversation of conversations) menus[conversation.id] = await menu(conversation);
  seen.menus = menus;
  await menu(conversations[0]!);
  await fixtureClick(items().find((item) => item.textContent?.trim() === "Remove worktree…")!);
  await until("the dialog", () => document.querySelector("[role=dialog]") !== null);
  seen.dialog = document.querySelector("[role=dialog] h2")?.textContent;
  const confirm = [...document.querySelectorAll("[role=dialog] button")].find((button) => button.textContent?.trim() === "Remove worktree")!;
  await fixtureClick(confirm);
  await until("the close", () => calls.length > 0);
  await until("the dialog to close", () => document.querySelector("[role=dialog]") === null);
} catch (error) {
  seen.error = String(error);
}
seen.calls = calls;
const result = document.createElement("pre");
result.id = "session-menu-result";
result.textContent = JSON.stringify(seen);
document.body.append(result);
