import { request } from "@/ipc/client";
import type { Project } from "@/ipc/generated";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/*
 * "Do you trust this folder?": the user's answer for a project's folder. Trusted, agents work
 * there as the session's permission level allows; not trusted, every session there runs under
 * Ask for approval. A folder never asked about (a new project, one from before the question,
 * one given another folder) behaves as before and is asked about.
 */

/** The project's folder: its first repository's, or `null` while it has none. */
export function projectFolder(project: Project): string | null {
  return project.repos[0]?.path ?? null;
}

/** The answer for the project's folder `path`: trusted or not, `null` when never asked. */
export function folderTrust(project: Project | null | undefined, path: string): boolean | null {
  return project?.trust.find((folder) => folder.path === path)?.trusted ?? null;
}

/** The project asked about next: of those whose folder has no answer, the oldest. */
export function nextToAsk(projects: Iterable<Project>): Project | null {
  let next: Project | null = null;
  for (const project of projects) {
    const folder = projectFolder(project);
    if (folder === null || folderTrust(project, folder) !== null) continue;
    if (
      next === null ||
      project.createdAtMs < next.createdAtMs ||
      (project.createdAtMs === next.createdAtMs && project.id < next.id)
    ) {
      next = project;
    }
  }
  return next;
}

/**
 * The toast for one thing the daemon couldn't write in an agent's own settings ("Codex: why").
 * The answer itself is saved either way.
 */
export function trustFailureText(failure: string, trusted: boolean): string {
  if (!trusted) {
    return `Saved. Brigadier couldn't take back the folder's earlier trust in an agent's own settings: ${failure}`;
  }
  const match = /^(Claude|Codex): ([\s\S]*)$/.exec(failure);
  if (!match) return `Saved. Brigadier couldn't record it everywhere: ${failure}`;
  const [, agent, reason] = match;
  return `Saved. Brigadier couldn't record it for ${agent}, so ${agent} may ask again in its terminal: ${reason}`;
}

/**
 * Records the answer for the project's folder `path` (its first repository's when `null`),
 * stores the project as the daemon returns it, and toasts what couldn't be written.
 */
export async function setFolderTrust(
  id: string,
  path: string | null,
  trusted: boolean,
): Promise<Project> {
  const { report } = await request({ method: "setFolderTrust", id, path, trusted });
  useApp.setState((state) => ({
    projects: { ...state.projects, [report.project.id]: report.project },
  }));
  for (const failure of report.failures) {
    toast(trustFailureText(failure, trusted), { tone: "error" });
  }
  return report.project;
}
