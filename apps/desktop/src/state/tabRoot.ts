import { draftProject } from "@/state/draftTabs";
import { useApp } from "@/state/store";

/** The folder a tab's relative paths start from. */
export function useTabRoot(owner: string, checkout?: string): string | null {
  return useApp((s) => {
    if (checkout) return checkout;
    const project = draftProject(owner);
    if (project) return s.projects[project]?.repos[0]?.path ?? null;
    const setup = s.conversations[owner]?.setup;
    if (setup?.type !== "session") return null;
    return setup.environment.type === "newWorktree" ? (setup.environment.path ?? setup.repo) : setup.repo;
  });
}
