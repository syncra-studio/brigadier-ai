import type { WorkRow } from "@/app/settings/routing/words";
import type { GuidanceRow, ProviderKind } from "@/ipc/generated";
import { cliFacts } from "@/lib/providerKinds";

/*
 * Kinds of work whose rows aren't an order (the first that can run takes it) but one model per
 * role: Adversarial review (the reviewer on the agent that didn't write the code), Fusion (both
 * consults at once) and Fallback (the model a session moves to). Each is one row in Who does
 * what and opens to its roles.
 */

/** The other of the two agents these kinds name. */
const other = (agent: ProviderKind): ProviderKind => (agent === "codex" ? "claude" : "codex");

export type RoleKind = {
  /** What it does, in plain words, on its row. */
  line: string;
  /** The note under its roles, opened. */
  note: string;
  /** A row's role: "Reviews Claude's work". */
  role: (row: GuidanceRow) => string;
};

export const ROLE_KINDS: Record<string, RoleKind> = {
  "Adversarial review": {
    line: "A model from the other agent reviews the work",
    note: "Work is always reviewed on the agent that didn't write it.",
    role: (row) => `Reviews ${cliFacts(other(row.agent)).vendor}'s work`,
  },
  Fusion: {
    line: "Both consults answer at once, then their answers merge",
    note: "A fusion asks both consults the same question at once and merges their answers into one.",
    role: (row) => `${cliFacts(row.agent).vendor} consult`,
  },
  Fallback: {
    line: "The model a session runs on when it moves to the other agent",
    note: "A session moves when you ask it to, or when every account of its own agent is at its limit.",
    role: (row) => `Moving to ${cliFacts(row.agent).label}`,
  },
};

const isConsult = (work: string) => work === "fusion-claude" || work === "fusion-codex";

/** The kinds of work with a fusion's consult rows (`fusion-claude`, `fusion-codex`) as one Fusion row, in the place of the first. */
export function withFusion(rows: readonly WorkRow[]): WorkRow[] {
  const consults = rows.filter((row) => isConsult(row.work));
  if (consults.length === 0) return [...rows];
  const fusion: WorkRow = {
    work: "Fusion",
    rows: consults.flatMap((row) => row.rows),
    shipped: consults.flatMap((row) => row.shipped),
    mine: consults.flatMap((row) => row.mine),
  };
  const at = rows.indexOf(consults[0]!);
  return rows.flatMap((row, index) => (index === at ? [fusion] : isConsult(row.work) ? [] : [row]));
}
