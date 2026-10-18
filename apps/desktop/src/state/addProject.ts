import { create } from "zustand";

import { pickFolders, request, takeOpenedFolders } from "@/ipc/client";
import type { FolderCheck, Project, RepoSuggestion } from "@/ipc/generated";
import { select, setProjectExpanded } from "@/state/actions";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/**
 * The Add project dialog: open, with the folder it starts on ("" for the default). Each
 * opening has its own `opening`, so opening it again (a folder dropped on it) starts over.
 */
export const useAddProject = create<{ open: boolean; path: string; opening: number }>(() => ({
  open: false,
  path: "",
  opening: 0,
}));

export function openAddProject(path = ""): void {
  useAddProject.setState((state) => ({ open: true, path, opening: state.opening + 1 }));
}

export function closeAddProject(): void {
  useAddProject.setState({ open: false });
}

function storeProject(project: Project): void {
  useApp.setState((state) => ({
    projects: { ...state.projects, [project.id]: project },
  }));
}

/** Shows a project: expanded in the sidebar, with a new session drafted in it. */
export function showProject(id: string): void {
  setProjectExpanded(id, true);
  select({ type: "draft", kind: "session", projectId: id });
}

/** The git repositories on this computer, most recent first: the Add project dialog's list. */
export async function findRepos(): Promise<RepoSuggestion[]> {
  const { repos } = await request({ method: "findRepos" });
  return repos;
}

export async function checkFolder(path: string): Promise<FolderCheck> {
  const { check } = await request({ method: "checkFolder", path });
  return check;
}

/**
 * Adds a folder as a project (the top-level folder of its repository); with `init`, a folder
 * in no repository gets one first. An empty name names it after the folder.
 */
export async function addProject(path: string, name: string, init: boolean): Promise<Project> {
  const { project } = await request({ method: "addProject", path, name, init });
  storeProject(project);
  return project;
}

/** Clones `url` into `parent/folder` and adds the clone as a project. */
export async function cloneProject(
  url: string,
  parent: string,
  folder: string,
  name: string,
): Promise<Project> {
  const { project } = await request({ method: "cloneProject", url, parent, folder, name });
  storeProject(project);
  return project;
}

/**
 * Folders opened with the app, picked in File › Open Folder… or dropped on the window: each
 * repository becomes a project (or shows the project it already is). A folder in no repository
 * opens the Add project dialog on it, which says what adding it would do.
 */
export async function openFolders(paths: readonly string[]): Promise<void> {
  let shown: string | null = null;
  const added: string[] = [];
  try {
    for (const path of paths) {
      const check = await checkFolder(path);
      if (check.kind === "invalid") {
        toast(check.reason, { tone: "error" });
        continue;
      }
      if (check.kind !== "repo") {
        openAddProject(path);
        break;
      }
      if (check.projectId) {
        shown = check.projectId;
        continue;
      }
      const project = await addProject(check.root, "", false);
      added.push(project.name);
      shown = project.id;
    }
  } catch (cause) {
    toast(cause instanceof Error ? cause.message : String(cause), { tone: "error" });
  }
  if (shown) showProject(shown);
  if (added.length === 1) toast(`Added ${added[0]}`);
  else if (added.length > 1) toast(`Added ${added.length} projects`);
}

/** File › Open Folder… (⌘O). */
export async function openFolderPicker(): Promise<void> {
  const paths = await pickFolders();
  if (paths.length > 0) await openFolders(paths);
}

/** Adds the folders opened with the app (Finder, the Dock, launch arguments) not yet taken. */
export async function takeFolders(): Promise<void> {
  const paths = await takeOpenedFolders();
  if (paths.length > 0) await openFolders(paths);
}
