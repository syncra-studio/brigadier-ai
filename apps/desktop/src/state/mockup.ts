import type { ArtifactRef, Task, TaskState } from "@/ipc/generated";

/**
 * A mockup first (SESSION-FIXES-PLAN.md Q10): the task that built a session's newest mockup,
 * its screenshots (`mockup-N`), and the real work that matches it with its live captures
 * (`live-N`) and side-by-side images (`compare-N`), paired by number.
 */
export type Mockup = {
  task: Task;
  /** The real work that matches it, once delegated. */
  matching: Task | null;
  /** Each time its worker was sent back (the user's notes) makes a new version. */
  version: number;
  status: MockupStatus;
  shots: ArtifactRef[];
  pairs: MockupPair[];
};

export type MockupStatus = "building" | "ready" | "stopped" | "matching" | "compared";

export type MockupPair = { number: number; mockup: ArtifactRef | null; live: ArtifactRef | null; compare: ArtifactRef | null };

const WORKING: readonly TaskState[] = ["queued", "starting", "running", "blocked", "paused", "takenOver"];
const ENDED: readonly TaskState[] = ["rejected", "stopped", "failed"];
/** Over: the worker is gone and takes no message (`TaskState::is_final`). */
const FINAL: readonly TaskState[] = ["landed", "done", ...ENDED];

/** The number in `<prefix>-N.<image ext>`, or null. */
export function shotNumber(name: string, prefix: string): number | null {
  const match = new RegExp(`^${prefix}-(\\d+)\\.(png|jpe?g|webp|gif)$`, "i").exec(name);
  return match ? Number(match[1]) : null;
}

const nameOf = (ref: ArtifactRef) => ref.fileName ?? ref.title;

const at = (refs: ArtifactRef[], prefix: string, number: number) =>
  refs.find((ref) => shotNumber(nameOf(ref), prefix) === number) ?? null;

/** A task's images named `<prefix>-N`, by number; for the mockup's own task, any other image after them. */
export function shotsOf(outputs: readonly ArtifactRef[], prefix: string, others = false): ArtifactRef[] {
  const images = outputs.filter((ref) => ref.mime.startsWith("image/"));
  const numbered = images
    .flatMap((ref) => { const n = shotNumber(nameOf(ref), prefix); return n === null ? [] : [{ n, ref }]; })
    .toSorted((a, b) => a.n - b.n)
    .map(({ ref }) => ref);
  if (!others) return numbered;
  const rest = images.filter((ref) => !numbered.includes(ref)).toSorted((a, b) => nameOf(a).localeCompare(nameOf(b)));
  return [...numbered, ...rest];
}

/** The session's newest mockup, if it has one. */
export function mockupOf(tasks: Readonly<Record<string, Task>>): Mockup | null {
  const all = Object.values(tasks);
  const task = all.filter((t) => t.mockup?.type === "builds").toSorted((a, b) => b.number - a.number)[0];
  if (!task) return null;
  const matching = all
    .filter((t) => t.mockup?.type === "matches" && t.mockup.task === task.id)
    .toSorted((a, b) => b.number - a.number)[0] ?? null;
  const shots = shotsOf(task.outputs, "mockup", true);
  const live = matching ? shotsOf(matching.outputs, "live") : [];
  const compare = matching ? shotsOf(matching.outputs, "compare") : [];
  // mockup-N, live-N and compare-N show screen N; the mockup's other images follow its last.
  const named = shots.flatMap((ref) => shotNumber(nameOf(ref), "mockup") ?? []);
  let next = Math.max(0, ...named);
  const screens = shots.map((ref) => ({ ref, number: shotNumber(nameOf(ref), "mockup") ?? ++next }));
  const numbers = [...new Set([
    ...screens.map((screen) => screen.number),
    ...live.map((ref) => shotNumber(nameOf(ref), "live")!),
    ...compare.map((ref) => shotNumber(nameOf(ref), "compare")!),
  ])].toSorted((a, b) => a - b);
  const pairs = numbers.map((number) => ({
    number,
    mockup: screens.find((screen) => screen.number === number)?.ref ?? null,
    live: at(live, "live", number),
    compare: at(compare, "compare", number),
  }));
  // Real work sent back is building again, whatever it captured before.
  const status: MockupStatus = matching && WORKING.includes(matching.state) ? "matching"
    : matching && live.length ? "compared"
      : matching && !ENDED.includes(matching.state) ? "matching"
        : WORKING.includes(task.state) ? "building"
          : ENDED.includes(task.state) && !shots.length ? "stopped" : "ready";
  return { task, matching, version: task.reworkRounds + 1, status, shots, pairs };
}

/**
 * Whether the mockup moved on since notes went on `sent`'s screenshots: its worker is done and
 * made new ones, or was sent back and is done again, even with the same ones.
 */
export function revisedSince(mockup: Mockup, sent: { version: number; shots: readonly string[] }): boolean {
  if (WORKING.includes(mockup.task.state)) return false;
  return mockup.shots.map((ref) => ref.id).join() !== sent.shots.join() || mockup.version > sent.version;
}

/**
 * Whether the user can mark notes on the mockup: it is built or building, and its worker
 * hasn't ended, since the notes go back to that worker. An ended one's screenshots stay to
 * look at.
 */
export function takesNotes(mockup: Mockup): boolean {
  return (mockup.status === "ready" || mockup.status === "building") && !FINAL.includes(mockup.task.state);
}
