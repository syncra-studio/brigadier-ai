/* oxlint-disable brigadier/no-raw-design-values -- A fixed width stands for a 760 px window's column. */
/**
 * A standalone Vite entry, excluded from the app entry and production build: rows inside an
 * opened work group, measured once the page holds still. IPC is mocked, no daemon. The result
 * goes to `<pre id="work-group-rows-result">`; `stop=1` holds the page for a screenshot.
 *
 * - A finished group of commands, opened and scrolled to its end: its last row opens to its Shell
 *   box, and the box's last line, how it ended, shows in the group's list.
 * - A running worker's view whose newest group is one finished command: "Thinking" is that row's
 *   own header, not a line under it.
 * - A group holding an edited file, opened to its diff and scrolled down it, in the 1280 px
 *   window's column and in a 760 px window's: the diff card's header stays in the group's list,
 *   clear of the group's own row above it.
 * - A live group of one running command, opened to its Shell box: the box stays open when the
 *   command ends and the group's row says "Thinking", and when the live line moves on.
 *
 * `drive=motion` instead opens and closes the first group and a row in it, reading each body's
 * state and the animation it runs as it starts ("none" with reduced motion).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ActivityGroup, StepRow } from "@/app/conversation/activity/ActivityGroup";
import { FileChangeRows } from "@/app/conversation/activity/EditedFiles";
import { InlineDiff } from "@/app/conversation/activity/InlineDiff";
import { ShellBox } from "@/app/conversation/activity/StepDetail";
import { stepWords } from "@/app/conversation/activity/words";
import { WorkerThread } from "@/app/side-panel/WorkerThread";
import { TooltipProvider } from "@/components/overlays";
import type { ItemStatus, RawEntry, Request, Task } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { rowOf, showRows, threadOf } from "@/fixtures/workerRows";

const query = new URLSearchParams(location.search);

/** An edit long enough to scroll: 60 lines changed. */
const LONG_DIFF = `@@ -1,30 +1,30 @@\n${Array.from({ length: 30 }, (_, index) => `-const v${index} = ${index};\n+const v${index} = ${index * 10};`).join("\n")}\n`;
const edited = { path: "/repo/src/math.js", insertions: 30, deletions: 30, binary: false, change: "update" as const };
const editDiffs = new Map([[edited.path, <InlineDiff key={edited.path} path={edited.path} diff={LONG_DIFF} />]]);

/** A finished group: a command, then the edit. */
function EditGroup() {
  return (
    <ActivityGroup<string>
      items={["ran", "edit"].map((step) => ({ type: "step", key: step, step }))}
      live={false}
      describe={(step) => ({ words: step === "ran" ? stepWords({ name: "shell", detail: "npm test", status: "completed" }) : { kind: "edit", doing: "Editing math.js", done: "Edited math.js", object: "math.js" }, status: "completed", exit: 0 })}
      renderStep={(step, key) => step === "ran"
        ? <StepRow key={key} words={stepWords({ name: "shell", detail: "npm test", status: "completed" })} status="completed" exit={0} />
        : <FileChangeRows key={key} files={[edited]} status="completed" diffs={editDiffs} />}
    />
  );
}

const CHAT = "work-group-rows";
const now = Date.now();

/** The worker's transcript: one command, done; nothing after it yet. */
const workerEntries: RawEntry[] = [
  { streamSeq: 1, atMs: now - 2000, event: { type: "command", itemId: "ls", command: "ls .", cwd: null, status: "completed", exitCode: 0, output: "a.ts", durationMs: 200 } },
];

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "workerThread") return { method: req.method, view: threadOf(workerEntries) };
  return { method: req.method };
});

const worker = {
  id: "t1", number: 1, title: "File summary", conversationId: CHAT, state: "running", spec: "List the files.",
  createdAtMs: now - 10_000, updatedAtMs: now - 2000, attempts: [], outputs: [], report: null, messages: [], error: null,
} as unknown as Task;

useBoard.setState({ board: { ...emptyBoard(CHAT), loaded: true } });
const workerRow = rowOf(worker, { lastEventAtMs: now - 2000 });
showRows(CHAT, [workerRow]);

/** A finished command in a group, its row opening to its Shell box. */
type Ran = { key: string; command: string };
const commands: Ran[] = Array.from({ length: 12 }, (_, index) => ({ key: `run-${index}`, command: `npm test -- part${index + 1}` }));

/** One command in the live turn's last group; the test moves it on: it ends, then a thought starts. */
type Lone = { status: ItemStatus; endedAtMs?: number; line: boolean };
const moveLone = (lone: Lone) => window.dispatchEvent(new CustomEvent<Lone>("move-lone", { detail: lone }));

function LoneLive() {
  const [lone, setLone] = useState<Lone>({ status: "inProgress", line: true });
  useEffect(() => {
    const move = (event: Event) => setLone((event as CustomEvent<Lone>).detail);
    window.addEventListener("move-lone", move);
    return () => window.removeEventListener("move-lone", move);
  }, []);
  const words = stepWords({ name: "shell", detail: "npm test", status: lone.status });
  return (
    <ActivityGroup<string>
      items={[{ type: "step", key: "run", step: "run" }]}
      live
      liveLabel={lone.line ? { text: "Thinking", tone: "busy" } : null}
      describe={() => ({ words, status: lone.status, exit: 0, endedAtMs: lone.endedAtMs })}
      renderStep={(_, key) => <StepRow key={key} words={words} status={lone.status} exit={0} detail={<ShellBox command="npm test" output="ok" status={lone.status} exit={0} />} />}
    />
  );
}

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
        <section data-case="edit-wide">
          <EditGroup />
        </section>
        {/* A 760 px window's thread column: the window less its margins. */}
        <section data-case="edit-narrow" style={{ width: 728 }}>
          <EditGroup />
        </section>
        <section data-case="lone-live">
          <LoneLive />
        </section>
        <section data-case="worker-lone" className="flex h-96 flex-col">
          <WorkerThread conversationId={CHAT} row={workerRow} />
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

  // An opened edit in an opened group, scrolled down its diff the way a wheel would: the
  // innermost scroller first, then the ones around it, up to the group's list.
  for (const id of ["edit-wide", "edit-narrow"]) {
    one<HTMLElement>("[data-slot=work-group] > button", at(id))?.click();
    await wait(500);
    one<HTMLElement>("[data-slot=edited-file] [role=button]", at(id))?.click();
    await wait(800);
    const group = one("[data-slot=work-group]", at(id))!;
    const groupList = one("[data-slot=work-group] [data-slot=collapsible-content] > div", at(id))!;
    const card = one("[data-slot=diff-card]", at(id))!;
    let remaining = card.getBoundingClientRect().top - groupList.getBoundingClientRect().top + 40;
    for (let element = card.parentElement; element && remaining > 0; element = element.parentElement) {
      if (element.scrollHeight <= element.clientHeight) continue;
      const before = element.scrollTop;
      element.scrollTop += remaining;
      remaining -= element.scrollTop - before;
      if (element === groupList) break;
    }
    await wait(100);
    const header = card.firstElementChild!.getBoundingClientRect();
    const row = one("button", group)!.getBoundingClientRect();
    const visible = groupList.getBoundingClientRect();
    const fade = parseFloat(getComputedStyle(groupList).getPropertyValue("--spacing-scroll-fade-top")) * parseFloat(getComputedStyle(document.documentElement).fontSize);
    out[id] = {
      scrolled: remaining <= 0,
      clearOfGroupRow: header.top >= row.bottom,
      clearOfFade: header.top >= visible.top + fade - 0.5,
      inList: header.bottom <= visible.bottom,
    };
  }

  // The worker's newest group is one finished command: its own row says "Thinking", no line under it.
  const thread = one("[data-slot=worker-thread]", at("worker-lone"));
  const column = thread?.querySelector(":scope > div");
  out.workerLone = {
    rowLine: one("[data-slot=group-live-line]", thread ?? document)?.textContent ?? null,
    linesUnder: [...(column?.children ?? [])].filter((child) => !child.matches("[data-slot=work-group]") && child.textContent === "Thinking").length,
  };

  // A lone running command opened to its Shell box; it ends, then the live line moves on.
  one<HTMLElement>("[role=button]", at("lone-live"))?.click();
  await wait(500);
  const boxOpen = () => !!one("[data-slot=shell-output]", at("lone-live"))?.checkVisibility();
  const opened = boxOpen();
  moveLone({ status: "completed", endedAtMs: Date.now() - 2000, line: true });
  await wait(1500);
  const ended = boxOpen();
  moveLone({ status: "completed", endedAtMs: Date.now() - 4000, line: false });
  await wait(500);
  out.loneLive = { opened, ended, movedOn: boxOpen() };
  return out;
}

/** A fold's body: its state and the animation it runs. */
const read = (body: Element | null | undefined) => (body ? { state: body.getAttribute("data-state"), animation: getComputedStyle(body).animationName } : null);

/** The group and its first row opened, then closed: each body's state and animation as it starts. */
async function motion(): Promise<object> {
  await wait(300);
  const group = one("[data-slot=work-group]", at("open-in-group"))!;
  const groupBody = () => one(":scope > [data-slot=collapsible-content]", group);
  const row = () => one("[data-slot=work-group-list] > [data-kind]", group);
  const toggleGroup = () => one<HTMLElement>(":scope > button", group)!.click();
  const toggleRow = () => one<HTMLElement>("[role=button]", row()!)!.click();
  const out: Record<string, unknown> = {};
  toggleGroup();
  await wait(16);
  out.groupOpens = read(groupBody());
  await wait(500);
  toggleRow();
  await wait(16);
  out.rowOpens = read(one(":scope > [data-slot=collapsible-content]", row()!));
  await wait(500);
  toggleRow();
  await wait(16);
  out.rowCloses = read(one(":scope > [data-slot=collapsible-content]", row()!));
  await wait(500);
  toggleGroup();
  await wait(16);
  out.groupCloses = read(groupBody());
  return out;
}

void (query.get("drive") === "motion" ? motion() : measure())
  .catch((error: unknown) => ({ error: String(error) }))
  .then(async (result) => {
    if (query.get("stop") === "1") await new Promise(() => {});
    const out = document.createElement("pre");
    out.id = "work-group-rows-result";
    out.textContent = JSON.stringify(result);
    document.body.append(out);
  });
