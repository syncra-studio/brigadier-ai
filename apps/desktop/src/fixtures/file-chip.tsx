/**
 * File links in an answer (BUILD-PLAN A1, Q84): a worker's file, linked as `outputs/…` or by
 * its removed scratch path, opens Brigadier's stored copy inside Brigadier, in the reader with
 * its Save: Markdown as the answer shows it, other text as written; an image opens in its own
 * app, and so does "Open in › Default app". No Save sits next to a link. A dead link with
 * nothing stored says "This file is no longer here". A chip in a message passes when the
 * message was written, so an `outputs/` link opens the task of its time. In a table's narrow
 * column, a chip's icon stays on its name's line. Driven with real clicks; IPC is mocked, no
 * daemon. `?stop=reader` holds the page with the reader open on rounding.md, for a screenshot.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { LinkConversationContext, LinkTimeContext } from "@/components/assistant-ui/file-links";
import { FileChip } from "@/components/assistant-ui/markdown-text";
import { Toaster } from "@/components/ui/toast";
import { TooltipProvider } from "@/components/ui/tooltip";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import type { ArtifactRef, Conversation, Request } from "@/ipc/generated";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const session = query.has("session");
const conversation: Conversation = {
  id: "chat-1", kind: session ? "session" : "chat", projectId: session ? "project-1" : null,
  title: "File links", pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, lifecycle: "active",
  forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
  setup: session ? {
    type: "session", repo: "/repo", environment: { type: "localCheckout", branch: "main" },
    permission: "fullAccess", orchestrator: { provider: "claude", model: null, effort: null },
    workersSeeUncommitted: null, planMode: false,
  } : null,
};
useApp.setState({ conversations: { [conversation.id]: conversation }, selection: { type: "conversation", id: conversation.id } });

const scratch = "/var/folders/xy/T/brigadier/scratch/task-1";
const links = {
  stored: `${scratch}/outputs/rounding.md:3`,
  lost: "/tmp/file-chip-fixture/never-kept.md",
  outputs: "outputs/task-1/rounding.md",
  dotOutputs: "./outputs/task-1/rounding.md",
  csv: "outputs/task-1/rates.csv",
  image: "outputs/task-1/chart.png",
};
const artifact = (id: string, fileName: string, mime: string): ArtifactRef => ({ id, title: fileName, kind: "note", mime, bytes: 10, fileName });
const stored: Record<string, ArtifactRef> = {
  "rounding.md": artifact("blob-1", "rounding.md", "text/markdown"),
  "rates.csv": artifact("blob-2", "rates.csv", "text/csv"),
  "chart.png": artifact("blob-3", "chart.png", "image/png"),
};
const contents: Record<string, string> = {
  "blob-1": "# Rounding\n\nTotals now round **half-even**, as the invoice test expects.",
  "blob-2": "currency,rate\nEUR,0.92\nMDL,17.8",
};

const calls: unknown[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") {
    calls.push({ command, ...(payload as object) });
    return command === "save_artifact" ? true : null;
  }
  const req = (payload as { request: Request }).request;
  if (req.method === "resolveFileLink") {
    calls.push({ method: req.method, conversationId: req.conversationId, link: req.link, path: req.path, atMs: req.atMs });
    const copy = req.link.startsWith(scratch) || /^(?:\.\/)?outputs\//.test(req.link) ? stored[req.path.split("/").at(-1)!] : undefined;
    return { method: req.method, exists: false, stored: copy ? { taskId: "task-1", artifact: copy } : null };
  }
  if (req.method === "readArtifact") {
    calls.push({ method: req.method, id: req.id });
    const text = contents[req.id] ?? "";
    return { method: req.method, text: { text, offset: 0, totalBytes: text.length, binary: !(req.id in contents) } };
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
    {/* The app's own toasts, so an error one would show (#49). */}
    <Toaster className="top-2" />
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
const chip = (id: string) => row(id)?.querySelector("[data-slot=file-chip]") as HTMLElement;
const notice = (id: string) => row(id).querySelector("[data-slot=file-chip-gone]")?.textContent ?? null;
const reader = () => document.querySelector<HTMLElement>("[data-slot=file-reader]");
const command = (call: unknown) => (call as { command?: string; method?: string }).command ?? (call as { method?: string }).method;

/** What the reader shows: its title, whether its body is Markdown or plain text, its words and its Save. */
function readerShows() {
  const open = reader()!;
  const body = open.querySelector("[data-slot=file-reader-body]")!;
  return {
    title: open.querySelector("h2")?.textContent ?? null,
    heading: body.querySelector("h1")?.textContent ?? null,
    bold: body.querySelector("strong")?.textContent ?? null,
    plain: body.querySelector("pre")?.textContent ?? null,
    save: open.querySelector("[data-slot=file-reader-save]")?.textContent ?? null,
  };
}

/** A menu item on screen, by its words. */
const item = (words: string) => [...document.querySelectorAll<HTMLElement>("[role=menu] [role=menuitem]")].find((el) => el.textContent === words);

async function closeReader() {
  for (let press = 1; press < 4; press += 1) {
    await fixtureInput({ type: "key", key: "Escape" });
    await sleep(400);
    if (!reader()) return press;
  }
  throw new Error(`reader stuck ${[...document.querySelectorAll("[data-slot=tooltip-content]")].map((el) => `${el.textContent}:${el.getAttribute("data-state")}`).join("|")}`);
}

const seen: Record<string, unknown> = {};
try {
  await until("the chips", () => Object.keys(links).every((id) => chip(id)));
  // The icon and the name's first line share a line: their middles within a few pixels.
  const cell = row("cell").querySelector("[data-slot=file-chip]")!;
  const icon = cell.querySelector("svg")!.getBoundingClientRect();
  const name = document.createRange();
  name.selectNodeContents([...cell.childNodes].find((node) => node.nodeType === Node.TEXT_NODE)!);
  const first = name.getClientRects()[0]!;
  seen.cellIconOnNameLine = Math.abs(icon.top + icon.height / 2 - (first.top + first.height / 2)) < 4;
  seen.before = Object.fromEntries(Object.keys(links).map((id) => [id, notice(id)]));
  seen.reader = {};
  for (const id of ["stored", "outputs", "dotOutputs", "csv"]) {
    await fixtureClick(chip(id));
    await until(`the reader on ${id}`, () => !!reader()?.querySelector("[data-slot=file-reader-body] :is(h1, pre)"));
    (seen.reader as Record<string, unknown>)[id] = { ...readerShows(), tooltip: !!document.querySelector("[data-slot=tooltip-content]:not([data-state=closed])") };
    if (id === "stored" && query.get("stop") === "reader") await new Promise(() => {});
    if (id === "stored") {
      await fixtureClick(reader()!.querySelector("[data-slot=file-reader-save]")!);
      await until("the save", () => calls.some((call) => command(call) === "save_artifact"));
    }
    ((seen.presses ??= []) as number[]).push(await closeReader());
  }
  // An image isn't text: it opens in its own app, with no reader.
  const before = calls.length;
  await fixtureClick(chip("image"));
  await until("the image to open", () => calls.slice(before).some((call) => command(call) === "open_artifact"));
  seen.imageReader = !!reader();
  await fixtureClick(chip("lost"));
  await until("the notice on lost", () => notice("lost") !== null);
  seen.after = Object.fromEntries(Object.keys(links).map((id) => [id, notice(id)]));
  // No Save next to any link: Save lives in the reader.
  seen.inlineSave = Object.keys(links).filter((id) => /Save/.test(row(id).textContent ?? ""));
  // Right-click › Open in › Default app opens a worker's file's stored copy in its own app.
  const menuBefore = calls.length;
  const box = chip("outputs").getBoundingClientRect();
  chip("outputs").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, clientX: box.x + 4, clientY: box.y + 4 }));
  await until("the menu", () => !!document.querySelector("[data-slot=file-chip-menu]"));
  item("Open in")!.focus();
  await fixtureInput({ type: "key", key: "ArrowRight" });
  await until("the Open in menu", () => !!item("Default app"));
  await fixtureClick(item("Default app")!);
  await until("the default app", () => calls.slice(menuBefore).some((call) => command(call) === "open_artifact"));
  seen.defaultApp = calls.slice(menuBefore).filter((call) => command(call) === "open_artifact");
  // An error toast would show by now: the failed open or save toasts as its call returns.
  await sleep(300);
  seen.toasts = document.querySelectorAll("[data-slot=toast][data-tone=error]").length;
} catch (error) {
  seen.error = String(error);
}
seen.calls = calls;
const result = document.createElement("pre");
result.id = "file-chip-result";
result.textContent = JSON.stringify(seen);
document.body.append(result);
