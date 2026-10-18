/**
 * The Add project dialog over a faked daemon: five repositories on this computer (`hello`
 * already a project) and a clone or a new repository that works. ?view=clone opens Clone from
 * URL with a URL pasted, ?view=new New repository. ?drive=1 searches, picks, adds, clones and
 * starts a repository, and writes what it saw to #add-project-result (AddProjectRender.test.ts
 * reads it). Without it the page stays as it opens, for screenshots.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { AddProjectDialog } from "@/app/dialogs/AddProjectDialog";
import type { Project, RepoSuggestion, Request } from "@/ipc/generated";
import { revealApp } from "@/lib/splash";
import { openAddProject } from "@/state/addProject";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}

const HOUR = 3_600_000;
const now = Date.now();
const repos: RepoSuggestion[] = [
  { path: "/Users/you/code/brigadier", name: "brigadier", lastUsedMs: now - 2 * HOUR, projectId: null },
  { path: "/Users/you/code/website", name: "website", lastUsedMs: now - 26 * HOUR, projectId: null },
  { path: "/Users/you/code/infra-scripts", name: "infra-scripts", lastUsedMs: now - 74 * HOUR, projectId: null },
  { path: "/private/tmp/launch-alpha-w25-fr1/repo/hello", name: "hello", lastUsedMs: now - 100 * HOUR, projectId: "p0" },
  { path: "/Users/you/code/design-tokens", name: "design-tokens", lastUsedMs: now - 15 * 24 * HOUR, projectId: null },
];
const project = (id: string, path: string): Project => ({
  id,
  name: path.split("/").pop()!,
  createdAtMs: now,
  repos: [{ path, name: path.split("/").pop()! }],
  prefs: {} as Project["prefs"],
  trust: [],
});
useApp.setState((state) => ({
  catalogLoaded: true,
  settings: { ...state.settings, onboarded: true },
  projects: { p0: project("p0", repos[3]!.path) },
}));

/** The requests that change something, as the daemon heard them. */
const asked: string[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "findRepos":
      return { method: req.method, repos };
    case "addProject":
      asked.push(`add ${req.path}${req.init ? " init" : ""}`);
      return { method: req.method, project: project("p9", req.path.replace(/^~/, "/Users/you")) };
    case "cloneProject":
      asked.push(`clone ${req.url} into ${req.parent}${req.folder}`);
      return { method: req.method, project: project("p8", `/Users/you/code/${req.folder}`) };
    default:
      return null;
  }
});
void revealApp();

// Timers only: the test's Chrome runs on virtual time, where frames may never come.
const settle = () => new Promise((resolve) => setTimeout(resolve, 100));
const card = () => document.querySelector<HTMLElement>("[data-testid=add-project-dialog]");
const button = (label: string) =>
  [...(card()?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find((found) => found.textContent?.trim() === label);
const type = async (input: HTMLInputElement, text: string) => {
  const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  set.call(input, text);
  input.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
};

/** The list as shown: each row's name, its last use or Added, and whether it is picked. */
function list() {
  return [...(card()?.querySelectorAll("[role=radio]") ?? [])].map((row) => {
    const spans = row.querySelectorAll(":scope > span");
    return `${spans[1]?.firstChild?.textContent} · ${spans[2]?.textContent}${row.getAttribute("aria-checked") === "true" ? " · picked" : ""}${(row as HTMLButtonElement).disabled ? " · can't pick" : ""}`;
  });
}

async function drive(): Promise<string> {
  const seen: Record<string, unknown> = {};
  openAddProject();
  await settle();
  await settle();
  seen.title = card()?.querySelector("h2")?.textContent;
  seen.width = card()?.getBoundingClientRect().width;
  seen.list = list();
  seen.buttons = [...card()!.querySelectorAll("button:not([role=radio])")].map((found) => found.getAttribute("aria-label") ?? found.textContent);
  const search = card()!.querySelector<HTMLInputElement>("input")!;
  await type(search, "web");
  seen.searched = list();
  seen.addLabel = button("Add website") ? "Add website" : null;
  await type(search, "");
  card()!.querySelectorAll<HTMLButtonElement>("[role=radio]")[2]!.click();
  await settle();
  seen.picked = list().find((row) => row.endsWith("picked"));
  button("Add infra-scripts")?.click();
  await settle();
  seen.closedAfterAdd = card() === null;

  openAddProject();
  await settle();
  button("Clone from URL")?.click();
  await settle();
  seen.cloneTitle = card()?.querySelector("h2")?.textContent;
  await type(card()!.querySelector<HTMLInputElement>("input")!, "https://github.com/acme/website.git");
  seen.saveTo = card()?.querySelector("[data-slot=save-to]")?.textContent;
  button("Clone and add")?.click();
  await settle();
  seen.closedAfterClone = card() === null;

  openAddProject();
  await settle();
  button("New repository")?.click();
  await settle();
  seen.newTitle = card()?.querySelector("h2")?.textContent;
  await type(card()!.querySelector<HTMLInputElement>("input")!, "notes");
  seen.newSaveTo = card()?.querySelector("[data-slot=save-to]")?.textContent;
  button("Create and add")?.click();
  await settle();
  seen.closedAfterNew = card() === null;
  seen.asked = asked;
  return JSON.stringify(seen);
}

/** Opens the view asked for, with a URL pasted in Clone. */
async function show() {
  openAddProject();
  await settle();
  const view = params.get("view");
  if (view === "clone" || view === "new") {
    button(view === "clone" ? "Clone from URL" : "New repository")?.click();
    await settle();
    await type(card()!.querySelector<HTMLInputElement>("input")!, view === "clone" ? "https://github.com/acme/website.git" : "notes");
  }
}

function Page() {
  const [result, setResult] = useState<string | null>(null);
  useEffect(() => {
    if (params.has("drive")) void drive().then(setResult);
    else void show();
  }, []);
  return (
    <div className="flex h-screen">
      <nav className="bg-sidebar w-sidebar" />
      <main className="flex-1" />
      <AddProjectDialog />
      {result && <pre id="add-project-result">{result}</pre>}
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Page />);
