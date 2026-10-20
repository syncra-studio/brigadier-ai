/**
 * The Add project dialog over a faked daemon: five repositories on this computer (`hello`
 * already a project) and a clone or a new repository that works. ?view=clone opens Clone from
 * URL with a URL pasted, ?view=new New repository. ?drive=1 searches, picks, adds, clones and
 * starts a repository, and writes what it saw to #add-project-result (AddProjectRender.test.ts
 * reads it). ?frames=1 (under renderFixtureLive) answers the scan late, as a real one is, and
 * records the dialog's top edge and height on every frame (a second, 30 frames at least) after it opens, after
 * Clone from URL and after the back arrow, into #add-project-frames. ?look=1 (under
 * renderFixtureLive too, for a row under the pointer) sets each view's parts beside the app's
 * shared ones and writes their looks to #add-project-look. Without any the page stays as it
 * opens, for screenshots.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { AddProjectDialog } from "@/app/dialogs/AddProjectDialog";
import { Canon, canon, look, type LookPairs } from "@/fixtures/dialogLook";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import type { Project, RepoSuggestion, Request } from "@/ipc/generated";
import { revealApp } from "@/lib/splash";
import { openAddProject } from "@/state/addProject";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
if (params.has("drive") || params.has("look")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}

const HOUR = 3_600_000;
/** How long the scan for repositories takes under ?frames=1. */
const SCAN_MS = 400;
// The page's clock reads noon today, so "2h ago" and "Yesterday" don't depend on the time of day.
const realNow = Date.now.bind(Date);
const offset = new Date(realNow()).setHours(12, 0, 0, 0) - realNow();
Date.now = () => realNow() + offset;
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
      // Recording frames, the scan takes a moment: the dialog shows before its list does.
      if (params.has("frames")) return new Promise((resolve) => setTimeout(() => resolve({ method: req.method, repos }), SCAN_MS));
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

/** Where the dialog is on one frame: its top edge (as drawn) and its height (as laid out). */
type Frame = { ms: number; top: number; height: number; title: string | null };

/** How many drawn frames of the dialog a recording holds at least; a slow machine draws fewer a second. */
const FRAMES = 30;

/**
 * The dialog on every frame from `act`, for a second and at least FRAMES frames with the dialog
 * drawn (10 s at most), so a slow machine's recording holds as many frames as a fast one's.
 */
function record(act: () => unknown): Promise<Frame[]> {
  return new Promise((resolve) => {
    const seen: Frame[] = [];
    const start = performance.now();
    const tick = () => {
      const found = card();
      if (found) {
        seen.push({
          ms: Math.round(performance.now() - start),
          top: Math.round(found.getBoundingClientRect().top * 10) / 10,
          height: found.offsetHeight,
          title: found.querySelector("h2")?.textContent ?? null,
        });
      }
      const elapsed = performance.now() - start;
      if (elapsed < 1000 || (seen.length < FRAMES && elapsed < 10000)) requestAnimationFrame(tick);
      else resolve(seen);
    };
    requestAnimationFrame(tick);
    void act();
  });
}

/** Opens the dialog, then Clone from URL, then back, recording each. */
async function frames(): Promise<string> {
  const opened = await record(openAddProject);
  const cloned = await record(() => fixtureClick(button("Clone from URL")!));
  const back = await record(() => fixtureClick(card()!.querySelector("button[aria-label=Back]")!));
  return JSON.stringify({ opened, cloned, back });
}

/** Fields as they rest, as the shared field is measured: nothing focused. */
const rest = () => (document.activeElement as HTMLElement | null)?.blur();

/** The box a field's input (or shown value) sits in. */
const field = (input: Element | null | undefined) => input?.closest("label, div");

/** Each view's parts beside the shared ones they are drawn with, once `Canon` is mounted. */
async function looks(): Promise<string> {
  openAddProject();
  for (let i = 0; i < 50 && !card()?.querySelector("[role=radio]"); i++) await settle();
  rest();
  const rows = [...card()!.querySelectorAll<HTMLElement>("[role=radio]")];
  // The pointer over a row not picked (website), for its hover.
  const over = rows[1]!.getBoundingClientRect();
  await fixtureInput({ type: "pointer", x: over.x + over.width / 2, y: over.y + over.height / 2 });
  await settle();
  const pairs: LookPairs = {
    card: [look(card(), "surface"), canon("dialog", "surface")],
    title: [look(card()?.querySelector("h2"), "text"), canon("title", "text")],
    line: [look(card()?.querySelector("p"), "text"), canon("line", "text")],
    close: [look(card()?.querySelector("[aria-label=Close]"), "button"), canon("icon", "button")],
    search: [look(field(card()?.querySelector("input")), "surface"), canon("search", "surface", "label")],
    chooseFolder: [look(button("Choose folder…"), "button"), canon("outline", "button")],
    picked: [look(rows[0], "fill"), canon("chosen", "fill")],
    pickedInk: [look(rows[0], "ink"), canon("chosen", "ink")],
    hovered: [look(rows[1], "fill"), canon("wash", "fill")],
    added: [look(rows[3]?.querySelector(":scope > span:nth-child(3)"), "button"), canon("badge", "button")],
    radio: [look(rows[0]?.querySelector(":scope > span:last-child"), "fill"), canon("checked", "fill")],
    footer: [look(button("Cancel")?.parentElement, "edgeTop"), canon("divider", "edgeTop")],
    cloneFromUrl: [look(button("Clone from URL"), "button"), canon("ghost", "button")],
    newRepository: [look(button("New repository"), "button"), canon("ghost", "button")],
    cancel: [look(button("Cancel"), "button"), canon("ghost", "button")],
    add: [look(button("Add brigadier"), "button"), canon("primary", "button")],
  };
  button("Clone from URL")?.click();
  await settle();
  rest();
  await settle();
  Object.assign(pairs, {
    back: [look(card()?.querySelector("[aria-label=Back]"), "button"), canon("icon", "button")],
    url: [look(field(card()?.querySelector("input")), "surface"), canon("input", "surface")],
    saveTo: [look(field(card()?.querySelector("[data-slot=save-to]")), "surface"), canon("input", "surface")],
    change: [look(button("Change…"), "button"), canon("outline", "button")],
    clone: [look(button("Clone and add"), "button"), canon("primary", "button")],
  } satisfies LookPairs);
  card()?.querySelector<HTMLElement>("[aria-label=Back]")?.click();
  await settle();
  button("New repository")?.click();
  await settle();
  rest();
  await settle();
  Object.assign(pairs, {
    name: [look(field(card()?.querySelector("input")), "surface"), canon("input", "surface")],
    create: [look(button("Create and add"), "button"), canon("primary", "button")],
  } satisfies LookPairs);
  return JSON.stringify(pairs);
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
  const [recorded, setFrames] = useState<string | null>(null);
  const [looked, setLooked] = useState<string | null>(null);
  useEffect(() => {
    if (params.has("drive")) void drive().then(setResult);
    else if (params.has("look")) void looks().then(setLooked);
    else if (params.has("frames")) void frames().then(setFrames);
    else void show();
  }, []);
  return (
    <div className="flex h-screen">
      <nav className="bg-sidebar w-sidebar" />
      <main className="flex-1" />
      <AddProjectDialog />
      {result && <pre id="add-project-result">{result}</pre>}
      {recorded && <pre id="add-project-frames">{recorded}</pre>}
      {params.has("look") && <Canon />}
      {looked && <pre id="add-project-look">{looked}</pre>}
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Page />);
