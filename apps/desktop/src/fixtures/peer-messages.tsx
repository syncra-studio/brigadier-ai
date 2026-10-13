/**
 * A standalone Vite entry, excluded from the app entry and production build: sessions of two
 * projects messaging each other (SESSION-MESSAGING-PLAN.md §6), in the app's own window layout,
 * over IPC answered here. Each session's history comes in pages through `getConversation` and
 * `listMessages`, as from the daemon.
 *
 * - The gallery (default): "API schema" (project Website) sent messages that stand in every
 *   state, its newest turn still sending; "Docs" (project Handbook) received them: a message that
 *   opened a turn, one steered into a running turn, a reply to an earlier request, and its own
 *   turn resting "Waiting for API schema". `open=docs` opens Docs. `unfold=1` opens every turn's
 *   work and every message row, for a look at all of it.
 * - `history=long`: both sessions with long histories, read ten messages a page: the messages
 *   between them sit in old turns, folded, on pages not yet read.
 *
 * `drive=gallery` reads every state off the page; `drive=jump` (with `history=long`) jumps from
 * a sent row to its message in the receiver and from a received message to its sent row, and
 * checks the sidebar's unread dot. Each writes what it saw to `<pre id="peer-result">`.
 * `density=compact` for the compact density.
 */
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { AppSidebar, AppStrip, TitlebarNav } from "@/app/AppSidebar";
import { ConversationView } from "@/app/ConversationView";
import { SidebarFoot } from "@/app/SidebarFoot";
import { SidebarPanel, SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type {
  Conversation,
  ConversationView as View,
  Message,
  OrchestratorStep,
  PeerMessage,
  PeerState,
  Project,
  Request,
  RequestState,
  UserRequest,
} from "@/ipc/generated";
import { openConversation } from "@/state/actions";
import { useBoard } from "@/state/board";
import { LAST_OPENED_KEY } from "@/state/lastOpened";
import { useReveal } from "@/state/reveal";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
document.documentElement.dataset.density = query.get("density") === "compact" ? "compact" : "normal";
document.documentElement.dataset.platform = "macos";
// Nothing opened before: the sidebar's unread marks start from none.
localStorage.removeItem(LAST_OPENED_KEY);
const long = query.get("history") === "long";

const API = "peer-api";
const DOCS = "peer-docs";
const NOTES = "peer-notes";
const STYLE = "peer-style";
/** A session deleted since it was messaged. */
const GONE = "peer-gone";
const WEBSITE = "peer-website";
const HANDBOOK = "peer-handbook";

/** One session's history as the daemon keeps it: its messages, requests, steps and records. */
type Side = {
  id: string;
  messages: Message[];
  requests: Record<string, UserRequest>;
  steps: OrchestratorStep[];
  peers: PeerMessage[];
  seq: number;
  at: number;
  /** Where its pages start, by message seq (`history=long`), besides every ten messages. */
  cuts: number[];
  /** Messages that share a page, by seq, from and to. */
  together: [number, number][];
};

const start = Date.now() - 6 * 3_600_000;
const sides: Record<string, Side> = {};
function side(id: string): Side {
  sides[id] ??= { id, messages: [], requests: {}, steps: [], peers: [], seq: 0, at: start, cuts: [], together: [] };
  return sides[id];
}

/** The next place in the session's stream, 20 seconds on. */
function tick(s: Side): { seq: number; at: number } {
  s.seq += 1;
  s.at += 20_000;
  return { seq: s.seq, at: s.at };
}

function message(s: Side, fields: Pick<Message, "id" | "role" | "text"> & Partial<Message>): Message {
  const { seq, at } = tick(s);
  const shaped = {
    conversationId: s.id,
    seq,
    blob: null,
    createdAtMs: at,
    attachments: [],
    mentions: [],
    model: null,
    requestId: null,
    parentId: s.messages.at(-1)?.id ?? null,
    ...fields,
  } as Message;
  s.messages.push(shaped);
  return shaped;
}

function request(s: Side, id: string, preview: string, atMs: number, more: Partial<UserRequest> = {}): void {
  s.requests[id] = {
    id,
    conversationId: s.id,
    preview,
    state: { type: "done" },
    startedAtMs: atMs,
    endedAtMs: null,
    steeredInto: null,
    steeredAfter: null,
    undo: null,
    worked: [{ fromMs: atMs, toMs: null }],
    quotaWait: false,
    ...more,
  };
}

/** The user asks: a request of their own. */
function ask(s: Side, id: string, text: string): void {
  const asked = message(s, { id, role: "user", text, requestId: id });
  request(s, id, text, asked.createdAtMs);
}

/** Another session's message as this one received it, filed under `filed` (its own request by default). */
function received(s: Side, id: string, from: string, text: string, options: { filed?: string; replyTo?: string; steeredInto?: string } = {}): void {
  const filed = options.filed ?? id;
  const shaped = message(s, {
    id,
    role: "peer",
    text,
    requestId: filed,
    peer: { messageId: id, from, ...(options.replyTo && { replyTo: options.replyTo }) },
  });
  s.peers.push({
    messageId: id,
    outgoing: false,
    peer: from,
    ...(options.replyTo && { replyTo: options.replyTo }),
    requestId: filed,
    state: { type: "seen" },
    text: "",
    blob: null,
    sentAtMs: null,
    settled: false,
    position: shaped.seq,
  });
  if (filed === id) request(s, id, text, shaped.createdAtMs, { fromPeer: from, steeredInto: options.steeredInto ?? null });
  const conversation = conversations[s.id];
  if (conversation) conversation.peerAtMs = Math.max(conversation.peerAtMs ?? 0, shaped.createdAtMs);
}

function sent(s: Side, filed: string, id: string, to: string, text: string, state: PeerState, replyTo?: string): void {
  const { seq, at } = tick(s);
  s.peers.push({
    messageId: id,
    outgoing: true,
    peer: to,
    ...(replyTo && { replyTo }),
    requestId: filed,
    state,
    text,
    blob: null,
    sentAtMs: state.type === "waitingForOk" ? null : at,
    settled: state.type !== "delivered" && state.type !== "seen",
    position: seq,
  });
}

function read(s: Side, filed: string, name: string): void {
  const { seq, at } = tick(s);
  s.steps.push({ requestId: filed, kind: { type: "readArtifact", name }, atMs: at, position: seq });
}

function refused(s: Side, filed: string, title: string, reason: string): void {
  const { seq, at } = tick(s);
  s.steps.push({ requestId: filed, kind: { type: "messageRefused", title, reason }, atMs: at, position: seq });
}

/** The thread's answer, which ends the request (or leaves it as `state` says). */
function answer(s: Side, filed: string, text: string, state: RequestState = { type: "done" }): void {
  const shaped = message(s, { id: `${filed}-answer-${s.seq + 1}`, role: "assistant", text, requestId: filed });
  const asked = s.requests[filed];
  if (!asked) return;
  const over = state.type !== "working" && state.type !== "waiting";
  asked.state = state;
  asked.endedAtMs = over ? shaped.createdAtMs : null;
  // Resting "Waiting for" a reply, it no longer works: its span ends, as the daemon ends it.
  asked.worked = [{ fromMs: asked.startedAtMs, toMs: state.type === "working" ? null : shaped.createdAtMs }];
}

/** Turns of other work, to make a history long. */
function filler(s: Side, count: number): void {
  for (let index = 0; index < count; index++) {
    const id = `${s.id}-r${s.seq + 1}`;
    ask(s, id, `Check item ${s.seq} of the checklist.`);
    answer(s, id, `Item ${s.seq - 1} is fine: nothing to change.`);
  }
}

const SCHEMA_CHANGED = [
  "The `orders` table gained a `status` column. Please update the API guide's **Orders** section:",
  "",
  "- `status` is one of `pending`, `paid` or `shipped`",
  "- it is set to `pending` when an order is created",
  "- `PATCH /orders/{id}` may move it forward, never back",
  "- the old `is_paid` flag stays for one release, then goes",
  "",
  "Tell me when it's done, and whether anything in the guide still names `is_paid` as the way to check payment.",
].join("\n");

const conversations: Record<string, Conversation> = {};
function session(id: string, projectId: string, title: string): void {
  conversations[id] = {
    id,
    kind: "session",
    projectId,
    title,
    pinnedAtMs: null,
    createdAtMs: start,
    updatedAtMs: start,
    lifecycle: "active",
    forkedFrom: null,
    sideOf: null,
    fallback: null,
    quotaWait: null,
    setup: {
      type: "session",
      repo: `/tmp/peer-fixture/${projectId}`,
      environment: { type: "newWorktree", branch: `brigadier/${id}/session`, base: "main" },
      permission: "askForApproval",
      planMode: false,
      workersSeeUncommitted: null,
      orchestrator: { provider: "claude", model: "opus", effort: "high" },
    },
  } as unknown as Conversation;
}
session(API, WEBSITE, "API schema");
session(NOTES, WEBSITE, "Release notes");
session(DOCS, HANDBOOK, "Docs");
session(STYLE, HANDBOOK, "Style guide");

if (!long) {
  const api = side(API);
  const docs = side(DOCS);
  // Docs' history before API schema's message came.
  filler(docs, 1);

  ask(api, "a1", "Tell the Docs session the schema changed and ask them to update the guide.");
  read(api, "a1", "schema.sql");
  read(api, "a1", "openapi.yaml");
  sent(api, "a1", "m1", DOCS, SCHEMA_CHANGED, { type: "replied" });
  received(docs, "m1", API, SCHEMA_CHANGED);
  read(docs, "m1", "guide/orders.md");
  const reply = "Done: the **Orders** section documents `status` and its three values, and `is_paid` is marked deprecated.";
  sent(docs, "m1", "m1-reply", API, reply, { type: "seen" }, "m1");
  answer(docs, "m1", "Updated the Orders section of the guide and told API schema.");
  received(api, "m1-reply", DOCS, reply, { filed: "a1", replyTo: "m1" });
  answer(api, "a1", "Docs updated the guide: `status` is documented and `is_paid` is marked deprecated.");

  ask(api, "a2", "Let Release notes know the schema changed.");
  sent(api, "a2", "m2", NOTES, "The `orders` table has a new `status` column; worth a line in the next release notes.", { type: "seen" });
  answer(api, "a2", "Release notes has it.");

  ask(docs, "d2", "Draft the changelog for 2.4.");
  read(docs, "d2", "CHANGELOG.md");
  ask(api, "a3", "Tell Docs about the new events endpoint.");
  sent(api, "a3", "m3", DOCS, "There's a new `GET /orders/{id}/events` endpoint: mention it in the changelog.", { type: "delivered" });
  received(docs, "m3", API, "There's a new `GET /orders/{id}/events` endpoint: mention it in the changelog.", { steeredInto: "d2" });
  answer(api, "a3", "Sent; Docs hasn't picked it up yet.");
  answer(docs, "d2", "Drafted the 2.4 changelog, with the new events endpoint.");
  answer(docs, "m3", "Added.");

  ask(api, "a4", "Ask Docs to drop the old payments page.");
  sent(api, "a4", "m4", DOCS, "Please delete `guide/payments-v1.md`.", { type: "declined" });
  answer(api, "a4", "You chose not to send it, so the page stays.");

  ask(api, "a5", "Tell the Billing session too.");
  refused(api, "a5", "Billing", "Billing is archived, so it can't take messages.");
  answer(api, "a5", "Billing is archived; unarchive it and I'll tell it.");

  ask(api, "a6", "Send the migration notes to the Payments session.");
  sent(api, "a6", "m6", GONE, "Migration notes: run `0042_order_status.sql` before deploying.", { type: "dropped", reason: "deleted" });
  answer(api, "a6", "The Payments session was deleted before it took the message.");

  ask(docs, "d3", "Ask API schema which fields are deprecated.");
  sent(docs, "d3", "q1", API, "Which fields of `orders` are deprecated?", { type: "replied" });
  answer(docs, "d3", "Asked API schema; I'll carry on when it answers.", { type: "waiting" });
  docs.requests["d3"]!.waitingFor = [API];
  ask(docs, "d4", "Meanwhile, fix the typo in the intro.");
  answer(docs, "d4", "Fixed: “recieve” is now “receive”.");
  docs.requests["d3"]!.waitingFor = [];
  received(docs, "q1-reply", API, "Deprecated: `is_paid` and `shipping_code`. Both go in 2.6.", { filed: "d3", replyTo: "q1" });
  answer(docs, "d3", "API schema says `is_paid` and `shipping_code` are deprecated; both are marked in the guide.");

  const style = side(STYLE);
  style.at = docs.at;
  received(docs, "s1", STYLE, "Headings use sentence case from now on: “Order status”, not “Order Status”.");
  answer(docs, "s1", "Switched the guide's headings to sentence case.");

  ask(docs, "d6", "Ask API schema to confirm the field names before I publish.");
  sent(docs, "d6", "q2", API, "Before I publish: are the field names in `guide/orders.md` final?", { type: "delivered" });
  answer(docs, "d6", "Asked API schema to confirm.", { type: "waiting" });
  docs.requests["d6"]!.waitingFor = [API];
  received(api, "q2", DOCS, "Before I publish: are the field names in `guide/orders.md` final?", { filed: "q2" });
  answer(api, "q2", "Confirmed them; the reply goes to Docs once I've checked the migration.", { type: "working" });

  // API schema's newest turn, still sending: one message waits for the user's OK, one goes out.
  ask(api, "a7", "Ask Docs to review the guide, and tell Release notes it's ready.");
  sent(api, "a7", "m7", DOCS, "Could you review the Orders section once more?", { type: "waitingForOk" });
  sent(api, "a7", "m8", NOTES, "The guide is ready for the release notes.", { type: "pending" });
  api.requests["a7"]!.state = { type: "working" };
  // The turn that answered Docs' question is over before it.
  api.requests["q2"]!.state = { type: "done" };
  api.requests["q2"]!.endedAtMs = api.requests["a7"]!.startedAtMs;
} else {
  const api = side(API);
  const docs = side(DOCS);
  filler(api, 4);
  filler(docs, 3);
  // Long ago: API schema tells Docs, which takes it in a turn of its own.
  ask(api, "j1", "Tell Docs the schema changed.");
  read(api, "j1", "schema.sql");
  read(api, "j1", "openapi.yaml");
  sent(api, "j1", "ml", DOCS, SCHEMA_CHANGED, { type: "seen" });
  answer(api, "j1", "Told Docs.");
  received(docs, "ml", API, SCHEMA_CHANGED);
  const first = docs.seq;
  answer(docs, "ml", "Updated the guide.");
  // Then Docs starts a long turn; a page of its history ends just after the turn's message, and
  // holds the message before it too.
  ask(docs, "jb", "Draft the release blog post.");
  docs.cuts.push(first, docs.seq + 1);
  docs.together.push([first, docs.seq]);
  for (let index = 0; index < 8; index++) message(docs, { id: `jb-update-${index}`, role: "assistant", text: `Drafting section ${index + 1}.`, requestId: "jb" });
  filler(api, 32);
  // API schema's message comes into that running turn, steered in.
  ask(api, "j2", "Tell Docs about the events endpoint.");
  read(api, "j2", "routes.ts");
  read(api, "j2", "events.ts");
  sent(api, "j2", "mk", DOCS, "There's a new `GET /orders/{id}/events` endpoint: mention it in the post.", { type: "seen" });
  answer(api, "j2", "Told Docs about the events endpoint.");
  received(docs, "mk", API, "There's a new `GET /orders/{id}/events` endpoint: mention it in the post.", { steeredInto: "jb" });
  answer(docs, "jb", "Drafted the post, with the events endpoint.");
  answer(docs, "mk", "Mentioned it.");
  // Docs works on, long after.
  filler(docs, 32);
}

/** Where each page of a long history starts: every ten messages from the newest, and the cuts asked for. */
function pageStarts(s: Side): number[] {
  if (!long) return [1];
  const starts = new Set([1, ...s.cuts]);
  for (let at = s.seq - 9; at > 1; at -= 10) {
    if (!s.together.some(([from, to]) => at > from && at <= to)) starts.add(at);
  }
  return [...starts].toSorted((a, b) => a - b);
}

function page(s: Side, before: number): { messages: Message[]; hasMore: boolean } {
  const from = pageStarts(s).findLast((at) => at < before) ?? 1;
  return { messages: s.messages.filter((m) => m.seq >= from && m.seq < before), hasMore: from > 1 };
}

function view(id: string): View {
  const s = side(id);
  const conversation = conversations[id]!;
  conversation.updatedAtMs = s.messages.at(-1)?.createdAtMs ?? start;
  const live = Object.values(s.requests).find((asked) => asked.state.type === "working");
  return {
    conversation,
    context: null,
    messages: page(s, Number.POSITIVE_INFINITY),
    tasks: [],
    approvals: [],
    questions: [],
    plans: [],
    requests: Object.values(s.requests),
    workerSteps: [],
    orchestratorSteps: s.steps,
    machineSteps: [],
    thinking: [],
    compactions: [],
    decisions: [],
    waiting: [],
    queue: { items: [], paused: false },
    run: live ? "running" : "idle",
    runRequest: live?.id ?? null,
    head: s.messages.at(-1)?.id ?? null,
    ratings: {},
    streaming: null,
    notices: [],
    memories: [],
    overnight: [],
    reviews: [],
    previews: [],
    peerMessages: s.peers,
  } as unknown as View;
}
for (const id of [API, NOTES, STYLE]) side(id);
for (const id of Object.keys(conversations)) conversations[id]!.updatedAtMs = side(id).messages.at(-1)?.createdAtMs ?? start;

const project = (id: string, name: string) =>
  ({ id, name, createdAtMs: start, repos: [{ path: `/tmp/peer-fixture/${id}`, name }], prefs: {}, trust: [] }) as unknown as Project;

useApp.setState((state) => ({
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { status: "connected", daemon: null, reason: null },
  settings: { ...state.settings, onboarded: true, introSeen: true },
  projects: { [WEBSITE]: project(WEBSITE, "Website"), [HANDBOOK]: project(HANDBOOK, "Handbook") },
  conversations,
  expandedProjects: { [WEBSITE]: true, [HANDBOOK]: true },
}));

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const asked = (payload as { request: Request }).request;
  switch (asked.method) {
    case "getConversation":
      return { method: asked.method, view: view(asked.id) };
    case "listMessages":
      return { method: asked.method, page: page(side(asked.conversationId), asked.before ?? Number.POSITIVE_INFINITY) };
    case "listFiles":
      return { method: asked.method, files: [], truncated: false };
    default:
      return { method: asked.method };
  }
}, { shouldMockEvents: true });
mockWindows("main");

openConversation(query.get("open") === "docs" ? DOCS : API);

/** The app's window layout (App.tsx) around the open session, without its dialogs. */
function PeerPage() {
  const selection = useApp((s) => s.selection);
  return (
    <TooltipProvider>
      <div className="bg-chrome flex h-screen flex-col">
        <SidebarProvider className="relative min-h-0 flex-1 flex-col" defaultOpen>
          <div
            aria-hidden
            data-slot="page-surface"
            className="bg-background rounded-page shadow-page top-titlebar start-surface-inset end-surface-inset bottom-surface-inset pointer-events-none absolute"
          />
          <div className="relative flex min-h-0 min-w-0 flex-1 ps-surface-inset pe-surface-inset pb-surface-inset">
            <SidebarPanel strip={<AppStrip />} foot={<SidebarFoot />}>
              <AppSidebar />
            </SidebarPanel>
            <main className="body-divider relative flex h-full min-w-0 flex-1">
              <div className="relative flex h-full min-w-0 flex-1 flex-col">
                <ConversationView selection={selection} />
              </div>
            </main>
          </div>
          <TitlebarNav />
        </SidebarProvider>
      </div>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<PeerPage />);

// ---- Driving the page -------------------------------------------------------------------------

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (check: () => boolean, ms = 8000) => {
  for (const end = Date.now() + ms; Date.now() < end && !check();) await wait(25);
  return check();
};
const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [...root.querySelectorAll<T>(selector)];
const one = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => root.querySelector<T>(selector);
const text = (element: Element | null) => element?.textContent?.replace(/\s+/g, " ").trim() ?? null;
const viewport = () => one("[data-slot=aui_thread-viewport]");

/** Waits for `id`'s thread to be read and shown. */
async function shown(id: string): Promise<boolean> {
  return until(() => {
    const board = useBoard.getState().board;
    const thread = useApp.getState().threads[id];
    return board?.conversationId === id && board.loaded && !!thread && !thread.loading && all("[data-role=assistant]").length > 0;
  });
}

/** Every turn's work and every group and message row opened, as a reader would. */
async function unfoldAll(): Promise<void> {
  for (const header of all("[data-slot=request-work-header][aria-expanded=false]")) header.click();
  await wait(450);
  for (const group of all("[data-slot=work-group] > button[data-state=closed]")) group.click();
  await wait(300);
  for (const row of all("[data-slot=orchestrator-step][data-kind=message] > [role=button][data-state=closed]")) row.click();
  await wait(300);
}

/** Where a jump landed: the message's element, how far its centre is from the view's, and what holds it. */
function landing(messageId: string) {
  const element = one(`[data-peer-message="${messageId}"]`);
  const box = viewport()?.getBoundingClientRect();
  const rect = element?.getBoundingClientRect();
  const scroller = viewport();
  return {
    found: !!element,
    revealed: element?.dataset["revealed"] === "true",
    offCentre: box && rect ? Math.round(rect.top + rect.height / 2 - (box.top + box.height / 2)) : null,
    inView: !!(box && rect && rect.top >= box.top && rect.bottom <= box.bottom),
    clamped: !!scroller && (scroller.scrollTop <= 0 || scroller.scrollTop >= scroller.scrollHeight - scroller.clientHeight - 1),
    group: element?.closest("[data-slot=work-group]")?.getAttribute("data-state") ?? null,
    workOpen: !!element?.closest("[data-slot=request-work]"),
    inTurn: !!element?.closest("[data-role=assistant]"),
    selection: (() => {
      const selected = useApp.getState().selection;
      return selected.type === "conversation" ? selected.id : null;
    })(),
  };
}

/** Waits for a jump to `messageId` to land, its folds still and its flash over. */
async function landed(messageId: string): Promise<void> {
  await until(() => useReveal.getState().reveal === null && one(`[data-peer-message="${messageId}"][data-revealed=true]`) !== null, 15000);
  await wait(800);
}

function dots(): Record<string, boolean> {
  return Object.fromEntries(
    [API, DOCS, NOTES, STYLE].map((id) => {
      const row = all("li > button").find((button) => button.getClientRects().length > 0 && text(button.querySelector("span")) === conversations[id]!.title);
      return [id, !!row?.querySelector("[data-status=unread]")];
    }),
  );
}

async function driveGallery(): Promise<object> {
  await shown(API);
  await unfoldAll();
  const rows = all("[data-slot=orchestrator-step][data-kind=message]").map((row) => ({
    state: row.getAttribute("data-state"),
    label: text(one("[role=button] > span, :scope > span", row)),
    link: !!one("[data-slot=peer-link]", row),
    line: text(one("[data-slot=peer-state]", row)),
  }));
  const groups = all("[data-slot=work-group] > button").map((group) => text(group));
  const replyInSender = all("[data-slot=peer-message]").map((block) => ({ caption: text(one("[data-slot=peer-caption]", block)), inTurn: !!block.closest("[data-role=assistant]") }));
  // "Open reply" jumps to the reply within the same session.
  one<HTMLButtonElement>("[data-slot=peer-open-reply]")?.click();
  await landed("m1-reply");
  const openReply = landing("m1-reply");

  openConversation(DOCS);
  await shown(DOCS);
  await wait(300);
  const blocks = all("[data-slot=peer-message]").map((block) => {
    const bubble = block.querySelector<HTMLElement>("[data-slot=peer-text]")?.parentElement?.parentElement ?? null;
    const style = bubble ? getComputedStyle(bubble) : null;
    return {
      id: block.getAttribute("data-peer-message"),
      caption: text(one("[data-slot=peer-caption]", block)),
      label: block.getAttribute("aria-label"),
      inTurn: !!block.closest("[data-role=assistant]"),
      asInput: !!block.closest("[data-role=user]"),
      outline: style?.borderTopWidth ?? null,
      fill: style?.backgroundColor ?? null,
      more: !!Array.from(block.querySelectorAll("button")).find((button) => text(button) === "Show more"),
    };
  });
  // Show more opens a long message in full.
  const longest = one("[data-peer-message=m1]");
  const clipped = longest?.querySelector<HTMLElement>("[data-slot=peer-text]")?.clientHeight ?? 0;
  Array.from(longest?.querySelectorAll("button") ?? []).find((button) => text(button) === "Show more")?.click();
  await wait(200);
  const opened = longest?.querySelector<HTMLElement>("[data-slot=peer-text]")?.clientHeight ?? 0;
  const waiting = {
    header: text(all("[data-slot=request-work-label]").at(-1) ?? null),
    status: text(one("[data-slot=thread-status]")),
  };
  await unfoldAll();
  const docsRows = all("[data-slot=orchestrator-step][data-kind=message]").map((row) => ({
    state: row.getAttribute("data-state"),
    label: text(one("[role=button] > span, :scope > span", row)),
  }));
  return { rows, groups, replyInSender, openReply, blocks, showMore: { clipped, opened }, waiting, docsRows };
}

async function driveJump(): Promise<object> {
  await shown(API);
  await wait(300);
  const before = dots();
  // From API schema's newest turn: its work opened, its group opened, the name clicked.
  all("[data-slot=request-work-header][aria-expanded=false]").at(-1)?.click();
  await wait(450);
  const row = () => one("[data-peer-message=mk]");
  all("[data-slot=work-group] > button[data-state=closed]").at(-1)?.click();
  await until(() => !!row());
  const loadedBefore = useApp.getState().threads[DOCS]?.items.length ?? 0;
  one<HTMLButtonElement>("[data-slot=peer-link]", row() ?? document)?.click();
  await landed("mk");
  const toReceiver = { ...landing("mk"), pagesRead: (useApp.getState().threads[DOCS]?.items.length ?? 0) > 10, loadedBefore };
  const afterOpen = dots();

  // From Docs' older received message back to its sent row in API schema, years of turns ago.
  const caption = one<HTMLButtonElement>("[data-peer-message=ml] [data-slot=peer-caption]");
  const hadCaption = !!caption;
  caption?.click();
  await landed("ml");
  const toSender = landing("ml");
  const afterBack = dots();

  // A message comes to Docs while API schema is open, and one to API schema itself.
  const later = Date.now() + 60_000;
  useApp.setState((state) => ({
    conversations: {
      ...state.conversations,
      [DOCS]: { ...state.conversations[DOCS]!, peerAtMs: later },
      [API]: { ...state.conversations[API]!, peerAtMs: later },
    },
  }));
  await wait(200);
  const afterArrival = dots();
  openConversation(DOCS);
  await shown(DOCS);
  await wait(200);
  const afterReopen = dots();
  return { before, toReceiver, afterOpen, hadCaption, toSender, afterBack, afterArrival, afterReopen };
}

Object.assign(window, { peerFixture: { useApp, useBoard, openConversation } });

const drive = query.get("drive");
if (drive === "gallery" || drive === "jump") {
  void (drive === "gallery" ? driveGallery() : driveJump())
    .catch((error: unknown) => ({ error: String(error) }))
    .then((result) => {
      const out = document.createElement("pre");
      out.id = "peer-result";
      out.textContent = JSON.stringify(result);
      document.body.append(out);
    });
} else if (query.get("unfold") === "1") {
  void shown(useApp.getState().selection.type === "conversation" ? (useApp.getState().selection as { id: string }).id : API).then(unfoldAll);
}
