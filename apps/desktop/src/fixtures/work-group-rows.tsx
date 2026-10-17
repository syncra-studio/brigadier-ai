/**
 * A standalone Vite entry, excluded from the app entry and production build: rows inside an
 * opened work group, measured once the page holds still. IPC is mocked, no daemon. The result
 * goes to `<pre id="work-group-rows-result">`; `stop=1` holds the page for a screenshot.
 *
 * - A finished group of commands, opened and scrolled to its end: its last row opens to its Shell
 *   box, and the box's last line, how it ended, shows in the group's list.
 * - A running worker's view whose newest group is one finished command: "Thinking" is that row's
 *   own header, not a line under it.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ActivityGroup, StepRow } from "@/app/conversation/activity/ActivityGroup";
import { ShellBox } from "@/app/conversation/activity/StepDetail";
import { stepWords } from "@/app/conversation/activity/words";
import { WorkerThread } from "@/app/conversation/WorkerThread";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { RawEntry, Request, Task } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";

const query = new URLSearchParams(location.search);

const CHAT = "work-group-rows";
const now = Date.now();

/** The worker's transcript: one command, done; nothing after it yet. */
const workerEntries: RawEntry[] = [
  { streamSeq: 1, atMs: now - 2000, event: { type: "command", itemId: "ls", command: "ls .", cwd: null, status: "completed", exitCode: 0, output: "a.ts", durationMs: 200 } },
];

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "listWorkerEvents") return { method: req.method, page: { entries: workerEntries, next: null } };
  return { method: req.method };
});

const worker = {
  id: "t1", number: 1, title: "File summary", conversationId: CHAT, state: "running", spec: "List the files.",
  createdAtMs: now - 10_000, updatedAtMs: now - 2000, attempts: [], outputs: [], report: null, messages: [], error: null,
} as unknown as Task;

useBoard.setState({
  board: { ...emptyBoard(CHAT), loaded: true, transcripts: { t1: { entries: workerEntries, loading: false, hasMore: false } } } as unknown as ReturnType<typeof emptyBoard>,
});

/** A finished command in a group, its row opening to its Shell box. */
type Ran = { key: string; command: string };
const commands: Ran[] = Array.from({ length: 12 }, (_, index) => ({ key: `run-${index}`, command: `npm test -- part${index + 1}` }));

function Page() {
  return (
    <TooltipProvider>
      <main className="bg-background text-foreground flex min-h-screen w-full max-w-thread flex-col gap-6 p-8">
        <section data-case="open-in-group">
          <ActivityGroup<Ran>
            items={commands.map((step) => ({ type: "step", key: step.key, step }))}
            live={false}
            describe={(step) => ({ words: stepWords({ name: "shell", detail: step.command, status: "completed" }), status: "completed", exit: 0 })}
            renderStep={(step, key) => (
              <StepRow
                key={key}
                words={stepWords({ name: "shell", detail: step.command, status: "completed" })}
                status="completed"
                exit={0}
                detail={<ShellBox command={step.command} output="ok" status="completed" exit={0} />}
              />
            )}
          />
        </section>
        <section data-case="worker-lone" className="flex h-96 flex-col">
          <WorkerThread task={worker} />
        </section>
      </main>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Page />);

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const one = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => root.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [...root.querySelectorAll<T>(selector)];
const at = (id: string) => one(`[data-case=${id}]`)!;

async function measure(): Promise<object> {
  await wait(300);
  const out: Record<string, unknown> = {};

  // The group opened and scrolled to its end; its last row opens below the list's bottom edge.
  one<HTMLElement>("[data-slot=work-group] > button", at("open-in-group"))?.click();
  await wait(500);
  const list = one("[data-slot=work-group] [data-slot=collapsible-content] > div", at("open-in-group"))!;
  list.scrollTop = list.scrollHeight;
  await wait(100);
  all<HTMLElement>("[role=button]", list).at(-1)?.click();
  await wait(800);
  const shown = list.getBoundingClientRect();
  const ending = one("[data-slot=shell-ending]", list)?.getBoundingClientRect();
  out.openInGroup = {
    scrolls: list.scrollHeight > list.clientHeight,
    endingShown: !!ending && ending.top >= shown.top && ending.bottom <= shown.bottom,
  };

  // The worker's newest group is one finished command: its own row says "Thinking", no line under it.
  const thread = one("[data-slot=worker-thread]", at("worker-lone"));
  const column = thread?.querySelector(":scope > div");
  out.workerLone = {
    rowLine: one("[data-slot=group-live-line]", thread ?? document)?.textContent ?? null,
    linesUnder: [...(column?.children ?? [])].filter((child) => !child.matches("[data-slot=work-group]") && child.textContent === "Thinking").length,
  };
  return out;
}

void measure()
  .catch((error: unknown) => ({ error: String(error) }))
  .then(async (result) => {
    if (query.get("stop") === "1") await new Promise(() => {});
    const out = document.createElement("pre");
    out.id = "work-group-rows-result";
    out.textContent = JSON.stringify(result);
    document.body.append(out);
  });
