/**
 * The folder trust question over a faked shell and daemon: four projects, `invoice-service` and
 * `ops-scripts` never asked about, `invoice-service` open in its new session.
 * ?long=1 for a path too long for its line. ?drive=1 moves between a
 * plain chat, an answered project and the two unanswered ones, answers both and comes back,
 * and writes what it saw to #trust-result
 * (TrustDialogRender.test.ts reads it). ?look=1 sets the open dialog, its two answers and the
 * path's tooltip beside the app's shared parts and writes their looks to #trust-look. ?type=1
 * (under renderFixtureLive) has the app's own new session behind the dialog, types, pastes and
 * presses Enter while it asks, answers, types on and writes what the message box held to
 * #trust-typed (#477). Without any the page stays as it opens, for screenshots.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { COMPOSER_EDITABLE } from "@/app/conversation/composerTarget";
import { TrustDialog } from "@/app/shell/dialogs/TrustDialog";
import { Folder } from "@/components/icons";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { Canon, canon, look, type LookPairs } from "@/fixtures/dialogLook";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import type { Conversation, Project, Request } from "@/ipc/generated";
import { revealApp } from "@/lib/splash";
import { type Selection, useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
if (params.has("drive") || params.has("look") || params.has("type")) {
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

localStorage.clear();
useApp.setState((state) => ({
  catalogLoaded: true,
  connection: { status: "connected", daemon: null, reason: null },
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
  if (req.method !== "setFolderTrust") return { method: req.method };
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

/** The message box behind the dialog under ?type=1. */
const field = () => document.querySelector<HTMLElement>(COMPOSER_EDITABLE);

/**
 * Types, pastes and presses Enter while the dialog asks, answers, and types on: what the
 * message box behind it held each time, and where the keyboard went.
 */
async function typeWhileAsked(): Promise<string> {
  const seen: Record<string, unknown> = {};
  for (let i = 0; i < 100 && !(dialog() && field()); i++) await settle();
  await fixtureInput({ type: "type", text: "Add a READMX" });
  await fixtureInput({ type: "key", key: "Backspace" });
  await fixtureInput({ type: "type", text: "E" });
  const pasted = new DataTransfer();
  pasted.setData("text/plain", " to the repo");
  document.activeElement?.dispatchEvent(
    new ClipboardEvent("paste", { clipboardData: pasted, bubbles: true, cancelable: true }),
  );
  // Enter neither sends the message nor answers.
  await fixtureInput({ type: "key", key: "Enter" });
  await settle();
  seen.whileAsked = { text: field()?.textContent, focused: dialog()?.focused, answers: [...answers] };
  const trust = [...document.querySelectorAll("[data-testid=trust-dialog] button")].find(
    (button) => button.textContent === "Trust folder",
  );
  if (trust) await fixtureClick(trust);
  // Typing on once the box has the keyboard back: under load that comes a frame or two after
  // the card goes.
  for (let i = 0; i < 50 && (dialog() || !field()?.contains(document.activeElement)); i++) await settle();
  await fixtureInput({ type: "type", text: ", please" });
  // Long enough for the draft to be kept (it waits for typing to rest).
  for (let i = 0; i < 6; i++) await settle();
  seen.answered = {
    text: field()?.textContent,
    focused: field()?.contains(document.activeElement) ?? false,
    answers,
    kept: JSON.parse(localStorage.getItem("brigadier.drafts") ?? "{}").new?.text,
  };
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
    card: [look(card, "surface"), canon("project-dialog", "surface")],
    title: [look(card?.querySelector("h2"), "text"), canon("project-title", "text")],
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
    if (params.has("type")) void typeWhileAsked().then(setResult);
  }, []);
  if (params.has("type")) {
    return (
      <TooltipProvider>
        <SidebarProvider className="flex h-screen flex-col" defaultOpen={false}>
          <main className="relative flex min-h-0 w-full flex-1 flex-col">
            <ConversationView key="draft" selection={{ type: "draft", kind: "session", projectId: "p2" }} />
          </main>
          <TrustDialog />
          {result && <pre id="trust-typed">{result}</pre>}
        </SidebarProvider>
      </TooltipProvider>
    );
  }
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
