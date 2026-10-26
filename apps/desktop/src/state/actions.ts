import { cancelSettingReveal, revealSetting } from "@/app/settings/reveal";
import { expectSentTurn } from "@/components/conversation/thread-scroll";
import { browserFocus, request } from "@/ipc/client";
import type {
  AnswerInput,
  Access,
  ApprovalDecision,
  AttachmentRef,
  BranchChoice,
  ContinueWhen,
  Conversation,
  ConversationStatus,
  DiffStat,
  FeedbackSent,
  ForkPlace,
  LifecycleOutcome,
  Mention,
  MessageMatches,
  MessageQueue,
  ModelChoice,
  QueuedMessage,
  Project,
  ProjectPatch,
  ProjectRemoval,
  ProviderKind,
  Rating,
  RawApprovals,
  RawSession,
  RemoveProjectReport,
  RepoInfo,
  RestoreOutcome,
  Setup,
  SetupRequest,
  UnlandedBranch,
} from "@/ipc/generated";
import { saveSummaryShown } from "@/lib/pinnedSummary";
import { startReveal } from "@/state/reveal";
import { recordPlace, replacePlace, stepPlace, type HistoryMode } from "@/state/history";
import { useAgentPageAsk } from "@/state/agentSignIn";
import { providerForSetting } from "@/state/providerDestination";
import { CHAT_TAB, openSideChatTab, selectTab } from "@/state/sessionTabs";
import { isTaskTab } from "@/state/tabStates";
import { addStop, dropStop, dropTurnStops, keepStopping, type Stop, useStopping } from "@/state/stopping";
import { onRealActivity, realActivity, workHolds } from "@/state/activity";
import { useCliThread } from "@/state/cliThread";
import { workerDone, workerStoppable } from "@/app/conversation/workerPresentation";
import {
  boardHeld,
  boardOf,
  collectBoardEvents,
  emptyBoard,
  keepBoard,
  placeView,
  takeKeptBoard,
  updateBoard,
  useBoard,
} from "@/state/board";
import {
  confirmSent,
  emptyDraft,
  emptyThread,
  mergeMessages,
  replaceCatalog,
  type AppState,
  type InspectorTab,
  type ProvidersState,
  type RawTranscript,
  type Selection,
  type SettingsPageId,
  type Thread,
  summaryKey,
  upsertRawSession,
  useApp,
} from "@/state/store";
import { replacingCatalog, trackCatalogChanges } from "@/state/catalogRefresh";
import { migrateDrafts } from "@/state/pageTabs";
import { pruneSideChats } from "@/state/sessionTabResources";
import { keepTabChats, prepareTabChatSend } from "@/state/tabChats";
import { forgetDraft } from "@/state/drafts";
import { forgetHeldDirect } from "@/state/heldDirect";
import { reopenable, rememberSelection, savedSelection } from "@/state/lastSelection";
import { forgetRightSidebarTabs, pruneRightSidebarTabs } from "@/state/rightSidebar";
import { stillGoing } from "@/state/tabStates";
import { dismissToast, toast, useToasts } from "@/state/toasts";

/** Messages shown when a conversation opens; older ones load on demand. */
const PAGE = 200;

function updateThread(id: string, update: (thread: Thread) => Thread): void {
  useApp.setState((state) => ({
    threads: {
      ...state.threads,
      [id]: update(state.threads[id] ?? emptyThread),
    },
  }));
}

/** Whether the launch has looked for the conversation to reopen (once, at the first catalog). */
let reopened = false;

/**
 * At launch, reopens the conversation the user last had open, unless they have already moved
 * on from the blank home (UI audit M2). Then remembers each conversation they open.
 */
function reopenLastSelection(): void {
  if (reopened) return;
  reopened = true;
  const { selection, conversations } = useApp.getState();
  const id = selection.type === "draft" && selection.kind === "chat" ? reopenable(savedSelection(), conversations) : null;
  if (id) select({ type: "conversation", id }, "replace");
  useApp.subscribe((state, previous) => {
    if (state.selection !== previous.selection) rememberSelection(state.selection);
  });
}

export async function loadCatalog(): Promise<void> {
  const changes = trackCatalogChanges();
  try {
    const { catalog } = await request({ method: "getCatalog" });
    // A live creation can beat this older snapshot's answer. Replacing it outright would
    // hide the row until another catalog read, even as its thread keeps receiving messages.
    const { projects, conversations } = changes.merge(catalog);
    changes.stop();
    // Another read in flight must not take this snapshot for live changes.
    replacingCatalog(() => replaceCatalog(projects, conversations, catalog.settings));
    reopenLastSelection();
    pruneRightSidebarTabs(conversations.map((conversation) => conversation.id));
    pruneSideChats(conversations);
    void keepTabChats().catch(console.error);
    void migrateDrafts();
  } finally {
    changes.stop();
  }
}

/**
 * Loads a conversation's view (newest messages, tasks, cards, queue, run state), replacing
 * what was loaded. Live events that arrived meanwhile are kept.
 */
/**
 * Reads a conversation's view: its newest page of messages, its board. History older than the
 * page that the thread already holds stays (#14): messages never change, and a thread kept in
 * the store takes every new one live, so what it holds joins the page, unless `fresh` says
 * events may have been missed (a resync), when only the page is trusted.
 */
export async function loadConversation(id: string, { fresh = false }: { fresh?: boolean } = {}): Promise<void> {
  updateThread(id, (thread) => ({ ...thread, loading: true }));
  const before = useApp.getState().conversations[id];
  const arrived = collectBoardEvents(id);
  try {
    const { view } = await request({ method: "getConversation", id, limit: PAGE });
    const events = arrived();
    const page = view.messages;
    const newest = page.messages.at(-1)?.seq ?? 0;
    updateThread(id, (thread) => {
      // The pages read before, older than this one, when they reach it: no gap between them.
      const oldest = page.messages[0];
      const joined = !fresh && page.hasMore && !!oldest && thread.items.some((message) => message.id === oldest.id);
      const earlier = joined ? thread.items.filter((message) => message.seq < oldest.seq) : [];
      return {
        ...thread,
        // Keep what the live feed delivered after the page was read. Without `earlier`, older
        // items may be stale (e.g. after missing events) and would leave a gap below this page.
        items: mergeMessages(
          [...earlier, ...page.messages],
          thread.items.filter((message) => message.seq > newest),
        ),
        hasMore: earlier.length > 0 ? thread.hasMore : page.hasMore,
        loading: false,
        loadError: null,
      };
    });
    // A copy live events changed while the view was read is newer (a new conversation is
    // renamed by its first message), and one they removed was deleted; whatever else the
    // view holds arrives as events too.
    useApp.setState((state) => {
      const live = state.conversations[id];
      return live !== before
        ? state
        : { conversations: { ...state.conversations, [id]: view.conversation } };
    });
    placeView(view, events);
  } catch (error) {
    arrived();
    updateThread(id, (thread) => ({ ...thread, loading: false, loadError: error instanceof Error ? error.message : String(error) }));
    throw error;
  }
}

export async function loadEarlier(id: string): Promise<void> {
  const thread = useApp.getState().threads[id];
  const oldest = thread?.items[0];
  if (!thread || !oldest || thread.loading) return;
  updateThread(id, (current) => ({ ...current, loading: true }));
  try {
    const { page } = await request({
      method: "listMessages",
      conversationId: id,
      before: oldest.seq,
      limit: PAGE,
    });
    updateThread(id, (current) => ({
      ...current,
      items: mergeMessages(current.items, page.messages),
      hasMore: page.hasMore,
      loading: false,
    }));
  } catch (error) {
    updateThread(id, (current) => ({ ...current, loading: false }));
    throw error;
  }
}

/**
 * Where `query` is, case aside, in the messages before `leaf` on its branch, oldest first: with
 * `leaf` the oldest message the thread shows, in the pages it hasn't loaded. At most `limit` matches.
 */
export async function searchEarlier(id: string, leaf: string, query: string, limit: number): Promise<MessageMatches> {
  const { found } = await request({ method: "searchMessages", conversationId: id, leaf, query, limit });
  return found;
}

/** Blob reads in flight, by conversation and message, so each full text is requested once. */
const fullTextLoading = new Set<string>();

/** Fetches the full text of a message whose body lives in the blob store. */
export async function loadFullText(
  conversationId: string,
  messageId: string,
  hash: string,
): Promise<void> {
  // By conversation too: a message between sessions has one id on both sides.
  const key = `${conversationId}:${messageId}`;
  if (fullTextLoading.has(key)) return;
  fullTextLoading.add(key);
  try {
    const { text } = await request({ method: "readBlobText", hash });
    updateThread(conversationId, (thread) => ({
      ...thread,
      fullText: { ...thread.fullText, [messageId]: text },
    }));
  } finally {
    fullTextLoading.delete(key);
  }
}

/** Where leaving Settings goes back to: what was shown when it opened. */
let beforeSettings: Selection = { type: "draft", kind: "chat" };

/**
 * Shows `selection`. By default it is a new entry of the history Back and Forward go through
 * (see state/history.ts); `mode` can put it in place of the current entry, or leave the
 * history alone. Resolves once it shows, or once it would have but for a later `select`.
 */
export function select(selection: Selection, mode: HistoryMode = "push"): Promise<void> {
  const asked = ++selecting;
  if (selection.type === "conversation" && !conversationHeld(selection.id)) {
    // Nothing of it is held yet: it shows once its view and the code of its replies are in,
    // whole and as it really stands (#484, #500), unless something else is asked for meanwhile.
    // A view that fails to load shows it anyway, with its error and Retry.
    const loaded = loadConversation(selection.id).catch((error: unknown) => {
      console.error("loading the conversation failed", error);
    });
    const code = import("@/app/conversation/threadCode").then((thread) => thread.preloadThreadCode()).catch(() => {});
    return Promise.all([loaded, code]).then(() => {
      if (asked === selecting) show(selection, mode, false);
    });
  }
  show(selection, mode, true);
  return Promise.resolve();
}

/** The newest `select`: an earlier one still waiting for its conversation shows nothing. */
let selecting = 0;

/** Whether a conversation's thread and board are held, so it can show at once as it stands. */
function conversationHeld(id: string): boolean {
  return !!useApp.getState().threads[id] && boardHeld(id);
}

function show(selection: Selection, mode: HistoryMode, refresh: boolean): void {
  const { selection: shown } = useApp.getState();
  if (selection !== shown) {
    cancelSettingReveal();
    useAgentPageAsk.setState({ ask: null });
  }
  // What leaving Settings goes back to, however it was reached (Back and Forward too).
  if (selection.type === "settings" && shown.type !== "settings") beforeSettings = shown;
  recordPlace(selection, mode);
  useApp.setState({ selection });
  const { board } = useBoard.getState();
  if (selection.type !== "conversation") {
    if (board) {
      // Left for Settings or a new chat, it is kept: coming back shows it as it stands.
      keepBoard(board);
      useBoard.setState({ board: null });
    }
    return;
  }
  if (board?.conversationId !== selection.id) {
    keepBoard(board);
    // Its kept board, or the one a side chat shows it with.
    const beside = boardOf(selection.id);
    useBoard.setState({ board: takeKeptBoard(selection.id) ?? (beside?.loaded ? beside : emptyBoard(selection.id)) });
  }
  // What is held is read again, in case something was missed; it shows meanwhile.
  if (!refresh) return;
  void loadConversation(selection.id).catch((error: unknown) => {
    console.error("loading the conversation failed", error);
  });
}

/** What was shown when Settings opened (what the Inspector's Orchestrator tab starts on). */
export function shownBeforeSettings(): Selection {
  return beforeSettings;
}

/**
 * The conversation the Inspector looks at: the open one, or in Settings (where the Inspector
 * lives) the one Settings was opened from.
 */
export function inspectedConversation(state: AppState): Conversation | null {
  const shown = state.selection.type === "settings" ? beforeSettings : state.selection;
  return shown.type === "conversation" ? (state.conversations[shown.id] ?? null) : null;
}

/** Pages merged into another, by the page they are part of now and the row each opens at. */
const MERGED_SETTINGS_PAGES = {
  conversations: { page: "configuration", row: "Model for sessions" },
  accounts: { page: "providers", row: "Accounts" },
  usage: { page: "providers", row: "Accounts" },
} as const satisfies Record<string, { page: SettingsPageId; row: string }>;

/** A Settings page's id, or one of a page since merged into another. */
export type SettingsPageLink = SettingsPageId | keyof typeof MERGED_SETTINGS_PAGES;

/**
 * Shows a page of Settings (what to go back to is remembered by `select`). A merged page's id
 * opens the page it is part of at the row it held.
 */
export function openSettings(page: SettingsPageLink = "general", row?: string): void {
  if (Object.hasOwn(MERGED_SETTINGS_PAGES, page)) {
    const merged = MERGED_SETTINGS_PAGES[page as keyof typeof MERGED_SETTINGS_PAGES];
    openSettings(merged.page, row ?? merged.row);
    return;
  }
  const provider = page === "providers" && row ? providerForSetting(row) : null;
  select(provider
    ? { type: "settings", page: "providers", detail: { provider } }
    : { type: "settings", page: page as SettingsPageId });
  if (row) revealSetting(row);
}

/** Leaves Settings for what was shown before it (a new chat if that conversation is gone). */
export function closeSettings(): void {
  if (useApp.getState().selection.type !== "settings") return;
  const back = beforeSettings;
  const gone = back.type === "conversation" && !useApp.getState().conversations[back.id];
  const lost = back.type === "draft" && back.kind === "session" && !useApp.getState().projects[back.projectId];
  select(gone || lost ? { type: "draft", kind: "chat" } : back);
}

/** ⌘, : opens Settings, or leaves it when it is open. */
export function toggleSettings(): void {
  if (useApp.getState().selection.type === "settings") closeSettings();
  else openSettings();
}

/**
 * Back (-1) and Forward (1) through the history. A conversation deleted since, or a draft whose
 * project was removed, can't show again: arriving there shows Home in its place.
 */
export function stepHistory(delta: -1 | 1): void {
  const target = stepPlace(delta);
  if (!target) return;
  // A native browser page can own the keyboard when the View menu asks to leave it.
  if (typeof document !== "undefined" && !document.hasFocus()) void browserFocus(null).catch(() => {});
  const { conversations, projects } = useApp.getState();
  const gone =
    (target.type === "conversation" && !conversations[target.id]) ||
    (target.type === "draft" && target.kind === "session" && !projects[target.projectId]);
  if (gone) {
    const home: Selection = { type: "draft", kind: "chat" };
    replacePlace(home);
    select(home, "none");
    return;
  }
  if (target.type === "conversation") {
    const conversation = conversations[target.id];
    if (conversation?.projectId) setProjectExpanded(conversation.projectId, true);
  }
  select(target, "none");
}

/** Where `openConversation` lands: a message between sessions, by its id (its row or its block). */
export type OpenAt = { message: string };

/**
 * Shows a conversation; `at` a message between sessions, the thread loads the page that holds
 * it, unfolds what folds it, scrolls it to the centre and flashes it once
 * (SESSION-MESSAGING-PLAN.md §6.3).
 */
export function openConversation(id: string, options: { at?: OpenAt } = {}): Promise<void> {
  const conversation = useApp.getState().conversations[id];
  if (conversation?.projectId) setProjectExpanded(conversation.projectId, true);
  const shown = useApp.getState().selection;
  // A jump within the open conversation stays where it is: only the thread moves.
  const showing = !options.at || shown.type !== "conversation" || shown.id !== id ? select({ type: "conversation", id }) : Promise.resolve();
  if (options.at) {
    // The message is in the thread: a session left on a file or its review shows its chat.
    selectTab(id, CHAT_TAB);
    void revealMessage(id, options.at.message).catch((error: unknown) => console.error("showing the message failed", error));
  }
  return showing;
}

/**
 * Closes a session's view (its chat tab's ×): nothing stops, its work goes on and it stays in
 * the sidebar with its state, where a click opens it again with its tabs as they were. The
 * project's new-session page shows in its place.
 */
export function closeSession(id: string): void {
  const shown = useApp.getState().selection;
  if (shown.type !== "conversation" || shown.id !== id) return;
  const projectId = useApp.getState().conversations[id]?.projectId;
  select(projectId ? { type: "draft", kind: "session", projectId } : { type: "draft", kind: "chat" });
  // Closing stops nothing: a session still going says where it went. An idle one closes quietly.
  if (stillGoing(id)) toast(CLOSED_STILL_RUNNING, { tone: "neutral" });
}

/** The note a session's chat tab leaves when it closes while the session still runs. */
export const CLOSED_STILL_RUNNING = "Still running. Reopen it from the sidebar.";

/** How many older pages a jump reads at most before it shows what it has. */

/** Resolves once `ready` holds, checked as the app's stores change; false after `withinMs`. */
function storesReach(ready: () => boolean, withinMs: number): Promise<boolean> {
  if (ready()) return Promise.resolve(true);
  return new Promise((resolve) => {
    const stop = () => {
      unsubscribeApp();
      unsubscribeBoard();
      clearTimeout(timer);
    };
    const check = () => {
      if (!ready()) return;
      stop();
      resolve(true);
    };
    const unsubscribeApp = useApp.subscribe(check);
    const unsubscribeBoard = useBoard.subscribe(check);
    const timer = setTimeout(() => {
      stop();
      resolve(ready());
    }, withinMs);
  });
}

/** Asks `id`'s thread to show `messageId` once it is loaded: the CLI's thread is read whole. */
async function revealMessage(id: string, messageId: string): Promise<void> {
  const loaded = () => {
    const board = useBoard.getState().board;
    const thread = useApp.getState().threads[id];
    return board?.conversationId === id && board.loaded && !!thread && !thread.loading;
  };
  if (!(await storesReach(loaded, 10_000))) return;
  startReveal(id, messageId);
}

export function setProjectExpanded(id: string, expanded: boolean): void {
  useApp.setState((state) => ({
    expandedProjects: { ...state.expandedProjects, [id]: expanded },
  }));
}

function storeProject(project: Project): void {
  useApp.setState((state) => ({
    projects: { ...state.projects, [project.id]: project },
  }));
}

/** Creates a project on a repository; an empty name names it after the folder. */
export async function createProject(name: string, repo: string | null): Promise<Project> {
  const { project } = await request({ method: "createProject", name, repo });
  storeProject(project);
  setProjectExpanded(project.id, true);
  return project;
}

export async function updateProject(id: string, patch: ProjectPatch): Promise<Project> {
  const { project } = await request({ method: "updateProject", id, patch });
  storeProject(project);
  return project;
}

export async function getRepoInfo(path: string): Promise<RepoInfo> {
  const { repo } = await request({ method: "getRepoInfo", path });
  return repo;
}

/**
 * A session checkout's files, for the composer's @-mentions and the Files tab; with `query`,
 * those whose path has its letters in order, from the whole checkout.
 */
export async function listFiles(
  conversationId: string,
  query: string | null = null,
): Promise<{ files: string[]; truncated: boolean }> {
  const { files, truncated } = await request({ method: "listFiles", conversationId, query });
  return { files, truncated };
}

/** A project folder's files, for the Files pane of a session not started yet. */
export async function listProjectFiles(
  projectId: string,
  query: string | null = null,
): Promise<{ files: string[]; truncated: boolean }> {
  const { files, truncated } = await request({ method: "listProjectFiles", projectId, query });
  return { files, truncated };
}

/** Rates an answer (a message id, or `task:<id>` for a worker's report); null clears the rating. */
export async function rateMessage(
  conversationId: string,
  subject: string,
  rating: Rating | null,
): Promise<void> {
  await request({ method: "rateMessage", conversationId, subject, rating });
}

/**
 * What a worktree session's branch changed against its base, and its commits the base lacks;
 * null for other conversations.
 */
export async function getSessionDiff(id: string): Promise<{ stat: DiffStat; commits: number } | null> {
  const { stat, commits } = await request({ method: "getSessionDiff", id });
  return stat ? { stat, commits } : null;
}

function storeConversation(conversation: Conversation): void {
  useApp.setState((state) => ({
    conversations: { ...state.conversations, [conversation.id]: conversation },
  }));
}

/** "Fork chat from here": opens a new conversation with the thread up to that answer. */
export async function forkConversation(
  conversationId: string,
  messageId: string,
  place: ForkPlace,
): Promise<void> {
  const { conversation } = await request({
    method: "forkConversation",
    conversationId,
    messageId,
    place,
  });
  storeConversation(conversation);
  openConversation(conversation.id);
}

export async function renameConversation(id: string, title: string): Promise<void> {
  const { conversation } = await request({
    method: "renameConversation",
    id,
    title,
  });
  storeConversation(conversation);
}

export async function setPinned(id: string, pinned: boolean): Promise<void> {
  const { conversation } = await request({ method: "setPinned", id, pinned });
  storeConversation(conversation);
}

/** A message as the composer hands it over. */
export type Outgoing = {
  text: string;
  attachments: AttachmentRef[];
  /** What the message @-mentions: workers, files, other conversations, the page it was sent from. */
  mentions: Mention[];
};

/** Turns the composer's choices for a draft into the new conversation's setup. */
export type DraftTarget =
  | { kind: "chat"; setup: SetupRequest | null }
  | { kind: "session"; projectId: string; setup: SetupRequest | null };

/**
 * Where a message sent while a turn runs goes: the daemon decides (`auto`: a Chat queues it, a
 * session's orchestrator sorts it), the user steers it in (a queued message's Steer), or it
 * goes back into the queue (an edited queued message).
 */
export type SendLane = "auto" | "steer" | "queue";

/**
 * Sends a user message to `to`, else the selected conversation. A draft first becomes a real
 * chat or session with the composer's setup; the message shows immediately and is replaced by
 * the daemon's copy once committed. While a turn runs the daemon queues it; in a session, what
 * is sent while the newest answer works waits in the queue while the orchestrator sorts it (it
 * joins that answer or waits for its own turn). Only an explicit Steer goes straight in. `queueIndex` puts a message that
 * waits back in its old slot (a queued message pulled out to edit, or a deleted one restored).
 */
export async function send(
  outgoing: Outgoing,
  draft?: DraftTarget,
  {
    lane = "auto",
    queueIndex = null,
    to = null,
    onTarget,
  }: {
    lane?: SendLane;
    queueIndex?: number | null;
    /** The conversation it goes to, when not the selected one (a side chat's). */
    to?: string | null;
    /** Told the conversation it goes to once that is known (a draft's is made first). */
    onTarget?: (conversationId: string) => void;
  } = {},
): Promise<void> {
  const { selection } = useApp.getState();
  let conversationId: string;
  if (to) {
    conversationId = to;
  } else if (selection.type === "conversation") {
    conversationId = selection.id;
  } else if (selection.type === "draft" && draft) {
    const { conversation } = await request({
      method: "createConversation",
      kind: draft.kind,
      projectId: draft.kind === "session" ? draft.projectId : null,
      title: null,
      setup: draft.setup,
    });
    storeConversation(conversation);
    conversationId = conversation.id;
    updateThread(conversationId, () => emptyThread);
    // A new conversation has nothing to read: it shows at once, its events filling it.
    keepBoard({ ...emptyBoard(conversationId), loaded: true });
    // Only follow the new conversation if the user is still looking at the draft.
    if (useApp.getState().selection === selection) {
      useApp.setState({ draft: emptyDraft(null) });
      // The draft is gone: Back goes to where it was opened from.
      select({ type: "conversation", id: conversationId }, "replace");
    }
  } else {
    return;
  }
  onTarget?.(conversationId);

  const prepared = prepareTabChatSend(conversationId);
  if (prepared) await prepared;
  const board = boardOf(conversationId);
  // A send still under way, or one whose turn the CLI's thread doesn't show yet: the daemon is
  // busy with it, so this one waits too.
  const starting = board !== null && turnStarting(conversationId);
  const running = board !== null && (board.run === "running" || board.run === "starting");
  const target = useApp.getState().conversations[conversationId];
  const session = target?.kind === "session";
  // A session's message waits in the queue while its CLI's turn is open; a session's or a
  // Chat's (not a side chat's) while its work still holds with no turn running (its workers,
  // its CLI's background commands).
  const activity = realActivity(conversationId);
  const brigade = session || (target?.kind === "chat" && !target.sideOf);
  const working = board !== null && ((session && (!!activity?.turnOpen || starting)) || (brigade && workHolds(activity)));
  const steer = lane === "steer";
  const queue = board?.queue ?? null;
  // Asked to queue with no slot: last.
  const slot = steer ? null : (queueIndex ?? (lane === "queue" ? (queue?.items.length ?? 0) : null));
  const paused = slot !== null && !!queue?.paused;
  // A message that waits shows in the queue at once, not the thread; a steered one arrives
  // as an event.
  const waits = running || starting || working || paused;
  const queued = waits && !steer;
  const localId = waits && steer ? null : crypto.randomUUID();
  // Whatever a Stop just showed, what it sends now shows as it really is.
  dropTurnStops(conversationId);
  if (localId) {
    // The thread places the turn it starts once it shows, under this id (see `shownIdOf`).
    if (!queued) expectSentTurn(conversationId, localId);
    useApp.setState((state) => ({
      pending: [
        ...state.pending,
        {
          localId,
          conversationId,
          text: outgoing.text,
          attachments: outgoing.attachments,
          mentions: outgoing.mentions,
          createdAtMs: Date.now(),
          queuedIds: queue?.items.map((item) => item.id) ?? [],
          // The session or Chat judges what is sent while it works (run2-D40); a side chat's
          // waits plainly.
          ...(queued ? { queued: { index: slot, deciding: (working || running) && !paused && !target?.sideOf } } : {}),
        },
      ],
    }));
  }
  try {
    const { outcome } = await request({
      method: "sendMessage",
      conversationId,
      text: outgoing.text,
      attachments: outgoing.attachments,
      mentions: outgoing.mentions,
      steer,
      queueIndex: slot,
    });
    if (outcome.type === "sent") {
      if (localId && !queued) confirmSent(conversationId, outcome.message, localId);
      updateThread(conversationId, (thread) => ({
        ...thread,
        items: mergeMessages(thread.items, [outcome.message]),
      }));
    } else {
      // Reconcile before publishing the rail item, including when the response beats its event.
      // Its echo, once its queue event removed it, needs no guard in the others.
      let unseen = false;
      useApp.setState((state) => {
        const pending = state.pending.filter((entry) => entry.localId !== localId);
        if (localId && pending.length === state.pending.length) return state;
        unseen = !!localId;
        return {
          pending: pending.map((entry) =>
            entry.conversationId === conversationId && !entry.queuedIds?.includes(outcome.item.id)
              ? { ...entry, queuedIds: [...(entry.queuedIds ?? []), outcome.item.id] }
              : entry),
        };
      });
      // Shown before its queue event arrives, in place of its row. Its row still showing means
      // no event brought it yet, so none sent or deleted it either; without one, a queue that
      // changed meanwhile has newer events, which bring it anyway.
      updateBoard(conversationId, (current) => {
        if (!unseen && current.queue !== queue) return current;
        if (current.queue.items.some((entry) => entry.id === outcome.item.id)) return current;
        const items = [...current.queue.items];
        items.splice(Math.min(slot ?? items.length, items.length), 0, outcome.item);
        return { ...current, queue: { ...current.queue, items } };
      });
    }
  } finally {
    if (localId && useApp.getState().pending.some((entry) => entry.localId === localId)) {
      useApp.setState((state) => ({
        pending: state.pending.filter((entry) => entry.localId !== localId),
      }));
    }
  }
}

/** How long after the daemon took a message its turn may still be missing from the CLI's thread. */
const TURN_SHOWS_MS = 30_000;

/**
 * Whether a turn is starting that the CLI's thread doesn't show yet: a send to the conversation
 * is still under way, or the daemon took a message lately that no turn of the CLI's carried.
 * Its news can come seconds after the send's answer while the connection is busy.
 */
function turnStarting(conversationId: string): boolean {
  const { pending, threads } = useApp.getState();
  if (pending.some((entry) => entry.conversationId === conversationId)) return true;
  const carried = new Set(useCliThread.getState().views[conversationId]?.turns.flatMap((turn) => turn.requests) ?? []);
  const since = Date.now() - TURN_SHOWS_MS;
  return (threads[conversationId]?.items ?? []).some(
    (message) => message.role === "user" && message.createdAtMs >= since && !!message.requestId && !carried.has(message.requestId),
  );
}

/** Changes what can still change after creation: model, effort and permission level. */
export async function updateSetup(id: string, setup: Setup): Promise<void> {
  const { conversation } = await request({ method: "updateSetup", id, setup });
  storeConversation(conversation);
}

/**
 * "Continue on <other CLI>" (S38c): the session's CLI writes its hand-off note, then a new
 * session starts on `provider` (on `model`, else the first model the user's rules allow there)
 * and carries on; while a turn runs, once it ends.
 */
export async function continueOn(id: string, provider: ProviderKind, model: ModelChoice | null): Promise<ContinueWhen> {
  const { when } = await request({ method: "continueOn", id, provider, model });
  return when;
}

/**
 * Stores a file in the daemon's blob store, for attaching to a message. `pasted`: the file is
 * text the user pasted, which goes to the model as part of their message. `name`: the name to
 * store it under, the file's own by default.
 */
export async function addAttachment(file: File, pasted = false, name = file.name): Promise<AttachmentRef> {
  const data = await fileToBase64(file);
  const { attachment } = await request({
    method: "addAttachment",
    name,
    mime: file.type || "application/octet-stream",
    data,
    pasted,
  });
  return attachment;
}

/** A stored attachment's bytes, for previews. */
export async function readAttachment(ref: AttachmentRef): Promise<Blob> {
  const { data } = await request({ method: "readAttachment", id: ref.id });
  const binary = atob(data);
  return new Blob([Uint8Array.from(binary, (char) => char.charCodeAt(0))], { type: ref.mime });
}

function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.addEventListener("load", () => {
      const url = String(reader.result);
      resolve(url.slice(url.indexOf(",") + 1));
    });
    reader.addEventListener("error", () =>
      reject(reader.error ?? new Error(`could not read ${file.name}`)),
    );
    reader.readAsDataURL(file);
  });
}

// ----- message queue ---------------------------------------------------------------------

function showQueue(conversationId: string, queue: MessageQueue): void {
  updateBoard(conversationId, (board) => ({ ...board, queue }));
}

export async function editQueued(
  conversationId: string,
  itemId: string,
  outgoing: Outgoing,
): Promise<void> {
  const { queue } = await request({
    method: "editQueued",
    conversationId,
    itemId,
    text: outgoing.text,
    attachments: outgoing.attachments,
    mentions: outgoing.mentions,
  });
  showQueue(conversationId, queue);
}

export async function deleteQueued(conversationId: string, itemId: string): Promise<void> {
  const { queue } = await request({ method: "deleteQueued", conversationId, itemId });
  showQueue(conversationId, queue);
}

export async function moveQueued(
  conversationId: string,
  itemId: string,
  index: number,
): Promise<void> {
  // Show the new order right away; the daemon's answer (or its event) confirms it.
  updateBoard(conversationId, (board) => {
    const items = board.queue.items.filter((item) => item.id !== itemId);
    const moved = board.queue.items.find((item) => item.id === itemId);
    if (!moved) return board;
    items.splice(index, 0, moved);
    return { ...board, queue: { ...board.queue, items } };
  });
  try {
    const { queue } = await request({ method: "moveQueued", conversationId, itemId, index });
    showQueue(conversationId, queue);
  } catch (error) {
    await loadConversation(conversationId).catch(() => {});
    throw error;
  }
}

/** Sends a queued message into the running turn now. */
/**
 * Puts a deleted queued message back in its slot (the delete toast's Undo). If nothing runs
 * or waits any more, it is sent instead, as a queued message would have been.
 */
export async function restoreQueued(
  conversationId: string,
  item: QueuedMessage,
  index: number,
): Promise<void> {
  const { outcome } = await request({
    method: "sendMessage",
    conversationId,
    text: item.text,
    attachments: item.attachments,
    mentions: item.mentions,
    steer: false,
    queueIndex: index,
  });
  if (outcome.type === "sent") {
    updateThread(conversationId, (thread) => ({
      ...thread,
      items: mergeMessages(thread.items, [outcome.message]),
    }));
  } else {
    updateBoard(conversationId, (board) => {
      if (board.queue.items.some((entry) => entry.id === outcome.item.id)) return board;
      const items = [...board.queue.items];
      items.splice(index, 0, outcome.item);
      return { ...board, queue: { ...board.queue, items } };
    });
  }
}

/**
 * Moves a queued message to a new Side chat tab: it leaves the queue first, so the turn can't
 * also take it; one the queue already sent or dropped goes nowhere (`false`). The side chat is
 * made, the message sent there and its tab opened; if that fails, the message goes back in its
 * slot and the error is thrown.
 */
export async function openQueuedInSideChat(conversationId: string, item: QueuedMessage): Promise<boolean> {
  const index = Math.max(0, boardOf(conversationId)?.queue.items.findIndex((entry) => entry.id === item.id) ?? 0);
  try {
    await deleteQueued(conversationId, item.id);
  } catch {
    // Already gone: the turn took it, or it was deleted meanwhile.
    await loadConversation(conversationId).catch(() => {});
    return false;
  }
  try {
    const { conversation } = await request({ method: "openSideChat", conversationId, sideChatId: crypto.randomUUID() });
    storeConversation(conversation);
    await send({ text: item.text, attachments: item.attachments, mentions: item.mentions }, undefined, { to: conversation.id });
    openSideChatTab(conversationId, conversation.id);
    return true;
  } catch (error) {
    await restoreQueued(conversationId, item, index);
    throw error;
  }
}

/**
 * "Open as a new session" on a message the session offered as one: it leaves the queue first,
 * then starts a new background session of the same project on the session's own model,
 * permission and plan mode; the user stays where they are. If that fails, it goes back in its
 * slot and the error is thrown; one the queue already sent goes nowhere (`false`).
 */
export async function openQueuedAsSession(conversationId: string, item: QueuedMessage): Promise<boolean> {
  const source = useApp.getState().conversations[conversationId];
  if (!source || !isTaskTab(source)) return false;
  const index = Math.max(0, boardOf(conversationId)?.queue.items.findIndex((entry) => entry.id === item.id) ?? 0);
  try {
    await deleteQueued(conversationId, item.id);
  } catch {
    await loadConversation(conversationId).catch(() => {});
    return false;
  }
  try {
    await startSessionFrom(source, item);
    return true;
  } catch (error) {
    await restoreQueued(conversationId, item, index);
    throw error;
  }
}

/** A new session's setup: `source`'s model, permission and plan mode (it starts from `source`'s base). */
function sessionSetupFrom(source: Conversation): SetupRequest | null {
  return source.setup?.type === "session"
    ? {
      type: "session",
      repo: source.setup.repo,
      // The daemon starts it from the branch `source` started from.
      environment: { type: "newWorktree", base: "", branch: null },
      permission: source.setup.permission,
      orchestrator: source.setup.orchestrator,
      // Plan mode on in the session it came from holds the new one to a plan too.
      planMode: source.setup.planMode,
    }
    : null;
}

/**
 * Starts `outgoing` as a new background session of `source`'s project (⌘⏎, "Open as a new
 * session", "Start as session"), on its own worktree; the user stays where they are. Its task
 * mentions stay behind: they name `source`'s workers.
 */
export async function startSessionFrom(source: Conversation, outgoing: Outgoing): Promise<Conversation> {
  const { conversation } = await request({
    method: "startSession",
    from: source.id,
    setup: sessionSetupFrom(source),
    text: outgoing.text,
    attachments: outgoing.attachments,
    mentions: outgoing.mentions.filter((mention) => mention.type !== "task"),
  });
  storeConversation(conversation);
  return conversation;
}

export async function steerQueued(conversationId: string, itemId: string): Promise<void> {
  await request({ method: "steerQueued", conversationId, itemId });
}

/** "Run here" on a message the session offered as a new session: it waits like any other and is sent once nothing works. */
export async function runQueuedHere(conversationId: string, itemId: string): Promise<void> {
  const { queue } = await request({ method: "runQueuedHere", conversationId, itemId });
  showQueue(conversationId, queue);
}

/** Dismisses the notice above the composer made at `atMs`, for good (D51). */
export async function dismissNotice(conversationId: string, atMs: number): Promise<void> {
  await request({ method: "dismissNotice", conversationId, atMs });
}

/** Dismisses the notice of a queued message the session sent on to one of its workers. */
export async function dismissSent(conversationId: string, itemId: string): Promise<void> {
  const { queue } = await request({ method: "dismissSent", conversationId, itemId });
  showQueue(conversationId, queue);
}

export async function resumeQueue(conversationId: string): Promise<void> {
  const { queue } = await request({ method: "resumeQueue", conversationId });
  showQueue(conversationId, queue);
}

/** How long a stop the daemon made may show before its real state does, at most. */
const STOP_SETTLE_MS = 10_000;

/**
 * Shows `stop` at once while `run` makes it (#498): the daemon ends a turn or a worker only once
 * its process has. A failed stop shows the real state again and throws. One made shows until
 * the real state says it too, `STOP_SETTLE_MS` at most; `stopped`, the workers it stopped.
 */
async function showStopped<T>(
  stop: Omit<Stop, "id" | "atMs">,
  run: () => Promise<T>,
  stopped?: (result: T) => readonly string[],
): Promise<T> {
  const shown = addStop(stop);
  let result: T;
  try {
    result = await run();
  } catch (error) {
    dropStop(shown.id);
    throw error;
  }
  if (stopped) keepStopping(shown.id, stopped(result));
  const settled = useStopping.getState().stops.find((each) => each.id === shown.id);
  if (settled) settleStop(settled);
  return result;
}

/** Whether `stop` shows in the real state: its workers over, its turn and answers ended. */
function stopShows(stop: Stop): boolean {
  const board = boardOf(stop.conversationId);
  const activity = realActivity(stop.conversationId);
  const over = stop.tasks.every((id) => {
    const task = board?.tasks[id];
    return task ? workerDone(task) : !(id in (activity?.tasks ?? {}));
  });
  if (!over || !stop.turn) return over;
  const run = board?.run ?? activity?.run ?? "idle";
  const answering = Object.values(board?.requests ?? {}).some((each) => each.state.type === "working");
  return run !== "running" && run !== "starting" && !answering;
}

function settleStop(stop: Stop): void {
  if (stopShows(stop)) {
    dropStop(stop.id);
    return;
  }
  const check = () => {
    if (stopShows(stop)) done();
  };
  const offActivity = onRealActivity(check);
  const offBoard = useBoard.subscribe(check);
  const timer = setTimeout(done, STOP_SETTLE_MS);
  function done() {
    offActivity();
    offBoard();
    clearTimeout(timer);
    dropStop(stop.id);
  }
}

/** Stops the running turn; the queue pauses until resumed. It shows stopped at once. */
export async function interrupt(conversationId: string): Promise<void> {
  const tasks = useBoard.getState().board;
  const ids = tasks?.conversationId === conversationId
    ? Object.values(tasks.tasks).filter((task) => ["queued", "starting", "running", "blocked", "paused", "landing", "readyToLand"].includes(task.state)).map((task) => task.id)
    : [];
  await showStopped({ conversationId, turn: true, tasks: ids }, async () => {
    // The turn first: the workers' stop then reaches the thread before its next turn, not
    // inside the one ending.
    const [interrupted] = await Promise.allSettled([request({ method: "interrupt", conversationId })]);
    const results = [interrupted, ...await Promise.allSettled(ids.map(stopTask))];
    const failed = results.find((result) => result.status === "rejected");
    if (failed?.status === "rejected") throw failed.reason;
  });
}

/** Stops a preview the thread started, or every running one of the conversation. */
export async function stopPreview(conversationId: string, previewId: string | null): Promise<void> {
  await request({ method: "stopPreview", conversationId, previewId });
}

/** Stops one of the thread's CLI's background tasks, as the CLI's own Stop does. */
/** Stops a background task of the session's CLI, or of its worker `workerId`'s. */
export async function stopBackgroundTask(conversationId: string, taskId: string, workerId?: string): Promise<void> {
  await request({ method: "stopBackgroundTask", conversationId, taskId, ...(workerId !== undefined && { workerId }) });
}

export async function pausePreview(conversationId: string, previewId: string): Promise<void> {
  await request({ method: "pausePreview", conversationId, previewId });
}

export async function resumePreview(conversationId: string, previewId: string): Promise<void> {
  await request({ method: "resumePreview", conversationId, previewId });
}

/** Runs an ended preview's own command again from its record (Start again, Run again). */
export async function restartPreview(conversationId: string, previewId: string): Promise<void> {
  await request({ method: "restartPreview", conversationId, previewId });
}

export async function previewLogTail(conversationId: string, previewId: string): Promise<{ tail: string; url: string | null }> {
  const { tail, url } = await request({ method: "previewLog", conversationId, previewId, lines: 80 });
  return { tail, url };
}

/** Continues the latest request after the user stopped it, in the same block. */
export async function resume(conversationId: string): Promise<void> {
  await request({ method: "resume", conversationId });
}

/** What `/status` shows: the model's CLI session and the usage left. */
export async function getConversationStatus(conversationId: string): Promise<ConversationStatus> {
  const { status } = await request({ method: "getConversationStatus", conversationId });
  return status;
}

/** Compacts a Chat's context now, in a turn of its own, the same as a `/compact` command. */
export async function compact(conversationId: string): Promise<void> {
  await request({ method: "compact", conversationId });
}

/** Replaces a sent message: the edit starts a branch beside it and gets its own answer. */
export async function editMessage(
  conversationId: string,
  messageId: string,
  text: string,
  attachments?: AttachmentRef[],
): Promise<void> {
  await request({ method: "editMessage", conversationId, messageId, text, ...(attachments ? { attachments } : {}) });
}

/** Answers a request again, from its user message. */
export async function regenerate(conversationId: string, requestId: string): Promise<void> {
  await request({ method: "regenerate", conversationId, requestId });
}

/** Shows the branch of a Chat that ends at `head`. */
export async function switchBranch(conversationId: string, head: string): Promise<void> {
  await request({ method: "switchBranch", conversationId, head });
}

// ----- cards and workers -----------------------------------------------------------------

export async function answerCard(
  conversationId: string,
  cardId: string,
  decision: ApprovalDecision,
): Promise<void> {
  await request({ method: "answerCard", conversationId, cardId, decision });
}

/** Answers a question card: one answer per question of its round. */
export async function answerQuestion(
  conversationId: string,
  cardId: string,
  answers: string[],
  input: AnswerInput,
): Promise<void> {
  await request({ method: "answerQuestion", conversationId, cardId, answers, input });
}

/** "Close workspace": removes the session's own worktree; says what went and what stayed. */
export async function closeWorkspace(conversationId: string): Promise<string> {
  const { text } = await request({ method: "closeWorkspace", conversationId });
  return text;
}

/**
 * The user's answer to the thread's feedback offer: opened on GitHub (`sent`, with what became of
 * its reports, kept for the card after a restart) or dismissed.
 */
export async function answerFeedbackOffer(conversationId: string, offerId: string, sent: boolean, results?: FeedbackSent[]): Promise<void> {
  await request({ method: "answerFeedbackOffer", conversationId, offerId, sent, ...(results && { results }) });
}

/** The user's Stop for one worker: the thread hears the user stopped it. */
export async function stopTask(taskId: string): Promise<void> {
  await request({ method: "stopTask", taskId });
}

/** "Continue now" on a session waiting for a limit: its waiting message goes now (MA5). */
export async function continueWaiting(conversationId: string): Promise<void> {
  await request({ method: "continueWaiting", conversationId });
}

/** "Don't resume": nothing goes on its own; the session is idle until the user sends (MA5). */
export async function dontResume(conversationId: string): Promise<void> {
  await request({ method: "dontResume", conversationId });
}

/**
 * The Workers strip's Stop all: stops every running or waiting worker of the conversation; the
 * ids stopped. They show stopped at once. The CLI's own background agents are the Subagents
 * strip's, with a Stop all of its own: they go on.
 */
export async function stopWorkers(conversationId: string): Promise<string[]> {
  const ids = Object.values(boardOf(conversationId)?.tasks ?? {}).filter(workerStoppable).map((task) => task.id);
  return showStopped(
    { conversationId, turn: false, tasks: ids },
    async () => (await request({ method: "stopWorkers", conversationId })).taskIds,
    (taskIds) => taskIds,
  );
}

/** The user's notes on a mockup, with the marked screenshots, to the mockup's worker. */
export async function sendMockupNotes(taskId: string, text: string, attachments: AttachmentRef[]): Promise<void> {
  await request({ method: "sendMockupNotes", taskId, text, attachments });
}

/**
 * The user's one message straight to one of the session's own workers (the composer's To:).
 * Resolves once the worker took it (`queued`: into its running turn); rejects when it didn't.
 */
export async function sendToWorker(
  conversationId: string,
  taskId: string,
  message: { text: string; attachments: AttachmentRef[]; mentions: Mention[] },
): Promise<{ queued: boolean }> {
  const { queued } = await request({ method: "sendToWorker", conversationId, taskId, ...message });
  return { queued };
}

export async function pauseTask(taskId: string): Promise<void> {
  await request({ method: "pauseTask", taskId });
}

export async function resumeTask(taskId: string): Promise<void> {
  await request({ method: "resumeTask", taskId });
}

/** Restores a task's saved patch as a new branch on its target branch. */
export async function restoreKeptWork(taskId: string): Promise<RestoreOutcome> {
  const { outcome } = await request({ method: "restoreKeptWork", taskId });
  return outcome;
}

export async function readArtifact(id: string, offset: number, limit: number) {
  const { text } = await request({ method: "readArtifact", id, offset, limit });
  return text;
}

/** A stored image (a mockup's screenshot) as a blob, for an `<img>`. */
export async function readStoredImage(hash: string): Promise<Blob> {
  const { data } = await request({ method: "readAttachment", id: hash });
  const binary = atob(data);
  return new Blob([Uint8Array.from(binary, (char) => char.charCodeAt(0))], { type: "image/png" });
}

// ----- lifecycle -------------------------------------------------------------------------

export async function hibernate(id: string): Promise<void> {
  const { conversation } = await request({ method: "hibernate", id });
  storeConversation(conversation);
}

/** "session", "chat", or "conversation" for a mix, made plural for `count` other than one. */
export function conversationNoun(conversations: Conversation[], count = conversations.length) {
  const kinds = new Set(conversations.map((conversation) => conversation.kind));
  const noun = kinds.size !== 1 ? "conversation" : kinds.has("chat") ? "chat" : "session";
  return count === 1 ? noun : `${noun}s`;
}

/** "“A”", "“A” and “B”", or "3 sessions". */
function naming(conversations: Conversation[]): string {
  if (conversations.length === 1) return `“${conversations[0]?.title}”`;
  if (conversations.length === 2) return conversations.map((c) => `“${c.title}”`).join(" and ");
  return `${conversations.length} ${conversationNoun(conversations)}`;
}

/** The errors of a batch's failures, in one plain sentence each way they failed. */
function failureText(verb: string, failed: { conversation: Conversation; error: string }[]) {
  const reasons = [...new Set(failed.map((entry) => entry.error))];
  return `Couldn't ${verb} ${naming(failed.map((entry) => entry.conversation))}: ${reasons.join("; ")}`;
}

/** "Archived session", "Archived 3 chats": the noun only, never the titles. */
function archivedText(list: Conversation[]): string {
  const noun = conversationNoun(list);
  return list.length === 1 ? `Archived ${noun}` : `Archived ${list.length} ${noun}`;
}

/** What each archive toast's Undo brings back, by toast. */
const archiveUndos = new Map<number, () => string[]>();

/** The newest archive toast, for ⌘Z: when it was shown and what had focus then. */
let lastArchive: { toast: number; atMs: number; focus: Element | null } | null = null;

function toastShows(id: number): boolean {
  return useToasts.getState().toasts.some((entry) => entry.id === id);
}

/**
 * Archives conversations and confirms it with an "Archived session · View · Undo" toast
 * ("Archived 3 sessions" for several). The rows leave at once; the daemon stops each and cleans
 * up in the background. Those whose archive fails come back, with one toast that says why; Undo
 * brings back only those archived.
 */
export async function archiveAll(ids: string[]): Promise<void> {
  const { conversations } = useApp.getState();
  const before = ids
    .map((id) => conversations[id])
    .filter((c): c is Conversation => c !== undefined && c.lifecycle !== "archived");
  if (before.length === 0) return;
  for (const conversation of before) storeConversation({ ...conversation, lifecycle: "archived" });
  const { selection } = useApp.getState();
  if (selection.type === "conversation" && before.some((c) => c.id === selection.id)) {
    select({ type: "draft", kind: "chat" }, "replace");
  }
  let archived = before;
  const confirm = (list: Conversation[]) => {
    const id = toast(archivedText(list), {
      actions: [
        { label: "View", run: () => openSettings("archived") },
        { label: "Undo", run: () => void undoArchive(id) },
      ],
    });
    for (const old of archiveUndos.keys()) if (!toastShows(old)) archiveUndos.delete(old);
    archiveUndos.set(id, () => archived.map((c) => c.id));
    lastArchive = { toast: id, atMs: performance.now(), focus: document.activeElement };
    return id;
  };
  let shown = confirm(before);
  const failed: { conversation: Conversation; error: string }[] = [];
  try {
    const { outcomes } = await request({ method: "archive", ids: before.map((c) => c.id) });
    for (const outcome of outcomes) {
      const conversation = before.find((c) => c.id === outcome.id);
      if (!conversation) continue;
      if (!outcome.conversation) failed.push({ conversation, error: outcome.error ?? "it failed" });
      // Not when it was deleted meanwhile.
      else if (useApp.getState().conversations[outcome.id]) storeConversation(outcome.conversation);
    }
  } catch (error) {
    for (const conversation of before) failed.push({ conversation, error: errorMessage(error) });
  }
  if (failed.length === 0) return;
  // Back as they were, unless something newer came in meanwhile.
  for (const { conversation } of failed) {
    if (useApp.getState().conversations[conversation.id]?.lifecycle === "archived") {
      storeConversation(conversation);
    }
  }
  archived = before.filter((c) => !failed.some((entry) => entry.conversation.id === c.id));
  const undoable = archiveUndos.delete(shown);
  dismissToast(shown);
  if (lastArchive?.toast === shown) lastArchive = null;
  if (archived.length > 0 && undoable) shown = confirm(archived);
  toast(failureText("archive", failed), { tone: "error" });
}

/** Archives one conversation (see {@link archiveAll}). */
export function archive(id: string): Promise<void> {
  return archiveAll([id]);
}

/**
 * The newest archive while its toast shows (what ⌘Z undoes): when it was shown and what had
 * focus then.
 */
export function lastArchiveShown(): { atMs: number; focus: Element | null } | null {
  return lastArchive && toastShows(lastArchive.toast) ? lastArchive : null;
}

/** ⌘Z right after an archive: its Undo, while its toast shows. Whether there was one. */
export function undoLastArchive(): boolean {
  if (!lastArchive || !lastArchiveShown()) return false;
  void undoArchive(lastArchive.toast);
  return true;
}

/** An archive toast's Undo: the archived rows are back at once (one opens again). */
async function undoArchive(toastId: number): Promise<void> {
  const ids = archiveUndos.get(toastId)?.();
  archiveUndos.delete(toastId);
  dismissToast(toastId);
  if (lastArchive?.toast === toastId) lastArchive = null;
  if (!ids) return;
  await restoreAll(ids, { open: ids.length === 1 });
}

/**
 * Brings archived conversations back. Their rows are back at once; those that fail leave again,
 * with one toast that says why. With `open`, the first one opens.
 */
export async function restoreAll(ids: string[], { open = false } = {}): Promise<void> {
  const { conversations } = useApp.getState();
  const before = ids
    .map((id) => conversations[id])
    .filter((c): c is Conversation => c !== undefined && c.lifecycle === "archived");
  if (before.length === 0) return;
  for (const conversation of before) storeConversation({ ...conversation, lifecycle: "active" });
  const first = before[0];
  if (open && first) openConversation(first.id);
  const failed: { conversation: Conversation; error: string }[] = [];
  try {
    const { outcomes } = await request({ method: "restore", ids: before.map((c) => c.id) });
    for (const outcome of outcomes) {
      const conversation = before.find((c) => c.id === outcome.id);
      if (!conversation) continue;
      if (!outcome.conversation) failed.push({ conversation, error: outcome.error ?? "it failed" });
      // Not when it was deleted meanwhile.
      else if (useApp.getState().conversations[outcome.id]) storeConversation(outcome.conversation);
    }
  } catch (error) {
    for (const conversation of before) failed.push({ conversation, error: errorMessage(error) });
  }
  if (failed.length === 0) return;
  for (const { conversation } of failed) {
    if (useApp.getState().conversations[conversation.id]?.lifecycle === "active") {
      storeConversation(conversation);
    }
  }
  toast(failureText("bring back", failed), { tone: "error" });
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Brings an archived conversation back: it is back at once (#499); if it can't be, it is
 * archived again and this throws.
 */
export async function restore(id: string): Promise<void> {
  const before = useApp.getState().conversations[id];
  if (before?.lifecycle === "archived") storeConversation({ ...before, lifecycle: "active" });
  let outcome: LifecycleOutcome | undefined;
  try {
    [outcome] = (await request({ method: "restore", ids: [id] })).outcomes;
  } catch (error) {
    unrestore(before);
    throw error;
  }
  // Not when it was deleted meanwhile.
  if (outcome?.conversation) {
    if (useApp.getState().conversations[id]) storeConversation(outcome.conversation);
    return;
  }
  unrestore(before);
  throw new Error(outcome?.error ?? "It couldn't be brought back.");
}

/** An archived conversation whose restore failed is archived again, unless something newer came in. */
function unrestore(before: Conversation | undefined): void {
  if (before?.lifecycle === "archived" && useApp.getState().conversations[before.id]?.lifecycle === "active") {
    storeConversation(before);
  }
}

/** Takes conversations out of the app's state (they are being deleted). */
function dropConversations(ids: string[]): void {
  const { selection } = useApp.getState();
  if (selection.type === "conversation" && ids.includes(selection.id)) {
    select({ type: "draft", kind: "chat" }, "replace");
  }
  useApp.setState((state) => {
    const conversations = { ...state.conversations };
    const threads = { ...state.threads };
    for (const id of ids) {
      delete conversations[id];
      delete threads[id];
    }
    return { conversations, threads };
  });
}

/**
 * Deletes conversations for good. The rows leave at once; the daemon cleans up in the
 * background. Those whose delete fails come back, with one toast that says why.
 */
export async function deleteAll(ids: string[]): Promise<void> {
  const { conversations } = useApp.getState();
  const before = ids
    .map((id) => conversations[id])
    .filter((c): c is Conversation => c !== undefined);
  if (before.length === 0) return;
  dropConversations(before.map((c) => c.id));
  // An archive toast whose rows all go now has nothing left to undo.
  for (const [toastId, undone] of archiveUndos) {
    if (undone().every((id) => before.some((c) => c.id === id))) {
      archiveUndos.delete(toastId);
      dismissToast(toastId);
      if (lastArchive?.toast === toastId) lastArchive = null;
    }
  }
  const failed: { conversation: Conversation; error: string }[] = [];
  try {
    const { outcomes } = await request({ method: "delete", ids: before.map((c) => c.id) });
    for (const outcome of outcomes) {
      const conversation = before.find((c) => c.id === outcome.id);
      if (conversation && outcome.error !== null) failed.push({ conversation, error: outcome.error });
    }
  } catch (error) {
    for (const conversation of before) failed.push({ conversation, error: errorMessage(error) });
  }
  if (failed.length > 0) {
    for (const { conversation } of failed) {
      if (!useApp.getState().conversations[conversation.id]) storeConversation(conversation);
    }
    toast(failureText("delete", failed), { tone: "error" });
  }
  const deleted = before.filter((c) => !failed.some((entry) => entry.conversation.id === c.id));
  // Their unsent drafts go only once they are surely going.
  for (const { id } of deleted) forgetDraft(id);
  forgetHeldDirect(deleted.map((conversation) => conversation.id));
  forgetRightSidebarTabs(deleted.map((conversation) => conversation.id));
}

/**
 * What deleting `ids` takes that the user may not expect: the branches whose work never landed
 * (or may not have), and the files a Local checkout session left uncommitted in the checkout.
 */
export async function previewDelete(
  ids: string[],
): Promise<{ branches: UnlandedBranch[]; files: string[] }> {
  const { branches, files } = await request({ method: "previewDelete", ids });
  return { branches, files };
}

/** What removing a project takes with it. */
export async function previewRemoveProject(id: string): Promise<ProjectRemoval> {
  return (await request({ method: "previewRemoveProject", id })).removal;
}

/**
 * Removes a project from Brigadier: its conversations are deleted, the picked branches go,
 * and its Brain and code index go to the Trash unless kept; its repository is untouched.
 */
export async function removeProject(
  id: string,
  deleteBranches: BranchChoice[],
  keepBrain: boolean,
): Promise<RemoveProjectReport> {
  const gone = Object.values(useApp.getState().conversations)
    .filter((conversation) => conversation.projectId === id)
    .map((conversation) => conversation.id);
  const { report } = await request({ method: "removeProject", id, deleteBranches, keepBrain });
  for (const conversation of gone) forgetDraft(conversation);
  forgetHeldDirect(gone);
  forgetRightSidebarTabs(gone);
  const { selection } = useApp.getState();
  if (
    (selection.type === "conversation" && gone.includes(selection.id)) ||
    (selection.type === "draft" && selection.kind === "session" && selection.projectId === id)
  ) {
    select({ type: "draft", kind: "chat" }, "replace");
  }
  useApp.setState((state) => {
    const { [id]: _removed, ...projects } = state.projects;
    const conversations = { ...state.conversations };
    const threads = { ...state.threads };
    for (const conversation of gone) {
      delete conversations[conversation];
      delete threads[conversation];
    }
    return { projects, conversations, threads };
  });
  return report;
}

// ----- settings --------------------------------------------------------------------------

/** Shows or hides a conversation's summary card, the one in front by default, until changed. */
export function setPinnedSummary(shown: boolean, key = summaryKey(useApp.getState().selection)): void {
  const summaryShown = { ...useApp.getState().summaryShown, [key]: shown };
  saveSummaryShown(summaryShown);
  useApp.setState({ summaryShown });
}

/** Opens the Inspector's page in Settings, on `tab` when given. */
export function openInspector(tab?: InspectorTab): void {
  if (tab) setInspectorTab(tab);
  openSettings("inspector");
}

/** ⌥⌘I : opens the Inspector, or leaves it when it is the page shown. */
export function toggleInspector(): void {
  const { selection } = useApp.getState();
  if (selection.type === "settings" && selection.page === "inspector") closeSettings();
  else openInspector();
}

export function setInspectorTab(tab: InspectorTab): void {
  useApp.setState((state) => ({ inspector: { ...state.inspector, tab } }));
}

export async function refreshDiagnostics(): Promise<void> {
  const { diagnostics } = await request({ method: "getDiagnostics" });
  useApp.setState((state) => ({
    inspector: {
      ...state.inspector,
      diagnostics,
      metrics: state.inspector.metrics ?? diagnostics.metrics,
    },
  }));
}

export async function setMetricsStreaming(enabled: boolean): Promise<void> {
  await request({ method: "setMetricsStreaming", enabled });
}

export async function runProbeBurst(count = 200, intervalMs = 5) {
  const { burst } = await request({ method: "probeBurst", count, intervalMs });
  return burst;
}

// ----- providers and raw sessions (Inspector) -------------------------------------------

/** Transcript entries fetched per page. */
const RAW_PAGE = 500;

function updateProviders(update: (providers: ProvidersState) => ProvidersState): void {
  useApp.setState((state) => ({ providers: update(state.providers) }));
}

function updateTranscript(id: string, update: (transcript: RawTranscript) => RawTranscript): void {
  updateProviders((providers) => ({
    ...providers,
    transcripts: {
      ...providers.transcripts,
      [id]: update(providers.transcripts[id] ?? { entries: [], hasMore: false, loading: false }),
    },
  }));
}

/** Shows a session returned by a request right away (its event may still be in flight). */
function showRawSession(session: RawSession): void {
  updateProviders((providers) => upsertRawSession(providers, session));
  selectRawSession(session.id);
}

export async function loadProviders(): Promise<void> {
  const { view } = await request({ method: "getProviders" });
  updateProviders((providers) => ({ ...providers, view }));
}

/** Checks every provider again (or only `provider`); results arrive as events. */
export async function refreshProviders(provider: ProviderKind | null = null): Promise<void> {
  await request({ method: "refreshProviders", provider });
}

/** Opens a raw session in the Providers tab (or goes back to the list with `null`). */
export function selectRawSession(id: string | null): void {
  updateProviders((providers) => ({ ...providers, selected: id }));
  if (id !== null && !useApp.getState().providers.transcripts[id]) {
    void loadRawTranscript(id);
  }
}

/** Loads the newest page of a transcript, keeping live entries that arrived meanwhile. */
export async function loadRawTranscript(id: string): Promise<void> {
  updateTranscript(id, (transcript) => ({ ...transcript, loading: true }));
  try {
    const { page } = await request({ method: "listRawEvents", id, before: null, limit: RAW_PAGE });
    const newest = page.entries.at(-1)?.streamSeq ?? 0;
    updateTranscript(id, (transcript) => ({
      entries: [
        ...page.entries,
        ...transcript.entries.filter((entry) => entry.streamSeq > newest),
      ],
      hasMore: page.hasMore,
      loading: false,
    }));
  } catch (error) {
    updateTranscript(id, (transcript) => ({ ...transcript, loading: false }));
    throw error;
  }
}

export async function loadEarlierRawEntries(id: string): Promise<void> {
  const transcript = useApp.getState().providers.transcripts[id];
  const oldest = transcript?.entries[0];
  if (!transcript || !oldest || transcript.loading) return;
  updateTranscript(id, (current) => ({ ...current, loading: true }));
  try {
    const { page } = await request({
      method: "listRawEvents",
      id,
      before: oldest.streamSeq,
      limit: RAW_PAGE,
    });
    updateTranscript(id, (current) => ({
      entries: [...page.entries, ...current.entries],
      hasMore: page.hasMore,
      loading: false,
    }));
  } catch (error) {
    updateTranscript(id, (current) => ({ ...current, loading: false }));
    throw error;
  }
}

export type StartRawSession = {
  provider: ProviderKind;
  cwd: string;
  model: string | null;
  effort: string | null;
  access: Access;
  approvals: RawApprovals;
  record: boolean;
};

export async function startRawSession(start: StartRawSession): Promise<void> {
  const { session } = await request({ method: "startRawSession", ...start });
  showRawSession(session);
}

export async function resumeRawSession(id: string): Promise<void> {
  const { session } = await request({ method: "resumeRawSession", id });
  updateProviders((providers) => upsertRawSession(providers, session));
}

export async function forkRawSession(id: string): Promise<void> {
  const { session } = await request({ method: "forkRawSession", id });
  showRawSession(session);
}

export async function sendRawSession(id: string, text: string, steer: boolean): Promise<void> {
  await request({ method: "sendRawSession", id, text, steer });
}

export async function interruptRawSession(id: string): Promise<void> {
  await request({ method: "interruptRawSession", id });
}

export async function answerApproval(
  id: string,
  approvalId: string,
  decision: ApprovalDecision,
): Promise<void> {
  await request({ method: "answerApproval", id, approvalId, decision });
}

export async function stopRawSession(id: string): Promise<void> {
  await request({ method: "stopRawSession", id });
}

export async function closeRawSession(id: string): Promise<void> {
  const { session } = await request({ method: "closeRawSession", id });
  updateProviders((providers) => upsertRawSession(providers, session));
}

export async function replayFixture(fixtureId: string): Promise<void> {
  const { session } = await request({ method: "replayFixture", fixtureId });
  showRawSession(session);
}

export async function simulateUsageLimit(provider: ProviderKind): Promise<void> {
  const { session } = await request({ method: "simulateUsageLimit", provider });
  showRawSession(session);
}
