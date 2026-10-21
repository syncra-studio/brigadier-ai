import type { BrainJobKind } from "@/ipc/generated";

export const count = new Intl.NumberFormat();

export const JOB_KINDS: Record<BrainJobKind, string> = {
  skeleton: "Skeleton pass",
  enrichment: "Enrichment",
};
