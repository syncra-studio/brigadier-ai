import type { OrchestratorStepKind, ProofKind } from "@/ipc/generated";
import { toast } from "@/state/toasts";

export type Proof = Extract<OrchestratorStepKind, { type: "proof" }>;

export function failed(cause: unknown): void {
  toast(cause instanceof Error ? cause.message : String(cause), { tone: "error" });
}

/** What a proof is: as it was recorded, else (a proof from before that) by its file's type. */
export function kindOf(proof: Proof): ProofKind {
  if (proof.kind) return proof.kind;
  return proof.mime.startsWith("video/") ? "recording" : proof.mime.startsWith("image/") ? "screenshot" : "log";
}

/** Log and test output: read as text, not looked at. */
export const printed = (proof: Proof) => ["log", "test"].includes(kindOf(proof));

/** The lines of log or test output shown: its last ones. */
export const LAST_LINES = 5;
