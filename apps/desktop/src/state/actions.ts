import { request } from "@/ipc/client";
import type {
  Access,
  ApprovalDecision,
  AttachmentRef,
  BranchChoice,
  Conversation,
  ConversationStatus,
  Density,
  DiffStat,
  ForkPlace,
  Mention,
  MessageQueue,
  OvernightRun,
  ProposedPlan,
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
} from "@/ipc/generated";
import { savePinnedSummary } from "@/lib/pinnedSummary";
import { setSetting } from "@/state/settings";
import {
  boardFromView,
  boardOf,
  collectBoardEvents,
  emptyBoard,
  ORCHESTRATOR_ENTRIES,
  updateBoard,
  useBoard,
  WORKER_ENTRIES,
  workingRequest,
  type WorkerTranscript,
} from "@/state/board";
import {
  confirmSent,
  emptyDraft,
  emptyThread,
  mergeMessages,
  replaceCatalog,
  type InspectorTab,
  type ProvidersState,
  type RawTranscript,
  type Selection,
  type SettingsPageId,
  type Thread,
  upsertRawSession,
  useApp,
} from "@/state/store";
import { forgetDraft } from "@/state/drafts";
import { dismissToast, toast } from "@/state/toasts";

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

export async function loadCatalog(): Promise<void> {
  const { catalog } = await request({ method: "getCatalog" });
  replaceCatalog(catalog.projects, catalog.conversations, catalog.settings);
}

/**
 * Loads a conversation's view (newest messages, tasks, cards, queue, run state), replacing
 * what was loaded. Live events that arrived meanwhile are kept.
 */
export async function loadConversation(id: string): Promise<void> {
  updateThread(id, (thread) => ({ ...thread, loading: true }));
  const before = useApp.getState().conversations[id];
  const arrived = collectBoardEvents(id);
  try {
    const { view } = await request({ method: "getConversation", id, limit: PAGE });
    const events = arrived();
    const page = view.messages;
    const newest = page.messages.at(-1)?.seq ?? 0;
    updateThread(id, (thread) => ({
      ...thread,
      // Keep only what the live feed delivered after the page was read. Older items may be
      // stale (e.g. after missing events) and would leave a gap below this page.
      items: mergeMessages(
        page.messages,
        thread.items.filter((message) => message.seq > newest),
      ),
      hasMore: page.hasMore,
      loading: false,
    }));
    // A copy live events changed while the view was read is newer (a new conversation is
    // renamed by its first message), and one they removed was deleted; whatever else the
    // view holds arrives as events too.
    useApp.setState((state) => {
      const live = state.conversations[id];
      return live !== before
        ? state
        : { conversations: { ...state.conversations, [id]: view.conversation } };
    });
    updateBoard(id, (board) => boardFromView(view, board, events));
  } catch (error) {
    arrived();
    updateThread(id, (thread) => ({ ...thread, loading: false }));
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

/** Blob reads in flight, by message id, so each full text is requested once. */
const fullTextLoading = new Set<string>();

/** Fetches the full text of a message whose body lives in the blob store. */
export async function loadFullText(
  conversationId: string,
  messageId: string,
  hash: string,
): Promise<void> {
  if (fullTextLoading.has(messageId)) return;
  fullTextLoading.add(messageId);
  try {
    const { text } = await request({ method: "readBlobText", hash });
    updateThread(conversationId, (thread) => ({
      ...thread,
      fullText: { ...thread.fullText, [messageId]: text },
    }));
  } finally {
    fullTextLoading.delete(messageId);
  }
}

export function select(selection: Selection): void {
  useApp.setState({ selection });
  const { board } = useBoard.getState();
  if (selection.type !== "conversation") {
    if (board) useBoard.setState({ board: null });
    return;
  }
  if (board?.conversationId !== selection.id) {
    useBoard.setState({ board: emptyBoard(selection.id) });
  }
  void loadConversation(selection.id).catch((error: unknown) => {
    console.error("loading the conversation failed", error);
  });
}

/** Where leaving Settings goes back to: what was shown when it opened. */
let beforeSettings: Selection = { type: "draft", kind: "chat" };

/** What was shown when Settings opened (what the Inspector's Orchestrator tab starts on). */
export function shownBeforeSettings(): Selection {
  return beforeSettings;
}

/** Shows a page of Settings, remembering what to go back to. */
export function openSettings(page: SettingsPageId = "general"): void {
  const { selection } = useApp.getState();
  if (selection.type !== "settings") beforeSettings = selection;
  select({ type: "settings", page });
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

export function openConversation(id: string): void {
  const conversation = useApp.getState().conversations[id];
  if (conversation?.projectId) setProjectExpanded(conversation.projectId, true);
  select({ type: "conversation", id });
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

/** Rates an answer: a message id, or `task:<id>` for a worker's report. */
export async function rateMessage(
  conversationId: string,
  subject: string,
  rating: Rating,
): Promise<void> {
  await request({ method: "rateMessage", conversationId, subject, rating });
}

/** What a worktree session's branch changed against its base; null for other conversations. */
export async function getSessionDiff(id: string): Promise<DiffStat | null> {
  const { stat } = await request({ method: "getSessionDiff", id });
  return stat;
}

/** Reads what each worker at work on a change changed so far into the conversation's board. */
export async function refreshWorkerDiffs(conversationId: string): Promise<void> {
  const { diffs } = await request({ method: "getWorkerDiffs", conversationId });
  updateBoard(conversationId, (board) => ({
    ...board,
    diffs: Object.fromEntries(diffs.map((diff) => [diff.taskId, diff.stat])),
  }));
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
  /** What the message @-mentions: workers, files, other conversations. */
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
  }: {
    lane?: SendLane;
    queueIndex?: number | null;
    /** The conversation it goes to, when not the selected one (a side chat's). */
    to?: string | null;
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
    // Only follow the new conversation if the user is still looking at the draft.
    if (useApp.getState().selection === selection) {
      useApp.setState({ draft: emptyDraft(null) });
      select({ type: "conversation", id: conversationId });
    }
  } else {
    return;
  }

  const board = boardOf(conversationId);
  const running = board !== null && (board.run === "running" || board.run === "starting");
  const session = useApp.getState().conversations[conversationId]?.kind === "session";
  const working = session && board !== null && workingRequest(board) !== null;
  const steer = lane === "steer";
  const queue = board?.queue ?? null;
  // Asked to queue with no slot: last.
  const slot = steer ? null : (queueIndex ?? (lane === "queue" ? (queue?.items.length ?? 0) : null));
  // A message that waits shows in the queue, not the thread; a steered one arrives as an event.
  const waits = running || working || (slot !== null && !!queue?.paused);
  const localId = waits ? null : crypto.randomUUID();
  if (localId) {
    useApp.setState((state) => ({
      pending: [
        ...state.pending,
        {
          localId,
          conversationId,
          text: outgoing.text,
          attachments: outgoing.attachments,
          createdAtMs: Date.now(),
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
      if (localId) confirmSent(conversationId, outcome.message, localId);
      updateThread(conversationId, (thread) => ({
        ...thread,
        items: mergeMessages(thread.items, [outcome.message]),
      }));
    } else {
      // Shown before its queue event arrives. If the queue changed meanwhile, the events are
      // newer than this answer (the item may already be sent or deleted) and bring it anyway.
      updateBoard(conversationId, (current) => {
        if (current.queue !== queue) return current;
        const items = [...current.queue.items];
        items.splice(slot ?? items.length, 0, outcome.item);
        return { ...current, queue: { ...current.queue, items } };
      });
    }
  } finally {
    if (localId) {
      useApp.setState((state) => ({
        pending: state.pending.filter((entry) => entry.localId !== localId),
      }));
    }
  }
}

/** Changes what can still change after creation: model, effort and permission level. */
export async function updateSetup(id: string, setup: Setup): Promise<void> {
  const { conversation } = await request({ method: "updateSetup", id, setup });
  storeConversation(conversation);
}

/**
 * Stores a file in the daemon's blob store, for attaching to a message. `pasted`: the file is
 * text the user pasted, which goes to the model as part of their message.
 */
export async function addAttachment(file: File, pasted = false): Promise<AttachmentRef> {
  const data = await fileToBase64(file);
  const { attachment } = await request({
    method: "addAttachment",
    name: file.name,
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

export async function steerQueued(conversationId: string, itemId: string): Promise<void> {
  await request({ method: "steerQueued", conversationId, itemId });
}

export async function resumeQueue(conversationId: string): Promise<void> {
  const { queue } = await request({ method: "resumeQueue", conversationId });
  showQueue(conversationId, queue);
}

/** Stops the running turn; the queue pauses until resumed. */
export async function interrupt(conversationId: string): Promise<void> {
  const tasks = useBoard.getState().board;
  const ids = tasks?.conversationId === conversationId
    ? Object.values(tasks.tasks).filter((task) => ["queued", "starting", "running", "blocked", "paused", "landing", "readyToLand"].includes(task.state)).map((task) => task.id)
    : [];
  const results = await Promise.allSettled([
    request({ method: "interrupt", conversationId }), ...ids.map(stopTask),
  ]);
  const failed = results.find((result) => result.status === "rejected");
  if (failed?.status === "rejected") throw failed.reason;
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

export async function answerQuestion(
  conversationId: string,
  cardId: string,
  answer: string,
): Promise<void> {
  await request({ method: "answerQuestion", conversationId, cardId, answer });
}

export async function decidePlan(
  conversationId: string,
  cardId: string,
  approve: boolean,
  message: string | null,
): Promise<void> {
  await request({ method: "decidePlan", conversationId, cardId, approve, message });
}

/**
 * Overnight runs. Each command carries a fresh id, so a retried one changes nothing; Start
 * names the proposal revision the user saw.
 */
export async function proposeOvernight(
  conversationId: string,
  words: string,
  plan: ProposedPlan | null,
): Promise<OvernightRun> {
  const commandId = crypto.randomUUID();
  const { run } = await request({
    method: "proposeOvernight",
    conversationId,
    commandId,
    words,
    plan,
  });
  return run;
}

export async function startOvernight(
  conversationId: string,
  runId: string,
  revision: number,
): Promise<OvernightRun> {
  const commandId = crypto.randomUUID();
  const { run } = await request({
    method: "startOvernight",
    conversationId,
    runId,
    commandId,
    revision,
  });
  return run;
}

export async function stopOvernight(conversationId: string, runId: string): Promise<OvernightRun> {
  const commandId = crypto.randomUUID();
  const { run } = await request({ method: "stopOvernight", conversationId, runId, commandId });
  return run;
}

export async function steerOvernight(
  conversationId: string,
  runId: string,
  words: string,
): Promise<OvernightRun> {
  const commandId = crypto.randomUUID();
  const { run } = await request({
    method: "steerOvernight",
    conversationId,
    runId,
    commandId,
    words,
  });
  return run;
}

export async function continueOvernight(
  conversationId: string,
  runId: string,
  words: string,
): Promise<OvernightRun> {
  const commandId = crypto.randomUUID();
  const { run } = await request({
    method: "continueOvernight",
    conversationId,
    runId,
    commandId,
    words,
  });
  return run;
}

export async function stopTask(taskId: string): Promise<void> {
  await request({ method: "stopTask", taskId });
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

/** The user did something only they could do ("Waiting on you"); the orchestrator hears it. */
export async function resolveWaiting(conversationId: string, id: string): Promise<void> {
  await request({ method: "resolveWaiting", conversationId, id });
}

export async function readArtifact(id: string, offset: number, limit: number) {
  const { text } = await request({ method: "readArtifact", id, offset, limit });
  return text;
}

/** Transcript entries fetched per page. */
const WORKER_PAGE = 500;

function updateWorkerTranscript(
  conversationId: string,
  taskId: string,
  update: (transcript: WorkerTranscript) => WorkerTranscript,
): void {
  updateBoard(conversationId, (board) => ({
    ...board,
    transcripts: {
      ...board.transcripts,
      [taskId]: update(
        board.transcripts[taskId] ?? { entries: [], hasMore: false, loading: false },
      ),
    },
  }));
}

/**
 * Starts following a worker's transcript: loads its newest page, keeping live entries that
 * arrived meanwhile. Transcripts load only when their card is opened.
 */
export async function openWorkerTranscript(conversationId: string, taskId: string): Promise<void> {
  const board = useBoard.getState().board;
  if (board?.conversationId !== conversationId || board.transcripts[taskId]) return;
  updateWorkerTranscript(conversationId, taskId, (transcript) => ({ ...transcript, loading: true }));
  try {
    const { page } = await request({
      method: "listWorkerEvents",
      taskId,
      before: null,
      limit: WORKER_PAGE,
    });
    const newest = page.entries.at(-1)?.streamSeq ?? 0;
    updateWorkerTranscript(conversationId, taskId, (transcript) => ({
      entries: [
        ...page.entries,
        ...transcript.entries.filter((entry) => entry.streamSeq > newest),
      ].slice(-WORKER_ENTRIES),
      hasMore: page.hasMore,
      loading: false,
    }));
  } catch (error) {
    // Forget it, so opening the card again retries the load.
    updateBoard(conversationId, (current) => {
      const { [taskId]: _failed, ...transcripts } = current.transcripts;
      return { ...current, transcripts };
    });
    throw error;
  }
}

export async function loadEarlierWorkerEntries(
  conversationId: string,
  taskId: string,
): Promise<void> {
  const transcript = useBoard.getState().board?.transcripts[taskId];
  const oldest = transcript?.entries[0];
  if (!transcript || !oldest || transcript.loading) return;
  updateWorkerTranscript(conversationId, taskId, (current) => ({ ...current, loading: true }));
  try {
    const { page } = await request({
      method: "listWorkerEvents",
      taskId,
      before: oldest.streamSeq,
      limit: WORKER_PAGE,
    });
    updateWorkerTranscript(conversationId, taskId, (current) => ({
      entries: [...page.entries, ...current.entries],
      hasMore: page.hasMore,
      loading: false,
    }));
  } catch (error) {
    updateWorkerTranscript(conversationId, taskId, (current) => ({ ...current, loading: false }));
    throw error;
  }
}

// ----- lifecycle -------------------------------------------------------------------------

export async function hibernate(id: string): Promise<void> {
  const { conversation } = await request({ method: "hibernate", id });
  storeConversation(conversation);
}

/**
 * Archives a conversation and confirms it with an "Archived chat · View · Undo" toast. The row
 * leaves at once; the daemon stops and cleans up in the background. If the archive fails, the
 * row comes back and a toast says why.
 */
export async function archive(id: string): Promise<void> {
  const before = useApp.getState().conversations[id];
  if (!before) {
    storeConversation((await request({ method: "archive", id })).conversation);
    return;
  }
  if (before.lifecycle === "archived") return;
  const noun = before.kind === "chat" ? "chat" : "session";
  storeConversation({ ...before, lifecycle: "archived" });
  const { selection } = useApp.getState();
  if (selection.type === "conversation" && selection.id === id) {
    select({ type: "draft", kind: "chat" });
  }
  const archived = toast(before.kind === "chat" ? "Archived chat" : "Archived session", {
    actions: [
      { label: "View", run: () => openSettings("archived") },
      { label: "Undo", run: () => void undoArchive(id) },
    ],
  });
  try {
    const { conversation } = await request({ method: "archive", id });
    storeConversation(conversation);
  } catch (error) {
    // Back as it was, unless something newer came in meanwhile.
    if (useApp.getState().conversations[id]?.lifecycle === "archived") storeConversation(before);
    dismissToast(archived);
    toast(`Couldn't archive the ${noun} "${before.title}": ${errorMessage(error)}`, {
      tone: "error",
    });
  }
}

/** The archive toast's Undo: the row is back at once and opens; it leaves again on failure. */
async function undoArchive(id: string): Promise<void> {
  const before = useApp.getState().conversations[id];
  if (before) storeConversation({ ...before, lifecycle: "active" });
  openConversation(id);
  try {
    await restore(id);
  } catch (error) {
    if (before && useApp.getState().conversations[id]?.lifecycle === "active") {
      storeConversation(before);
    }
    toast(`Couldn't bring it back: ${errorMessage(error)}`, { tone: "error" });
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export async function restore(id: string): Promise<void> {
  const { conversation } = await request({ method: "restore", id });
  storeConversation(conversation);
}

/**
 * Deletes a conversation for good. The space compacting the database would give back, when
 * that is worth offering.
 */
export async function deleteConversation(
  id: string,
  deleteBranches: boolean,
  forgetBrain: boolean,
): Promise<number | null> {
  const { compactableBytes } = await request({
    method: "delete",
    id,
    deleteBranches,
    forgetBrain,
  });
  forgetDraft(id);
  const { selection } = useApp.getState();
  if (selection.type === "conversation" && selection.id === id) {
    select({ type: "draft", kind: "chat" });
  }
  useApp.setState((state) => {
    const { [id]: _deleted, ...conversations } = state.conversations;
    const { [id]: _thread, ...threads } = state.threads;
    return { conversations, threads };
  });
  // An older daemon says nothing about it.
  return compactableBytes ?? null;
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
  const { selection } = useApp.getState();
  if (
    (selection.type === "conversation" && gone.includes(selection.id)) ||
    (selection.type === "draft" && selection.kind === "session" && selection.projectId === id)
  ) {
    select({ type: "draft", kind: "chat" });
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

// ----- orchestrator log (Inspector) ------------------------------------------------------

/** Orchestrator log entries fetched per page. */
const ORCHESTRATOR_PAGE = 1_000;

/** Follows a conversation's orchestrator log in the Inspector, or stops with `null`. */
export async function openOrchestratorLog(conversationId: string | null): Promise<void> {
  if (conversationId === null) {
    useBoard.setState({ orchestrator: null });
    return;
  }
  useBoard.setState({
    orchestrator: { conversationId, entries: [], hasMore: false, loading: true, thresholds: null },
  });
  try {
    const { page } = await request({
      method: "listOrchestratorLog",
      conversationId,
      before: null,
      limit: ORCHESTRATOR_PAGE,
    });
    const log = useBoard.getState().orchestrator;
    if (log?.conversationId !== conversationId) return;
    const newest = page.entries.at(-1)?.streamSeq ?? 0;
    useBoard.setState({
      orchestrator: {
        conversationId,
        entries: [
          ...page.entries,
          ...log.entries.filter((entry) => entry.streamSeq > newest),
        ].slice(-ORCHESTRATOR_ENTRIES),
        hasMore: page.hasMore,
        loading: false,
        thresholds: page.thresholds,
      },
    });
  } catch (error) {
    const log = useBoard.getState().orchestrator;
    if (log?.conversationId === conversationId) {
      useBoard.setState({ orchestrator: { ...log, loading: false } });
    }
    throw error;
  }
}

export async function loadEarlierOrchestratorLog(): Promise<void> {
  const log = useBoard.getState().orchestrator;
  const oldest = log?.entries[0];
  if (!log || !oldest || log.loading) return;
  useBoard.setState({ orchestrator: { ...log, loading: true } });
  const { conversationId } = log;
  try {
    const { page } = await request({
      method: "listOrchestratorLog",
      conversationId,
      before: oldest.streamSeq,
      limit: ORCHESTRATOR_PAGE,
    });
    const current = useBoard.getState().orchestrator;
    if (current?.conversationId !== conversationId) return;
    useBoard.setState({
      orchestrator: {
        ...current,
        entries: [...page.entries, ...current.entries],
        hasMore: page.hasMore,
        loading: false,
      },
    });
  } catch (error) {
    const current = useBoard.getState().orchestrator;
    if (current?.conversationId === conversationId) {
      useBoard.setState({ orchestrator: { ...current, loading: false } });
    }
    throw error;
  }
}

// ----- settings --------------------------------------------------------------------------

export async function setDensity(density: Density): Promise<void> {
  await setSetting("density", density);
}

export function setPinnedSummary(shown: boolean): void {
  savePinnedSummary(shown);
  useApp.setState({ pinnedSummary: shown });
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
