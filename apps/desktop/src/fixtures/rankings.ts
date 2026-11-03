import type { RankingsRefresh } from "@/ipc/generated";

/** No ratings research has run yet. */
export const IDLE_RESEARCH: RankingsRefresh = {
  state: "idle",
  jobId: null,
  startedAtMs: null,
  finishedAtMs: null,
  model: null,
  provider: null,
  baseRevision: null,
  changes: [],
  errors: [],
  found: [],
  overlayApplied: false,
  overlayAtMs: null,
};
