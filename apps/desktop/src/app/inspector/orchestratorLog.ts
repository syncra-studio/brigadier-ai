import type {
  ContextInjection,
  InjectionKind,
  OrchestratorLogEntry,
  RebirthRecord,
} from "@/ipc/generated";

/**
 * Injections grouped for the context chart. Messages and reports are what the orchestrator's
 * context should grow by; instructions (and a rebirth's reseed) are its briefing; anything
 * else is worth noticing, so it gets its own band.
 */
export type InjectionGroup = "instructions" | "messages" | "reports" | "other";

/** Stack order, bottom to top. Colors are validated for dark mode against the background
 * (CVD ΔE ≥ 14 between every pair); "other" is a recessive neutral, labeled in the legend. */
export const GROUPS: readonly {
  id: InjectionGroup;
  label: string;
  fill: string;
  swatch: string;
}[] = [
  { id: "instructions", label: "Instructions", fill: "fill-chart-4", swatch: "bg-chart-4" },
  { id: "messages", label: "Your messages", fill: "fill-chart-2", swatch: "bg-chart-2" },
  { id: "reports", label: "Worker reports", fill: "fill-chart-1", swatch: "bg-chart-1" },
  {
    id: "other",
    label: "Other",
    fill: "fill-muted-foreground/50",
    swatch: "bg-muted-foreground/50",
  },
];

export const KIND_LABELS: Record<InjectionKind, string> = {
  instructions: "instructions",
  userMessage: "user message",
  report: "report",
  workerQuestion: "worker question",
  decision: "decision",
  taskFailed: "task failed",
  toolResult: "tool result",
  artifact: "artifact",
  reseed: "reseed",
  resume: "resume",
  followUp: "follow-up",
  briefing: "briefing",
  reminder: "reminder",
  run: "overnight run",
};

export function groupOf(kind: InjectionKind): InjectionGroup {
  switch (kind) {
    case "instructions":
    case "reseed":
    case "briefing":
      return "instructions";
    case "userMessage":
    case "resume":
    case "followUp":
      return "messages";
    case "report":
      return "reports";
    default:
      return "other";
  }
}

export type GroupTotals = Record<InjectionGroup, number>;

export type InjectionRow = {
  streamSeq: number;
  atMs: number;
  injection: ContextInjection;
};

/** The CLI's reported context size, placed after the injections that preceded it. */
export type ContextPoint = {
  /** Injections logged before this report. */
  after: number;
  usedTokens: number;
  windowTokens: number | null;
  atMs: number;
};

/** An orchestrator rebirth, placed after the injections that preceded it. */
export type RebirthRow = {
  streamSeq: number;
  atMs: number;
  /** Injections logged before it: the new CLI's context starts here. */
  after: number;
  record: RebirthRecord;
};

/** Something the orchestrator's CLI must never do happened (it compacted its context). */
export type BreachRow = { streamSeq: number; atMs: number; message: string };

/** One CLI session of the orchestrator: the first (0), then one per rebirth. */
export type Generation = {
  generation: number;
  /** The first injection into it. */
  from: number;
  /** The rebirth that started it; `null` for the first. */
  rebirth: RebirthRow | null;
};

export type DerivedLog = {
  injections: InjectionRow[];
  /** Each injection's CLI generation (same order as `injections`). */
  generationOf: number[];
  /**
   * Cumulative estimated tokens per group after each injection, counted from the start of its
   * generation (a rebirth starts the new CLI's context afresh). Same order as `injections`.
   */
  cumulative: GroupTotals[];
  /** Everything injected, over all generations. */
  totals: GroupTotals;
  counts: GroupTotals;
  /** What was injected into the current CLI. */
  current: GroupTotals;
  context: ContextPoint[];
  /** The newest context report of the current CLI. */
  latestContext: ContextPoint | null;
  generations: Generation[];
  rebirths: RebirthRow[];
  breaches: BreachRow[];
};

function zero(): GroupTotals {
  return { instructions: 0, messages: 0, reports: 0, other: 0 };
}

export function deriveLog(entries: readonly OrchestratorLogEntry[]): DerivedLog {
  const injections: InjectionRow[] = [];
  const generationOf: number[] = [];
  const cumulative: GroupTotals[] = [];
  const context: ContextPoint[] = [];
  const rebirths: RebirthRow[] = [];
  const breaches: BreachRow[] = [];
  const generations: Generation[] = [{ generation: 0, from: 0, rebirth: null }];
  let running = zero();
  let totals = zero();
  const counts = zero();
  let generation = 0;
  let windowTokens: number | null = null;
  let latestContext: ContextPoint | null = null;
  // Injections logged before the current CLI's latest exit. A new CLI's instructions and
  // briefing are logged before its rebirth entry, so the rebirth reaches back to here.
  let exitedAfter: number | null = null;
  for (const { streamSeq, atMs, entry } of entries) {
    switch (entry.type) {
      case "injection": {
        const group = groupOf(entry.injection.kind);
        const tokens = entry.injection.tokensEstimate;
        running = { ...running, [group]: running[group] + tokens };
        totals = { ...totals, [group]: totals[group] + tokens };
        counts[group] += 1;
        injections.push({ streamSeq, atMs, injection: entry.injection });
        generationOf.push(generation);
        cumulative.push(running);
        break;
      }
      case "provider":
        if (entry.event.type === "exited") {
          exitedAfter = injections.length;
        } else if (entry.event.type === "contextSize") {
          windowTokens = entry.event.windowTokens ?? windowTokens;
          latestContext = {
            after: injections.length,
            usedTokens: entry.event.usedTokens,
            windowTokens,
            atMs,
          };
          context.push(latestContext);
        }
        break;
      case "rebirth": {
        const previous = generations[generations.length - 1]!;
        const from = Math.max(previous.from, exitedAfter ?? injections.length);
        const row = { streamSeq, atMs, after: from, record: entry.record };
        rebirths.push(row);
        generation = entry.record.generation;
        generations.push({ generation, from, rebirth: row });
        // The new CLI started with an empty context and has reported nothing yet; what was
        // injected into it before this entry is counted from zero again.
        running = zero();
        for (let index = from; index < injections.length; index++) {
          const { kind, tokensEstimate } = injections[index]!.injection;
          const group = groupOf(kind);
          running = { ...running, [group]: running[group] + tokensEstimate };
          generationOf[index] = generation;
          cumulative[index] = running;
        }
        latestContext = null;
        exitedAfter = null;
        break;
      }
      case "contractBreach":
        breaches.push({ streamSeq, atMs, message: entry.message });
        break;
    }
  }
  return {
    injections,
    generationOf,
    cumulative,
    totals,
    counts,
    current: running,
    context,
    latestContext,
    generations,
    rebirths,
    breaches,
  };
}

export function sumOf(totals: GroupTotals): number {
  return totals.instructions + totals.messages + totals.reports + totals.other;
}

export { formatTokens } from "@/lib/format";
