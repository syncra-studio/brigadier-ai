import type { ReactNode } from "react";

import type { LeadStep } from "@/app/conversation/activity/group";
import { LeadItemDetail } from "@/app/conversation/activity/LeadItemDetail";
import { LeadToolDetail } from "@/app/conversation/activity/LeadToolDetail";
import { cliThreadOn } from "@/state/cliThread";

/** The kinds of work whose row opens while it runs: a command, to its Shell box as the output comes. */
const OPEN_WHILE_RUNNING = new Set(["run"]);

/**
 * Whether a lead's call is kept in full: with the request's calls (BUILD-PLAN B1b), or as its
 * item in the CLI's session file.
 */
export function keptInFull(kind: LeadStep["kind"]): boolean {
  return kind.type === "tool" && (kind.logSeq !== undefined || cliThreadOn());
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
  // A call kept with the request's calls (BUILD-PLAN B1b) opens as a worker's does; an older one as it was kept.
  if (keptInFull(step.kind)) return <LeadItemDetail conversationId={conversationId} requestId={step.requestId ?? null} kind={step.kind} />;
  return <LeadToolDetail conversationId={conversationId} kind={step.kind} />;
}
