/* oxlint-disable brigadier/no-raw-design-values -- Fixed widths stand in for the thread column. */
/**
 * A standalone Vite entry, excluded from the production build: the strips above the composer
 * (`ComposerStatusSlot` in a real rail over the composer's card) from synthetic thread views:
 * the session's workers, its own subagents (its CLI's Agent calls, as plain rows) and its own commands, with a wait folded into the
 * test it watches and a command queued behind a build. `?scene=` shows one of them alone
 * (`subagents`, `checks`, `all`, `long`, `empty`). `drive=1` opens each strip, opens a check's
 * output, presses a Stop and Stop all, and writes what it saw as JSON into
 * `#composer-rails-result`; `ComposerRailsRender.test.ts` checks it. IPC stays mocked.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { type FC, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ComposerCard } from "@/app/conversation/ComposerCard";
import { ComposerStatusSlot } from "@/app/conversation/ComposerStatusSlot";
import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import { ComposerRail } from "@/components/conversation";
import { TooltipProvider } from "@/components/overlays";
import { rowOf, showRows } from "@/fixtures/workerRows";
import type { BackgroundTask, FileEntry, Request, Task, WorkerRow } from "@/ipc/generated";
import { sessionTabs } from "@/state/sessionTabs";

const now = Date.now();
const query = new URLSearchParams(location.search);

/** The worker or subagent whose thread a strip row opened. */
let panelOpened: string | null | undefined;

/** Every request the page made, by method and task. */
const calls: string[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "stopBackgroundTask") calls.push(`stopBackgroundTask ${req.taskId}`);
  if (req.method === "readBackgroundTaskOutput") return { method: req.method, output: "ok\n" };
  return { method: req.method };
});

const worker = (conversationId: string, id: string, number: number, title: string, agoMs: number, patch: Partial<WorkerRow>): WorkerRow =>
  rowOf({ id, number, title, conversationId, state: "running", createdAtMs: now - agoMs, updatedAtMs: now - 5_000, route: null } as unknown as Task, patch);

const task = (id: string, kind: BackgroundTask["kind"], label: string, agoMs: number, patch: Partial<BackgroundTask> = {}): BackgroundTask => ({
  id, itemId: `call-${id}`, kind, label, status: "running", startedAtMs: now - agoMs, endedAtMs: null,
  outputFile: kind === "shell" ? `/private/tmp/cli/session/tasks/${id}.output` : null, summary: null, lastLine: null, ...patch,
});
const ended = (status: BackgroundTask["status"], agoMs: number, summary: string | null = null): Partial<BackgroundTask> => ({ status, endedAtMs: now - agoMs, summary });

/** The calls that started them, as the session's thread holds them. */
const calledAs = (id: string, name: string, input: Record<string, unknown>): FileEntry => ({
  event: { type: "toolCall", itemId: `call-${id}`, name, input: JSON.stringify(input), status: "inProgress", output: null },
});
const bash = (id: string, command: string, description: string) => calledAs(id, "Bash", { command, description, run_in_background: true });
const agent = (id: string, type: string, description: string) => calledAs(id, "Agent", { description, prompt: "…", subagent_type: type, run_in_background: true });

const SUBAGENTS = [
  task("sa1", "agent", "Find where the composer strips are drawn", 48_000),
  task("sa2", "agent", "Read the parser change for mistakes", 63_000),
  task("sa3", "agent", "Outline the three migration steps", 130_000, ended("completed", 4_000)),
  task("sa4", "agent", "Search the old crash logs", 34_000, ended("failed", 3_000)),
];
const SUBAGENT_CALLS = [agent("sa1", "Explore", "Find where the composer strips are drawn"), agent("sa2", "code-review", "Read the parser change for mistakes"),
  agent("sa3", "Plan", "Outline the three migration steps"), agent("sa4", "Explore", "Search the old crash logs")];

const tests = task("ct1", "shell", "Run full core tests", 134_000, { lastLine: "✓ parser/reader.test.ts (48 tests) 2.1s" });
const CHECKS = [
  tests,
  // Watches the tests' own output: folded into their row.
  task("ct2", "shell", "Wait for the full core tests to finish", 120_000),
  task("ct3", "shell", "Build the desktop app", 58_000, { lastLine: "transforming (812) src/app/conversation/Composer.tsx" }),
  // Waits for the build's log, then runs the smoke test: queued behind it.
  task("ct4", "shell", "Smoke test the built app", 50_000),
  task("ct5", "shell", "Lint the changed files", 30_000, { ...ended("completed", 4_000, "Background command completed (exit code 1)"), lastLine: "2 problems (2 errors, 0 warnings)" }),
  // Waits on a process by its number: nothing says which command that is, so a row of its own.
  task("ct6", "shell", "Wait for the benchmark process to exit", 20_000),
  task("ct7", "shell", "Type check", 90_000, ended("completed", 2_000, "Background command completed (exit code 0)")),
];
const CHECK_CALLS = [
  bash("ct1", "pnpm --filter core test 2>&1; echo \"test exit $?\"", "Run full core tests"),
  bash("ct2", `until grep -q "^test exit" /private/tmp/cli/session/tasks/ct1.output 2>/dev/null; do sleep 10; done; cat /private/tmp/cli/session/tasks/ct1.output`, "Wait for the full core tests to finish"),
  bash("ct3", "pnpm --filter desktop build > /tmp/rails/build.log 2>&1", "Build the desktop app"),
  bash("ct4", "until grep -q 'built in' /tmp/rails/build.log; do sleep 5; done && pnpm smoke", "Smoke test the built app"),
  bash("ct5", "pnpm lint", "Lint the changed files"),
  bash("ct6", "while kill -0 25996 2>/dev/null; do sleep 20; done; tail -30 /tmp/w16/bench-full.log", "Wait for the benchmark process to exit"),
  bash("ct7", "pnpm typecheck", "Type check"),
];

const LONG_SUBAGENTS = [
  task("ls1", "agent", "Find every place the composer rail decides which strips to show and in what order", 168_000),
  task("ls2", "agent", "Read the whole change to the approval cards for anything that could run a command without asking", 243_000),
];
const LONG_CHECKS = [
  task("lc1", "shell", "Run the core tests with the slow integration suite against a fresh database", 734_000, { lastLine: "✓ migrations/stored-sessions.test.ts (212 tests) 48.2s — moving 1,204 stored sessions to the new thread format" }),
  task("lc2", "shell", "Wait for the slow integration suite", 700_000),
];
const LONG_CALLS = [
  agent("ls1", "general-purpose", "Find every place the composer rail decides which strips to show and in what order"),
  agent("ls2", "security-review", "Read the whole change to the approval cards for anything that could run a command without asking"),
  bash("lc1", "pnpm --filter core test --integration --database fresh", "Run the core tests with the slow integration suite against a fresh database"),
  bash("lc2", "until grep -q '^exit' /private/tmp/cli/session/tasks/lc1.output; do sleep 20; done; tail -40 /private/tmp/cli/session/tasks/lc1.output", "Wait for the slow integration suite"),
];

const SCENES = {
  subagents: { caption: "Subagents alone: two working, one done, one failed", workers: [], background: SUBAGENTS, entries: SUBAGENT_CALLS },
  checks: { caption: "Checks alone: the wait folded into the tests, a smoke test queued behind the build, a failed lint", workers: [], background: CHECKS, entries: CHECK_CALLS },
  all: {
    caption: "All three, Checks nearest the composer",
    workers: [
      worker("all", "w1", 1, "Parser fix", 312_000, { doing: { type: "call", step: { name: "shell", detail: "pnpm test parser" } }, diff: { insertions: 42, deletions: 7, files: [] } }),
      worker("all", "w2", 2, "Readme draft", 95_000, { waiting: [{ kind: "permission", what: "pnpm install", card: null }] }),
    ],
    background: [...SUBAGENTS.slice(0, 3), tests, CHECKS[1]!, CHECKS[6]!],
    entries: [...SUBAGENT_CALLS, ...CHECK_CALLS],
  },
  long: {
    caption: "Long names",
    workers: [worker("long", "l1", 1, "Rework the composer's attachment tray so pasted screenshots keep their order", 1_212_000, { doing: { type: "said", line: "Checking every attachment path in the composer before changing the order" } })],
    background: [...LONG_SUBAGENTS, ...LONG_CHECKS],
    entries: LONG_CALLS,
  },
  // What ended a while ago: no strip.
  empty: {
    caption: "Nothing live: no strips",
    workers: [],
    background: [task("e1", "agent", "Plan the steps", 200_000, ended("completed", 40_000)), task("e2", "shell", "Type check", 200_000, ended("completed", 60_000, "Background command completed (exit code 0)"))],
    entries: [],
  },
} satisfies Record<string, { caption: string; workers: WorkerRow[]; background: BackgroundTask[]; entries: FileEntry[] }>;
type Scene = keyof typeof SCENES;

for (const [id, scene] of Object.entries(SCENES)) showRows(id, scene.workers, { background: scene.background, entries: scene.entries });

const asked = query.get("scene") as Scene | null;
const shown = (asked && asked in SCENES ? [asked] : Object.keys(SCENES)) as Scene[];

/** One session's rail over its composer, at a thread column's width. */
const Stage: FC<{ id: Scene }> = ({ id }) => (
  <figure className="flex w-[720px] flex-col gap-2" data-scene={id}>
    <figcaption className="text-foreground/60 text-xs">{SCENES[id].caption}</figcaption>
    <div className="flex flex-col">
      <ComposerRail>
        <ComposerStatusSlot conversationId={id} />
      </ComposerRail>
      <ComposerCard>
        <p className="text-foreground/40 min-h-16 px-2 py-1 text-sm">Ask for follow-up changes</p>
      </ComposerCard>
    </div>
  </figure>
);

const frame = () => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 0)));
const scene = (id: Scene) => document.querySelector<HTMLElement>(`[data-scene="${id}"]`)!;
const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, " ").trim() ?? null;

/** Opens every strip of a scene and reads it: each strip's closed line, then its rows. */
async function read(id: Scene) {
  const strips = [...scene(id).querySelectorAll<HTMLElement>('[data-slot="composer-rail-item"]')];
  const closed = strips.map((strip) => text(strip.querySelector("button[aria-expanded]")));
  for (const strip of strips) strip.querySelector<HTMLButtonElement>('button[aria-expanded="false"]')?.click();
  await frame();
  return {
    order: strips.map((strip) => strip.getAttribute("aria-label")),
    closed,
    subagents: [...scene(id).querySelectorAll('[data-slot="subagents-strip"] [data-slot="subagents-strip-row"]')].map((row) => ({
      state: row.getAttribute("data-worker-state"),
      text: text(row),
      stop: !!row.parentElement?.querySelector('[data-slot="worker-stop"]'),
    })),
    checks: [...scene(id).querySelectorAll('[data-slot="checks-strip"] [data-slot="check-row"]')].map((row) => ({
      state: row.getAttribute("data-state"),
      text: text(row),
      waitedOn: !!row.querySelector('[data-slot="strip-waited-on"]'),
      stop: !!row.parentElement?.querySelector('[data-slot="check-stop"]'),
      opens: row.tagName === "BUTTON",
    })),
  };
}

async function drive() {
  await frame();
  const seen: Record<string, unknown> = {
    emptyItems: scene("empty").querySelectorAll('[data-slot="composer-rail-item"]').length,
    all: await read("all"),
    subagents: await read("subagents"),
    checks: await read("checks"),
  };
  // A check's row opens what it printed, as the summary card's Background row does.
  [...scene("checks").querySelectorAll<HTMLButtonElement>('[data-slot="checks-strip"] button[data-slot="check-row"]')].find((row) => row.textContent?.includes("Build the desktop app"))?.click();
  await frame();
  seen.opened = sessionTabs("checks").active;
  // A subagent's row opens its transcript, as a worker's does.
  scene("subagents").querySelector<HTMLButtonElement>('[data-slot="subagents-strip"] [data-slot="subagents-strip-row"]')?.click();
  await frame();
  seen.panel = panelOpened;
  // Per-row Stop owns the producer; its watcher is independent.
  scene("checks").querySelector<HTMLButtonElement>('[aria-label="Stop Run full core tests"]')?.click();
  scene("subagents").querySelector<HTMLButtonElement>('[data-slot="subagents-strip-stop-all"]')?.click();
  await frame();
  seen.calls = calls.toSorted();
  return seen;
}

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (query.get("drive") === "1") void drive().then((seen) => setResult(JSON.stringify(seen)), (reason: unknown) => setResult(JSON.stringify({ error: String(reason) })));
  }, []);
  return (
    <TooltipProvider>
      <AgentsPanelContext.Provider value={{ panel: undefined, setPanel: (panel) => { panelOpened = panel; } }}>
      <main className="bg-background flex w-fit flex-col gap-8 p-8">
        {shown.map((id) => <Stage key={id} id={id} />)}
        <pre id="composer-rails-result" className="hidden">{result}</pre>
      </main>
      </AgentsPanelContext.Provider>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
