/**
 * Project settings and Remove project over a faked daemon, for screenshots. ?show= picks what
 * opens: settings (the default), settings-norepo, remove, remove-small (no repository, nothing
 * in it), remove-blocked, remove-loading, removed (Remove pressed, with a branch kept and a
 * failure).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ProjectDialog } from "@/app/shell/dialogs/ProjectDialog";
import { RemoveProjectDialog } from "@/app/shell/dialogs/RemoveProjectDialog";
import type { Project, ProjectRemoval, RemovalBranch, Request } from "@/ipc/generated";
import { revealApp } from "@/lib/splash";
import { useApp } from "@/state/store";

const show = new URLSearchParams(location.search).get("show") ?? "settings";
const MB = 1024 * 1024;
const folder = "/Users/you/code/website";
const project: Project = {
  id: "p1",
  name: "Website",
  createdAtMs: 0,
  repos: show === "settings-norepo" ? [] : [{ path: folder, name: "website" }],
  prefs: { permission: null, orchestrator: null, environment: null, secretFiles: [".env.local", ".env.development.local"], branchPrefix: null },
  trust: [{ path: folder, trusted: true }],
} as Project;
useApp.setState((state) => ({ catalogLoaded: true, settings: { ...state.settings, onboarded: true }, projects: { p1: project } }));

const branch = (name: string, merged: boolean): RemovalBranch => ({
  repo: folder,
  name,
  target: "main",
  tip: "abc123",
  merged,
  ahead: merged ? 0 : 2,
  checkedOut: false,
  command: `git -C ~/code/website branch -D ${name}`,
});
const small = show === "remove-small";
const removal: ProjectRemoval = {
  projectId: "p1",
  name: small ? "Notes" : "Website",
  repo: small ? null : folder,
  conversations: small
    ? []
    : Array.from({ length: 5 }, (_, i) => ({ id: `c${i}`, title: `Session ${i}`, kind: "session", archived: false, running: false })),
  worktrees: small
    ? []
    : [
        { path: `${folder}-wt/task-1`, bytes: 700 * MB, hasChanges: false },
        { path: `${folder}-wt/task-2`, bytes: 528 * MB, hasChanges: true },
      ],
  branches: small ? [] : [branch("brigadier/task-1", true), branch("brigadier/task-3", false)],
  brainBytes: small ? 0 : 84 * MB,
  blocked: show === "remove-blocked" ? "A session is still working. Stop it first." : null,
} as ProjectRemoval;

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "previewRemoveProject":
      return show === "remove-loading" ? new Promise(() => {}) : { method: req.method, removal };
    case "removeProject":
      return {
        method: req.method,
        report: {
          keptBranches: [branch("brigadier/task-3", false)],
          brainTrashedBytes: 84 * MB,
          failures: ["The worktree ~/code/website-wt/task-5 couldn't be removed: it is in use."],
        },
      };
    default:
      return null;
  }
});
void revealApp();

function Page() {
  const settings = show.startsWith("settings");
  return (
    <>
      <ProjectDialog open={settings} onOpenChange={() => {}} project={settings ? project : null} />
      <RemoveProjectDialog project={settings ? null : project} onOpenChange={() => {}} />
    </>
  );
}

createRoot(document.getElementById("root")!).render(<Page />);
/** Presses Remove once the form shows. */
function pressRemove() {
  const remove = [...document.querySelectorAll("button")].find((found) => found.textContent === "Remove");
  if (remove) remove.click();
  else setTimeout(pressRemove, 50);
}
if (show === "removed") pressRemove();
