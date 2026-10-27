/**
 * A standalone Vite entry, excluded from the app entry and production build: the composer's `@`
 * and `/` menus (issue #158) in the app's own conversation view, IPC mocked, in the rich field
 * and then the plain one. A session lists its checkout's files; a new chat and a new session
 * open the `@` menu too. What it saw goes as JSON into `<pre id="composer-menus-result">`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { useCliCommands } from "@/app/conversation/useCliCommands";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { fusionProviders } from "@/fixtures/fusionProviders";
import { fixtureInput } from "@/fixtures/input";
import type { Conversation, ConversationView as View, Mention, Project, ProviderKind, Request } from "@/ipc/generated";
import { select } from "@/state/actions";
import { setRichComposer } from "@/state/composerMode";
import { armFusionSend } from "@/state/fusionSend";
import { type Selection, useApp } from "@/state/store";

const id = "menus-session";
const start = Date.now() - 600_000;
const FILES = ["README.md", "src/lib/menu.ts", "src/lib/chips.ts", "src/app/Composer.tsx", "docs/guide.md"];

const conversation = (cid: string, title: string, kind: "chat" | "session", age: number) => ({
  id: cid, kind, projectId: kind === "session" ? "p1" : null, title, pinnedAtMs: null, createdAtMs: start,
  updatedAtMs: start + age, lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null,
  setup: kind === "chat" ? { type: "chat", model: { provider: "claude", model: null, effort: null, fast: false }, permission: "acceptEdits" } : null,
}) as unknown as Conversation;
const session = conversation(id, "Menus", "session", 3);
const other = conversation("other-chat", "Release notes", "chat", 2);
const project = { id: "p1", name: "menus", createdAtMs: start, repos: [{ path: "/tmp/menus", name: "menus" }], prefs: {}, trust: [] } as unknown as Project;

const sends: { text: string; mentions: Mention[] }[] = [];
const listings: string[] = [];
const fusionSends: { fusion: boolean; text: string }[] = [];
let sequence = 10;
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getRepoInfo":
      return { method: req.method, repo: { path: req.path, name: "menus", currentBranch: "main", dirty: false, branches: [{ name: "main", commit: "a1", checkedOutAt: req.path }] } };
    case "createConversation":
      return { method: req.method, conversation: conversation(`draft-${sequence++}`, "Draft", req.kind, 0) };
    case "getConversation":
      return {
        method: req.method,
        view: {
          conversation: useApp.getState().conversations[req.id] ?? session, context: null,
          messages: { messages: [], hasMore: false }, tasks: [], approvals: [], questions: [],
          requests: [], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [],
          compactions: [], decisions: [], queue: { items: [], paused: false }, run: "idle",
          runRequest: null, head: null, ratings: {}, streaming: null, notices: [],
          previews: [],
        } as unknown as View,
      };
    case "listFiles":
      listings.push(`session:${req.conversationId}`);
      return { method: req.method, files: FILES, truncated: false };
    case "getPluginCatalog":
      return { method: req.method, catalog: { rows: [], version: "composer-menus" } };
    case "listCliCommands":
      // As the session's CLI reports them: one of the user's skills.
      return { method: req.method, commands: [{ name: "fusion", text: "/fusion", description: "Fusion", argumentHint: null }, { name: "grill-me", text: "/grill-me", kind: "skill", source: "skill:grill-me:fixture", invocation: "grill-me", agentOnly: null, description: "A relentless interview.", argumentHint: null }] };
    case "listProjectFiles":
      listings.push(`project:${req.projectId}`);
      return { method: req.method, files: FILES, truncated: false };
    case "sendMessage": {
      sends.push({ text: req.text, mentions: req.mentions });
      if (req.fusion) fusionSends.push({ fusion: req.fusion, text: req.text });
      const message = {
        id: `sent-${sequence}`, conversationId: req.conversationId, seq: sequence++, role: "user", text: req.fusion ? `$fusion ${req.text}` : req.text, blob: null,
        attachments: req.attachments, mentions: req.mentions, createdAtMs: Date.now(), model: null,
        requestId: `sent-request-${sequence}`, parentId: null,
      };
      return { method: req.method, outcome: { type: "sent", message } };
    }
    default:
      return { method: req.method };
  }
});
useApp.setState({
  conversations: { [id]: session, [other.id]: other },
  projects: { p1: project },
  connection: { status: "connected", daemon: null, reason: null },
});

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(what: string, check: () => boolean, ms = 10_000) {
  const end = performance.now() + ms;
  while (!check()) {
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}: ${document.querySelector("[data-slot=composer]")?.textContent ?? "no composer"}`);
    await sleep(16);
  }
}
const field = () => document.querySelector<HTMLElement>("[data-slot=composer-input] [contenteditable=true]");
const typed = () => field()?.textContent ?? "";
const key = (name: string, modifiers?: ("Alt" | "Control" | "Meta" | "Shift")[]) =>
  fixtureInput({ type: "key", key: name, ...(modifiers ? { modifiers } : {}) });
const type = (text: string) => fixtureInput({ type: "type", text });
const mentions = () => document.querySelector("[data-slot=composer-mentions]");
const commands = () => document.querySelector("[data-slot=composer-commands]");
/** The `@` menu as the user reads it: headings (#), rows (by name) and notes (~), in order. */
const menu = () =>
  [...(mentions()?.querySelectorAll("p, button, [role=option]") ?? [])].map((element) =>
    element.tagName === "P" ? `${element.classList.contains("pt-2") ? "#" : "~"} ${element.textContent}` : (element.querySelector(":scope > span:nth-child(2)")?.textContent ?? element.textContent),
  );

async function clear() {
  field()!.focus();
  document.execCommand("selectAll");
  await sleep(60);
  await key("Backspace");
  await until("an empty field", () => typed() === "");
}

const root = createRoot(document.getElementById("root")!);
function show(selection: Selection, mode: string) {
  select(selection);
  root.render(
    <TooltipProvider>
      <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
        <main className="relative flex min-h-0 w-full flex-1 flex-col">
          <ConversationView key={`${mode}-${JSON.stringify(selection)}`} selection={selection} />
        </main>
      </SidebarProvider>
    </TooltipProvider>,
  );
}

const plus = () => document.querySelector<HTMLButtonElement>('[data-slot="plus-trigger"]');
const plusRows = () => [...document.querySelectorAll<HTMLElement>('[data-slot="plus-menu"] [role=menuitem]')];

const fusionRow = () => plusRows().find((row) => row.textContent?.includes("Fusion"));
const fusionChip = () => document.querySelector<HTMLButtonElement>('[data-slot="fusion-chip"]');
const fusionOn = () => fusionChip()?.getAttribute("aria-pressed") === "true";
const openPlus = async () => { plus()!.click(); await until("the + menu", () => plusRows().length > 0); };

async function exercise(mode: "rich" | "plain") {
  armFusionSend("new", false);
  armFusionSend(other.id, false);
  useApp.setState({ conversations: { [id]: session, [other.id]: other } });
  const seen: Record<string, unknown> = {};
  setRichComposer(mode === "rich");
  show({ type: "conversation", id }, mode);
  await until("the field", () => field() !== null);
  seen.sessionFusion = !!fusionChip();
  field()!.focus();
  await until("the files listed", () => listings.includes(`session:${id}`));

  // Before anything is typed: a heading per kind, Files with only its note.
  await type("@");
  await until("the @ menu", () => mentions() !== null && menu().includes("# Chats"));
  seen.empty = menu();
  // A query: one ranked list, no headings; a folder can be mentioned.
  await type("lib");
  await until("the results", () => menu()[0] === "lib");
  seen.query = menu();
  await key("Enter");
  await until("the folder chip", () => field()!.querySelector("[data-chip=folder]") !== null);
  await type("now");
  const before = sends.length;
  await key("Enter");
  await until("the send", () => sends.length > before);
  seen.sent = sends.at(-1);

  // `/` opens the commands after leading spaces.
  await until("an empty field", () => typed() === "");
  await type("  /");
  await until("the / menu", () => commands() !== null);
  seen.slashAfterSpaces = true;
  await key("Escape");
  await clear();
  // Mid-text after a space too, with what the CLI runs; picking one types it there.
  await type("a /");
  await until("the / menu mid-text", () => commands() !== null);
  seen.slashMidSentence = true;
  await type("gri");
  await until("the CLI's skill", () => commands()?.textContent?.includes("grill-me") === true);
  await key("Enter");
  await until("the skill typed", () => commands() === null && typed().includes("grill-me"));
  seen.picked = typed().replaceAll("\u00a0", " ").trimEnd();
  seen.pickedChip = field()?.querySelector('[data-slot="composer-chip"][data-chip="skill"]') !== null;
  const beforeSkillSend = sends.length;
  await key("Enter");
  await until("the skill send", () => sends.length > beforeSkillSend);
  seen.pickedSent = { text: sends.at(-1)!.text.trim(), mentions: sends.at(-1)!.mentions };
  await until("the empty field after the skill", () => typed() === "");
  await clear();
  // In the middle of a message, the text after it kept.
  await type("z tail");
  for (let step = 0; step < 5; step++) await key("ArrowLeft");
  await type(" /gri");
  await until("the / menu in the middle", () => commands()?.textContent?.includes("grill-me") === true);
  await key("Enter");
  await until("the skill typed in the middle", () => commands() === null && typed().includes("grill-me"));
  seen.pickedInside = typed().replaceAll("\u00a0", " ");
  await clear();
  // Never inside a word: a path.
  await type("see a/");
  await sleep(150);
  seen.slashInWord = commands() !== null;
  await clear();

  // A new chat: the @ menu has the other conversations, and no files.
  const beforeChat = field();
  show({ type: "draft", kind: "chat" }, mode);
  // Its own field: the one shown before stays until the new view renders.
  await until("the new chat's field", () => field() !== null && field() !== beforeChat);
  seen.draftFusionVisible = fusionChip()?.getAttribute("aria-pressed") === "false";
  field()!.focus();
  await type("@");
  await until("the new chat's @ menu", () => menu().includes("# Chats"));
  seen.newChat = menu();
  await key("Escape");
  await clear();

  // An armed Chat draft must not turn a subsequent session draft's send into Fusion.
  useApp.setState((state) => ({ providers: { ...state.providers, view: { providers: fusionProviders, sessions: [], fixtures: [] } } }));
  await until("the draft's Fusion toggle ready", () => fusionChip()?.disabled === false);
  fusionChip()!.click();
  await until("the draft's Fusion toggle on", fusionOn);

  // A new session: its project's files.
  const beforeSession = field();
  show({ type: "draft", kind: "session", projectId: "p1" }, mode);
  // Its own field: the one shown before stays until the new view renders.
  await until("the new session's field", () => field() !== null && field() !== beforeSession);
  field()!.focus();
  await type("@men");
  await until("the new session's files", () => menu().includes("menu.ts"));
  seen.newSession = listings.includes("project:p1");
  await key("Escape");
  await clear();
  // Fusion is absent in sessions, including their drafts.
  seen.sessionDraftChip = !!fusionChip();
  const beforeDraftSend = sends.length;
  field()!.focus();
  await type("Keep this a session.");
  await key("Enter");
  await until("the session draft's send", () => sends.length > beforeDraftSend);
  seen.sessionDraftFusion = fusionSends.at(-1)?.text === "Keep this a session.";

  // The Chat draft kept its toggle while visiting a session. Its created Chat inherits it.
  const beforeReturn = field();
  show({ type: "draft", kind: "chat" }, mode);
  await until("the Chat draft again", () => field() !== null && field() !== beforeReturn);
  await until("the draft still on", fusionOn);
  const beforeDraftFusion = fusionSends.length;
  field()!.focus();
  await type("Compare draft options.");
  await key("Enter");
  await until("the Chat draft sent", () => fusionSends.length > beforeDraftFusion);
  const created = useApp.getState().selection;
  if (created.type !== "conversation") throw new Error("The Chat draft did not become a conversation");
  const beforeCreated = field();
  show(created, mode);
  await until("the created Chat", () => field() !== null && field() !== beforeCreated && fusionOn());
  seen.draftFusionCarry = true;
  field()!.focus();
  await type("Compare once more.");
  await key("Enter");
  await until("the created Chat's next Fusion send", () => fusionSends.length === beforeDraftFusion + 2);
  seen.draftFusionSticky = fusionOn();
  const beforeFresh = field();
  show({ type: "draft", kind: "chat" }, mode);
  await until("a fresh draft", () => field() !== null && field() !== beforeFresh);
  seen.freshDraftFusionOff = fusionChip()?.getAttribute("aria-pressed") === "false";

  const old = field();
  show({ type: "conversation", id: other.id }, mode);
  await until("the Chat field", () => field() !== null && field() !== old);
  seen.fusionVisible = fusionChip()?.getAttribute("aria-pressed") === "false";
  useApp.setState((state) => ({ providers: { ...state.providers, view: { providers: fusionProviders.slice(0, 1), sessions: [], fixtures: [] } } }));
  await until("the disabled Fusion toggle", () => fusionChip()?.disabled === true);
  seen.fusionDisabled = fusionChip()!.disabled;
  const disabledChip = fusionChip()!.getBoundingClientRect();
  await fixtureInput({ type: "pointer", x: disabledChip.x + disabledChip.width / 2, y: disabledChip.y + disabledChip.height / 2 });
  await until("the disabled Fusion hint", () => document.querySelector('[role="tooltip"]')?.textContent === "Needs two ready CLIs");
  seen.fusionHint = true;
  await fixtureInput({ type: "pointer", x: 0, y: 0 });
  field()!.focus();
  useApp.setState((state) => ({ providers: { ...state.providers, view: { providers: fusionProviders, sessions: [], fixtures: [] } } }));
  await until("the Fusion toggle ready", () => fusionChip()?.disabled === false);
  await openPlus();
  seen.fusionMenuEntry = !!fusionRow();
  await key("Escape");
  fusionChip()!.click();
  await until("Fusion turned on", fusionOn);
  fusionChip()!.click();
  await until("Fusion turned off", () => fusionChip()?.getAttribute("aria-pressed") === "false");
  seen.fusionOff = !!fusionChip();
  fusionChip()!.click();
  await until("Fusion turned on again", fusionOn);
  const beforeFusion = fusionSends.length;
  field()!.focus();
  await type("Compare the options.");
  await key("Enter");
  await until("the Fusion send", () => fusionSends.length > beforeFusion);
  seen.fusionSend = fusionSends.at(-1);
  await until("the Fusion pill", () => document.querySelector('[data-slot="fusion-message-pill"]') !== null);
  seen.fusionPill = document.querySelector('[data-slot="fusion-message-pill"]')?.textContent;
  seen.rawFusion = document.querySelector('[data-role="user"]')?.textContent?.includes("$fusion");
  await until("an empty field after Fusion", () => typed() === "");
  await type("Compare sync safety.");
  await key("Enter");
  await until("the next Fusion send", () => fusionSends.length === beforeFusion + 2 && typed() === "");
  seen.fusionSticky = fusionOn();
  fusionChip()!.click();
  await until("Fusion off after sending", () => !fusionOn());
  await type("A normal message.");
  const beforeNormal = sends.length;
  await key("Enter");
  await until("a normal send", () => sends.length > beforeNormal && typed() === "");
  seen.normalAfterFusion = fusionSends.length === beforeFusion + 2;
  await type("/");
  await until("the Chat / menu", () => commands() !== null);
  seen.fusionSlashRow = commands()?.textContent?.includes("Fusion");
  await key("Escape");
  await clear();
  await type("/fusion A typed question.");
  const beforeTyped = sends.length;
  await key("Enter");
  await until("a typed Fusion invocation", () => sends.length > beforeTyped && typed() === "");
  seen.typedFusion = sends.at(-1)?.text;
  return seen;
}

function CommandConsumer({ provider }: { provider: ProviderKind }) {
  const [commands] = useCliCommands({ provider, projectId: null, repo: null });
  return <span data-command-provider={provider}>{commands.some((command) => command.name === "grill-me") ? "ready" : "waiting"}</span>;
}

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    seen.rich = await exercise("rich");
    seen.plain = await exercise("plain");
    root.render(<><CommandConsumer provider="claude" /><CommandConsumer provider="codex" /></>);
    await until("both command consumers", () => ["claude", "codex"].every((provider) => document.querySelector(`[data-command-provider="${provider}"]`)?.textContent === "ready"));
    seen.independentCommands = true;
  } catch (error) {
    seen.error = String(error);
  }
  const result = document.createElement("pre");
  result.id = "composer-menus-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

void drive();
