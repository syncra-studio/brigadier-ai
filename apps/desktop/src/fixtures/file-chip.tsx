/**
 * File links in an answer whose files are gone (BUILD-PLAN A1): a link into a worker's removed
 * scratch folder opens the copy Brigadier stored, and both dead links say "This file is no
 * longer here", the stored one with its Save. A chip in a message passes when the message was
 * written, so an `outputs/` link opens the task of its time. In a table's narrow column, a
 * chip's icon stays on its name's line. Driven with real clicks; IPC is mocked, no daemon.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { LinkConversationContext, LinkTimeContext } from "@/components/assistant-ui/file-links";
import { FileChip } from "@/components/assistant-ui/markdown-text";
import { TooltipProvider } from "@/components/ui/tooltip";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import type { Request } from "@/ipc/generated";

const scratch = "/var/folders/xy/T/brigadier/scratch/task-1";
const links = {
  stored: `${scratch}/outputs/rounding.md:3`,
  lost: "/tmp/file-chip-fixture/never-kept.md",
};
const stored = {
  taskId: "task-1",
  artifact: { id: "blob-1", title: "rounding.md", kind: "note", mime: "text/markdown", bytes: 10, fileName: "rounding.md" },
};

const calls: unknown[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") {
    calls.push({ command, ...(payload as object) });
    return command === "save_artifact" ? true : null;
  }
  const req = (payload as { request: Request }).request;
  if (req.method === "resolveFileLink") {
    calls.push({ method: req.method, conversationId: req.conversationId, link: req.link, atMs: req.atMs });
    return { method: req.method, exists: false, stored: req.link.startsWith(scratch) ? stored : null };
  }
  return { method: req.method };
});

createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <LinkConversationContext.Provider value="chat-1">
      <main className="bg-background text-foreground flex min-h-screen w-96 flex-col gap-24 p-8">
        {Object.entries(links).map(([id, href]) => (
          // Apart, so one chip's tooltip never covers the next.
          // The stored one sits in a message written at 1700 ms: its click says so.
          <LinkTimeContext.Provider key={id} value={id === "stored" ? 1700 : null}>
            <p data-fixture={id}>
              The list is in <FileChip href={href}>{href.split("/").at(-1)}</FileChip>.
            </p>
          </LinkTimeContext.Provider>
        ))}
        {/* A table's first column, narrower than the file's name (as an answer's table squeezes it). */}
        <table className="aui-md-table w-full border-collapse text-sm">
          <tbody>
            <tr>
              <td data-fixture="cell" className="aui-md-td w-16 px-3 py-2 align-top">
                <FileChip href="/repo/test/invoice.test.ts">test/invoice.test.ts</FileChip>
              </td>
              <td className="aui-md-td px-3 py-2 align-top">Expected values updated for half-even rounding.</td>
            </tr>
          </tbody>
        </table>
      </main>
    </LinkConversationContext.Provider>
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
const notice = (id: string) => row(id).querySelector("[data-slot=file-chip-gone]")?.textContent ?? null;

const seen: Record<string, unknown> = {};
try {
  await until("the chips", () => Object.keys(links).every((id) => row(id)?.querySelector("[data-slot=file-chip]")));
  // The icon and the name's first line share a line: their middles within a few pixels.
  const cell = row("cell").querySelector("[data-slot=file-chip]")!;
  const icon = cell.querySelector("svg")!.getBoundingClientRect();
  const name = document.createRange();
  name.selectNodeContents([...cell.childNodes].find((node) => node.nodeType === Node.TEXT_NODE)!);
  const first = name.getClientRects()[0]!;
  seen.cellIconOnNameLine = Math.abs(icon.top + icon.height / 2 - (first.top + first.height / 2)) < 4;
  seen.before = Object.fromEntries(Object.keys(links).map((id) => [id, notice(id)]));
  for (const id of Object.keys(links)) {
    await fixtureClick(row(id).querySelector("[data-slot=file-chip]")!);
    await until(`the notice on ${id}`, () => notice(id) !== null);
  }
  seen.after = Object.fromEntries(Object.keys(links).map((id) => [id, notice(id)]));
  seen.saves = Object.fromEntries(Object.keys(links).map((id) => [id, !!row(id).querySelector("[data-slot=file-chip-save]")]));
  // The pointer leaves the chips, so no chip's tooltip stands over the Save.
  await fixtureInput({ type: "pointer", x: 1, y: 1 });
  await until("the tooltips to close", () => !document.querySelector("[data-slot=tooltip-content]"));
  await fixtureClick(row("stored").querySelector("[data-slot=file-chip-save]")!);
  await until("the save", () => calls.some((call) => (call as { command?: string }).command === "save_artifact"));
  seen.toasts = document.querySelectorAll("[data-slot=toast]").length;
} catch (error) {
  seen.error = String(error);
}
seen.calls = calls;
const result = document.createElement("pre");
result.id = "file-chip-result";
result.textContent = JSON.stringify(seen);
document.body.append(result);
