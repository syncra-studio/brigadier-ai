import { request } from "@/ipc/client";
import type { Project } from "@/ipc/generated";
import { type AppState, useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/*
 * "Trust this folder?": the user's answer for a project's folder. Trusted, agents work
 * there as the session's permission level allows; not trusted, every session there runs under
 * Ask. A folder never asked about (a new project, one from before the question,
 * one given another folder) is asked about when the user first enters its project; until then
 * work there runs as if not trusted.
 */

/** The project's folder: its first repository's, or `null` while it has none. */
export function projectFolder(project: Project): string | null {
  return project.repos[0]?.path ?? null;
}

/** The answer for the project's folder `path`: trusted or not, `null` when never asked. */
export function folderTrust(project: Project | null | undefined, path: string): boolean | null {
  return project?.trust.find((folder) => folder.path === path)?.trusted ?? null;
}

/**
 * The project asked about now: the one the user is in (its new session, or a chat or session
 * in it), when its folder has no answer. Each project is asked once, the first time it's used;
 * adding many at once asks nothing until each is entered.
 */
export function toAsk(state: Pick<AppState, "selection" | "projects" | "conversations">): Project | null {
  const { selection } = state;
  const id =
    selection.type === "draft" && selection.kind === "session"
      ? selection.projectId
      : selection.type === "conversation"
        ? state.conversations[selection.id]?.projectId
        : null;
  const project = id ? state.projects[id] : undefined;
  if (!project) return null;
  const folder = projectFolder(project);
  return folder !== null && folderTrust(project, folder) === null ? project : null;
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
