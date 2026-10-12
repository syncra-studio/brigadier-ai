/**
 * The right sidebar's Search over a mocked daemon: results streamed in batches (one before the
 * search's id is known), grouped by file with matches marked, a line opening its file, the
 * options asking again, the Replace field not searching, Replace all after its confirm with the
 * search's hashes, a refresh after it, a search asked again after the connection came back, an
 * invalid pattern's error, a cleared query, and no
 * Replace for a session whose own copy doesn't exist yet.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SearchPanel } from "@/app/conversation/SearchPanel";
import type { Request, SearchFile } from "@/ipc/generated";
import { useApp } from "@/state/store";
import { emitSearchUpdate } from "@/state/workspaceSearch";

const files: SearchFile[] = [
  { path: "src/app.ts", matches: 2, hash: "h-app", lines: [
    { line: 3, preview: "const foo = foo();", ranges: [{ start: 6, end: 9 }, { start: 12, end: 15 }] },
  ] },
  { path: "README.md", matches: 1, hash: "h-readme", lines: [{ line: 1, preview: "# foo", ranges: [{ start: 2, end: 5 }] }] },
];
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
    // The first batch comes before the answer with the id; the rest after.
    emitSearchUpdate(id, { type: "files", files: [files[0]!] });
    setTimeout(() => {
      emitSearchUpdate(id, { type: "files", files: [files[1]!] });
      emitSearchUpdate(id, { type: "done", files: 2, matches: 3, capped: false });
    }, 30);
    return { method: req.method, id };
  }
  if (req.method === "replaceInFiles")
    return { method: req.method, outcome: { files: 1, matches: 2, skipped: [{ path: "README.md", reason: "it changed since the search" }] } };
  return { method: req.method };
});

const session = { type: "conversation", conversationId: "search-fixture" } as const;
const waiting = { type: "conversation", conversationId: "search-waiting" } as const;

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
const summary = () => inPanel("session").querySelector('[data-slot="search-summary"]')?.textContent ?? null;
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
      seen.groups = [...inPanel("session").querySelectorAll('[aria-label="Search results"] > li')].map((li) => ({
        name: li.querySelector("button[aria-expanded] span.truncate")?.textContent,
        count: li.querySelector("span.tabular-nums")?.textContent,
        marks: [...li.querySelectorAll("mark")].map((mark) => mark.textContent),
      }));
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
      await until(() => summary() === "3 results in 2 files");
      // The connection drops mid-search, which the new one doesn't carry on: it is asked again.
      silent = true;
      button("session", "Match case")!.click();
      await until(() => starts().length === 4);
      connection("disconnected");
      await sleep(50);
      silent = false;
      connection("connected");
      await until(() => starts().length === 5 && summary() === "3 results in 2 files");
      seen.reconnected = (starts()[4] as Extract<Request, { method: "startSearch" }>).query.matchCase;
      button("session", "Regular expression")!.click();
      type(field("session", "Search"), "(");
      await until(() => !!inPanel("session").querySelector('[role="alert"]'));
      seen.patternError = inPanel("session").querySelector('[role="alert"]')?.textContent;
      type(field("session", "Search"), "");
      await until(() => summary() === null && !inPanel("session").querySelector('[role="alert"]'));
      seen.cleared = true;
      seen.waitingReplace = !!field("waiting", "Replace");
      seen.waitingNote = inPanel("waiting").querySelector('[data-slot="own-copy-note"]')?.textContent ?? null;
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
    {result && <pre id="search-result">{JSON.stringify({ ...JSON.parse(result), opened })}</pre>}
  </div>;
}

createRoot(document.getElementById("root")!).render(<Fixture />);
