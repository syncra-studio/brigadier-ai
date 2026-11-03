import type { FileEntry, ThreadView } from "@/ipc/generated";

/** How Brigadier's word to write a hand-off note, and to fix it, begins (`thread_view.rs`). */
const NUDGE_MARK = "[Brigadier] A fresh session will continue this work.";
/** A successor that started by itself: Brigadier's word to it (`seam.rs` CARRY_ON). */
const CARRY_ON = "Carry on from the hand-off note.";

/** What a worker's panel says where its CLI compacted, as the thread's divider does. */
export const REFRESHED = "Context refreshed · nothing was lost";

const divider = (entry: FileEntry, message: string): FileEntry => ({ ...entry, event: { type: "notice", level: "info", message } });

/**
 * A worker's thread as its panel shows it, the same as a session's: each hand-off's turns (the
 * note, its fix, the successor's carry-on word) under the hood with no mark of their own, and one
 * divider where its CLI compacted (core-goals D193).
 */
export function withSeams(view: ThreadView): FileEntry[] {
  const seams = (view.seams ?? []).filter((seam) => !seam.open && seam.at);
  const out: FileEntry[] = [];
  let hidden = false;
  for (const entry of view.entries) {
    const { event } = entry;
    if (!entry.parent && event.type === "message" && event.role === "user") {
      hidden = event.text.trimStart().startsWith(NUDGE_MARK);
      const seam = seams.find((each) => each.at === event.itemId);
      if (seam && (seam.carried || event.text.trim() === CARRY_ON)) continue;
    }
    if (hidden) continue;
    out.push(!entry.parent && event.type === "compactionEnded" && !event.error ? divider(entry, REFRESHED) : entry);
  }
  return out;
}
