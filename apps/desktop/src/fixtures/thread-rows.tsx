/**
 * A standalone Vite entry, excluded from the app entry and production build: the thread's rows
 * and what they open to, each in its states, measured once the page holds still. IPC is mocked,
 * no daemon. The result goes to `<pre id="thread-rows-result">`; `stop=1` holds the page for a
 * screenshot.
 *
 * - Shell boxes: a long coloured output shown whole with its success footer, a failure's exit
 *   code, a running command (no footer), output the daemon cut short, a command clamped to two
 *   lines until clicked.
 * - A turn the user stopped: a command still running reads "Stopped … after 4s" with a stop mark,
 *   and its Shell box ends "Stopped".
 * - An open live group: its running command reads "Running command for 3s", a running read is
 *   hidden until done.
 * - A web search: its `site:` operator after a bar, and nothing to open.
 * - Diffs: a real patch numbered, coloured and word-marked; an edit rebuilt from its call
 *   unnumbered; an empty and a missing diff's placeholders.
 * - The user's own message: its Markdown rendered, each line break kept, an unknown worker's
 *   mention left as written.
 * - A turn that throws while rendering: "This turn couldn't render" with Try again, its
 *   neighbour unharmed; Try again renders it afresh.
 * - The image viewer: a set of three opened from its first, stepped with →, closed by a click
 *   on the backdrop.
 * - An answer's inline code: a file of the checkout is a chip (from the file list, or asked by
 *   its exact path when the list is cut short), anything else stays code.
 * - An answer's web links: a globe until the daemon has the site's icon, and when it has none.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import type { ReactNode } from "react";
import { createRoot } from "react-dom/client";

import { ActivityGroup, type Described, StepRow } from "@/app/conversation/activity/ActivityGroup";
import { describeLeadStep } from "@/app/conversation/activity/LeadStep";
import { ShellBox, ToolResult } from "@/app/conversation/activity/StepDetail";
import { StoppedBy } from "@/app/conversation/activity/stopped";
import { stepWords } from "@/app/conversation/activity/words";
import { InlineDiff } from "@/app/conversation/activity/InlineDiff";
import { ArtifactDialog } from "@/app/conversation/ArtifactDialog";
import { UserMarkdownText } from "@/app/conversation/Mentions";
import { TurnDiff } from "@/app/conversation/TurnDiff";
import { ViewContext } from "@/app/conversation/viewContext";
import { TurnBoundary } from "@/components/assistant-ui/turn-boundary";
import { ImagePreview } from "@/components/assistant-ui/elements/image-viewer";
import { MarkdownBlock } from "@/components/assistant-ui/thread";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, OrchestratorStep, Request } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const CHAT = "thread-rows";
const ROOT = "/repo";
const asked: string[] = [];
/** A worker's diff of six files, forty new lines each: taller than any window. */
const bigPatch = Array.from({ length: 6 }, (_, file) =>
  `diff --git a/src/f${file}.ts b/src/f${file}.ts\n--- a/src/f${file}.ts\n+++ b/src/f${file}.ts\n@@ -0,0 +1,40 @@\n` + Array.from({ length: 40 }, (_line, line) => `+export const v${line} = ${line};\n`).join(""),
).join("");
const siteAsks: string[] = [];
const reviewAsks: string[] = [];
/** The turn's patch, as the Review diff gives it: five files. */
const TURN_FILES = ["README.md", "src/math.js", "src/strings.js", "src/a.js", "src/b.js"];
const turnPatch = TURN_FILES.map((path) => `diff --git a/${path} b/${path}\n--- a/${path}\n+++ b/${path}\n@@ -1,1 +1,1 @@\n-old ${path}\n+new ${path}\n`).join("");
const NODE_ICON = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYPj/HwADAgH/eL9GtQAAAABJRU5ErkJggg==";
let letIconsCome = () => {};
const iconsCome = new Promise<void>((resolve) => (letIconsCome = resolve));
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "listFiles") return { method: req.method, files: ["src/cli.ts", "src/math.js"], truncated: true };
  if (req.method === "resolveFileLink") {
    asked.push(req.path);
    return { method: req.method, exists: req.path === `${ROOT}/src/made-later.ts`, stored: null };
  }
  if (req.method === "getReviewDiff") {
    reviewAsks.push(JSON.stringify(req.scope));
    return { method: req.method, review: { patch: turnPatch } };
  }
  if (req.method === "readArtifact") return { method: req.method, text: { text: bigPatch, offset: 0, totalBytes: bigPatch.length, binary: false } };
  if (req.method === "siteIcon") {
    siteAsks.push(req.origin);
    return iconsCome.then(() => ({ method: req.method, icon: req.origin === "https://nodejs.org" ? NODE_ICON : null }));
  }
  return { method: req.method };
});
useApp.setState((state) => ({
  conversations: {
    [CHAT]: {
      id: CHAT, kind: "session", title: "Rows", projectId: null, pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, lifecycle: "active",
      forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
      setup: { type: "session", repo: ROOT, environment: { type: "local" }, permission: "fullAccess", planMode: false, workersSeeUncommitted: null, orchestrator: { provider: "codex", model: null, effort: null } },
    } as unknown as Conversation,
  },
  selection: { type: "conversation", id: CHAT },
  settings: state.settings,
}));

const landed = (id: string, requestId: string, number: number, files: string[]) => ({
  id, requestId, number, landed: "abc", state: "landed",
  candidate: { diffStat: { files: files.map((path) => ({ path, insertions: 1, deletions: 1, binary: false })), insertions: files.length, deletions: files.length } },
});
useBoard.setState({
  board: {
    ...emptyBoard(CHAT), loaded: true,
    tasks: { t1: landed("t1", "many", 1, TURN_FILES), t2: landed("t2", "one", 2, ["scripts/run.mjs"]) },
  } as unknown as ReturnType<typeof emptyBoard>,
});
const view = { selection: { type: "conversation" as const, id: CHAT }, conversation: { id: CHAT } as Conversation, embedded: false };

const patch = "--- a/src/math.js\n+++ b/src/math.js\n@@ -1,4 +1,4 @@\n export function round(value) {\n-  return Math.round(value * 100) / 100;\n+  return Math.round((value + Number.EPSILON) * 100) / 100;\n }\n \n@@ -20,2 +20,3 @@\n const a = 1;\n+const b = 2;\n const c = 3;\n";
const links = "See [the release](https://nodejs.org/en/about) and [its schedule](https://nodejs.org/en/about/previous-releases), [a page](https://none.example/x) and [the dev server](http://localhost:3000). Write to [the team](mailto:team@example.com).";
const answer = "The CLI parses it in `src/cli.ts:64`, the new helper is `src/made-later.ts`, rounding uses `Math.round`, and `src/gone.ts` was removed.";

const now = Date.now();
const longOutput = Array.from({ length: 60 }, (_, index) => (index === 59 ? "\u001b[32m✓ 60 tests passed\u001b[0m" : `line ${index + 1}`)).join("\n");
const longCommand = `pnpm vitest run ${Array.from({ length: 40 }, (_, index) => `src/module-${index}.test.ts`).join(" ")}`;

const command = (line: string, status: Described["status"], more: Partial<Described> = {}): Described => ({
  words: stepWords({ name: "shell", detail: line, status }),
  status,
  ...more,
});

/** Three small images, drawn in the palette's colours. */
const PIXELS = ["red", "green", "blue"].map((name) => `data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 48"><rect width="64" height="48" fill="${name}"/></svg>`)}`);

/** A turn that fails to render until `broken` is cleared. */
let broken = true;
function Breaks() {
  if (broken) throw new Error("render failed");
  return <p data-slot="mended">Rendered again</p>;
}

/** One labelled case on the page. */
function Case({ id, children }: { id: string; children: ReactNode }) {
  return (
    <section data-case={id} className="flex flex-col gap-1">
      {children}
    </section>
  );
}

const search: OrchestratorStep = {
  requestId: "r1", position: 1, atMs: now,
  kind: { type: "searchedWeb", query: "node lts release site:nodejs.org" },
} as OrchestratorStep;

const group: { key: string; described: Described }[] = [
  { key: "read-done", described: { words: stepWords({ name: "Read", detail: "/repo/src/math.js", status: "completed" }), status: "completed" } },
  { key: "test", described: command("npm test", "inProgress", { startedAtMs: now - 3200 }) },
  { key: "read-running", described: { words: stepWords({ name: "Read", detail: "/repo/src/round.js", status: "inProgress" }), status: "inProgress" } },
];

/** A finished group whose first step is project memory: its icon is its summary's first segment's. */
const doneGroup: { key: string; described: Described }[] = [
  { key: "memory", described: { words: stepWords({ name: "query_brain", detail: "rounding", status: "completed" }), status: "completed" } },
  { key: "ran", described: command("npm test", "completed", { startedAtMs: now - 41000, endedAtMs: now }) },
  { key: "read", described: { words: stepWords({ name: "Read", detail: "/repo/src/math.js", status: "completed" }), status: "completed" } },
];

/** One row of each kind, for its icon. */
const kinds: { key: string; described: Described }[] = [
  { key: "run", described: command("npm test", "completed") },
  { key: "read", described: { words: stepWords({ name: "Read", detail: "/repo/a.ts", status: "completed" }), status: "completed" } },
  { key: "search", described: { words: stepWords({ name: "Grep", detail: "round", status: "completed" }), status: "completed" } },
  { key: "list", described: command("ls src", "completed") },
  { key: "edit", described: { words: stepWords({ name: "Edit", detail: "/repo/a.ts", status: "completed" }), status: "completed" } },
  { key: "web", described: command("curl -s https://nodejs.org/dist/index.json", "completed") },
];

function Page() {
  return (
    <TooltipProvider>
      <main className="bg-background text-foreground flex min-h-screen w-full max-w-thread flex-col gap-6 p-8">
        <Case id="shell-long">
          <ShellBox command="npm test" output={longOutput} status="completed" exit={0} />
        </Case>
        <Case id="shell-failed">
          <ShellBox command="npm test" output="1 failed" status="failed" exit={1} />
        </Case>
        <Case id="shell-running">
          <ShellBox command="sleep 30" output="" status="inProgress" exit={null} />
        </Case>
        <Case id="shell-clipped">
          <ShellBox command="cat big.log" output={`head of the log… [12345 more bytes]`} status="completed" exit={0} />
        </Case>
        <Case id="shell-command">
          <ShellBox command={longCommand} output="ok" status="completed" exit={0} />
        </Case>
        <Case id="stopped">
          <StoppedBy.Provider value={{ atMs: now - 1000 }}>
            <StepRow
              {...command("sleep 30", "inProgress", { startedAtMs: now - 5000 })}
              detail={<ShellBox command="sleep 30" output="" status="inProgress" exit={null} />}
            />
          </StoppedBy.Provider>
        </Case>
        <Case id="group">
          <ActivityGroup
            items={group.map(({ key }) => ({ type: "step", key, step: key }))}
            live
            describe={(key) => group.find((each) => each.key === key)!.described}
            renderStep={(key) => <StepRow key={key} {...group.find((each) => each.key === key)!.described} slot="child" />}
          />
        </Case>
        <Case id="done-group">
          <ActivityGroup
            items={doneGroup.map(({ key }) => ({ type: "step", key, step: key }))}
            live={false}
            describe={(key) => doneGroup.find((each) => each.key === key)!.described}
            renderStep={(key) => <StepRow key={key} {...doneGroup.find((each) => each.key === key)!.described} suffix={key === "ran" ? "in 41s" : undefined} slot="child" />}
          />
        </Case>
        <Case id="lone-ran">
          <StepRow {...command("npm test", "completed")} suffix="in 41s" />
        </Case>
        <Case id="kinds">
          {kinds.map(({ key, described }) => (
            <StepRow key={key} {...described} slot={`kind-${key}`} />
          ))}
        </Case>
        <Case id="search">
          <StepRow {...describeLeadStep(search)} />
        </Case>
        <Case id="tool-json">
          <ToolResult name="lookup" status="completed" input={'{"id":7}'} output={'{"ok":true,"items":[1,2]}'} />
        </Case>
        <Case id="tool-text">
          <ToolResult name="lookup" status="completed" input={'{"id":7}'} output="Found it." />
        </Case>
        <Case id="tool-empty">
          <ToolResult name="lookup" status="completed" input={'{"id":7}'} output="" />
        </Case>
        <Case id="tool-failed">
          <ToolResult name="lookup" status="failed" input={'{"id":7}'} output="Not found: 7" />
        </Case>
        <Case id="edit-row">
          <StepRow
            words={stepWords({ name: "Edit", detail: "src/math.js", status: "completed" })}
            status="completed"
            detail={<InlineDiff path="src/math.js" diff={patch} />}
          />
        </Case>
        <ViewContext.Provider value={view}>
          <Case id="turn-diff-many">
            <TurnDiff requestId="many" />
          </Case>
          <Case id="turn-diff-one">
            <TurnDiff requestId="one" />
          </Case>
        </ViewContext.Provider>
        <Case id="diff-patch">
          <InlineDiff path="src/math.js" diff={patch} />
        </Case>
        <Case id="diff-edit">
          <InlineDiff path="src/math.js" diff={"-const a = 1;\n+const a = 2;"} />
        </Case>
        <Case id="diff-empty">
          <InlineDiff path="src/math.js" diff="" />
        </Case>
        <Case id="diff-missing">
          <InlineDiff path="src/math.js" diff={null} />
        </Case>
        <Case id="user">
          <UserMarkdownText
            text={"**1. CSV** — first *line*\nsecond line\n\nAsk @task-9 about `round` and `@task-9`\n\nBefore [Image #1] after.\n\n```\nping @task-9\n```"}
            attachments={[]}
          />
        </Case>
        <Case id="boundary">
          <TurnBoundary>
            <Breaks />
          </TurnBoundary>
          <p data-slot="neighbour">The next turn</p>
        </Case>
        <Case id="viewer">
          <ImagePreview url={PIXELS[0]!} name="one.png" gallery={{ images: PIXELS.map((url, index) => ({ url, name: `image-${index + 1}.png` })), index: 0 }}>
            <button type="button" data-slot="viewer-thumb" aria-label="View image-1.png">
              <img src={PIXELS[0]} alt="" className="size-icon-lg" />
            </button>
          </ImagePreview>
        </Case>
        <Case id="chips">
          <MarkdownBlock text={answer} />
        </Case>
        <Case id="links">
          <MarkdownBlock text={links} />
        </Case>
      </main>
    </TooltipProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Page />);

/* ----- measuring ------------------------------------------------------------------------- */

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const frame = () => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve)));
const one = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => root.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [...root.querySelectorAll<T>(selector)];
const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, " ").trim() ?? null;
const at = (id: string) => one(`[data-case=${id}]`)!;

/** A Shell box as it reads. */
function shell(id: string) {
  const box = at(id);
  const output = one("[data-slot=shell-output] pre", box);
  return {
    command: text(one("[data-slot=shell-command]", box)),
    clamped: !!one("[data-slot=shell-command].line-clamp-2", box),
    outputLines: output?.textContent?.split("\n").length ?? 0,
    output: output ? (output.textContent ?? "") : null,
    colored: all("[data-slot=shell-output] span[style]", box).map((span) => [span.textContent, span.style.color]),
    ending: text(one("[data-slot=shell-ending]", box)),
    clipped: text(one("[data-slot=shell-clipped]", box)),
    folded: all("button", box).some((button) => /lines/.test(button.textContent ?? "")),
  };
}

async function measure(): Promise<object> {
  await wait(300);
  for (let index = 0; index < 5; index++) await frame();
  const out: Record<string, unknown> = {};
  for (const id of ["shell-long", "shell-failed", "shell-running", "shell-clipped", "shell-command"]) out[id] = shell(id);
  one<HTMLElement>("[data-slot=shell-command]", at("shell-command"))?.click();
  await frame();
  out.commandClicked = shell("shell-command").clamped;
  const stoppedRow = one("[data-step-state]", at("stopped"));
  out.stopped = { row: text(stoppedRow), state: stoppedRow?.getAttribute("data-step-state"), shimmer: !!one(".shimmer", at("stopped")) };
  one<HTMLElement>("[role=button]", at("stopped"))?.click();
  await wait(400);
  out.stoppedShell = shell("stopped").ending;
  // The open live group: its child rows.
  const trigger = one<HTMLElement>("[data-slot=work-group] button", at("group"));
  out.groupRow = text(trigger);
  trigger?.click();
  await wait(400);
  out.children = all("[data-slot=child]", at("group")).map((row) => text(row));
  // A tool's result: JSON as a code block, text, nothing, a failure; never its arguments inline.
  for (const end = performance.now() + 8000; performance.now() < end && !one("[data-slot=tool-json] code", at("tool-json")); ) await frame();
  const tool = (id: string) => ({
    json: text(one("[data-slot=tool-json] code", at(id))),
    text: text(one("[data-slot=tool-text]", at(id))),
    empty: text(one("[data-slot=tool-empty]", at(id))),
    error: !!one("[data-slot=error-state]", at(id)),
    argumentsShown: (at(id).textContent ?? "").includes('"id"'),
    raw: text(one("[data-slot=tool-raw]", at(id))),
  });
  out.tools = Object.fromEntries(["tool-json", "tool-text", "tool-empty", "tool-failed"].map((id) => [id, tool(id)]));
  one<HTMLButtonElement>("[data-slot=tool-raw]", at("tool-json"))?.click();
  await wait(300);
  out.toolRaw = text(one("[role=dialog]"));
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  one("[role=dialog]")?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await wait(300);
  // An edit row opened to its diff reads "Edited file": the diff names the file.
  const editRow = one<HTMLElement>("[role=button]", at("edit-row"));
  const editClosed = text(editRow);
  editRow?.click();
  await wait(400);
  out.editRow = { closed: editClosed, open: text(editRow) };
  // The turn's diff card: several files, the first three listed; one file, named in the title, no list.
  const card = (id: string) => ({
    title: text(one("[data-slot=turn-diff-title]", at(id))),
    counts: text(one("[data-slot=turn-diff-counts]", at(id))),
    buttons: all("button", at(id)).filter((button) => !button.closest("[data-slot=turn-diff-file]") && button.getAttribute("data-slot") !== "turn-diff-more").map((button) => button.getAttribute("aria-label") ?? text(button)),
    files: all("[data-slot=turn-diff-file]", at(id)).map((row) => [text(one("[data-slot=turn-diff-dir]", row)), text(one("[data-slot=turn-diff-name]", row))]),
    more: text(one("[data-slot=turn-diff-more]", at(id))),
  });
  const many = card("turn-diff-many");
  one<HTMLButtonElement>("[data-slot=turn-diff-more]", at("turn-diff-many"))?.click();
  await wait(100);
  const all5 = card("turn-diff-many");
  one<HTMLButtonElement>("[data-slot=turn-diff-file] button", at("turn-diff-many"))?.click();
  for (const end = performance.now() + 8000; performance.now() < end && !one("[data-slot=turn-diff-file-body] [data-slot=diff-name]", at("turn-diff-many")); ) await frame();
  out.turnDiff = {
    many, all: { files: all5.files.length, more: all5.more }, one: card("turn-diff-one"),
    opened: { name: text(one("[data-slot=turn-diff-file-body] [data-slot=diff-name]", at("turn-diff-many"))), counts: text(one("[data-slot=turn-diff-file-body] [data-slot=diff-counts]", at("turn-diff-many"))) },
    asks: reviewAsks,
  };
  // The diff engine draws in its own shadow root.
  const inside = (id: string, selector: string) =>
    all("diffs-container", at(id)).flatMap((host) => (host.shadowRoot ? [...host.shadowRoot.querySelectorAll<HTMLElement>(selector)] : []));
  const diff = (id: string) => ({
    name: text(one("[data-slot=diff-name]", at(id))),
    counts: text(one("[data-slot=diff-counts]", at(id))),
    // Numbers only where the gutter shows them.
    numbers: inside(id, "[data-gutter] [data-line-number-content]").filter((cell) => cell.getBoundingClientRect().width > 0).map((cell) => cell.textContent?.trim() ?? ""),
    kinds: inside(id, "[data-content] > [data-line-type]").map((row) => row.getAttribute("data-line-type")),
    gaps: inside(id, "[data-gutter] [data-separator-content]").map((row) => row.textContent?.trim() ?? ""),
    marks: inside(id, "[data-diff-span]").map((mark) => mark.textContent),
    empty: text(one("[data-slot=diff-empty]", at(id))),
    coloured: inside(id, "[data-code] span[style]").length > 0,
  });
  // Syntax colours come once the engine and the grammar load.
  for (const end = performance.now() + 8000; performance.now() < end && !diff("diff-patch").coloured; ) await frame();
  for (const id of ["diff-patch", "diff-edit", "diff-empty", "diff-missing"]) out[id] = diff(id);
  // The answer's chips, once the checkout's files are known.
  for (const end = performance.now() + 8000; performance.now() < end && all("[data-slot=file-chip]", at("chips")).length < 2; ) await frame();
  await wait(300);
  out.chips = all("[data-slot=file-chip]", at("chips")).map((chip) => [text(chip), chip.getAttribute("href")]);
  out.code = all("code", at("chips")).map((code) => text(code));
  out.asked = asked.toSorted();
  const boundary = at("boundary");
  out.boundary = { fallback: text(one("[data-slot=turn-render-error] [data-slot=error-state]", boundary))?.startsWith("This turn couldn’t render.") ?? false, neighbour: text(one("[data-slot=neighbour]", boundary)) };
  broken = false;
  all<HTMLButtonElement>("button", boundary).find((button) => text(button) === "Try again")?.click();
  await wait(100);
  out.mended = text(one("[data-slot=mended]", boundary));
  // The viewer: opened from its thumbnail (morphing where the browser can), stepped, closed from the backdrop.
  one<HTMLElement>("[data-slot=viewer-thumb]")?.click();
  for (const end = performance.now() + 3000; performance.now() < end && !one("[data-slot=image-preview] img"); ) await frame();
  await wait(400);
  const viewer = () => one("[data-slot=image-preview]");
  const first = { open: !!viewer(), count: text(one("[data-slot=image-preview-count]")), alt: one("[data-slot=image-preview] img")?.getAttribute("alt") ?? null };
  viewer()?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
  await wait(100);
  const second = { count: text(one("[data-slot=image-preview-count]")), alt: one("[data-slot=image-preview] img")?.getAttribute("alt") ?? null };
  // Zoom keys work from wherever focus is, the Close button too.
  const percent = () => all("span", viewer() ?? undefined).map((node) => text(node)).find((words) => words?.endsWith("%")) ?? null;
  const fitted = percent();
  one("[data-slot=image-preview] [aria-label='Close image preview']")?.dispatchEvent(new KeyboardEvent("keydown", { key: "+", bubbles: true }));
  await wait(100);
  const zoomed = percent();
  one("[data-slot=image-preview] [aria-label='Close image preview']")?.dispatchEvent(new KeyboardEvent("keydown", { key: "0", bubbles: true }));
  await wait(100);
  out.zoomKeys = [fitted, zoomed, percent()];
  one<HTMLElement>("[data-slot=image-preview-frame]")?.click();
  for (const end = performance.now() + 3000; performance.now() < end && viewer(); ) await frame();
  out.viewer = { first, second, closed: !viewer() };
  const user = at("user");
  out.user = {
    strong: all("strong", user).map((node) => text(node)),
    em: all("em", user).map((node) => text(node)),
    breaks: all("br", user).length,
    paragraphs: all("p", user).length,
    code: all("code", user).map((node) => text(node)),
    text: all("p", user).slice(0, 2).map((node) => text(node)).join(" "),
    image: all("p", user).map((node) => (one("[data-slot=inline-image]", node) ? text(node) : null)).filter(Boolean),
  };
  // Web links: a globe until the daemon has the site's icon, kept when it has none; each site asked once.
  const icons = () => all("a", at("links")).map((link) => {
    const icon = one("[data-slot=site-icon]", link);
    return icon ? (icon.tagName === "IMG" ? icon.getAttribute("src") === NODE_ICON ? "icon" : "img" : icon.hasAttribute("data-mail") ? "mail" : "globe") : "none";
  });
  const before = icons();
  letIconsCome();
  for (const end = performance.now() + 3000; performance.now() < end && !icons().includes("icon"); ) await frame();
  await wait(100);
  out.links = { before, after: icons(), asked: siteAsks.toSorted() };
  // A link's address on focus or hover; its icon held to its first word; a dashed underline on hover.
  const firstLink = one<HTMLAnchorElement>("a", at("links"));
  firstLink?.focus();
  for (const end = performance.now() + 2000; performance.now() < end && !one("[data-slot=link-address]"); ) await frame();
  const menuItems = async (target: Element | null | undefined, slot: string) => {
    target?.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 40, clientY: 40 }));
    for (const end = performance.now() + 2000; performance.now() < end && !one(`[data-slot=${slot}]`); ) await frame();
    const items = all("[role=menuitem]", one(`[data-slot=${slot}]`) ?? undefined).map((item) => text(item));
    document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    for (const end = performance.now() + 2000; performance.now() < end && one(`[data-slot=${slot}]`); ) await frame();
    return items;
  };
  out.linkExtras = {
    address: text(one("[data-slot=link-address]")),
    joined: firstLink?.querySelector("[data-slot=site-icon] + span")?.textContent === "\u2060",
    dashed: firstLink?.className.includes("hover:decoration-dashed") ?? false,
    menu: await menuItems(firstLink, "link-menu"),
    mailMenu: await menuItems(all("a", at("links")).at(-1), "link-menu"),
    chipMenu: await menuItems(one("[data-slot=file-chip]", at("chips")), "file-chip-menu"),
  };
  // A worker's diff in its dialog: within the window, its files scrolling, none squashed.
  const host = document.createElement("div");
  document.body.append(host);
  createRoot(host).render(
    <TooltipProvider>
      <ArtifactDialog artifact={{ id: "a", title: "Diff of task-1", kind: "diff", mime: "text/x-diff", bytes: bigPatch.length, fileName: null }} onOpenChange={() => {}} />
    </TooltipProvider>,
  );
  for (const end = performance.now() + 3000; performance.now() < end && all("[data-slot=diff-card]", host.ownerDocument).length < 6 + 4; ) await frame();
  await wait(300);
  const dialog = one("[role=dialog]");
  const list = one("[data-slot=artifact-diff]");
  out.diffDialog = {
    fits: !!dialog && dialog.getBoundingClientRect().top >= 0 && dialog.getBoundingClientRect().bottom <= innerHeight,
    files: all("[data-slot=diff-card]", list ?? document.createElement("div")).length,
    scrolls: !!list && list.scrollHeight > list.clientHeight,
    // Each file's card as tall as its 40 lines: none squeezed to fit.
    whole: all<HTMLElement>("[data-slot=diff-card] [data-slot=diff-lines]", list ?? document.createElement("div")).every((lines) => lines.scrollHeight <= lines.clientHeight + 1),
  };
  // A finished group: its summary, its icon from the summary's first segment, and inside it
  // the command's time; a row on its own has none.
  const doneTrigger = one<HTMLElement>("[data-slot=work-group] button", at("done-group"));
  out.doneGroup = { row: text(doneTrigger), icon: one("[data-icon]", doneTrigger ?? undefined)?.getAttribute("data-icon") ?? null };
  doneTrigger?.click();
  await wait(400);
  out.doneChildren = all("[data-slot=child]", at("done-group")).map((row) => text(row));
  out.loneRan = text(at("lone-ran").firstElementChild);
  out.kindIcons = Object.fromEntries(kinds.map(({ key }) => [key, one(`[data-slot=kind-${key}] [data-icon]`)?.getAttribute("data-icon") ?? null]));
  const searchRow = at("search").firstElementChild;
  out.search = { row: text(searchRow), opens: !!one("[aria-expanded]", at("search")) || searchRow?.getAttribute("role") === "button" };
  return out;
}

void measure()
  .catch((error: unknown) => ({ error: String(error) }))
  .then(async (result) => {
    if (query.get("stop") === "1") await new Promise(() => {});
    const out = document.createElement("pre");
    out.id = "thread-rows-result";
    out.textContent = JSON.stringify(result);
    document.body.append(out);
  });
