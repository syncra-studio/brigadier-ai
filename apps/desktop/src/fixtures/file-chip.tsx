/**
 * File links in an answer (BUILD-PLAN A1, Q84): a worker's file, linked as `outputs/…` or by
 * its removed scratch path, opens Brigadier's stored copy inside Brigadier, in the reader with
 * its Save: Markdown as the answer shows it, other text as written; a picture shows under its
 * link as an 80px tile, and the link opens it in the image viewer; "Open in › Default app" opens
 * the file in its own app. No Save sits next to a link. A dead link with
 * nothing stored says "This file is no longer here". A chip in a message passes when the
 * message was written, so an `outputs/` link opens the task of its time. In a table's narrow
 * column, a chip's icon stays on its name's line. Driven with real clicks; IPC is mocked, no
 * daemon. `?stop=reader` holds the page with the reader open on rounding.md, for a screenshot.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { LinkConversationContext, LinkTimeContext } from "@/components/conversation";
import { FileChip } from "@/components/conversation/markdown-text";
import { Toaster, TooltipProvider } from "@/components/overlays";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import type { ArtifactRef, Conversation, Request } from "@/ipc/generated";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const session = query.has("session");
const conversation: Conversation = {
  id: "chat-1", kind: session ? "session" : "chat", projectId: session ? "project-1" : null,
  title: "File links", pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, lifecycle: "active",
  forkedFrom: null, sideOf: null, quotaWait: null,
  setup: session ? {
    type: "session", repo: "/repo", environment: { type: "localCheckout", branch: "main" },
    permission: "full", orchestrator: { provider: "claude", model: null, effort: null },
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
  "first.png": artifact("blob-4", "first.png", "image/png"),
  "second.png": artifact("blob-5", "second.png", "image/png"),
  "early.png": artifact("blob-6", "early.png", "image/png"),
  "swapped.png": artifact("blob-7", "swapped.png", "image/png"),
};
const contents: Record<string, string> = {
  "blob-1": "# Rounding\n\nTotals now round **half-even**, as the invoice test expects.",
  "blob-2": "currency,rate\nEUR,0.92\nMDL,17.8",
};
// A 2×2 orange PNG: chart.png's stored bytes.
const CHART = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGP4v5QBiBggFAAx+gaR3bxTGwAAAABJRU5ErkJggg==";

// Reads of a picture held until the page lets them end, so a slow read is a fact, not a race.
function gate() {
  const held = { open: () => undefined as void };
  const shut = new Promise<void>((resolve) => {
    held.open = resolve;
  });
  return { shut, open: () => held.open() };
}
const gates: Record<string, ReturnType<typeof gate>> = { "blob-4": gate(), "blob-6": gate() };
// The list's and the early click's pictures: their lookups aren't part of the calls checked.
const quiet = /(?:first|second|early|swapped)\.png$/;
// The worker stores swapped.png again: its link resolves to the new copy from then on.
let swapped = false;
// The pictures' reads, in order.
const reads: string[] = [];

const calls: unknown[] = [];
mockIPC(async (command, payload) => {
  if (command !== "ipc_request") {
    calls.push({ command, ...(payload as object) });
    return command === "save_artifact" ? true : null;
  }
  const req = (payload as { request: Request }).request;
  if (req.method === "resolveFileLink") {
    if (!quiet.test(req.link)) calls.push({ method: req.method, conversationId: req.conversationId, link: req.link, path: req.path, atMs: req.atMs });
    const name = req.path.split("/").at(-1)!;
    const copy = swapped && name === "swapped.png" ? artifact("blob-8", "swapped.png", "image/png") : req.link.startsWith(scratch) || /^(?:\.\/)?outputs\//.test(req.link) ? stored[name] : undefined;
    return { method: req.method, exists: false, stored: copy ? { taskId: "task-1", artifact: copy } : null };
  }
  if (req.method === "readAttachment") {
    reads.push(req.id);
    await gates[req.id]?.shut;
    if (req.id === "blob-3") calls.push({ method: req.method, id: req.id });
    return { method: req.method, data: CHART };
  }
  if (req.method === "readArtifact") {
    calls.push({ method: req.method, id: req.id });
    const text = contents[req.id] ?? "";
    return { method: req.method, text: { text, offset: 0, totalBytes: text.length, binary: !(req.id in contents) } };
  }
  return { method: req.method };
});

// An answer still streaming: words come after the list item's pictures once they show.
let streamOn: (() => void) | null = null;
function Streamed() {
  const [more, setMore] = useState(false);
  useEffect(() => {
    streamOn = () => setMore(true);
  }, []);
  return (
    <ul className="aui-md-ul">
      <li data-fixture="list">
        See <FileChip href="outputs/task-1/first.png">first.png</FileChip> and <FileChip href="outputs/task-1/second.png">second.png</FileChip>
        {more && " for details."}
      </li>
    </ul>
  );
}

createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <LinkConversationContext.Provider value="chat-1">
      <main className="bg-background text-foreground flex min-h-screen w-96 flex-col gap-24 p-8">
        {/* Beside the rest, in view, and out of their layout. */}
        <aside className="fixed top-8 left-112 flex w-72 flex-col gap-8">
          <Streamed />
          <p data-fixture="early">
            Here it is: <FileChip href="outputs/task-1/early.png">early.png</FileChip>.
          </p>
          <p data-fixture="swapped">
            Redrawn: <FileChip href="outputs/task-1/swapped.png">swapped.png</FileChip>.
          </p>
        </aside>
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
const viewer = () => document.querySelector("[data-slot=image-preview]");
const opened = () => calls.filter((call) => (call as { command?: string }).command === "open_artifact").length;

/** Closes the image viewer, and waits for its morph back into the tile to end. */
async function closeViewer() {
  await fixtureClick(document.querySelector<HTMLElement>("[aria-label='Close image preview']")!);
  await until("the viewer to close", () => !viewer() && document.body.style.pointerEvents !== "none" && !document.documentElement.matches(":active-view-transition"));
}
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
  // The list's second tile shows on its own and moves the early link down: clicked only once it did.
  await until("the second tile", () => !!document.querySelector("[data-fixture=list] [data-slot=file-chip-picture]"));
  // A picture clicked before its read ends opens in the image viewer once it ends, never elsewhere.
  await fixtureClick(document.querySelector<HTMLElement>("[data-fixture=early] [data-slot=file-chip]")!);
  await sleep(100);
  seen.earlyBefore = { viewer: !!viewer(), opened: opened() };
  gates["blob-6"]!.open();
  await until("the early picture's viewer", () => !!viewer());
  seen.earlyViewer = viewer()!.querySelector("img")?.getAttribute("alt") ?? null;
  seen.earlyOpened = opened();
  await closeViewer();
  // In a list item the second picture's read ends first, yet the tiles keep their links' order;
  // words streamed in after them stay in the sentence, the tiles after it.
  const list = document.querySelector<HTMLElement>("[data-fixture=list]")!;
  const listTiles = () => [...list.querySelectorAll("[data-slot=file-chip-picture] img")].map((img) => img.getAttribute("alt"));
  await until("the second tile", () => listTiles().length === 1);
  gates["blob-4"]!.open();
  await until("both tiles", () => listTiles().length === 2);
  seen.listOrder = listTiles();
  streamOn!();
  await until("the streamed words", () => !!list.textContent?.includes("for details."));
  await sleep(50);
  const shelf = list.querySelector("[data-slot=file-chip-pictures]");
  seen.listShelfLast = list.lastChild === shelf;
  seen.listSentence = [...list.childNodes].filter((node) => node !== shelf).map((node) => node.textContent).join("");
  // An output stored again since its tile showed: the link reads the new copy and the viewer and
  // the tile show it.
  const swappedRow = document.querySelector<HTMLElement>("[data-fixture=swapped]")!;
  const swappedTile = () => swappedRow.nextElementSibling?.querySelector<HTMLImageElement>("[data-slot=file-chip-picture] img") ?? null;
  await until("the swapped tile", () => !!swappedTile());
  const firstUrl = swappedTile()!.src;
  swapped = true;
  await fixtureClick(swappedRow.querySelector<HTMLElement>("[data-slot=file-chip]")!);
  await until("the swapped picture's viewer", () => !!viewer());
  const shownUrl = viewer()!.querySelector("img")?.src;
  seen.swapped = { reads: reads.filter((id) => id === "blob-7" || id === "blob-8"), newUrl: shownUrl !== firstUrl, tileShowsIt: swappedTile()?.src === shownUrl, opened: opened() };
  await closeViewer();
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
  // A picture shows under its link as a tile, its stored bytes read once; the link opens the
  // image viewer, never the reader or another app.
  // Its tiles sit right after the link's paragraph, which reads on unbroken.
  const picture = () => row("image").nextElementSibling?.querySelector<HTMLElement>("[data-slot=file-chip-picture] button") ?? null;
  await until("the picture's tile", () => !!picture()?.querySelector("img")?.complete && picture()!.querySelector("img")!.naturalWidth > 0);
  const tileBox = picture()!.getBoundingClientRect();
  const linkBox = chip("image").getBoundingClientRect();
  seen.pictureTile = { width: tileBox.width, height: tileBox.height, below: tileBox.top >= linkBox.bottom, label: picture()!.getAttribute("aria-label") };
  seen.otherTiles = Object.keys(links).filter((id) => id !== "image" && row(id).nextElementSibling?.matches("[data-slot=file-chip-pictures]"));
  seen.sentence = row("image").textContent;
  const before = calls.length;
  await fixtureClick(chip("image"));
  await until("the image viewer", () => !!viewer());
  seen.viewerImage = viewer()!.querySelector("img")?.getAttribute("alt") ?? null;
  seen.imageReader = !!reader();
  seen.imageOpenedElsewhere = calls.slice(before).some((call) => command(call) === "open_artifact");
  await closeViewer();
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
