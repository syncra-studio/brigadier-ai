/**
 * A standalone Vite entry, excluded from the app entry and production build: the thread's own
 * work as its rows show it once the daemon keeps each call in full (phase T2), measured once the
 * page holds still. IPC is mocked, no daemon. The result goes to `<pre id="thread-work-result">`;
 * `stop=1` holds the page for a screenshot, `case=<id>` shows one case with its rows opened.
 *
 * - A running own `run` in an open live group: "Running command for 3s", opening to its Shell
 *   box, which follows the output as the row hears more of it.
 * - A running Codex command, its output streamed, the same way.
 * - A finished command that printed more than 8 KiB: the whole output, read with the request's
 *   calls.
 * - A file change of three files: a row each, Created / Edited / Deleted, the name a link that
 *   opens the file, "+7 −2", a dot on the made and the deleted file, the diff numbered.
 * - Images the thread looked at ("Viewed 2 images") and made ("Created an image"), opening to
 *   tiles; the images a turn made, gathered.
 * - Images being made ("Creating 3 images"): the row open at once, a dot grid in the place of
 *   each image still being made, beside the one made already; one finishes into its tile in the
 *   same place, one fails and leaves nothing. `making=1` measures only these.
 * - A row shown closed with an image made already: one more starting to be made opens it.
 * - Background terminals: running, finished (opening to its output), stopped.
 * - A Codex find in a page: "Searched the web for 'LTS' in https://nodejs.org/en".
 * - The turn's changed files from the request's own record: one file named in the title; four
 *   files with one Undo leaves as it is; Undo, its toast naming that file, then Reapply.
 * - The files-changed pill of a working request, from its record, opening its first file.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { type ReactNode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ActivityGroup } from "@/app/conversation/activity/ActivityGroup";
import { ImagesRow, MadeImages } from "@/app/conversation/activity/ImageRows";
import { describeLeadStep, LeadStepRow } from "@/app/conversation/activity/LeadStep";
import type { LeadStep } from "@/app/conversation/activity/group";
import { ComposerCapsule } from "@/app/conversation/ComposerCapsule";
import { ComposerTargetContext, type ComposerTarget } from "@/app/conversation/composerTarget";
import { ChangesCard } from "@/app/conversation/ChangesCard";
import { ViewContext } from "@/app/conversation/viewContext";
import { OpenFileContext } from "@/components/conversation";
import { TooltipProvider } from "@/components/overlays";
import type { BackgroundRun, Conversation, OrchestratorStepKind, ProviderEvent, RawEntry, Request, UserRequest } from "@/ipc/generated";
import { sessionTabs } from "@/state/sessionTabs";
import { emptyBoard, useBoard } from "@/state/board";
import { useApp } from "@/state/store";
import { useToasts } from "@/state/toasts";

const query = new URLSearchParams(location.search);
const CHAT = "thread-work";
const ROOT = "/repo";
const now = Date.now();

/** What the running commands printed so far: the row hears of more by its byte count. */
let ownPrinted = "compiling 1/3\n";
let codexPrinted = "test a ... ok\n";
/** More than 8 KiB: 400 lines of 40 characters. */
const BIG = Array.from({ length: 400 }, (_, index) => `line ${String(index + 1).padStart(3, "0")} ${"·".repeat(30)}`).join("\n");
const MATH_DIFF = "@@ -10,6 +10,11 @@ export function round(value) {\n const a = 1;\n-const b = 2;\n-const c = 3;\n+const b = 20;\n+const c = 30;\n+const d = 40;\n+const e = 50;\n+const f = 60;\n+const g = 70;\n+const h = 80;\n const z = 9;\n";
const NEW_DIFF = "@@ -0,0 +1,3 @@\n+export const one = 1;\n+export const two = 2;\n+export const three = 3;\n";
const OLD_DIFF = "@@ -1,4 +0,0 @@\n-a\n-b\n-c\n-d\n";
const SVG = (fill: string) => btoa(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 48"><rect width="64" height="48" fill="${fill}"/></svg>`);

const asked = { items: 0, images: [] as string[], runs: [] as string[], undo: [] as string[], opened: [] as string[] };

const entry = (streamSeq: number, event: ProviderEvent): RawEntry => ({ streamSeq, atMs: now, event });

/** The request's calls, as `listThreadItems` reads them now. */
function threadItems(): RawEntry[] {
  return [
    entry(10, { type: "toolCall", itemId: "own-run", name: "mcp__brigadier__run", input: '{"command":"npm run build"}', status: "inProgress", output: null }),
    entry(10, { type: "commandOutputDelta", itemId: "own-run", text: ownPrinted }),
    entry(11, { type: "command", itemId: "codex-run", command: "/bin/zsh -lc 'cargo test'", cwd: ROOT, status: "inProgress", exitCode: null, output: null, durationMs: null }),
    entry(11, { type: "commandOutputDelta", itemId: "codex-run", text: codexPrinted }),
    entry(12, { type: "command", itemId: "big", command: "cat build.log", cwd: ROOT, status: "completed", exitCode: 0, output: BIG, durationMs: 1200 }),
    entry(13, {
      type: "fileChanges",
      itemId: "edit",
      status: "completed",
      changes: [
        { path: `${ROOT}/src/math.js`, kind: "update", diff: MATH_DIFF, added: 7, removed: 2 },
        { path: `${ROOT}/src/new.ts`, kind: "add", diff: NEW_DIFF, added: 3, removed: 0 },
        { path: `${ROOT}/src/old.ts`, kind: "delete", diff: OLD_DIFF, added: 0, removed: 4 },
      ],
    } as ProviderEvent),
  ];
}

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "listThreadItems":
      asked.items += 1;
      return { method: req.method, page: { entries: threadItems(), next: null } };
    case "readThreadImage":
      asked.images.push(req.itemId);
      if (req.itemId === "view-gone") return Promise.reject(new Error("This image is no longer there"));
      return { method: req.method, data: SVG(req.itemId === "made" ? "purple" : req.itemId === "view-1" ? "teal" : "orange"), mime: "image/svg+xml" };
    case "readBackgroundRun":
      asked.runs.push(req.runId);
      // A running one has printed nothing yet; the ended ones, their whole output.
      if (req.runId === "run-1") return { method: req.method, output: "", bytes: 0 };
      return { method: req.method, output: "served on :5173\nshut down", bytes: 26 };
    case "undoChanges":
      asked.undo.push(`${req.requestId}:${req.reapply ? "reapply" : "undo"}`);
      if (req.requestId === "one") return Promise.reject(new Error("Later changes touch the same files (run.mjs)"));
      return {
        method: req.method,
        outcome: {
          restored: ["src/a.ts", "src/b.ts", "src/c.ts"],
          held: [{ path: "src/notes.md", reason: "src/notes.md also changed during this request in a way Brigadier didn't see (by you, or by one of the thread's own tools), so Undo leaves it as it is." }],
        },
      };
    case "getReviewDiff":
      return { method: req.method, review: { patch: "", files: [], insertions: 0, deletions: 0 } };
    default:
      return { method: req.method };
  }
});

useApp.setState((state) => ({
  conversations: {
    [CHAT]: {
      id: CHAT, kind: "session", title: "Work", projectId: null, pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, lifecycle: "active",
      forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
      setup: { type: "session", repo: ROOT, environment: { type: "local" }, permission: "fullAccess", planMode: false, workersSeeUncommitted: null, orchestrator: { provider: "codex", model: null, effort: null } },
    } as unknown as Conversation,
  },
  selection: { type: "conversation", id: CHAT },
  settings: state.settings,
}));

const stat = (files: [string, number, number][]) => ({
  files: files.map(([path, insertions, deletions]) => ({ path, insertions, deletions, binary: false })),
  insertions: files.reduce((sum, [, added]) => sum + added, 0),
  deletions: files.reduce((sum, [, , removed]) => sum + removed, 0),
});
const request = (id: string, state: UserRequest["state"]["type"], changes: UserRequest["changes"]): UserRequest =>
  ({
    id, conversationId: CHAT, preview: id, state: { type: state }, startedAtMs: now - 60_000, endedAtMs: state === "working" ? null : now,
    steeredInto: null, steeredAfter: null, undo: null, worked: [], quotaWait: false, ...(changes && { changes }),
  }) as UserRequest;
const run = (id: string, itemId: string, state: BackgroundRun["state"]): BackgroundRun => ({
  id, conversationId: CHAT, requestId: "r1", tool: "run", command: "npm run dev -- --port 5173", startedAtMs: now - 90_000,
  endedAtMs: state.type === "running" ? null : now, state, itemId,
});

useBoard.setState({
  board: {
    ...emptyBoard(CHAT), loaded: true,
    requests: {
      one: request("one", "done", { stat: stat([["scripts/run.mjs", 4, 1]]), settled: true }),
      four: request("four", "done", {
        stat: stat([["src/a.ts", 3, 1], ["src/b.ts", 2, 0], ["src/c.ts", 1, 1], ["src/notes.md", 5, 0]]),
        held: [{ path: "src/notes.md", reason: "src/notes.md also changed during this request in a way Brigadier didn't see (by you, or by one of the thread's own tools), so Undo leaves it as it is." }],
        settled: true,
      }),
      live: request("live", "working", { stat: stat([["src/math.js", 8, 2], ["src/new.ts", 3, 0]]), settled: false }),
    },
    backgroundRuns: {
      "run-1": run("run-1", "bg-running", { type: "running" }),
      "run-2": run("run-2", "bg-finished", { type: "finished", status: "exit 0" }),
      "run-3": run("run-3", "bg-stopped", { type: "stopped" }),
    },
  } as unknown as ReturnType<typeof emptyBoard>,
});

const view = { selection: { type: "conversation" as const, id: CHAT }, conversation: { id: CHAT } as Conversation, embedded: false };

type Tool = Extract<OrchestratorStepKind, { type: "tool" }>;
const tool = (position: number, kind: Partial<Tool> & Pick<Tool, "itemId" | "name">, atMs = now): LeadStep => ({
  position,
  atMs,
  requestId: "r1",
  kind: { type: "tool", detail: null, status: "completed", throughPosition: 0, logSeq: position, turn: 1, ...kind },
});

const readStep = tool(1, { itemId: "read", name: "Read", detail: `${ROOT}/src/math.js`, item: "tool", endedAtMs: now });
const ownRun = (bytes: number) => tool(10, { itemId: "own-run", name: "mcp__brigadier__run", detail: "npm run build", status: "inProgress", item: "command", outputBytes: bytes }, now - 3200);
const codexRun = (bytes: number) => tool(11, { itemId: "codex-run", name: "shell", detail: "/bin/zsh -lc 'cargo test'", status: "inProgress", item: "command", outputBytes: bytes }, now - 3200);
const bigRun = tool(12, { itemId: "big", name: "shell", detail: "cat build.log", item: "command", exit: 0, endedAtMs: now, fullOutput: "out-1" }, now - 1200);
const editStep = tool(13, {
  itemId: "edit", name: "apply_patch", detail: `${ROOT}/src/math.js and 2 more`, item: "files", endedAtMs: now,
  files: [
    { path: `${ROOT}/src/math.js`, insertions: 7, deletions: 2, binary: false, change: "update" },
    { path: `${ROOT}/src/new.ts`, insertions: 3, deletions: 0, binary: false, change: "add" },
    { path: `${ROOT}/src/old.ts`, insertions: 0, deletions: 4, binary: false, change: "delete" },
  ],
});
// A shell command that edited a file, and the row Brigadier files after it for the change it owns (#87).
const shellRun = tool(15, { itemId: "bash-1", name: "shell", detail: "echo 'thread' >> notes.md", item: "command", exit: 0, endedAtMs: now });
const shellEdits = tool(16, {
  itemId: "bash-1/edits", name: "apply_patch", detail: `${ROOT}/notes.md`, item: "files", endedAtMs: now,
  files: [{ path: `${ROOT}/notes.md`, insertions: 1, deletions: 0, binary: false, change: "update" }],
});
const findStep = tool(14, { itemId: "find", name: "web_search", detail: "'LTS' in https://nodejs.org/en", item: "web" });
const views = [tool(20, { itemId: "view-1", name: "view_image", item: "image" }), tool(21, { itemId: "view-2", name: "view_image", item: "image" })];
const made = [tool(22, { itemId: "made", name: "imagegen", detail: "a purple square", item: "image" })];
/** Being made: one made already, one that finishes, one that fails, when `settle` says. */
let settle: () => void = () => {};
function MakingRow() {
  const [settled, setSettled] = useState(false);
  useEffect(() => {
    settle = () => setSettled(true);
  }, []);
  const steps = [
    tool(23, { itemId: "made-2", name: "imagegen", detail: "a teal square", item: "image" }),
    tool(24, { itemId: "made-3", name: "imagegen", detail: "an orange square", item: "image", status: settled ? "completed" : "inProgress" }),
    tool(25, { itemId: "made-4", name: "imagegen", detail: "a red square", item: "image", status: settled ? "failed" : "inProgress" }),
  ];
  return <ImagesRow steps={steps} />;
}
/** Made already, then another started when `begin` says. */
let begin: () => void = () => {};
function LaterRow() {
  const [begun, setBegun] = useState(false);
  useEffect(() => {
    begin = () => setBegun(true);
  }, []);
  const steps = [tool(26, { itemId: "made-5", name: "imagegen", detail: "a teal square", item: "image" })];
  if (begun) steps.push(tool(27, { itemId: "made-6", name: "imagegen", detail: "a blue square", item: "image", status: "inProgress" }));
  return <ImagesRow steps={steps} />;
}
const bg = (itemId: string, position: number) => tool(position, { itemId, name: "mcp__brigadier__run", detail: "npm run dev -- --port 5173", item: "command" });

/** The live group, its steps moving on as `grow` says: the running commands hear more output. */
let grow: (by: number) => void = () => {};
function LiveGroup() {
  const [bytes, setBytes] = useState(ownPrinted.length);
  useEffect(() => {
    grow = (by) => setBytes((before) => before + by);
  }, []);
  const steps = [readStep, ownRun(bytes), codexRun(bytes)];
  return (
    <ActivityGroup
      items={steps.map((step) => ({ type: "step", key: `step:${step.position}`, step }))}
      live
      describe={(step) => describeLeadStep(step)}
      renderStep={(step, key) => <LeadStepRow key={key} step={step} />}
    />
  );
}

const capsuleTarget = { conversation: { id: CHAT } as Conversation } as unknown as ComposerTarget;

function Case({ id, children }: { id: string; children: ReactNode }) {
  const only = query.get("case");
  if (only && only !== id) return null;
  return (
    <section data-case={id} className="flex flex-col gap-1">
      {children}
    </section>
  );
}

function Page() {
  return (
    <TooltipProvider>
      <ViewContext.Provider value={view}>
        <OpenFileContext.Provider value={(path) => (asked.opened.push(path), true)}>
          <main className="bg-background text-foreground flex min-h-screen w-full max-w-thread flex-col gap-6 p-8">
            <Case id="live-group">
              <LiveGroup />
            </Case>
            <Case id="big">
              <LeadStepRow step={bigRun} />
            </Case>
            <Case id="edit">
              <LeadStepRow step={editStep} />
            </Case>
            <Case id="shell-edit">
              <LeadStepRow step={shellRun} />
              <LeadStepRow step={shellEdits} />
            </Case>
            <Case id="find">
              <LeadStepRow step={findStep} />
            </Case>
            <Case id="viewed">
              <ImagesRow steps={views} />
            </Case>
            <Case id="made">
              <ImagesRow steps={made} />
            </Case>
            <Case id="making">
              <MakingRow />
            </Case>
            <Case id="later">
              <LaterRow />
            </Case>
            <Case id="made-gathered">
              <MadeImages steps={[...views, ...made]} />
            </Case>
            <Case id="background">
              <LeadStepRow step={bg("bg-running", 30)} />
              <LeadStepRow step={bg("bg-finished", 31)} />
              <LeadStepRow step={bg("bg-stopped", 32)} />
            </Case>
            <Case id="turn-diff-one">
              <ChangesCard requestIds={["one"]} />
            </Case>
            <Case id="turn-diff-four">
              <ChangesCard requestIds={["four"]} />
            </Case>
            <Case id="pill">
              <ComposerTargetContext.Provider value={capsuleTarget}>
                <ComposerCapsule />
              </ComposerTargetContext.Provider>
            </Case>
          </main>
        </OpenFileContext.Provider>
      </ViewContext.Provider>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Page />);

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const frame = () => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve)));
const one = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => root.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [...root.querySelectorAll<T>(selector)];
const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, " ").trim() ?? null;
const at = (id: string) => one(`[data-case=${id}]`)!;
/** Waits up to 8 s for `ready`. */
async function until(ready: () => unknown): Promise<void> {
  for (const end = performance.now() + 8000; performance.now() < end && !ready(); ) await frame();
}

async function measure(): Promise<object> {
  await wait(300);
  const out: Record<string, unknown> = {};

  // The live group: opened, its running commands tick; each opens to its Shell box, following the output.
  one<HTMLElement>("[data-slot=work-group] button", at("live-group"))?.click();
  await wait(400);
  const children = () => all("[data-slot=orchestrator-step]", at("live-group"));
  out.liveChildren = children().map((row) => text(row));
  for (const row of children().slice(1)) one<HTMLElement>("[role=button]", row)?.click();
  await until(() => all("[data-slot=shell-output]", at("live-group")).length === 2);
  const outputs = () => all("[data-slot=shell-output]", at("live-group")).map((box) => text(box));
  const commands = () => all("[data-slot=shell-command]", at("live-group")).map((box) => text(box));
  out.liveFirst = { outputs: outputs(), commands: commands(), endings: all("[data-slot=shell-ending]", at("live-group")).length };
  const readsBefore = asked.items;
  ownPrinted += "compiling 2/3\ncompiling 3/3\n";
  codexPrinted += "test b ... ok\n";
  grow(40);
  await until(() => outputs().every((output) => (output ?? "").includes("3/3") || (output ?? "").includes("test b")));
  out.liveLater = { outputs: outputs(), readAgain: asked.items > readsBefore };

  // A finished command's whole output, past 8 KiB.
  one<HTMLElement>("[role=button]", at("big"))?.click();
  await until(() => one("[data-slot=shell-output]", at("big")));
  const bigOutput = one("[data-slot=shell-output] pre", at("big"))?.textContent ?? "";
  out.big = { bytes: bigOutput.length, last: bigOutput.split("\n").at(-1), ending: text(one("[data-slot=shell-ending]", at("big"))), clipped: !!one("[data-slot=shell-clipped]", at("big")) };

  // A file change: a row per file, each with its verb, link, counts and dot.
  const fileRows = () => all("[data-slot=edited-file]", at("edit"));
  out.edit = fileRows().map((row) => ({
    row: text(row),
    link: text(one("[data-slot=edited-file-link]", row)),
    counts: text(one("[data-slot=edited-file-counts]", row)),
    dot: one("[data-slot=edited-file-dot]", row)?.getAttribute("data-change") ?? null,
  }));
  out.shellEdit = all("[data-slot=edited-file]", at("shell-edit")).map((row) => text(row));
  const openedBefore = asked.opened.length;
  one<HTMLElement>("[data-slot=edited-file-link]", fileRows()[0])?.click();
  await wait(200);
  out.editLink = { opened: asked.opened.slice(openedBefore), rowOpened: !!one("[data-slot=diff-card]", at("edit")) };
  one<HTMLElement>("[role=button]", fileRows()[0])?.click();
  // The diff engine draws in its own shadow root; its gutter numbers the lines the real patch names.
  const numbers = () =>
    all("diffs-container", at("edit"))
      .flatMap((host) => (host.shadowRoot ? [...host.shadowRoot.querySelectorAll<HTMLElement>("[data-gutter] [data-line-number-content]")] : []))
      .filter((cell) => cell.getBoundingClientRect().width > 0)
      .map((cell) => cell.textContent?.trim() ?? "");
  await until(() => numbers().length > 0);
  out.editDiff = {
    row: text(one("[role=button]", fileRows()[0])),
    name: text(one("[data-slot=diff-name]", at("edit"))),
    numbers: [...new Set(numbers())].slice(0, 3),
  };

  out.find = text(at("find").firstElementChild);

  // Images: their rows, then their tiles.
  out.images = { viewed: text(one("[data-slot=images-row]", at("viewed"))), made: text(one("[data-slot=images-row]", at("made"))) };
  one<HTMLElement>("[role=button]", at("viewed"))?.click();
  await until(() => all("[data-slot=image-tile] img", at("viewed")).length === 2);
  out.viewedTiles = all("[data-slot=image-tile] img", at("viewed")).map((image) => image.getAttribute("alt"));
  await until(() => all("[data-slot=image-tile] img", at("made-gathered")).length === 1);
  out.gathered = all("[data-slot=image-tile] img", at("made-gathered")).map((image) => image.getAttribute("alt"));

  out.making = await measureMaking();
  const later = () => one("[data-slot=images-row]", at("later"));
  await until(() => later());
  const shut = later()!.getAttribute("data-state");
  begin();
  await until(() => all("[data-slot=image-generation]", at("later")).length === 1);
  out.later = [shut, later()!.getAttribute("data-state"), text(one("[role=button]", later()!))];

  // Background terminals: their words; a finished one opens to its output.
  out.background = all("[data-slot=background-run-row]", at("background")).map((row) => ({
    row: text(row),
    state: row.getAttribute("data-state"),
    icon: one("[data-icon]", row)?.getAttribute("data-icon") ?? null,
  }));
  one<HTMLElement>("[data-state=finished] [role=button]", at("background"))?.click();
  await until(() => one("[data-state=finished] [data-slot=shell-output]", at("background")));
  out.backgroundOutput = {
    output: text(one("[data-state=finished] [data-slot=shell-output]", at("background"))),
    ending: text(one("[data-state=finished] [data-slot=shell-ending]", at("background"))),
    asked: [...asked.runs],
  };

  // The turn's changed files, from the request's own record.
  const card = (id: string) => ({
    title: text(one("[data-slot=turn-diff-title]", at(id))),
    counts: text(one("[data-slot=turn-diff-counts]", at(id))),
    files: all("[data-slot=turn-diff-file]", at(id)).length,
    more: text(one("[data-slot=turn-diff-more]", at(id))),
    undo: all("button", at(id)).find((button) => /Discard|Reapply/.test(button.textContent ?? "")) ?? null,
  });
  const oneCard = card("turn-diff-one");
  const fourCard = card("turn-diff-four");
  out.turnDiff = {
    one: { ...oneCard, undo: text(oneCard.undo) },
    four: { ...fourCard, undo: text(fourCard.undo), held: fourCard.undo?.getAttribute("title") ?? null },
  };
  fourCard.undo?.click();
  await until(() => useToasts.getState().toasts.length > 0);
  out.undone = { asked: [...asked.undo], toast: useToasts.getState().toasts.at(-1)?.text ?? null };
  useBoard.setState((state) => ({
    board: state.board && { ...state.board, requests: { ...state.board.requests, four: { ...state.board.requests["four"]!, undo: { reverted: true, commits: [] } } } },
  }));
  await wait(200);
  out.reapply = text(card("turn-diff-four").undo);
  // The toast a click brings, newer than any before it (toasts leave on a timer).
  const toastAfter = async (click: () => void) => {
    const before = Math.max(-1, ...useToasts.getState().toasts.map((shown) => shown.id));
    click();
    await until(() => useToasts.getState().toasts.some((shown) => shown.id > before));
    return useToasts.getState().toasts.find((shown) => shown.id > before)?.text ?? null;
  };
  out.reapplied = await toastAfter(() => card("turn-diff-four").undo?.click());
  // A refused Undo says which way it failed.
  out.refused = await toastAfter(() => oneCard.undo?.click());

  // The pill of the working request, from its record; a click opens its first file.
  await until(() => one("[data-slot=composer-capsule]", at("pill")));
  out.pill = text(one("[data-slot=composer-capsule]", at("pill")));
  one<HTMLElement>("[data-slot=composer-capsule] button", at("pill"))?.click();
  await wait(200);
  const tabs = sessionTabs(CHAT);
  const opened = tabs.tabs.find((tab) => tab.id === tabs.active);
  out.pillTab = opened?.kind === "file" ? opened.path : (opened?.kind ?? null);
  return out;
}

/**
 * Images being made, untouched: the row's words and whether it's open, each dot grid's and
 * tile's box from the row's corner, the dots' animation; then, once they settle, the tiles.
 */
async function measureMaking(): Promise<object> {
  const row = () => one("[data-slot=images-row]", at("making"))!;
  const grids = () => all("[data-slot=image-generation]", at("making"));
  const tiles = () => all("[data-slot=image-tile]", at("making"));
  const box = (element: Element) => {
    const corner = row().getBoundingClientRect();
    const { left, top, width, height } = element.getBoundingClientRect();
    return { x: Math.round(left - corner.left), y: Math.round(top - corner.top), width: Math.round(width), height: Math.round(height) };
  };
  await until(() => one("[data-case=making] [data-slot=images-row]"));
  await until(() => grids().length === 2 && all("img", at("making")).length === 1);
  const making = {
    row: text(one("[role=button]", row())),
    open: row().getAttribute("data-state"),
    grids: grids().map((grid) => ({ label: grid.getAttribute("aria-label"), box: box(grid) })),
    tiles: tiles().map(box),
    animation: getComputedStyle(one("[data-slot=image-generation] span", at("making"))!).animationName,
  };
  settle();
  await until(() => grids().length === 0 && all("img", at("making")).length === 2);
  return { making, made: { row: text(one("[role=button]", row())), open: row().getAttribute("data-state"), grids: grids().length, tiles: tiles().map(box) } };
}

/** One case on its own, for a screenshot: its rows opened, once each. */
async function openCase(): Promise<object> {
  await wait(300);
  // A group's rows come once it opens: open what shows, then what that showed.
  const done = new Set<Element>();
  for (let pass = 0; pass < 3; pass++) {
    for (const trigger of all<HTMLElement>("[data-slot=work-group] > button, [role=button][aria-expanded=false]")) {
      if (done.has(trigger)) continue;
      done.add(trigger);
      trigger.click();
      await wait(150);
    }
    await wait(400);
  }
  await wait(1200);
  return { opened: query.get("case") };
}

void (query.get("case") ? openCase() : query.get("making") ? measureMaking() : measure())
  .catch((error: unknown) => ({ error: String(error) }))
  .then(async (result) => {
    if (query.get("stop") === "1") await new Promise(() => {});
    const out = document.createElement("pre");
    out.id = "thread-work-result";
    out.textContent = JSON.stringify(result);
    document.body.append(out);
  });
