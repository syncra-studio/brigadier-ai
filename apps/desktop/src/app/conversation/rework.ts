import type { UserRequest } from "@/ipc/generated";
import type { Board } from "@/state/board";

/** The conversation's newest request (the daemon orders them the same way). */
export function latestRequest(board: Board): UserRequest | null {
  let latest: UserRequest | null = null;
  for (const request of Object.values(board.requests)) {
    if (
      !latest ||
      request.startedAtMs > latest.startedAtMs ||
      (request.startedAtMs === latest.startedAtMs && request.id > latest.id)
    ) {
      latest = request;
    }
  }
  return latest;
}

/**
 * The session request the user may still edit or have answered again: the latest, until
 * anything it started has landed, is landing with their approval, or was merged at their
 * word (the daemon's `branches::landed` reads the same).
 */
export function reworkableRequest(board: Board): string | null {
  const latest = latestRequest(board);
  if (!latest) return null;
  const id = latest.id;
  const landed =
    Object.values(board.tasks).some((task) => task.requestId === id && task.state === "landed") ||
    board.orchestratorSteps.some(
      (step) => step.kind.type === "merged" && (step.kind.askedIn === id || step.requestId === id),
    ) ||
    Object.values(board.approvals).some(
      (approval) =>
        approval.requestId === id &&
        approval.state.type === "allowed" &&
        (approval.subject.type === "landing" || approval.subject.type === "finishSession"),
    );
  return landed ? null : id;
}
