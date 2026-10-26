/* oxlint-disable brigadier/no-raw-design-values -- Fixed widths stand in for the thread column. */
/**
 * Mockup only, a standalone Vite entry excluded from the production build: the strips above the
 * composer with a Subagents strip and a Checks strip (the session's own tests, builds,
 * verifications and waits) beside the real Workers strip, in three ways of stacking them.
 * `?board=1..7` picks a board of scenes; nothing reaches a daemon.
 */
import { type ComponentProps, type FC, type ReactNode, useEffect, useRef } from "react";
import { createRoot } from "react-dom/client";

import { WorkersStrip, useWorkersStrip } from "@/app/conversation/activity/WorkersStrip";
import { ComposerCard } from "@/app/conversation/ComposerCard";
import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import { ComposerRail, ComposerRailItem } from "@/components/conversation";
import { TooltipProvider } from "@/components/overlays";
import { MockGroupedStrip, type MockLine, MockSplitStrip, MockStrip, type WorkersPart } from "@/fixtures/composer-rails-strips";
import { rowOf, showRows } from "@/fixtures/workerRows";
import type { Task, WorkerRow } from "@/ipc/generated";

const now = Date.now();
const worker = (conversationId: string, id: string, number: number, title: string, agoMs: number, patch: Partial<WorkerRow>): WorkerRow =>
  rowOf({ id, number, title, conversationId, state: "running", createdAtMs: now - agoMs, updatedAtMs: now - 5_000, route: null } as unknown as Task, patch);

// The session's workers, as the real strip reads them.
const SHORT = "rails";
const LONG = "rails-long";
showRows(SHORT, [
  worker(SHORT, "w1", 1, "Parser fix", 312_000, { doing: { type: "call", step: { name: "shell", detail: "pnpm test parser" } }, diff: { insertions: 42, deletions: 7, files: [] } }),
  worker(SHORT, "w2", 2, "Readme draft", 95_000, { waiting: [{ kind: "permission", what: "pnpm install", card: null }] }),
]);
showRows(LONG, [
  worker(LONG, "l1", 1, "Rework the composer's attachment tray so pasted screenshots keep their order", 1_212_000, { doing: { type: "said", line: "Checking every attachment path in the composer and the rich composer input before changing the order" } }),
  worker(LONG, "l2", 2, "Investigate why the nightly updater sometimes reports a finished download twice", 455_000, { doing: { type: "thinking" } }),
]);
const workersPart = (conversationId: string, rows: string[]): WorkersPart => ({ conversationId, rows, working: 1, waiting: 1 });
const SHORT_WORKERS = workersPart(SHORT, ["w1", "w2"]);
const LONG_WORKERS = workersPart(LONG, ["l1", "l2"]);

// The session's own subagents.
const SUBAGENTS: MockLine[] = [
  { id: "s1", name: "Explore", doing: "Finding where the composer strips are drawn", state: "working", time: "48s" },
  { id: "s2", name: "Code review", doing: "Reading the parser change for mistakes", state: "working", time: "1m 3s" },
  { id: "s3", name: "Plan", doing: "Outlined the three migration steps", state: "done", word: "done", time: "2m 10s" },
];
const SUBAGENTS_FAILED: MockLine[] = [
  ...SUBAGENTS,
  { id: "s4", name: "Explore", doing: "Searching old crash logs", state: "failed", word: "failed: hit the usage limit", time: "34s" },
];
const SUBAGENTS_LONG: MockLine[] = [
  { id: "s1", name: "Explore the desktop app's conversation folder", doing: "Finding every place the composer rail decides which strips to show and in what order", state: "working", time: "2m 48s" },
  { id: "s2", name: "Security review", doing: "Reading the whole change to the approval cards for anything that could run a command without asking", state: "working", time: "4m 3s" },
  { id: "s3", name: "Plan", doing: "Outlined how the old stored sessions move to the new thread format without losing a message", state: "done", word: "done", time: "6m 10s" },
];

// The session's own commands: what each is, what it is for, and the one the session waits on.
const CHECKS: MockLine[] = [
  { id: "c1", name: "Core tests", doing: "Full run, to prove the parser change broke nothing", state: "working", time: "2m 14s", waitedOn: true, command: "pnpm --filter core test" },
  { id: "c2", name: "Type check", doing: "Makes sure the app still compiles after the rename", state: "passed", word: "passed", time: "41s", command: "pnpm typecheck" },
];
const CHECKS_ALL: MockLine[] = [
  CHECKS[0]!,
  { id: "c3", name: "Desktop build", doing: "Builds the app the screenshots are taken from", state: "working", time: "58s", command: "pnpm --filter desktop build" },
  { id: "c4", name: "Smoke test", doing: "Opens the built app and clicks through the composer", state: "queued", word: "starts after Desktop build", command: "pnpm smoke" },
  { id: "c5", name: "Lint", doing: "Style rules on the changed files", state: "failed", word: "failed: 2 problems", time: "12s", command: "pnpm lint" },
  { id: "c6", name: "Pause", doing: "Then checks the preview is up", state: "pausing", word: "1m 20s left", command: "sleep 120" },
];
const CHECKS_LONG: MockLine[] = [
  { id: "c1", name: "Core tests with the slow integration suite", doing: "Full run against a fresh database, to prove the migration of stored sessions broke nothing", state: "working", time: "12m 14s", waitedOn: true, command: "pnpm --filter core test --integration --database fresh" },
  { id: "c2", name: "Desktop production build", doing: "Builds the signed app the release notes' screenshots are taken from", state: "queued", word: "starts after Core tests with the slow integration suite" },
  { id: "c3", name: "Type check", doing: "Makes sure every package still compiles after the rename of the thread types", state: "passed", word: "passed", time: "1m 41s" },
];

/** Opens the real Workers strip of a scene, as a click on its count does. */
function useOpenWorkers(open: boolean) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (open) ref.current?.querySelector<HTMLButtonElement>('[data-slot="workers-strip"] button[aria-expanded="false"]')?.click();
  }, [open]);
  return ref;
}

/** The real Workers strip, in its own rail item, as `ComposerStatusSlot` puts it. */
const RealWorkers: FC<{ conversationId: string }> = ({ conversationId }) => {
  const view = useWorkersStrip(conversationId);
  return view ? (
    <ComposerRailItem label="Live work">
      <WorkersStrip conversationId={conversationId} view={view} />
    </ComposerRailItem>
  ) : null;
};

/** A rail over the composer, at a thread column's width. */
const Scene: FC<{ caption: string; note?: string; openWorkers?: boolean; children: ReactNode }> = ({ caption, note, openWorkers = false, children }) => {
  const ref = useOpenWorkers(openWorkers);
  return (
    <figure className="flex w-[720px] flex-col gap-2" data-scene={caption}>
      <figcaption className="flex flex-col gap-0.5">
        <span className="text-foreground text-sm font-medium">{caption}</span>
        {note && <span className="text-foreground/60 text-xs">{note}</span>}
      </figcaption>
      <div ref={ref} className="flex flex-col">
        <ComposerRail>{children}</ComposerRail>
        <ComposerCard>
          <p className="text-foreground/40 min-h-16 px-2 py-1 text-sm">Ask for follow-up changes</p>
        </ComposerCard>
      </div>
    </figure>
  );
};

/** Variant A: three strips, each its own rail item. */
const Stacked: FC<{ workers?: string; subagents?: MockLine[]; checks?: MockLine[]; open?: "subagents" | "checks" | "all" }> = ({ workers, subagents, checks, open }) => (
  <>
    {workers && <RealWorkers conversationId={workers} />}
    {subagents && (
      <ComposerRailItem label="Subagents">
        <MockStrip kind="subagents" lines={subagents} defaultOpen={open === "subagents" || open === "all"} />
      </ComposerRailItem>
    )}
    {checks && (
      <ComposerRailItem label="Checks">
        <MockStrip kind="checks" lines={checks} defaultOpen={open === "checks" || open === "all"} />
      </ComposerRailItem>
    )}
  </>
);

const Board: FC<{ title: string; intro: string; children: ReactNode }> = ({ title, intro, children }) => (
  <main className="bg-background flex w-fit flex-col gap-8 p-8">
    <header className="flex w-[720px] flex-col gap-1">
      <h1 className="text-foreground text-base font-semibold">{title}</h1>
      <p className="text-foreground/60 text-sm">{intro}</p>
    </header>
    {children}
  </main>
);

/** Variants B and C: one strip, in one rail item. */
const Split: FC<ComponentProps<typeof MockSplitStrip>> = (props) => (
  <ComposerRailItem label="Live work"><MockSplitStrip {...props} /></ComposerRailItem>
);
const Grouped: FC<ComponentProps<typeof MockGroupedStrip>> = (props) => (
  <ComposerRailItem label="Live work"><MockGroupedStrip {...props} /></ComposerRailItem>
);

const BOARDS: Record<string, () => ReactNode> = {
  1: () => (
    <Board title="A · Three strips, stacked · closed" intro="Each kind has its own strip, the Workers strip's look line for line: its icon, what it is about in words, a chevron to open it, and Stop all while something runs. A strip shows only while it has something.">
      <Scene caption="Workers alone (as today)"><Stacked workers={SHORT} /></Scene>
      <Scene caption="Subagents alone" note="The session's own helpers. Finished ones stay until the session's turn ends."><Stacked subagents={SUBAGENTS_FAILED} /></Scene>
      <Scene caption="Checks alone" note="The session's own tests, builds and checks. A wait is not a row: the line says what the session waits on."><Stacked checks={CHECKS} /></Scene>
      <Scene caption="All three"><Stacked workers={SHORT} subagents={SUBAGENTS} checks={CHECKS} /></Scene>
    </Board>
  ),
  2: () => (
    <Board title="A · Three strips, stacked · open" intro="A click on a strip's line opens its rows; each opens on its own. A row opens what it is (a subagent's transcript, a command's output); its stop sign stops only it. Hover a check for its command.">
      <Scene caption="Checks open" note="Core tests carries 'Session waits on this': the wait is part of the thing it waits on. A pause with nothing to wait on is one row with the time left."><Stacked workers={SHORT} subagents={SUBAGENTS} checks={CHECKS_ALL} open="checks" /></Scene>
      <Scene caption="Subagents open"><Stacked workers={SHORT} subagents={SUBAGENTS_FAILED} checks={CHECKS} open="subagents" /></Scene>
      <Scene caption="All three open" openWorkers><Stacked workers={SHORT} subagents={SUBAGENTS} checks={CHECKS} open="all" /></Scene>
    </Board>
  ),
  3: () => (
    <Board title="A · Three strips, stacked · long names" intro="Only the words truncate; the state, the wait mark, the time and Stop keep their room.">
      <Scene caption="All three, closed"><Stacked workers={LONG} subagents={SUBAGENTS_LONG} checks={CHECKS_LONG} /></Scene>
      <Scene caption="All three, open" openWorkers><Stacked workers={LONG} subagents={SUBAGENTS_LONG} checks={CHECKS_LONG} open="all" /></Scene>
    </Board>
  ),
  4: () => (
    <Board title="B · One strip, three parts" intro="One line for everything, a part for each kind that has something. A part opens its own rows below, one at a time; Stop all at the line's end stops the open part's.">
      <Scene caption="Checks alone"><Split checks={CHECKS} /></Scene>
      <Scene caption="All three, closed"><Split workers={SHORT_WORKERS} subagents={SUBAGENTS} checks={CHECKS} /></Scene>
      <Scene caption="Workers open"><Split workers={SHORT_WORKERS} subagents={SUBAGENTS} checks={CHECKS} defaultOpen="workers" /></Scene>
      <Scene caption="Subagents open"><Split workers={SHORT_WORKERS} subagents={SUBAGENTS_FAILED} checks={CHECKS} defaultOpen="subagents" /></Scene>
      <Scene caption="Checks open"><Split workers={SHORT_WORKERS} subagents={SUBAGENTS} checks={CHECKS_ALL} defaultOpen="checks" /></Scene>
    </Board>
  ),
  5: () => (
    <Board title="B · One strip, three parts · long names" intro="The parts share one line, so each says less: a count, or what the session waits on.">
      <Scene caption="All three, closed"><Split workers={LONG_WORKERS} subagents={SUBAGENTS_LONG} checks={CHECKS_LONG} /></Scene>
      <Scene caption="Checks open"><Split workers={LONG_WORKERS} subagents={SUBAGENTS_LONG} checks={CHECKS_LONG} defaultOpen="checks" /></Scene>
    </Board>
  ),
  6: () => (
    <Board title="C · One strip, one grouped list" intro="One line names all three; open, one list under three headings, each heading with its own Stop all.">
      <Scene caption="Checks alone, closed"><Grouped checks={CHECKS} /></Scene>
      <Scene caption="All three, closed"><Grouped workers={SHORT_WORKERS} subagents={SUBAGENTS} checks={CHECKS} /></Scene>
      <Scene caption="All three, open"><Grouped workers={SHORT_WORKERS} subagents={SUBAGENTS_FAILED} checks={CHECKS_ALL} defaultOpen /></Scene>
    </Board>
  ),
  7: () => (
    <Board title="C · One strip, one grouped list · long names" intro="Everything opens at once, so a long list scrolls inside the strip.">
      <Scene caption="All three, closed"><Grouped workers={LONG_WORKERS} subagents={SUBAGENTS_LONG} checks={CHECKS_LONG} /></Scene>
      <Scene caption="All three, open"><Grouped workers={LONG_WORKERS} subagents={SUBAGENTS_LONG} checks={CHECKS_LONG} defaultOpen /></Scene>
    </Board>
  ),
};

const board = new URLSearchParams(location.search).get("board") ?? "1";
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <AgentsPanelContext.Provider value={{ panel: undefined, setPanel: () => {} }}>
      {(BOARDS[board] ?? BOARDS[1]!)()}
    </AgentsPanelContext.Provider>
  </TooltipProvider>,
);
