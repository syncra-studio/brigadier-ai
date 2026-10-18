/**
 * A standalone Vite entry, excluded from the app entry and production build: the composer's `@`
 * and `/` menus (issue #158) in the app's own conversation view, IPC mocked, in the rich field
 * and then the plain one. A session lists its checkout's files; a new chat and a new session
 * open the `@` menu too. What it saw goes as JSON into `<pre id="composer-menus-result">`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/ConversationView";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import { fixtureInput } from "@/fixtures/input";
import type { Conversation, ConversationView as View, Mention, Project, Request } from "@/ipc/generated";
import { select } from "@/state/actions";
import { setRichComposer } from "@/state/composerMode";
import { type Selection, useApp } from "@/state/store";

const id = "menus-session";
const start = Date.now() - 600_000;
const FILES = ["README.md", "src/lib/menu.ts", "src/lib/chips.ts", "src/app/Composer.tsx", "docs/guide.md"];

const conversation = (cid: string, title: string, kind: "chat" | "session", age: number) => ({
  id: cid, kind, projectId: kind === "session" ? "p1" : null, title, pinnedAtMs: null, createdAtMs: start,
  updatedAtMs: start + age, lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
  setup: kind === "chat" ? { type: "chat", model: { provider: "claude", model: null, effort: null, fast: false } } : null,
}) as unknown as Conversation;
const session = conversation(id, "Menus", "session", 3);
const other = conversation("other-chat", "Release notes", "chat", 2);
const project = { id: "p1", name: "menus", createdAtMs: start, repos: [{ path: "/tmp/menus", name: "menus" }], prefs: {}, trust: [] } as unknown as Project;

const sends: { text: string; mentions: Mention[] }[] = [];
const listings: string[] = [];
let sequence = 10;
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getConversation":
      return {
        method: req.method,
        view: {
          conversation: useApp.getState().conversations[req.id] ?? session, context: null,
          messages: { messages: [], hasMore: false }, tasks: [], approvals: [], questions: [],
          plans: [], requests: [], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [],
          compactions: [], decisions: [], waiting: [], queue: { items: [], paused: false }, run: "idle",
          runRequest: null, head: null, ratings: {}, streaming: null, notices: [], memories: [], overnight: [],
          reviews: [], previews: [],
        } as unknown as View,
      };
    case "listFiles":
      listings.push(`session:${req.conversationId}`);
      return { method: req.method, files: FILES, truncated: false };
    case "listProjectFiles":
      listings.push(`project:${req.projectId}`);
      return { method: req.method, files: FILES, truncated: false };
    case "sendMessage": {
      sends.push({ text: req.text, mentions: req.mentions });
      const message = {
        id: `sent-${sequence}`, conversationId: req.conversationId, seq: sequence++, role: "user", text: req.text, blob: null,
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

async function exercise(mode: "rich" | "plain") {
  const seen: Record<string, unknown> = {};
  setRichComposer(mode === "rich");
  show({ type: "conversation", id }, mode);
  await until("the field", () => field() !== null);
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
  await type("a /");
  await sleep(150);
  seen.slashMidSentence = commands() !== null;
  await clear();

  // A new chat: the @ menu has the other conversations, and no files.
  show({ type: "draft", kind: "chat" }, mode);
  await until("the new chat's field", () => field() !== null);
  field()!.focus();
  await type("@");
  await until("the new chat's @ menu", () => menu().includes("# Chats"));
  seen.newChat = menu();
  await key("Escape");
  await clear();

  // A new session: its project's files.
  show({ type: "draft", kind: "session", projectId: "p1" }, mode);
  await until("the new session's field", () => field() !== null);
  field()!.focus();
  await type("@men");
  await until("the new session's files", () => menu().includes("menu.ts"));
  seen.newSession = listings.includes("project:p1");
  await key("Escape");
  await clear();
  return seen;
}

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    seen.rich = await exercise("rich");
    seen.plain = await exercise("plain");
  } catch (error) {
    seen.error = String(error);
  }
  const result = document.createElement("pre");
  result.id = "composer-menus-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

void drive();
