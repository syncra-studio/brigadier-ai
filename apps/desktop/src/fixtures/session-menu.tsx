/**
 * The session tab's ⋯ "Session actions" (issue 213): Copy › has the path and the id the CLI uses; "Permanently delete" asks with the Archived page's confirm and sends
 * `delete`; ⌥⌘P still pins. Driven with the browser's real clicks and keys; IPC is mocked and
 * records the calls, no daemon.
 */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@/components/conversation";
import { mockIPC } from "@tauri-apps/api/mocks";
import type { ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";

import { ChatActions } from "@/app/shell/ChatActions";
import { DeleteDialog } from "@/app/shell/dialogs/DeleteDialog";
import { TooltipProvider } from "@/components/overlays";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import type { Conversation, Environment, Request, Setup } from "@/ipc/generated";
import { useApp } from "@/state/store";

const base: Conversation = {
  id: "worktree", kind: "session", projectId: "project", title: "Tidy the release notes",
  pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: null, lifecycle: "active",
  forkedFrom: null, sideOf: null, quotaWait: null,
};
const setup = (environment: Environment) => ({
  type: "session", repo: "/tmp/session-menu-fixture", environment, permission: "full", planMode: false,
  workersSeeUncommitted: null, orchestrator: { provider: "claude", model: null, effort: null },
}) as unknown as Setup;
const conversations: Conversation[] = [
  { ...base, setup: setup({ type: "newWorktree", base: "main", branch: "brigadier/session", path: "/tmp/session-menu-fixture/session", start: null }) },
  { ...base, id: "repo", setup: setup({ type: "localCheckout", branch: "main" }) },
  { ...base, id: "chat", kind: "chat", projectId: null },
];
/** A session with an id as the daemon makes them, for Copy session id. */
const real: Conversation = { ...conversations[0]!, id: "019a2b3c-4d5e-7f60-8a9b-0c1d362537be" };
conversations.push(real);
useApp.setState({
  conversations: Object.fromEntries(conversations.map((conversation) => [conversation.id, conversation])),
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
});

const calls: unknown[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "setPinned") {
    calls.push({ method: req.method, id: req.id, pinned: req.pinned });
    return { method: req.method, conversation: { ...useApp.getState().conversations[req.id]!, pinnedAtMs: req.pinned ? 1 : null } };
  }
  if (req.method === "previewDelete") return { method: req.method, branches: [], files: [] };
  if (req.method === "delete") {
    calls.push({ method: req.method, ids: req.ids });
    return { method: req.method, outcomes: req.ids.map((id) => ({ id, error: null, conversation: null })) };
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
        <DeleteDialog />
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
  [...(document.querySelector(`[data-fixture="${id}"]`)?.querySelectorAll("button") ?? [])].find((button) => ["Chat actions", "Session actions"].includes(button.textContent?.trim() ?? ""));
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

// What Copy writes, as the clipboard would hold it.
const copied: string[] = [];
Object.defineProperty(navigator, "clipboard", { value: { writeText: async (text: string) => void copied.push(text) } });

/** The open submenu's items, after a real click on `trigger` in the menu. */
async function submenu(trigger: string) {
  await fixtureClick(items().find((item) => item.textContent?.trim() === trigger)!);
  await until(`the ${trigger} submenu`, () => document.querySelectorAll("[role=menu]").length > 1);
  // Where it comes to rest: it slides in.
  await sleep(300);
  return [...[...document.querySelectorAll("[role=menu]")].at(-1)!.querySelectorAll("[role=menuitem]")];
}

try {
  const menus: Record<string, unknown> = {};
  const copies: Record<string, unknown> = {};
  for (const conversation of conversations.slice(0, 3)) {
    menus[conversation.id] = await menu(conversation);
    copies[conversation.id] = (await submenu("Copy")).map((item) => item.textContent?.trim());
  }
  seen.menus = menus;
  seen.copies = copies;
  await menu(real);
  for (const [index, name] of ["Copy session id", "Copy path"].entries()) {
    if (!document.querySelector("[role=menu]")) await open(real.id);
    const item = (await submenu("Copy")).find((each) => each.textContent?.trim() === name)!;
    seen.step = `clicking ${name}`;
    await fixtureClick(item);
    await until(`the copy of ${name}`, () => copied.length > index && document.querySelector("[role=menu]") === null);
  }
  seen.copied = copied;
  // ⌥⌘P pins the conversation whose ⋯ is on screen, with Pin gone from its menu.
  seen.step = "pressing ⌥⌘P";
  await fixtureInput({ type: "key", key: "p", modifiers: ["Alt", "Meta"] });
  await until("the pin", () => calls.length > 0);
  seen.step = "deleting";
  await menu(real);
  await fixtureClick(items().find((item) => item.textContent?.trim() === "Permanently delete")!);
  await until("the delete dialog", () => document.querySelector("[role=dialog]") !== null);
  seen.deleteDialog = document.querySelector("[role=dialog] h2")?.textContent;
  const remove = [...document.querySelectorAll("[role=dialog] button")].find((button) => button.textContent?.trim() === "Delete session")!;
  await fixtureClick(remove);
  await until("the delete", () => calls.length > 1);
} catch (error) {
  seen.error = String(error);
}
seen.calls = calls;
const result = document.createElement("pre");
result.id = "session-menu-result";
result.textContent = JSON.stringify(seen);
document.body.append(result);
