import type { Board } from "@/state/board";
import { useApp } from "@/state/store";

/**
 * A plan the user reads as a document (THREAD-PARITY-PLAN.md §6): the thread's own proposal
 * (`plan`, a plan with a body), or a lead's outline the user is asked to start (`outline`, an
 * approval).
 */
export type PlanDocRef = { type: "plan" | "outline"; id: string };

/** Whether a plan in this conversation opens full screen: only a session has tabs to open it in. */
export function usePlanOpens(conversationId: string | null): boolean {
  return useApp((s) => !!conversationId && s.conversations[conversationId]?.kind === "session");
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
