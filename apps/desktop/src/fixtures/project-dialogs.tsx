/**
 * Project settings and Remove project over a faked daemon, for screenshots. ?show= picks what
 * opens: settings (the default), settings-norepo, remove, remove-small (no repository, nothing
 * in it), remove-blocked, remove-loading, removed (Remove pressed, with a branch kept and a
 * failure).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { AppSidebar } from "@/app/shell/AppSidebar";
import { SidebarPanel, SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import { ProjectDialog } from "@/app/shell/dialogs/ProjectDialog";
import { RemoveProjectDialog } from "@/app/shell/dialogs/RemoveProjectDialog";
import type { Project, ProjectRemoval, RemovalBranch, Request } from "@/ipc/generated";
import { revealApp } from "@/lib/splash";
import { useApp } from "@/state/store";

const drive = new URLSearchParams(location.search).has("drive");
const calls: Request[] = [];
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
useApp.setState((state) => ({ catalogLoaded: true, settings: { ...state.settings, onboarded: true }, projects: drive ? Object.fromEntries(Array.from({ length: 8 }, (_, i) => [`p${i + 1}`, { ...project, id: `p${i + 1}`, name: `Project ${i + 1}`, createdAtMs: 8 - i }])) : { p1: project } }));

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
  calls.push(req);
  switch (req.method) {
    case "previewRemoveProject":
      return show === "remove-loading" ? new Promise(() => {}) : { method: req.method, removal: drive ? { ...removal, projectId: req.id, name: useApp.getState().projects[req.id]!.name } : removal };
    case "removeProject":
      if (drive) return { method: req.method, report: { keptBranches: [], brainTrashedBytes: 0, failures: [] } };
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
  const [result, setResult] = useState("");
  useEffect(() => {
    if (drive) void exerciseRemoval().then((seen) => setResult(JSON.stringify(seen))).catch((error: unknown) => setResult(JSON.stringify({ error: String(error) })));
  }, []);
  if (drive) return <TooltipProvider><SidebarProvider keepOpen><SidebarPanel><AppSidebar /></SidebarPanel><main /></SidebarProvider>{result && <pre id="remove-project-result">{result}</pre>}</TooltipProvider>;
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


const pause = () => new Promise<void>((resolve) => setTimeout(resolve, 120));
const button = (label: string) => {
  const found = [...document.querySelectorAll<HTMLElement>("button, [role=menuitem]")].find((item) => item.getAttribute("aria-label") === label || item.textContent?.trim() === label);
  if (!found) throw new Error(`No button: ${label}`);
  return found;
};
async function exerciseRemoval() {
  await pause();
  await fixtureClick(button("Actions for Project 1")); await pause();
  const row = button("Remove project…").getBoundingClientRect();
  const press = () => fixtureInput({ type: "click", x: row.x + row.width / 2, y: row.y + row.height / 2 });
  await press(); await pause();
  const dialog = document.querySelector('[role=dialog]');
  const seen: Record<string, unknown> = {
    title: dialog?.querySelector('h2')?.textContent,
    choices: dialog?.textContent,
    beforeConfirm: calls.filter((req) => req.method === "removeProject"),
    remaining: Object.keys(useApp.getState().projects).length,
  };
  // Repeated clicks where the menu item was, and Escape: no removal can precede confirmation.
  for (let i = 0; i < 7; i++) { await press(); await pause(); }
  await fixtureInput({ type: "key", key: "Escape" }); await pause();
  seen.cancelled = { removals: calls.filter((req) => req.method === "removeProject"), remaining: Object.keys(useApp.getState().projects).length };
  await fixtureClick(button("Actions for Project 1")); await pause();
  await fixtureClick(button("Remove project…")); await pause();
  await fixtureClick(document.querySelector('[role=checkbox]')!); await pause();
  await fixtureClick(button("Remove")); await pause();
  for (let i = 0; i < 7; i++) { await press(); await fixtureInput({ type: "key", key: "Escape" }); await pause(); }
  seen.removals = calls.filter((req) => req.method === "removeProject");
  seen.remaining = Object.keys(useApp.getState().projects);
  return seen;
}
