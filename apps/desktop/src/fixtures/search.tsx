/**
 * The right sidebar's Search over a mocked daemon: results streamed in batches (one before the
 * search's id is known), grouped by file with matches marked, a line opening its file, the
 * options asking again, the Replace field not searching, Replace all after its confirm with the
 * search's hashes, a refresh after it, a search asked again after the connection came back, an
 * invalid pattern's error, a cleared query, and no
 * Replace for a session whose own copy doesn't exist yet. Then many results: only those in view
 * drawn, the arrow keys moving through all of them, Collapse all, and Replace all for every file.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SearchPanel } from "@/app/side-panel/SearchPanel";
import type { Request, SearchFile } from "@/ipc/generated";
import { useApp } from "@/state/store";
import { emitSearchUpdate, runSearch } from "@/state/workspaceSearch";

const files: SearchFile[] = [
  { path: "src/app.ts", matches: 2, hash: "h-app", lines: [
    { line: 3, preview: "const foo = foo();", ranges: [{ start: 6, end: 9 }, { start: 12, end: 15 }] },
  ] },
  { path: "README.md", matches: 1, hash: "h-readme", lines: [{ line: 1, preview: "# foo", ranges: [{ start: 2, end: 5 }] }] },
];
/** Many results: 300 files of 5 matching lines each. */
const many: SearchFile[] = Array.from({ length: 300 }, (_file, index) => ({
  path: `src/many-${index}.ts`, matches: 5, hash: `h-${index}`,
  lines: Array.from({ length: 5 }, (_line, at) => ({ line: at + 1, preview: `many ${at}`, ranges: [{ start: 0, end: 4 }] })),
}));
const asked: Request[] = [];
let ids = 0;
/** While set, a search is started but never says what it found: the connection dropped. */
let silent = false;
const connection = (status: "connected" | "disconnected") =>
  useApp.setState({ connection: { status, daemon: null, reason: null } });
connection("connected");
mockIPC((command, args) => {
  if (command !== "ipc_request") return null;
  const req = (args as { request: Request }).request;
  asked.push(req);
  if (req.method === "startSearch") {
    const id = ++ids;
    if (req.query.regex && req.query.text === "(") throw { code: "invalid", message: "the search isn't a valid regular expression: unclosed group" };
    if (silent) return { method: req.method, id };
    if (req.query.text === "many") {
      setTimeout(() => {
        emitSearchUpdate(id, { type: "files", files: many });
        emitSearchUpdate(id, { type: "done", files: many.length, matches: many.length * 5, capped: false });
      }, 10);
      return { method: req.method, id };
    }
    // The first batch comes before the answer with the id; the rest after.
    emitSearchUpdate(id, { type: "files", files: [files[0]!] });
    setTimeout(() => {
      emitSearchUpdate(id, { type: "files", files: [files[1]!] });
      emitSearchUpdate(id, { type: "done", files: 2, matches: 3, capped: false });
    }, 30);
    return { method: req.method, id };
  }
  if (req.method === "replaceInFiles") {
    // The first replace goes through; the second meets a file changed meanwhile, and one it
    // couldn't put back.
    if (req.files.length < 10) return { method: req.method, outcome: { files: req.files.length, matches: req.files.reduce((sum, file) => sum + file.matches, 0), skipped: [], unrestored: [] } };
    return {
      method: req.method,
      outcome: {
        files: 0, matches: 0,
        skipped: [{ path: "src/many-7.ts", reason: "it changed while the files were being replaced" }],
        unrestored: [{ path: "src/many-3.ts", reason: "it changed while the files were being replaced; its text from before is in src/.many-3.ts.replaced-1" }],
      },
    };
  }
  return { method: req.method };
});

const session = { type: "conversation", conversationId: "search-fixture" } as const;
const waiting = { type: "conversation", conversationId: "search-waiting" } as const;
const crowded = { type: "conversation", conversationId: "search-many" } as const;

function until(check: () => boolean, timeoutMs = 3000): Promise<void> {
  const start = performance.now();
  return new Promise((resolve, reject) => {
    const tick = () => {
      if (check()) resolve();
      else if (performance.now() - start > timeoutMs) reject(new Error(`timed out at ${JSON.stringify(asked.map((r) => r.method))}`));
      else setTimeout(tick, 20);
    };
    tick();
  });
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const inPanel = (slot: string) => document.querySelector<HTMLElement>(`[data-panel="${slot}"]`)!;
const field = (slot: string, label: string) => inPanel(slot).querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)!;
const button = (slot: string, label: string) => [...inPanel(slot).querySelectorAll<HTMLButtonElement>("button")]
  .find((b) => b.getAttribute("aria-label") === label || b.textContent?.trim() === label || b.querySelector(".sr-only")?.textContent === label);
const summary = (slot = "session") => inPanel(slot).querySelector('[data-slot="search-summary"]')?.textContent ?? null;
const drawn = (slot: string) => [...inPanel(slot).querySelectorAll<HTMLElement>('[aria-label="Search results"] > li')];
/** Presses a key where the keyboard is, and says which row has it then. */
async function key(name: string): Promise<string | null> {
  document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true }));
  await sleep(80);
  return document.activeElement?.getAttribute("title") ?? null;
}
const starts = () => asked.filter((req) => req.method === "startSearch");

/** Types into a React-controlled field. */
function type(input: HTMLInputElement, value: string) {
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

function Fixture() {
  const [result, setResult] = useState("");
  const [opened, setOpened] = useState<unknown[]>([]);
  useEffect(() => {
    const seen: Record<string, unknown> = {};
    void (async () => {
      type(field("session", "Search"), "foo");
      await until(() => summary() === "3 results in 2 files");
      seen.starts = starts().length;
      seen.query = starts()[0]!.method === "startSearch" ? (starts()[0] as Extract<Request, { method: "startSearch" }>).query : null;
      const groups: { name: string | null | undefined; count: string | null | undefined; marks: (string | null)[] }[] = [];
      for (const row of drawn("session")) {
        if (row.dataset.row === "file") groups.push({
          name: row.querySelector("button[aria-expanded] span.truncate")?.textContent,
          count: row.querySelector("span.tabular-nums")?.textContent,
          marks: [],
        });
        else groups.at(-1)!.marks.push(...[...row.querySelectorAll("mark")].map((mark) => mark.textContent));
      }
      seen.groups = groups;
      inPanel("session").querySelector<HTMLButtonElement>('button[title="src/app.ts:3"]')!.click();
      await sleep(50);
      button("session", "Match case")!.click();
      await until(() => starts().length === 2);
      seen.matchCase = (starts()[1] as Extract<Request, { method: "startSearch" }>).query.matchCase;
      await until(() => summary() === "3 results in 2 files");
      type(field("session", "Replace"), "bar");
      await sleep(400);
      seen.replaceTypingSearched = starts().length !== 2;
      button("session", "Replace in this file")!.click();
      await until(() => !!document.querySelector('[role="dialog"]'));
      seen.fileConfirm = document.querySelector('[role="dialog"] h2')?.textContent;
      seen.noReplaceBeforeConfirm = !asked.some((req) => req.method === "replaceInFiles");
      [...document.querySelectorAll<HTMLButtonElement>('[role="dialog"] button')].find((b) => b.textContent === "Cancel")!.click();
      await until(() => !document.querySelector('[role="dialog"][data-state="open"]'));
      await sleep(100);
      const dismiss = button("session", "Dismiss")!;
      inPanel("session").querySelector<HTMLButtonElement>('button[title="src/app.ts"]')!.focus();
      await sleep(50);
      seen.dismissTabIndex = dismiss.tabIndex;
      dismiss.click();
      await until(() => summary() === "1 result in 1 file");
      seen.dismissedSummary = summary();
      seen.dismissedFocus = document.activeElement?.getAttribute("title");
      seen.dismissedRows = drawn("session").map((row) => row.querySelector("[data-row-button]")?.getAttribute("title"));
      button("session", "Replace all")!.click();
      await until(() => !!document.querySelector('[role="dialog"]'));
      seen.confirm = document.querySelector('[role="dialog"] h2')?.textContent;
      [...document.querySelectorAll<HTMLButtonElement>('[role="dialog"] button')].find((b) => b.textContent === "Replace")!.click();
      await until(() => asked.some((req) => req.method === "replaceInFiles"));
      const replace = asked.find((req) => req.method === "replaceInFiles") as Extract<Request, { method: "replaceInFiles" }>;
      seen.replace = { replacement: replace.replacement, files: replace.files, matchCase: replace.query.matchCase };
      await until(() => !!inPanel("session").querySelector('[role="status"]'));
      seen.outcome = inPanel("session").querySelector('[role="status"]')?.textContent;
      // The replace changed the checkout: the search runs again.
      await until(() => starts().length === 3);
      await until(() => summary() === "1 result in 1 file");
      seen.refreshDismissed = !inPanel("session").querySelector('[title="src/app.ts"]');
      button("session", "Dismiss")!.click();
      await sleep(80);
      seen.emptySummary = summary();
      seen.emptyFocus = document.activeElement?.getAttribute("aria-label");
      // An automatic refresh must distinguish hidden matches from no matches.
      silent = true;
      const refreshQuery = (starts().at(-1) as Extract<Request, { method: "startSearch" }>).query;
      runSearch(session, refreshQuery);
      await until(() => summary() === "Searching…");
      seen.dismissedStreaming = summary();
      emitSearchUpdate(ids, { type: "files", files });
      await sleep(50);
      seen.filteredStreaming = summary();
      emitSearchUpdate(ids, { type: "done", files: 2, matches: 3, capped: false });
      await until(() => summary()?.startsWith("All results dismissed") === true);
      seen.filteredDone = summary();
      runSearch(session, refreshQuery);
      await until(() => summary() === "Searching…");
      emitSearchUpdate(ids, { type: "done", files: 0, matches: 0, capped: false });
      await until(() => summary() === "No results");
      seen.emptyRefresh = summary();
      // The connection drops mid-search, which the new one doesn't carry on: it is asked again.
      silent = true;
      button("session", "Match case")!.click();
      await until(() => starts().length === 6);
      connection("disconnected");
      await sleep(50);
      silent = false;
      connection("connected");
      await until(() => starts().length === 7 && summary() === "3 results in 2 files");
      seen.reconnected = (starts()[6] as Extract<Request, { method: "startSearch" }>).query.matchCase;
      button("session", "Regular expression")!.click();
      type(field("session", "Search"), "(");
      await until(() => !!inPanel("session").querySelector('[role="alert"]'));
      seen.patternError = inPanel("session").querySelector('[role="alert"]')?.textContent;
      type(field("session", "Search"), "");
      await until(() => summary() === null && !inPanel("session").querySelector('[role="alert"]'));
      seen.cleared = true;
      seen.waitingReplace = !!field("waiting", "Replace");
      seen.waitingNote = inPanel("waiting").querySelector('[data-slot="own-copy-note"]')?.textContent ?? null;
      // Many results: only the rows in view are drawn.
      type(field("many", "Search"), "many");
      await until(() => summary("many") === "1,500 results in 300 files");
      seen.manyDrawn = drawn("many").length;
      // The keyboard moves through all of them, one row taking Tab.
      drawn("many")[0]!.querySelector<HTMLElement>("[data-row-button]")!.focus();
      seen.tabStops = inPanel("many").querySelectorAll('[aria-label="Search results"] [data-row-button][tabindex="0"]').length;
      seen.down = await key("ArrowDown");
      seen.end = await key("End");
      seen.endDrawn = drawn("many").length;
      seen.left = await key("ArrowLeft");
      seen.home = await key("Home");
      seen.collapse = (await key("ArrowLeft"), inPanel("many").querySelector('[title="src/many-0.ts"]')?.getAttribute("aria-expanded"));
      seen.expand = (await key("ArrowRight"), inPanel("many").querySelector('[title="src/many-0.ts"]')?.getAttribute("aria-expanded"));
      seen.intoLines = await key("ArrowRight");
      button("many", "Collapse all")!.click();
      await sleep(80);
      seen.collapsedRows = drawn("many").every((row) => row.dataset.row === "file");
      seen.collapsedSummary = summary("many");
      drawn("many")[0]!.querySelector<HTMLElement>("[data-row-button]")!.focus();
      await sleep(50);
      seen.deleteFocus = await key("Delete");
      seen.deleteSummary = summary("many");
      await key("End");
      seen.backspaceFocus = await key("Backspace");
      document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "Backspace", metaKey: true, bubbles: true, cancelable: true }));
      await sleep(80);
      seen.metaBackspaceFocus = document.activeElement?.getAttribute("title");
      // Replace all takes every file found, drawn or not.
      type(field("many", "Replace"), "few");
      button("many", "Replace all")!.click();
      await until(() => !!document.querySelector('[role="dialog"]'));
      seen.manyConfirm = document.querySelector('[role="dialog"] h2')?.textContent;
      [...document.querySelectorAll<HTMLButtonElement>('[role="dialog"] button')].find((b) => b.textContent === "Replace")!.click();
      await until(() => asked.filter((req) => req.method === "replaceInFiles").length === 2);
      seen.manyReplaced = (asked.filter((req) => req.method === "replaceInFiles")[1] as Extract<Request, { method: "replaceInFiles" }>).files.length;
      await until(() => !!inPanel("many").querySelector('[role="status"]'));
      seen.manyOutcome = inPanel("many").querySelector('[role="status"]')?.textContent;
      setResult(JSON.stringify(seen));
    })().catch((error: unknown) => setResult(JSON.stringify({ error: String(error), seen })));
  }, []);
  useEffect(() => {
    if (opened.length) document.body.dataset.opened = JSON.stringify(opened);
  }, [opened]);
  return <div className="bg-background text-foreground flex h-screen">
    <div data-panel="session" className="flex w-80 flex-col">
      <SearchPanel target={session} readOnly={false} onOpen={(...args) => setOpened((list) => [...list, args])} />
    </div>
    <div data-panel="waiting" className="flex w-80 flex-col">
      <SearchPanel target={waiting} readOnly onOpen={() => undefined} />
    </div>
    <div data-panel="many" className="flex w-80 flex-col">
      <SearchPanel target={crowded} readOnly={false} onOpen={() => undefined} />
    </div>
    {result && <pre id="search-result">{JSON.stringify({ ...JSON.parse(result), opened })}</pre>}
  </div>;
}

createRoot(document.getElementById("root")!).render(<Fixture />);
