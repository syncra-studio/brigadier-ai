import { isRunRequest } from "@/app/conversation/blocks";
import type { Plan, TaskState, UserRequest } from "@/ipc/generated";

export type PlanStepStatus = "pending" | "active" | "done" | "failed";
export type PlanProgress = {
  label: string;
  status: "review" | PlanStepStatus;
  tone: "live" | "done" | "warning" | "failed" | "quiet";
};

export function planStepStatus(state: TaskState | undefined): PlanStepStatus {
  switch (state) {
    case undefined:
    case "queued":
      return "pending";
    case "landed":
    case "done":
      return "done";
    case "failed":
    case "rejected":
    case "stopped":
      return "failed";
    default:
      return "active";
  }
}

/** Lifecycle comes first: review work is not execution of the plan's steps. */
export function planProgress(plan: Plan, states: readonly (TaskState | undefined)[]): PlanProgress {
  switch (plan.state.type) {
    case "proposed":
      return { label: "Plan proposed", status: "review", tone: "warning" };
    case "inReview":
      return { label: "Plan in review", status: "review", tone: "live" };
    case "revising":
      return { label: "Revising plan", status: "review", tone: "warning" };
    case "rejected":
      return { label: "Plan rejected", status: "failed", tone: "failed" };
    case "superseded":
      return { label: "Plan superseded", status: "pending", tone: "quiet" };
    case "approved": {
      const steps = plan.steps.map((_, index) => planStepStatus(states[index]));
      const failed = steps.indexOf("failed");
      if (failed !== -1) return { label: `Step ${failed + 1} failed`, status: "failed", tone: "failed" };
      const running = steps.indexOf("active");
      if (running !== -1) {
        const count = steps.filter((status) => status === "active").length;
        return {
          label: `Step ${running + 1} of ${steps.length}: ${plan.steps[running]!.title}${count > 1 ? ` · ${count} running` : ""}`,
          status: "active",
          tone: "live",
        };
      }
      const done = steps.filter((status) => status === "done").length;
      if (done > 0) {
        const complete = done === steps.length;
        return {
          label: `${done} of ${steps.length} done`,
          status: complete ? "done" : "pending",
          tone: complete ? "done" : "quiet",
        };
      }
      return { label: "Approved, not started", status: "pending", tone: "quiet" };
    }
  }
}

/** The newest non-steered request that is still working or waiting owns the surface. */
export function activePlanRequest(requests: Readonly<Record<string, UserRequest>>): UserRequest | null {
  return (
    Object.values(requests)
      .filter((request) =>
        request.steeredInto === null &&
        (request.state.type === "working" || request.state.type === "waiting"),
      )
      .toSorted((a, b) => b.startedAtMs - a.startedAtMs)[0] ?? null
  );
}

export function currentRequestPlan(
  plans: Readonly<Record<string, Plan>>,
  requestId: string | null,
): Plan | null {
  return (
    Object.values(plans)
      .filter((plan) => requestId !== null && plan.requestId === requestId && plan.state.type !== "superseded")
      .toSorted((a, b) => b.createdAtMs - a.createdAtMs || b.position - a.position)[0] ?? null
  );
}

/** The context card keeps the last session plan after its request ends. */
export function contextPlanId(
  plans: Readonly<Record<string, Plan>>,
  planIds: readonly string[],
  activeRequest: UserRequest | null,
): string | null {
  if (!activeRequest) return planIds.at(-1) ?? null;
  const plan = currentRequestPlan(plans, activeRequest.id);
  return plan && planIds.includes(plan.id) ? plan.id : null;
}

/** Overnight stays phase-driven and still makes room for pending decisions. */
export function capsuleMode(
  overnight: boolean,
  pending: boolean,
  request: UserRequest | null,
  plan: Plan | null,
): "run" | "request" | null {
  if (overnight) return pending ? null : "run";
  if (!request || isRunRequest(request.id)) return null;
  if (request.state.type !== "working" && request.state.type !== "waiting") return null;
  if (plan) return "request";
  return request.state.type === "working" && !pending ? "request" : null;
}
