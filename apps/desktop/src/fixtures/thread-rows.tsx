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
 * - An answer's inline code: a file of the checkout is a chip (from the file list, or asked by
 *   its exact path when the list is cut short), anything else stays code.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import type { ReactNode } from "react";
import { createRoot } from "react-dom/client";

import { ActivityGroup, type Described, StepRow } from "@/app/conversation/activity/ActivityGroup";
import { describeLeadStep } from "@/app/conversation/activity/LeadStep";
import { ShellBox } from "@/app/conversation/activity/StepDetail";
import { StoppedBy } from "@/app/conversation/activity/stopped";
import { stepWords } from "@/app/conversation/activity/words";
import { InlineDiff } from "@/app/conversation/activity/InlineDiff";
import { MarkdownBlock } from "@/components/assistant-ui/thread";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, OrchestratorStep, Request } from "@/ipc/generated";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const CHAT = "thread-rows";
const ROOT = "/repo";
const asked: string[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "listFiles") return { method: req.method, files: ["src/cli.ts", "src/math.js"], truncated: true };
  if (req.method === "resolveFileLink") {
    asked.push(req.path);
    return { method: req.method, exists: req.path === `${ROOT}/src/made-later.ts`, stored: null };
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

const patch = "--- a/src/math.js\n+++ b/src/math.js\n@@ -1,4 +1,4 @@\n export function round(value) {\n-  return Math.round(value * 100) / 100;\n+  return Math.round((value + Number.EPSILON) * 100) / 100;\n }\n \n@@ -20,2 +20,3 @@\n const a = 1;\n+const b = 2;\n const c = 3;\n";
const answer = "The CLI parses it in `src/cli.ts:64`, the new helper is `src/made-later.ts`, rounding uses `Math.round`, and `src/gone.ts` was removed.";

const now = Date.now();
const longOutput = Array.from({ length: 60 }, (_, index) => (index === 59 ? "\u001b[32m✓ 60 tests passed\u001b[0m" : `line ${index + 1}`)).join("\n");
const longCommand = `pnpm vitest run ${Array.from({ length: 40 }, (_, index) => `src/module-${index}.test.ts`).join(" ")}`;

const command = (line: string, status: Described["status"], more: Partial<Described> = {}): Described => ({
  words: stepWords({ name: "shell", detail: line, status }),
  status,
  ...more,
});

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
        <Case id="search">
          <StepRow {...describeLeadStep(search)} />
        </Case>
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
        <Case id="chips">
          <MarkdownBlock text={answer} />
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
  const diff = (id: string) => ({
    name: text(one("[data-slot=diff-name]", at(id))),
    counts: text(one("[data-slot=diff-counts]", at(id))),
    numbers: all("[data-slot=diff-number]", at(id)).map((cell) => text(cell)),
    kinds: all("[data-slot=diff-lines] [data-kind]", at(id)).map((row) => row.getAttribute("data-kind")),
    gaps: all("[data-slot=diff-gap]", at(id)).map((row) => text(row)),
    marks: all("mark", at(id)).map((mark) => mark.textContent),
    empty: text(one("[data-slot=diff-empty]", at(id))),
    coloured: all("[data-slot=diff-lines] span[style]", at(id)).length > 0,
  });
  // Syntax colours come once the grammar loads.
  for (const end = performance.now() + 8000; performance.now() < end && !diff("diff-patch").coloured; ) await frame();
  for (const id of ["diff-patch", "diff-edit", "diff-empty", "diff-missing"]) out[id] = diff(id);
  // The answer's chips, once the checkout's files are known.
  for (const end = performance.now() + 8000; performance.now() < end && all("[data-slot=file-chip]", at("chips")).length < 2; ) await frame();
  await wait(300);
  out.chips = all("[data-slot=file-chip]", at("chips")).map((chip) => [text(chip), chip.getAttribute("href")]);
  out.code = all("code", at("chips")).map((code) => text(code));
  out.asked = asked.toSorted();
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
