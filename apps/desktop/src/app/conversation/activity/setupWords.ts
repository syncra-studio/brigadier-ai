import type { StartStep, StartStepRecord } from "@/ipc/generated";

export type Names = { base: string; branch: string; cli: string };
export type Shown = { step: StartStep; error: string | null; status: "done" | "running" | "failed" };

/** What a start step's row says: running, done, or failed. */
export function setupStepWords(step: StartStep, status: Shown["status"], { base, branch, cli }: Names): string {
  switch (step) {
    case "fetching":
      return status === "running" ? `Fetching the latest ${base}` : status === "done" ? `Fetched the latest ${base}` : `Couldn't fetch the latest ${base}`;
    case "worktree":
      return status === "running" ? `Creating ${branch} from ${base}` : status === "done" ? `Created ${branch} from ${base}` : "Couldn't create the worktree";
    case "setupScript":
      return status === "running" ? "Running the setup script" : status === "done" ? "Ran the setup script" : "The setup script failed";
    case "cli":
      return status === "running" ? `Starting ${cli}` : status === "done" ? `Started ${cli}` : `Couldn't start ${cli}`;
  }
}

/**
 * The rows of a setup line: one per step reached, a step said again keeping its last word (its
 * failure); the last one runs while the turn still starts.
 */
export function setupRows(records: readonly StartStepRecord[], starting: boolean): Shown[] {
  const rows: Shown[] = [];
  for (const record of records) {
    const last = rows.at(-1);
    if (last?.step === record.step) last.error = record.error;
    else rows.push({ step: record.step, error: record.error, status: "done" });
  }
  rows.forEach((row, index) => {
    row.status = row.error !== null ? "failed" : starting && index === rows.length - 1 ? "running" : "done";
  });
  return rows;
}
