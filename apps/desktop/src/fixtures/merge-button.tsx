/**
 * The side panel's branch row with its Git actions (BUILD-PLAN A4): "Merge into ‹base›" shows only
 * for a worktree session with work to merge, and "Close workspace…" only with a workspace. Driven
 * with the browser's real clicks and keys; IPC is mocked and records the merge, no daemon.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { GitActions, type SessionWorkspace } from "@/app/conversation/GitActions";
import { TooltipProvider } from "@/components/ui/tooltip";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import type { Request } from "@/ipc/generated";

const workspace: SessionWorkspace = { path: "/tmp/merge-button-fixture/session", branch: "brigadier/session", base: "main" };
const rows: { id: string; workspace: SessionWorkspace | null; mergeable: boolean }[] = [
  { id: "with-work", workspace, mergeable: true },
  { id: "no-work", workspace, mergeable: false },
  { id: "no-workspace", workspace: null, mergeable: true },
];

const calls: unknown[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "mergeSession") {
    calls.push({ method: req.method, conversationId: req.conversationId });
    return { method: req.method, text: "Merged brigadier/session into main" };
  }
  if (req.method === "getGitState") return { method: req.method, state: null };
  return { method: req.method };
});

createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <main className="bg-background text-foreground flex min-h-screen w-96 flex-col gap-4 p-8">
      {rows.map((row) => (
        <div key={row.id} data-fixture={row.id}>
          <GitActions conversationId={row.id} workspace={row.workspace} mergeable={row.mergeable}>
            <span className="min-w-0 flex-1 truncate">brigadier/session</span>
          </GitActions>
        </div>
      ))}
    </main>
  </TooltipProvider>,
);

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(what: string, check: () => boolean, ms = 5000) {
  const end = performance.now() + ms;
  while (!check()) {
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(16);
  }
}
const row = (id: string) => document.querySelector(`[data-fixture="${id}"]`)!;
/** The row's ⋯ button, named by its screen-reader words. */
const dots = (id: string) =>
  [...(document.querySelector(`[data-fixture="${id}"]`)?.querySelectorAll("button") ?? [])].find((button) => button.textContent?.trim() === "Git actions");
const merges = (id: string) =>
  [...row(id).querySelectorAll("[data-slot=merge-session]")].map((button) => button.textContent?.trim());
/** The ⋯ menu's items, opened with a real click and closed with Esc. */
async function menu(id: string) {
  await fixtureClick(dots(id)!);
  await until("the Git actions menu", () => document.querySelector("[role=menu]") !== null);
  const items = [...document.querySelectorAll("[role=menu] [role=menuitem]")].map((item) => item.textContent?.trim());
  await fixtureInput({ type: "key", key: "Escape" });
  await until("the menu to close", () => document.querySelector("[role=menu]") === null);
  return items;
}

const seen: Record<string, unknown> = {};
try {
  await until("the rows", () => rows.every(({ id }) => dots(id) !== undefined));
  seen.buttons = Object.fromEntries(rows.map(({ id }) => [id, merges(id)]));
  seen.menus = { "with-work": await menu("with-work"), "no-workspace": await menu("no-workspace") };
  await fixtureClick(row("with-work").querySelector("[data-slot=merge-session]")!);
  await until("the merge", () => calls.length > 0);
} catch (error) {
  seen.error = String(error);
}
seen.calls = calls;
const result = document.createElement("pre");
result.id = "merge-button-result";
result.textContent = JSON.stringify(seen);
document.body.append(result);
