/**
 * A standalone Vite entry, excluded from the app entry and production build: when a session's
 * summary card (the top bar's Context button) opens, and where it sits, in the app's own
 * conversation view on a fresh profile. A window as wide as the default window's thread pane
 * beside its sidebar (1280px less 18rem), then a narrow one. Requests are answered here and never reach a daemon.
 * - A new chat, first thing: the card is closed.
 * - A new session's draft with its card closed: its first send opens the card by itself, once.
 * - A session with no project and branch yet: still closed, with nothing to show.
 * - Its setup arrives: the card opens by itself, and the user's long one-line message wraps
 *   before it.
 * - Closed by the user: a preview arriving or coming back to the session leaves it closed.
 * - In a pane too narrow to keep the thread beside it, a session's setup doesn't open it; a new
 *   session's first send there opens it once, floating, and closed it stays closed.
 *
 * The checks go to `<pre id="summary-card-result">`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { revealNewPreviews, useSummary } from "@/app/conversation/summaryState";
import { fixtureInput } from "@/fixtures/input";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, ConversationView as View, EventEnvelope, Message, Request } from "@/ipc/generated";
import { select, send } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

const start = Date.now() - 60_000;
const LONG = "Please tidy the release notes for the next version so every entry reads as one plain sentence, keeps its issue number at the end, and the sections run from new features to fixes to the small changes nobody asked about";

function session(id: string, title: string): Conversation {
  return {
    id, kind: "session", projectId: "project", title, pinnedAtMs: null, createdAtMs: start, updatedAtMs: start,
    setup: null, lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null,
  } as unknown as Conversation;
}

/** The setup a session gets once its folder and branch are made. */
function withSetup(conversation: Conversation): Conversation {
  return {
    ...conversation,
    setup: {
      type: "session", repo: "/tmp/summary-card-fixture", environment: { type: "localCheckout", branch: "main" },
      permission: "ask", planMode: false, workersSeeUncommitted: null,
      orchestrator: { provider: "claude", model: null, effort: null },
    },
  } as Conversation;
}

const conversations: Record<string, Conversation> = {
  first: session("first", "Tidy the release notes"),
  other: session("other", "Another session"),
  narrow: session("narrow", "A narrow window"),
};

/** The sessions the new-session screen's first sends make, in turn, set up as the daemon makes them. */
const made = [withSetup(session("started", "A new session")), withSetup(session("startedNarrow", "A new narrow session"))];

function messages(id: string): Message[] {
  if (id !== "first") return [];
  return [
    { id: "first-u", conversationId: id, seq: 1, role: "user", text: LONG, blob: null, createdAtMs: start,
      attachments: [], mentions: [], model: null, requestId: "first-r", parentId: null },
    { id: "first-a", conversationId: id, seq: 2, role: "assistant", text: "On it.", blob: null, createdAtMs: start + 1000,
      attachments: [], mentions: [], model: null, requestId: "first-r", parentId: "first-u" },
  ] as Message[];
}

function view(id: string): View {
  const stored = messages(id);
  return {
    conversation: useApp.getState().conversations[id], context: null,
    messages: { messages: stored, hasMore: false },
    tasks: [], approvals: [], questions: [], requests: [], workerSteps: [], orchestratorSteps: [],
    machineSteps: [], thinking: [], compactions: [], decisions: [],
    queue: { items: [], paused: false },
    run: "idle", runRequest: null, head: stored.at(-1)?.id ?? null, ratings: {}, streaming: null,
    notices: [], previews: [],
  } as unknown as View;
}

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getConversation":
      return { method: req.method, view: view(req.id) };
    case "createConversation":
      return { method: req.method, conversation: made.shift() };
    case "sendMessage":
      return { method: req.method, outcome: { type: "sent" } };
    case "listMessages":
      return { method: req.method, page: { items: [], hasMore: false } };
    case "getPullRequest":
      return { method: req.method, pullRequest: null };
    case "listFiles":
      return { method: req.method, files: [], truncated: false };
    default:
      return { method: req.method };
  }
});

useApp.setState((state) => ({
  conversations,
  projects: { project: { id: "project", name: "release-notes", createdAtMs: start, repos: [{ path: "/tmp/summary-card-fixture", name: "release-notes" }], prefs: {}, trust: [] } } as unknown as typeof state.projects,
  connection: { status: "connected", daemon: null, reason: null },
  info: { ...state.info, platform: "macos" } as typeof state.info,
}));

const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const toggle = () => $<HTMLButtonElement>('button[aria-label="Context"]');
const pressed = () => toggle()?.getAttribute("aria-pressed") === "true";
/** The card can be seen: pinned beside the thread, or floating over it. */
const open = () => pressed() || useSummary.getState().floating;
const card = () => [...document.querySelectorAll<HTMLElement>('[data-slot="summary-card"]')].find((element) => element.getClientRects().length > 0
  && !element.closest('[aria-hidden="true"]'));
const bubble = () => $('[data-slot="aui_user-message-root"] .aui-user-message-content');
/** The thread pane has its layout for the card, from its own width. */
const measured = () => !!$("[data-summary]")?.dataset.summary;
const settled = () => [...document.querySelectorAll<HTMLElement>('[data-slot="aui_thread-column"], aside[aria-label="Context"]')]
  .every((element) => element.getAnimations().every((animation) => animation.playState !== "running"));

function until(check: () => unknown, what: string, timeoutMs = 10000): Promise<void> {
  return new Promise((resolve, reject) => {
    const started = performance.now();
    const poll = () => {
      if (check()) resolve();
      else if (performance.now() - started > timeoutMs) reject(new Error(`timed out waiting for ${what}`));
      else setTimeout(poll, 20);
    };
    poll();
  });
}
const frames = () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(resolve, 50))));

/** The setup arriving, as the daemon's update of the conversation does. */
function setUp(id: string): void {
  useApp.setState((state) => ({ conversations: { ...state.conversations, [id]: withSetup(state.conversations[id]!) } }));
}

async function drive(): Promise<Record<string, unknown>> {
  const seen: Record<string, unknown> = {};
  await fixtureInput({ type: "viewport", width: 992, height: 900 });
  await until(() => toggle() && measured(), "the new chat's top bar");
  await frames();
  seen.freshClosed = !open() && !card();

  // A new session's screen, its card left closed (opened and closed again), then its first send.
  await select({ type: "draft", kind: "session", projectId: "project" });
  await until(() => toggle() && measured(), "the new session's screen");
  toggle()!.click();
  await until(pressed, "Context to open the draft's card");
  toggle()!.click();
  await until(() => !pressed(), "Context to close the draft's card");
  await frames();
  seen.draftClosed = !open();
  void send({ text: "Start the release notes", attachments: [], mentions: [] }, { kind: "session", projectId: "project", setup: null }).catch(() => {});
  await until(() => useApp.getState().selection.type === "conversation" && measured(), "the new session's thread");
  await until(() => pressed() && card(), "the new session's card to open by itself").catch(() => {});
  seen.firstSendOpened = pressed() && !!card();
  toggle()!.click();
  await until(() => !pressed(), "Context to close the new session's card");
  await frames();
  useApp.setState((state) => ({ conversations: { ...state.conversations, started: { ...state.conversations.started!, title: "Renamed" } } }));
  await frames();
  seen.firstSendOnce = !open();

  await select({ type: "conversation", id: "first" });
  await until(() => bubble() && measured(), "the session's thread");
  await frames();
  seen.layout = useSummary.getState().layout;
  seen.noSetupClosed = !open() && !card();

  setUp("first");
  await until(() => pressed() && card(), "the card to open by itself").catch(() => {});
  seen.firstContentOpened = pressed() && !!card();
  await until(settled, "the card and the thread to settle").catch(() => {});
  await frames();
  const shown = card();
  seen.cardText = shown?.textContent ?? null;
  seen.messageRight = bubble()?.getBoundingClientRect().right ?? null;
  seen.cardLeft = shown?.getBoundingClientRect().left ?? null;

  toggle()!.click();
  await until(() => !pressed(), "Context to close the card");
  await frames();
  const preview = { id: "dev", conversationId: "first", name: "Web", command: "pnpm dev", workdir: "/tmp", workspace: "/tmp",
    pid: null, log: null, state: { type: "running" }, startedAtMs: Date.now(), endedAtMs: null, kind: "web", building: false };
  revealNewPreviews([{ stream: "conversation:first", event: { type: "previewUpdated", preview } }] as unknown as EventEnvelope[]);
  await frames();
  seen.closedAfterPreview = !open();
  await select({ type: "conversation", id: "other" });
  await until(() => useBoard.getState().board?.conversationId === "other", "the other session");
  await select({ type: "conversation", id: "first" });
  await until(() => bubble() && useBoard.getState().board?.loaded, "the session again");
  await frames();
  seen.closedOnReturn = !open();

  await fixtureInput({ type: "viewport", width: 760, height: 900 });
  await until(() => useSummary.getState().layout === "float", "the narrow pane to float the card");
  await select({ type: "conversation", id: "narrow" });
  await until(() => useBoard.getState().board?.conversationId === "narrow" && useBoard.getState().board?.loaded, "the narrow session");
  await frames();
  setUp("narrow");
  await frames();
  await frames();
  seen.narrowClosed = !open() && !card();

  // The same narrow pane: a new session's first send opens its card once, floating over the thread.
  await select({ type: "draft", kind: "session", projectId: "project" });
  await until(() => toggle() && measured() && useSummary.getState().layout === "float", "the narrow new session's screen");
  await frames();
  seen.narrowDraftClosed = !open() && !card();
  void send({ text: "Start in a narrow window", attachments: [], mentions: [] }, { kind: "session", projectId: "project", setup: null }).catch(() => {});
  await until(() => useApp.getState().selection.type === "conversation" && measured(), "the narrow new session's thread");
  await until(() => useSummary.getState().floating && card(), "the narrow new session's card to float open").catch(() => {});
  seen.narrowFirstSendFloats = useSummary.getState().floating && !!card();
  if (seen.narrowFirstSendFloats) {
    toggle()!.click();
    await until(() => !useSummary.getState().floating, "Context to close the floating card");
    await frames();
  }
  seen.narrowClosedStays = !open() && useApp.getState().summaryShown.startedNarrow === false;
  return seen;
}

function Page() {
  const selection = useApp((s) => s.selection);
  const [result, setResult] = useState("");
  useEffect(() => {
    void drive().catch((error: unknown) => ({ error: String(error) })).then((seen) => setResult(JSON.stringify(seen)));
  }, []);
  return (
    <TooltipProvider>
      <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
        <main className="relative flex min-h-0 w-full flex-1 flex-col">
          <ConversationView key={selection.type === "conversation" ? selection.id : selection.type} selection={selection} />
        </main>
        {result && <pre id="summary-card-result">{result}</pre>}
      </SidebarProvider>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Page />);
