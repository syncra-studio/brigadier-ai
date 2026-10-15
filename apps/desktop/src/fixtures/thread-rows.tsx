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
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import type { ReactNode } from "react";
import { createRoot } from "react-dom/client";

import { ActivityGroup, type Described, StepRow } from "@/app/conversation/activity/ActivityGroup";
import { describeLeadStep } from "@/app/conversation/activity/LeadStep";
import { ShellBox } from "@/app/conversation/activity/StepDetail";
import { StoppedBy } from "@/app/conversation/activity/stopped";
import { stepWords } from "@/app/conversation/activity/words";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { OrchestratorStep } from "@/ipc/generated";

const query = new URLSearchParams(location.search);
mockIPC(() => null);

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
