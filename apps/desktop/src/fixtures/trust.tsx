/**
 * The folder trust question over a faked shell and daemon: four projects, `invoice-service` and
 * `ops-scripts` never asked about, `invoice-service` open in its new session. ?density=compact
 * for the compact sizes, ?long=1 for a path too long for its line. ?drive=1 moves between a
 * plain chat, an answered project and the two unanswered ones, answers both and comes back,
 * and writes what it saw to #trust-result
 * (TrustDialogRender.test.ts reads it). ?look=1 sets the open dialog, its two answers and the
 * path's tooltip beside the app's shared parts and writes their looks to #trust-look. Without
 * either the page stays as it opens, for screenshots.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { TrustDialog } from "@/app/dialogs/TrustDialog";
import { Folder } from "@/components/ui/icon";
import { Canon, canon, look, type LookPairs } from "@/fixtures/dialogLook";
import type { Conversation, Project, Request } from "@/ipc/generated";
import { revealApp } from "@/lib/splash";
import { type Selection, useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
document.documentElement.dataset.density = params.get("density") === "compact" ? "compact" : "normal";
if (params.has("drive") || params.has("look")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}

const names = ["brigadier-ai", "landing-site", "invoice-service", "ops-scripts"];
const unanswered = new Set(["invoice-service", "ops-scripts"]);
// ?long=1: invoice-service lives deep in iCloud Drive, too long for the dialog's line.
const LONG =
  "/Users/you/Library/Mobile Documents/com~apple~CloudDocs/Work/clients/acme-corporation/2026/q4-launch/repositories/invoice-service";
const folderOf = (name: string) =>
  params.has("long") && name === "invoice-service" ? LONG : `/Users/you/Development/${name}`;
const projects: Project[] = names.map((name, index) => ({
  id: `p${index}`,
  name,
  createdAtMs: index + 1,
  repos: [{ path: folderOf(name), name }],
  prefs: {} as Project["prefs"],
  trust: unanswered.has(name) ? [] : [{ path: folderOf(name), trusted: true, decidedAtMs: 1 }],
}));
// A session in `invoice-service`, the newest project.
const session = { id: "s1", projectId: "p2", title: "Add invoice export" } as Conversation;

useApp.setState((state) => ({
  catalogLoaded: true,
  settings: { ...state.settings, onboarded: true },
  projects: Object.fromEntries(projects.map((project) => [project.id, project])),
  conversations: { [session.id]: session },
  selection: { type: "draft", kind: "session", projectId: "p2" },
}));

/** The answers sent, as "project trusted". */
const answers: string[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method !== "setFolderTrust") return null;
  answers.push(`${req.id} ${req.trusted}`);
  const project = useApp.getState().projects[req.id]!;
  const path = req.path ?? project.repos[0]!.path;
  return {
    method: req.method,
    report: { project: { ...project, trust: [{ path, trusted: req.trusted, decidedAtMs: 2 }] }, failures: [] },
  };
});
void revealApp();

// Timers only: the test's Chrome runs on virtual time, where frames may never come.
const settle = () => new Promise((resolve) => setTimeout(resolve, 100));

/** What the dialog shows now, or `null` when it's closed. */
function dialog() {
  const card = document.querySelector<HTMLElement>("[data-testid=trust-dialog]");
  if (!card) return null;
  const pills = [...card.querySelectorAll("button:not([data-slot=trust-path])")];
  return {
    title: card.querySelector("h2")?.textContent,
    path: card.querySelector("[data-slot=trust-path]")?.textContent,
    buttons: pills.map((button) => button.textContent),
    focused: (document.activeElement as HTMLElement | null)?.dataset.testid ?? document.activeElement?.textContent,
    width: card.getBoundingClientRect().width,
  };
}

async function visit(selection: Selection) {
  useApp.setState({ selection });
  await settle();
  return dialog();
}

async function answer(label: string) {
  [...document.querySelectorAll<HTMLButtonElement>("[data-testid=trust-dialog] button")]
    .find((button) => button.textContent === label)
    ?.click();
  for (let i = 0; i < 50 && dialog(); i++) await settle();
}

async function drive(): Promise<string> {
  const seen: Record<string, unknown> = {};
  seen.chat = await visit({ type: "draft", kind: "chat" });
  seen.answered = await visit({ type: "draft", kind: "session", projectId: "p0" });
  seen.session = await visit({ type: "conversation", id: "s1" });
  await answer("Trust folder");
  seen.afterTrust = dialog();
  seen.newSession = await visit({ type: "draft", kind: "session", projectId: "p3" });
  await answer("Don't trust");
  seen.backAgain = await visit({ type: "draft", kind: "session", projectId: "p2" });
  seen.answers = answers;
  return JSON.stringify(seen);
}

/** The dialog's parts and the shared ones they are drawn with, once `Canon` is mounted. */
async function looks(): Promise<string> {
  for (let i = 0; i < 50 && !dialog(); i++) await settle();
  const card = document.querySelector<HTMLElement>("[data-testid=trust-dialog]");
  card?.querySelector<HTMLElement>("[data-slot=trust-path]")?.focus();
  await settle();
  const labelled = (label: string) =>
    [...(card?.querySelectorAll("button") ?? [])].find((button) => button.textContent === label);
  const pairs: LookPairs = {
    card: [look(card, "surface"), canon("dialog", "surface")],
    title: [look(card?.querySelector("h2"), "text"), canon("title", "text")],
    line: [look(card?.querySelector("p"), "text"), canon("line", "text")],
    trust: [look(labelled("Trust folder"), "button"), canon("primary", "button")],
    dontTrust: [look(labelled("Don't trust"), "button"), canon("outline", "button")],
    tooltip: [look(card?.querySelector("[data-slot=trust-path-tooltip]"), "surface"), canon("tooltip", "surface")],
  };
  return JSON.stringify(pairs);
}

function Shell() {
  const [result, setResult] = useState<string | null>(null);
  const [looked, setLooked] = useState<string | null>(null);
  useEffect(() => {
    if (params.has("drive")) void drive().then(setResult);
    if (params.has("look")) void looks().then(setLooked);
  }, []);
  return (
    <div className="flex h-screen">
      <nav className="bg-sidebar text-sidebar-foreground w-sidebar grid content-start gap-1 p-3">
        <p className="text-muted-foreground px-2 pb-1 text-xs">Projects</p>
        {names.map((name) => (
          <div
            key={name}
            className={`rounded-control flex items-center gap-2 px-2 py-1.5 ${name === "invoice-service" ? "bg-accent" : ""}`}
          >
            <Folder className="size-icon-md text-muted-foreground" />
            {name}
          </div>
        ))}
      </nav>
      <main className="grid flex-1 place-content-center gap-1 text-center">
        <p className="text-xl">New session in invoice-service</p>
        <p className="text-muted-foreground">Describe what to build in this project</p>
      </main>
      <TrustDialog />
      {result && <pre id="trust-result">{result}</pre>}
      {params.has("look") && <Canon />}
      {looked && <pre id="trust-look">{looked}</pre>}
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Shell />);
