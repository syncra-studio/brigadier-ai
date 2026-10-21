import { finishOnboarding } from "@/state/onboarding";

export type Step = "intro" | "name" | "agents" | "projects";

/** The steps after the intro, in the dots at the top. */
export const STEPS: readonly Step[] = ["name", "agents", "projects"];
export const STEP_LABELS: Record<Step, string> = {
  intro: "Welcome",
  name: "Name",
  agents: "Agents",
  projects: "Projects",
};

/** Closes a run of the setup opened from Settings. */
export function closeSetup(): void {
  void finishOnboarding().catch((error: unknown) => console.error(error));
}
