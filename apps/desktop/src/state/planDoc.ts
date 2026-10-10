import { create } from "zustand";

import type { Board } from "@/state/board";

/**
 * A plan the user reads as a document (THREAD-PARITY-PLAN.md §6): the thread's own proposal
 * (`plan`, a plan with a body), or a lead's outline the user is asked to start (`outline`, an
 * approval).
 */
export type PlanDocRef = { type: "plan" | "outline"; id: string };

/** The plan each session's Plan tab (in its right sidebar) shows, kept only while the app runs. */
export const usePlanTab = create<{ shown: Record<string, PlanDocRef> }>(() => ({ shown: {} }));

export function showPlanDoc(conversationId: string, ref: PlanDocRef): void {
  usePlanTab.setState(({ shown }) => ({ shown: { ...shown, [conversationId]: ref } }));
}

export type PlanDoc = { title: string; markdown: string };

/** The plan's text as it reads, its title first: the body's own `# Title`, else the plan's title. */
export function planMarkdown(title: string, body: string): string {
  const text = body.trim();
  return /^#\s/.test(text) ? text : `# ${title}\n\n${text}`;
}

/** The document a reference points to, while the board has it. */
export function planDoc(board: Pick<Board, "plans" | "approvals"> | null, ref: PlanDocRef | null): PlanDoc | null {
  if (!board || !ref) return null;
  if (ref.type === "plan") {
    const plan = board.plans[ref.id];
    return plan?.body ? { title: plan.title, markdown: planMarkdown(plan.title, plan.body) } : null;
  }
  const subject = board.approvals[ref.id]?.subject;
  return subject?.type === "outline"
    ? { title: subject.title, markdown: planMarkdown(subject.title, subject.outline) }
    : null;
}
