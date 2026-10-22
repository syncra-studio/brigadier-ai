import type { ReactNode } from "react";

import type { LeadStep } from "@/app/conversation/activity/group";
import { LeadItemDetail } from "@/app/conversation/activity/LeadItemDetail";

/** The kinds of work whose row opens while it runs: a command, to its Shell box as the output comes. */
const OPEN_WHILE_RUNNING = new Set(["run"]);

/** Whether a lead's call is kept in full: as its item in the CLI's session file. */
export function keptInFull(kind: LeadStep["kind"]): boolean {
  return kind.type === "tool";
}

/** Whether a lead's step has a detail to open to; `kind` is its words' kind. */
export function leadStepOpens(step: LeadStep, kind?: string): boolean {
  if (step.kind.type !== "tool") return false;
  if (step.kind.status !== "inProgress") return true;
  return keptInFull(step.kind) && kind !== undefined && OPEN_WHILE_RUNNING.has(kind);
}

/** What a lead's step opens to, in its conversation; `kind` is its words' kind. */
export function leadStepDetail(conversationId: string | null, step: LeadStep, kind?: string): ReactNode {
  if (!conversationId || step.kind.type !== "tool" || !leadStepOpens(step, kind)) return undefined;
  return <LeadItemDetail conversationId={conversationId} kind={step.kind} />;
}
