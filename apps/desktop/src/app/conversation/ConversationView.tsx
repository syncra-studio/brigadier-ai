import { type AppendMessage, AssistantRuntimeProvider, type ExternalStoreBranchChange, ExportedMessageRepository, type ExternalThreadQueueAdapter, type FeedbackAdapter, type QueueItemState, type ThreadMessage, type ThreadMessageLike, useExternalStoreRuntime, type OpenPastedText, PastedTextOpenerContext, type AttachmentReader, AttachmentReaderContext, ThreadHistoryContext, type ThreadHistory, Thread, type ThreadComponents, LinkConversationContext, OpenFileContext, keptPlace, ThreadListContext, ThreadListSource, type ThreadHolding } from "@/components/conversation";
import {
  type FC,
  memo,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useShallow } from "zustand/react/shallow";

import { ConversationToolbar } from "@/app/shell/ConversationToolbar";
import { WorkspacePane, WorkspacePaneContext } from "@/app/side-panel/WorkspacePane";
import { RightSidebar, RightSidebarContext } from "@/app/side-panel/RightSidebar";
import { ConversationComposer } from "@/app/conversation/Composer";
import { takeQueueSend } from "@/app/conversation/PausedSend";
import { TerminalPane } from "@/app/side-panel/TerminalTab";
import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import { ChatActions, RenameDialog } from "@/app/shell/ChatActions";
import { PinnedSummary, SummaryFloat, SummaryPane } from "@/app/conversation/PinnedSummary";
import { SessionTabBar, SessionTabViews } from "@/app/shell/SessionTabBar";
import { PastedTextPane } from "@/app/side-panel/PastedTextPane";
import { reworkableRequest } from "@/app/conversation/rework";
import { ProjectCombobox } from "@/app/conversation/RailPickers";
import { useSidePanel } from "@/app/side-panel/SidePanel";
import { BrigadierGlyph } from "@/components/icons";
import { queuedImageRefs, reconcileImages } from "@/lib/inlineImages";
import { BlobAttachmentAdapter } from "@/app/conversation/attachments";
import { sendLane } from "@/app/conversation/composerKeys";
import {
  type Block,
  type BoardDigest,
  type PeerOf,
  peerOf,
  type ThreadNode,
} from "@/app/conversation/blocks";
import { cliThreadTree } from "@/app/conversation/cliBlocks";
import { heldRows, keepDrawnBlocks } from "@/app/conversation/heldThread";
import { PeerUserMessage } from "@/app/conversation/PeerMessageBlock";
import {
  type ComposerTarget,
  ComposerTargetContext,
  PulledSlot,
} from "@/app/conversation/composerTarget";
import { useResolvedDraft } from "@/app/conversation/draftSetup";
import { useRuleRefusals } from "@/app/conversation/ruleRefusals";
import { putBack, restoreDraft } from "@/app/conversation/composerDraft";
import { NEW_CHAT_SCOPE } from "@/state/drafts";
import { workerName } from "@/app/conversation/rowWords";
import { type BlockMeta, RequestBlock } from "@/app/conversation/RequestBlock";
import { StatusCardContext } from "@/app/conversation/StatusCard";
import { ThreadFind } from "@/app/conversation/ThreadFind";
import { AboveComposer } from "@/app/conversation/AboveComposer";
import { ComposerCapsule } from "@/app/conversation/ComposerCapsule";
import { LoadEarlier } from "@/app/conversation/LoadEarlier";
import { UserAttachments } from "@/app/conversation/UserAttachments";
import { ViewContext } from "@/app/conversation/viewContext";
import {
  MentionMemory,
  mentionsIn,
  pageMentions,
  type MentionTarget,
  UserMessageText,
} from "@/app/conversation/Mentions";
import { TopBar } from "@/app/shell/TopBar";
import { Banner } from "@/components/shared";
import { useNow } from "@/hooks/use-now";
import type {
  AttachmentRef,
  Conversation,
  Mention,
  ModelChoice,
  Rating,
  ThreadView,
} from "@/ipc/generated";
import { cn } from "@/lib/utils";
import {
  deleteQueued,
  editMessage,
  editQueued,
  interrupt,
  loadEarlier,
  loadFullText,
  rateMessage,
  readAttachment,
  moveQueued,
  regenerate,
  restoreQueued,
  resume,
  send,
  type SendLane,
  startSessionFrom,
  steerQueued,
  switchBranch,
  sendToWorker,
} from "@/state/actions";
import { toast } from "@/state/toasts";
import { takeBackgroundSend } from "@/state/backgroundSend";
import { beginDirectSend, directGenerationOf, directToOf, endDirectSend, setDirectTo } from "@/state/directTo";
import { holdDirect } from "@/state/heldDirect";
import { useActivity } from "@/state/activity";
import { useBoard } from "@/state/board";
import { loadWorkerThread, readSessionImageBlob, sessionImageOf, useThreadView, useWorkerRows } from "@/state/cliThread";
import { isNativeWorker } from "@/state/nativeWorkers";
import { lastFinished, markOpened } from "@/state/lastOpened";
import { landReveal, useReveal } from "@/state/reveal";
import { CHAT_TAB, openFileTab, openPastedTab, selectTab, useSessionTabsOf } from "@/state/sessionTabs";
import { isTaskTab } from "@/state/tabStates";
import { turnStopping, useStopping } from "@/state/stopping";
import { placeOf } from "@/state/terminalPlaces";
import {
  emptyThread,
  type PendingMessage,
  type Selection,
  storedIdOf,
  useApp,
} from "@/state/store";

type Item = { id: string; parentId: string | null } & (
  | { kind: "user"; block: Block; rework: boolean }
  | { kind: "block"; block: Block; meta: BlockMeta; texts: string[]; rating: Rating | null }
);

/** Extra message data the footers read back from assistant-ui's message state. */
export type Custom = {
  attachments?: AttachmentRef[];
  block?: BlockMeta;
  /** The message can be edited now (see `canRework`). */
  rework?: boolean;
  /** What a user message @-mentions, so ↑ recalls its chips. */
  mentions?: Mention[];
  /** Another session sent it: its block shows in the user message's place. */
  peer?: PeerOf;
};

/** An attachment-only message has no text part, so no empty bubble shows above its files. */
function textContent(text: string): ThreadMessageLike["content"] {
  return text ? [{ type: "text", text }] : [];
}

function blockStatus(block: Block): ThreadMessageLike["status"] {
  switch (block.state) {
    case "working":
      return { type: "running" };
    case "waiting":
      return { type: "requires-action", reason: "interrupt" };
    case "done":
      return { type: "complete", reason: "stop" };
    case "stopped":
      return { type: "incomplete", reason: "cancelled" };
    case "failed":
      return { type: "incomplete", reason: "error", error: block.error ?? "The reply failed." };
  }
}

function convertMessage(item: Item): ThreadMessageLike {
  const { block } = item;
  if (item.kind === "user") {
    const user = block.user;
    if (user?.kind === "pending") {
      return {
        id: user.pending.localId,
        role: "user",
        content: textContent(user.pending.text),
        createdAt: new Date(user.pending.createdAtMs),
        metadata: { custom: { attachments: user.pending.attachments, rework: false } satisfies Custom },
      };
    }
    const message = user?.message;
    const peer = message && user?.kind === "peer" ? peerOf(message, user.earlier) : undefined;
    return {
      id: item.id,
      role: "user",
      content: textContent(user?.text ?? ""),
      createdAt: new Date(message?.createdAtMs ?? block.startedAtMs),
      metadata: {
        custom: {
          attachments: message?.attachments ?? [],
          rework: item.rework,
          mentions: message?.mentions ?? [],
          ...(peer && { peer }),
        } satisfies Custom,
      },
    };
  }
  return {
    id: item.id,
    role: "assistant",
    content: item.texts.map((text) => ({ type: "text" as const, text })),
    createdAt: new Date(block.startedAtMs),
    status: blockStatus(block),
    metadata: {
      custom: { block: item.meta } satisfies Custom,
      ...(item.rating && {
        submittedFeedback: { type: item.rating === "good" ? "positive" : "negative" },
      }),
      timing: {
        streamStartTime: block.startedAtMs,
        ...(block.endedAtMs !== null && { totalStreamTime: block.endedAtMs - block.startedAtMs }),
        totalChunks: item.texts.length,
        toolCallCount: block.tasks.length,
      },
    },
  };
}

/** A block's shape without its text, to reuse the previous item while only text is unchanged. */
function signature(
  block: Block,
  picked: ModelChoice | null,
  session: boolean,
  rework: boolean,
  rating: Rating | null,
): string {
  return JSON.stringify([
    rework,
    rating,
    block.state,
    block.error,
    block.startedAtMs,
    block.endedAtMs,
    block.worked,
    block.cardWait,
    block.texts.map((text) => [text.messageId, text.position, text.model, text.anchors, text.answerAtMs, text.commentary]),
    block.cards,
    block.tasks,
    block.rows,
    block.orchestratorSteps,
    block.thinking,
    block.compactions,
    block.steers.map((steer) => [steer.message.id, steer.text]),
    block.requestIds,
    picked,
    session,
  ]);
}

/**
 * The thread's messages: per request, the user's message and one assistant block, each under
 * its parent. Items whose block did not change keep their identity, so assistant-ui converts
 * and renders only the block that streams or changed.
 */
function useItems(
  conversationId: string | null,
  nodes: readonly ThreadNode[],
  picked: ModelChoice | null,
  session: boolean,
  canRework: (requestId: string) => boolean,
  ratings: Partial<Record<string, Rating>>,
): Item[] {
  return useMemo(() => {
    // Each conversation's, for the last few opened: one opened again (its view drawn anew) finds
    // its items made.
    const cache = itemsMade.get(conversationId) ?? new Map<string, ItemEntry>();
    itemsMade.delete(conversationId);
    itemsMade.set(conversationId, cache);
    for (const key of itemsMade.keys()) if (itemsMade.size > KEPT_ITEMS) itemsMade.delete(key);
    const items: Item[] = [];
    const seen = new Set<string>();
    for (const node of nodes) {
      seen.add(node.id);
      const { block } = node;
      const entry = cache.get(node.id);
      // The request it answers: its user message's.
      const requestId = block.requestIds[0] ?? block.key;
      // A block that joined steered requests answers the last of them, not its own message.
      const rework =
        block.user?.kind === "message" &&
        canRework(requestId) &&
        (node.kind === "user" || block.steers.length === 0);
      if (node.kind === "user") {
        if (
          entry?.item.kind !== "user" ||
          entry.item.block.user !== block.user ||
          entry.item.parentId !== node.parentId ||
          entry.item.rework !== rework
        ) {
          cache.set(node.id, {
            item: { id: node.id, parentId: node.parentId, kind: "user", block, rework },
            sig: "",
            texts: [],
            from: block,
          });
        }
        items.push((cache.get(node.id) as { item: Item }).item);
        continue;
      }
      // The answer, not an update line under it.
      const answerId = (block.texts.findLast((text) => !text.update) ?? block.texts.at(-1))?.messageId ?? null;
      const forkId = block.texts.at(-1)?.messageId ?? null;
      const rating = (answerId && ratings[answerId]) || null;
      // The same block, read the same way: nothing to compare (most of a long thread, per streamed word).
      if (entry?.item.kind === "block" && entry.from === block && entry.item.parentId === node.parentId && entry.item.rating === rating && entry.item.meta.picked === picked && entry.item.meta.session === session && entry.item.meta.rework === rework) {
        items.push(entry.item);
        continue;
      }
      const texts = block.texts.map((text) => text.text);
      // The shapes are compared only where one was drawn before (made then, if it wasn't yet).
      let sig: string | null = null;
      const changed =
        entry?.item.kind !== "block" ||
        entry.item.parentId !== node.parentId ||
        entry.texts.length !== texts.length ||
        entry.texts.some((text, index) => text !== texts[index]) ||
        (entry.sig ??= signature(entry.from, entry.item.meta.picked, entry.item.meta.session, entry.item.meta.rework, entry.item.rating)) !==
          (sig = signature(block, picked, session, rework, rating));
      if (changed) {
        const meta: BlockMeta = {
          texts: block.texts.map((text) => ({
            messageId: text.messageId,
            ...(text.atMs !== undefined && { atMs: text.atMs }),
            position: text.position,
            model: text.model,
            update: text.update,
            ...(text.anchors && { anchors: text.anchors }),
            ...(text.answerAtMs !== undefined && { answerAtMs: text.answerAtMs }),
            ...(text.commentary && { commentary: true }),
          })),
          cards: block.cards,
          rows: block.rows,
          orchestratorSteps: block.orchestratorSteps,
          thinking: block.thinking,
          compactions: block.compactions,
          seams: block.seams,
          nudges: block.nudges,
          taskNotices: block.taskNotices,
          steers: block.steers.map((steer) => {
            const peer = peerOf(steer.message);
            return {
              position: steer.position,
              text: steer.text,
              atMs: steer.message.createdAtMs,
              attachments: steer.message.attachments,
              requestId: steer.message.requestId ?? steer.message.id,
              ...(peer && { peer }),
            };
          }),
          state: block.state,
          startedAtMs: block.startedAtMs,
          endedAtMs: block.endedAtMs,
          worked: block.worked,
          cardWait: block.cardWait,
          picked,
          session,
          rework,
          requestId,
          requestIds: block.requestIds,
          answerId,
          forkId,
        };
        cache.set(node.id, { item: { id: node.id, parentId: node.parentId, kind: "block", block, meta, texts, rating }, sig, texts, from: block });
      }
      const kept = cache.get(node.id) as ItemEntry;
      kept.from = block;
      items.push(kept.item);
    }
    for (const key of cache.keys()) if (!seen.has(key)) cache.delete(key);
    return items;
  }, [conversationId, nodes, picked, session, canRework, ratings]);
}

/** An item as made, with the block it was made from and that block's shape (`signature`, made once needed). */
type ItemEntry = { item: Item; sig: string | null; texts: string[]; from: Block };

/** Conversations whose items stay made while others open. */
const KEPT_ITEMS = 4;

/** The items made of the last few conversations opened, by node id. */
const itemsMade = new Map<string | null, Map<string, ItemEntry>>();

/** assistant-ui's form of each item, converted once per item. */
const converted = new WeakMap<Item, ThreadMessage>();

function toThreadMessage(item: Item): ThreadMessage {
  let message = converted.get(item);
  if (!message) {
    const [entry] = ExportedMessageRepository.fromBranchableArray([
      { message: convertMessage(item), parentId: item.parentId },
    ]).messages;
    message = (entry as { message: ThreadMessage }).message;
    converted.set(item, message);
  }
  return message;
}

/** The branch shown, oldest first: its items, and assistant-ui's form of each. */
function branchOf(items: readonly Item[], headId: string | null): { items: Item[]; messages: ThreadMessage[] } {
  const byId = new Map(items.map((item) => [item.id, item]));
  const path: Item[] = [];
  const seen = new Set<string>();
  for (let item = headId ? byId.get(headId) : undefined; item && !seen.has(item.id); item = item.parentId ? byId.get(item.parentId) : undefined) {
    seen.add(item.id);
    path.push(item);
  }
  path.reverse();
  return { items: path, messages: path.map(toThreadMessage) };
}

/** Messages at the branch's end assistant-ui holds of a thread before its list asks for any. */
const OPEN_HOLD = 40;

/** What assistant-ui holds of a thread its list hasn't asked of yet: its end, and where its kept place starts. */
function openingHold(branch: readonly Item[], key: string | null): ThreadHolding {
  const kept = keptPlace(key);
  return { key, from: Math.max(0, branch.length - OPEN_HOLD), to: branch.length - 1, ids: kept && !kept.bottom && kept.anchor !== null ? [kept.anchor] : [] };
}

/**
 * What assistant-ui holds of the thread (#627): the turns `heldRows` picks. Each message of them
 * goes under the one before it held, so assistant-ui sees one branch with gaps left out; the
 * other versions of each go beside it, under the same one, for its branch picker.
 */
function heldRepository(items: readonly Item[], branch: readonly Item[], headId: string | null, hold: ThreadHolding): ExportedMessageRepository {
  const held = heldRows(branch, hold);
  // Other versions there are only where the branch isn't everything.
  let versions: Map<string | null, Item[]> | null = null;
  if (items.length !== branch.length) {
    versions = new Map();
    for (const item of items) {
      const siblings = versions.get(item.parentId);
      if (siblings) siblings.push(item);
      else versions.set(item.parentId, [item]);
    }
  }
  const messages: ExportedMessageRepository["messages"][number][] = [];
  let parentId: string | null = null;
  branch.forEach((item, index) => {
    if (!held[index]) return;
    for (const each of versions?.get(item.parentId) ?? [item]) messages.push({ message: toThreadMessage(each), parentId });
    parentId = item.id;
  });
  return { headId, messages };
}

function textOf(message: AppendMessage): string {
  return message.content
    .map((part) => (part.type === "text" ? part.text : ""))
    .join("")
    .trim();
}

/**
 * The row of the thread holding message `messageId` between sessions: a message another session
 * sent, or a turn that sent one or answers with one.
 */
function rowHolding(nodes: readonly ThreadNode[], messageId: string): string | null {
  for (const node of nodes) {
    const { block } = node;
    if (node.kind === "user") {
      if (block.user?.kind === "peer" && block.user.message.id === messageId) return node.id;
      continue;
    }
    if (
      block.texts.some((text) => text.anchors?.includes(messageId)) ||
      block.steers.some((steer) => steer.message.id === messageId) ||
      block.orchestratorSteps.some((step) => step.kind.type === "peerSent" && step.kind.messageIds.includes(messageId))
    ) {
      return node.id;
    }
  }
  return null;
}

const NO_PENDING: PendingMessage[] = [];
const EMPTY_VIEW: ThreadView = { entries: [], turns: [] };
const NO_RATINGS: Partial<Record<string, Rating>> = {};

const EMPTY_DIGEST: BoardDigest = {
  tasks: {},
  questions: {},
  orchestratorSteps: [],
  machineSteps: [],
  decisions: [],
  compactions: {},
};

/**
 * Those of `targets` the composer's next message can go to directly: the session's own
 * workers whose CLI still runs (not the CLI's own agents, not one gone or closed).
 */
function useDirectTargets(conversationId: string | null, targets: readonly MentionTarget[]): MentionTarget[] {
  const rows = useWorkerRows(conversationId);
  return useMemo(() => {
    const live = new Set(rows.filter((row) => !isNativeWorker(row) && !row.gone).map((row) => row.id));
    return targets.filter((target) => live.has(target.id));
  }, [rows, targets]);
}

/** The open conversation's workers, for @-mentions. */
function useMentionTargets(conversationId: string | null): MentionTarget[] {
  const flat = useBoard(
    useShallow((s) => {
      const board = s.board;
      if (!board || board.conversationId !== conversationId) return [];
      return Object.values(board.tasks)
        .toSorted((a, b) => a.number - b.number)
        // Each by its name, as everywhere the user reads it.
        .flatMap((task) => [task.id, task.number, workerName(board.tasks, task), task.state]);
    }),
  );
  return useMemo(() => {
    const targets: MentionTarget[] = [];
    for (let i = 0; i < flat.length; i += 4) {
      targets.push({
        id: flat[i] as string,
        number: flat[i + 1] as number,
        title: flat[i + 2] as string,
        state: flat[i + 3] as string,
      });
    }
    return targets;
  }, [flat]);
}

/* The session's tabs and its summary: drawn again only as the session itself changes, not with the
   view around them, which draws on every streamed word. */
const SessionTop = memo(function SessionTop({ conversation, kind, onRename }: { conversation: Conversation; kind: "session" | "chat"; onRename: () => void }) {
  return (
    <SessionTabBar conversation={conversation} onRename={onRename}>
      <ConversationToolbar kind={kind} onContext={() => selectTab(conversation.id, CHAT_TAB)} />
    </SessionTabBar>
  );
});

const SessionSummary = memo(function SessionSummary({ conversation }: { conversation: Conversation | null }) {
  return (
    <PinnedSummary conversation={conversation} />
  );
});

/**
 * A conversation's thread and composer, with its title bar, pinned card and side panel; or,
 * `embedded` in another one's side panel (a side chat), the thread and composer alone.
 */
export function ConversationView({
  selection,
  embedded = false,
}: {
  selection: Selection;
  embedded?: boolean;
}) {
  const conversationId = selection.type === "conversation" ? selection.id : null;
  const conversation = useApp((s) =>
    conversationId ? (s.conversations[conversationId] ?? null) : null,
  );
  const thread = useApp((s) =>
    conversationId ? (s.threads[conversationId] ?? emptyThread) : emptyThread,
  );
  const pending = useApp(
    useShallow((s) =>
      conversationId
        // One sent to wait shows in the queue rail instead.
        ? s.pending.filter((entry) => entry.conversationId === conversationId && !entry.queued)
        : NO_PENDING,
    ),
  );
  const digest = useBoard(
    useShallow((s): BoardDigest | null =>
      s.board?.conversationId === conversationId
        ? {
            tasks: s.board.tasks,
            workerSteps: s.board.workerSteps,
            questions: s.board.questions,
            orchestratorSteps: s.board.orchestratorSteps,
            machineSteps: s.board.machineSteps,
            decisions: s.board.decisions,
            compactions: s.board.compactions,
            peerMessages: s.board.peerMessages,
            queueItems: s.board.queue.items,
          }
        : null,
    ),
  );
  const run = useBoard((s) =>
    s.board?.conversationId === conversationId ? s.board.run : "idle",
  );
  const runError = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.runError : null));
  const queueItems = useBoard((s) =>
    s.board?.conversationId === conversationId ? s.board.queue.items : null,
  );
  const targets = useMentionTargets(conversationId);
  const direct = useDirectTargets(conversationId, targets);
  const resolved = useResolvedDraft(selection);
  const [error, setError] = useState<string | null>(null);

  // Blob-backed messages show their preview until the full text arrives.
  useEffect(() => {
    if (!conversationId) return;
    for (const message of thread.items) {
      if (message.blob && thread.fullText[message.id] === undefined) {
        void loadFullText(conversationId, message.id, message.blob).catch(
          (cause: unknown) => setError(String(cause)),
        );
      }
    }
  }, [conversationId, thread.items, thread.fullText]);

  const session = conversation?.kind === "session" || resolved.kind === "session";
  // The thread is the CLI's own, read from its session file (S9, S10).
  const cliView = useThreadView(conversationId);
  // The view opens with what it has: what arrives as it draws (its board, the messages read
  // again) is built in a task of its own, the frame after, not in the one that opens it (#628).
  const live = { cliView, items: thread.items, fullText: thread.fullText, digest, pending, run, runError };
  const [opening, setOpening] = useState(true);
  const [openedWith] = useState(live);
  useEffect(() => {
    const timer = setTimeout(() => setOpening(false), 0);
    return () => clearTimeout(timer);
  }, []);
  const built = opening ? openedWith : live;
  const full = useMemo(
    () =>
      conversationId
        ? keepDrawnBlocks(conversationId, cliThreadTree(built.cliView ?? EMPTY_VIEW, conversationId, built.items, built.fullText, built.pending, built.digest ?? EMPTY_DIGEST, { state: built.run, error: built.runError }, session))
        : cliThreadTree(EMPTY_VIEW, "", [], {}, built.pending, EMPTY_DIGEST, { state: built.run, error: built.runError }),
    [built.cliView, conversationId, built.items, built.fullText, built.digest, built.pending, built.run, built.runError, session],
  );
  // A jump to a message between sessions here: its row is mounted, and it lands once shown.
  const reveal = useReveal((s) => (!embedded && s.reveal?.conversationId === conversationId ? s.reveal : null));
  const revealRow = useMemo(() => (reveal ? rowHolding(full.nodes, reveal.messageId) : null), [reveal, full.nodes]);
  const tree = full;
  // An older page that failed to load offers Try again, in the conversation it failed in.
  const [olderFailedIn, setOlderFailedIn] = useState<string | null>(null);
  // The CLI's thread is read whole.
  const hasMore = false;
  const oldest = thread.items[0]?.seq ?? null;
  const searchOlder = null;
  const earlier = useMemo<ThreadHistory>(
    () => ({
      showing: revealRow,
      hasMore,
      failed: olderFailedIn !== null && olderFailedIn === conversationId,
      loadOlder: async () => {
        if (conversationId === null) return;
        setOlderFailedIn(null);
        try {
          await loadEarlier(conversationId);
        } catch (cause) {
          setOlderFailedIn(conversationId);
          throw cause;
        }
      },
      oldest,
      searchOlder,
    }),
    [revealRow, conversationId, hasMore, olderFailedIn, oldest, searchOlder],
  );
  // Open, what it finished is seen: the sidebar's dot clears, and stays clear.
  const finishedAtMs = lastFinished(conversation) ?? 0;
  useEffect(() => {
    if (!embedded && conversationId) markOpened(conversationId, Math.max(Date.now(), finishedAtMs));
  }, [embedded, conversationId, finishedAtMs]);
  useEffect(() => {
    if (!reveal) return;
    return landReveal(reveal.messageId, reveal.nonce);
  }, [reveal]);
  const setup = conversation?.setup;
  const picked =
    setup?.type === "chat" ? setup.model : setup?.type === "session" ? setup.orchestrator : null;
  const running = run === "running" || run === "starting";
  // Messages waiting for quota: nothing runs, but Stop (and Esc) drops them.
  // The user's Stop shows at once (#498), messages waiting for quota dropped with it.
  const stopping = useStopping((s) => turnStopping(s.stops, conversationId));
  const waitingForQuota = conversation?.quotaWait != null && !stopping;
  const reworkable = useBoard((s) =>
    s.board?.conversationId === conversationId && s.board ? reworkableRequest(s.board) : null,
  );
  // The CLI's turn is open: Stop ends it.
  const answerWorking = useActivity((s) => (conversationId ? (s.byConversation[conversationId]?.turnOpen ?? false) : false));
  // The user stopped the CLI's last turn, or it was cut off (Brigadier quit while it ran), and
  // nothing runs: the send button resumes it.
  const lastEnd = cliView?.turns.at(-1)?.end ?? null;
  const stopped = lastEnd === "stopped" || lastEnd === "interrupted";
  // Not while the orchestrator's turn for the request runs (the composer can stop it); a
  // request whose workers still run can be redone.
  const runRequest = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.runRequest : null));
  const canRework = useCallback(
    (requestId: string) =>
      session ? requestId === reworkable && !(running && runRequest === requestId) : !running,
    [session, reworkable, running, runRequest],
  );
  const ratings = useBoard((s) =>
    s.board?.conversationId === conversationId ? s.board.ratings : NO_RATINGS,
  );
  const items = useItems(conversationId, tree.nodes, picked, session, canRework, ratings);
  // The branch shown, whole, for what reads it all (`thread-list.ts`); assistant-ui holds only the
  // turns the list asks for and the newest one (#627).
  const branch = useMemo(() => branchOf(items, tree.headId), [items, tree.headId]);
  const [list] = useState(() => new ThreadListSource());
  const [holding, setHolding] = useState<ThreadHolding | null>(null);
  list.hold = setHolding;
  const held = useMemo(
    () => (holding?.key === conversationId ? holding : openingHold(branch.items, conversationId)),
    // Opening, what it holds is the branch's end as it opened (once read), not as it grows.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
    [holding, conversationId, branch.items.length === 0],
  );
  list.held = held;
  const repository = useMemo<ExportedMessageRepository>(
    () => heldRepository(items, branch.items, tree.headId, held),
    [items, branch, tree.headId, held],
  );
  // Told before assistant-ui is (its runtime's effect below): both draw in the same pass.
  useEffect(() => list.set(conversationId, branch.messages), [list, conversationId, branch]);
  // What each item's branch ends at, for the branch picker.
  const heads = useMemo(
    () => new Map(tree.nodes.map((node) => [node.id, node.head])),
    [tree.nodes],
  );
  const { panel: sidePanel, agents, workspace } = useSidePanel(
    conversationId,
    !embedded && (conversation?.kind === "session" || (selection.type === "draft" && selection.kind === "session")),
    embedded,
    selection.type === "draft" && selection.kind === "session" ? selection.projectId : conversation?.projectId ?? null,
    !embedded && !!conversation,
  );
  const [renaming, setRenaming] = useState(false);
  const rename = useCallback(() => setRenaming(true), []);

  // A new scope must not inherit uploads or refs from the previous composer.
  // oxlint-disable-next-line react-hooks/exhaustive-deps, react/memo-dependencies
  const attachments = useMemo(() => new BlobAttachmentAdapter(), [conversationId]);
  const reader = useMemo<AttachmentReader>(() => ({
    // A user message of the CLI's thread carries its images in the session file.
    read: (ref) => {
      const image = sessionImageOf(ref);
      return image && conversationId ? readSessionImageBlob(conversationId, image) : readAttachment(ref);
    },
    composerRef: (id) => attachments.refOf(id),
  }), [attachments, conversationId]);
  const [mentions] = useState(() => new MentionMemory());
  const [pulled] = useState(() => new PulledSlot());
  // This view's visit of the conversation: a direct send's end acts here only while it lasts.
  const visit = useRef<object>({});
  useEffect(() => {
    const here = {};
    visit.current = here;
    return () => {
      visit.current = {};
      // The To: is for one message in this visit: leaving takes it away.
      if (conversationId) setDirectTo(conversationId, null);
    };
  }, [conversationId]);
  const draftTarget = resolved.target;
  // The conversation whose `/status` card shows (none once another one opens).
  const [statusFor, setStatusFor] = useState<string | null>(null);
  const statusCard = useMemo(
    () => ({
      open: statusFor !== null && statusFor === conversationId,
      setOpen: (open: boolean) => setStatusFor(open ? conversationId : null),
    }),
    [statusFor, conversationId],
  );
  const fail = useCallback((cause: unknown) => {
    setError(cause instanceof Error ? cause.message : String(cause));
  }, []);
  // The user's rules' refusals read in their words (as Settings › Routing words a rule).
  const refusals = useRuleRefusals(resolved.groups);
  const { setPanel: showWorker } = agents;
  /**
   * The message to the composer's To: worker alone: past the session's queue, its paused
   * state and a new background session. Its end touches only the visit and the To: it was
   * sent from. Refused, the whole message goes back into the composer with its To: when
   * nothing changed there meanwhile, else it is kept whole apart, offered back on the rail;
   * taken, the To: goes and the worker's thread opens. Nothing after the worker took it fails
   * the send.
   */
  const sendDirect = useCallback(
    (message: AppendMessage, from: string, taskId: string) => {
      const text = textOf(message);
      const refs = attachments.messageRefs(text, message.attachments ?? []);
      if (!text && refs.length === 0) return;
      const draft = { text, attachments: refs, mentions: mentionsIn(text, targets, mentions.known()) };
      const here = visit.current;
      const generation = directGenerationOf(from);
      // This visit, and the To: it was sent with, still showing.
      const unchanged = () => visit.current === here && directGenerationOf(from) === generation;
      const keep = (cause: unknown) => {
        if (!(unchanged() && restoreDraft(from, draft))) holdDirect(from, taskId, draft);
        if (visit.current === here) fail(cause);
        else toast(cause instanceof Error ? cause.message : String(cause), { tone: "error" });
      };
      // A queued message being edited is the session's; it never goes to a worker.
      if (pulled.holds(from)) {
        keep(new Error("Send the queued message you're editing first"));
        return;
      }
      if (!beginDirectSend(from)) {
        keep(new Error("Your message to the worker is still on its way"));
        return;
      }
      setError(null);
      sendToWorker(from, taskId, draft).then(
        () => {
          endDirectSend(from);
          try {
            if (unchanged()) {
              setDirectTo(from, null);
              showWorker(taskId);
            }
            void loadWorkerThread(from, taskId).catch((cause: unknown) =>
              console.error("reading the worker's thread failed", cause),
            );
          } catch (cause) {
            console.error("showing the worker after its message failed", cause);
          }
        },
        (cause: unknown) => {
          endDirectSend(from);
          keep(cause);
        },
      );
    },
    [attachments, targets, mentions, pulled, fail, showWorker],
  );
  const submit = useCallback(
    (message: AppendMessage, lane: SendLane | "followUp") => {
      // To the composer's To: worker, before anything of the session's queue is taken.
      const directTo = directToOf(conversationId);
      if (conversationId && directTo !== null) {
        // This send's marks are void: it goes to neither a new session nor the paused queue.
        takeBackgroundSend(conversationId);
        takeQueueSend(conversationId);
        sendDirect(message, conversationId, directTo);
        return;
      }
      const text = textOf(message);
      const refs = attachments.messageRefs(text, message.attachments ?? []);
      // ⌘⏎ asked for a new background session.
      const background = takeBackgroundSend(conversationId);
      // Asked to wait in the paused queue (Send message?): at its end.
      const keepPaused = takeQueueSend(conversationId);
      // A queued message pulled out to edit goes back to its slot, unless steered in now.
      const slot = pulled.take(conversationId);
      if (!text && refs.length === 0) return;
      setError(null);
      const known = [...mentions.known(), ...(slot?.mentions ?? [])];
      const outgoing = { text, attachments: refs, mentions: [...mentionsIn(text, targets, known), ...pageMentions(slot?.mentions)] };
      const source = conversationId ? useApp.getState().conversations[conversationId] : undefined;
      // What wasn't sent goes back into the composer: a refusal never takes the typed message. A
      // draft's goes to the conversation made for it, which shows by then.
      let target = conversationId;
      const keep = (cause: unknown) => {
        putBack(target ?? NEW_CHAT_SCOPE, text);
        fail(cause);
      };
      if (background && source && isTaskTab(source)) {
        void startSessionFrom(source, outgoing).catch(keep);
        return;
      }
      const into = keepPaused
        ? "queue"
        : sendLane(lane, { pulled: !!slot, followUp: useApp.getState().settings.followUp });
      send(
        outgoing,
        draftTarget ?? undefined,
        {
          lane: into,
          queueIndex: into === "steer" ? null : (slot?.index ?? null),
          to: conversationId,
          onTarget: (id) => {
            target = id;
          },
        },
      ).catch(keep);
    },
    [attachments, pulled, conversationId, targets, mentions, draftTarget, fail, sendDirect],
  );

  // assistant-ui's queue over the daemon's: sends go through it so the composer stays usable
  // while a turn runs, and the queue card's steer, move and delete come back through it.
  const queue = useMemo<ExternalThreadQueueAdapter>(() => {
    const queued = queueItems ?? [];
    const states: QueueItemState[] = queued.map((item) => ({
      id: item.id,
      prompt: item.text,
      parts: [{ type: "text", text: item.text }],
    }));
    const find = (id: string) => queued.findIndex((item) => item.id === id);
    // What waits includes a send not yet in the daemon's queue, read when asked: assistant-ui
    // keeps its own copy of a sent message in the thread unless the queue grew as it sent, and
    // a queued message shows in the rail only (#491).
    let shown: { waiting: readonly PendingMessage[]; items: readonly QueueItemState[] } | null = null;
    return {
      get items() {
        const waiting = useApp.getState().pending.filter((entry) => entry.conversationId === conversationId && entry.queued);
        if (waiting.length === 0) return states;
        if (shown && shown.waiting.length === waiting.length && shown.waiting.every((entry, index) => entry === waiting[index])) {
          return shown.items;
        }
        const rows = [
          ...states,
          ...waiting.map((entry) => ({ id: entry.localId, prompt: entry.text, parts: [{ type: "text" as const, text: entry.text }] })),
        ];
        shown = { waiting, items: rows };
        return rows;
      },
      steerItems: [],
      // Sent with nothing running, or the follow-up's other way when steering is the default.
      enqueue: (message) => submit(message, "auto"),
      // Sent while an answer works: the default way, unless the other key asked to steer.
      steer: (message) => submit(message, message.steer ? "steer" : "followUp"),
      move: (id, placement) => {
        if (!conversationId) return;
        if (placement.lane === "steer") {
          void steerQueued(conversationId, id).catch(fail);
          return;
        }
        const rest = queued.filter((item) => item.id !== id);
        const at = (anchor: string) => rest.findIndex((item) => item.id === anchor);
        const index =
          placement.insertAfter !== undefined
            ? placement.insertAfter === null
              ? 0
              : at(placement.insertAfter) + 1
            : placement.insertBefore
              ? at(placement.insertBefore)
              : rest.length;
        void moveQueued(conversationId, id, Math.max(0, index)).catch(fail);
      },
      edit: (id, message) => {
        if (!conversationId) return;
        const text = textOf(message);
        const item = queued[find(id)];
        void editQueued(conversationId, id, {
          text,
          attachments: queuedImageRefs(text, item?.attachments ?? [], attachments.messageRefs(text, message.attachments ?? [])),
          mentions: [
            ...mentionsIn(text, targets, [...mentions.known(), ...(item?.mentions ?? [])]),
            ...pageMentions(item?.mentions),
          ],
        }).catch(fail);
      },
      remove: (id) => {
        const index = find(id);
        const item = queued[index];
        if (!conversationId || !item) return;
        void deleteQueued(conversationId, id)
          .then(() =>
            toast("Queued message deleted", {
              actions: [
                {
                  label: "Undo",
                  run: () =>
                    void restoreQueued(conversationId, item, index)
                      .then(() => toast("Queued message restored"))
                      .catch(fail),
                },
              ],
            }),
          )
          .catch(fail);
      },
    };
  }, [queueItems, submit, conversationId, attachments, targets, mentions, fail]);

  const archived = conversation?.lifecycle === "archived";
  // "Good response" / "Bad response" on an answer: kept by the daemon, on this machine.
  const feedback = useMemo<FeedbackAdapter>(
    () => ({
      submit: ({ message, type }) => {
        const answerId = (message.metadata.custom as Custom).block?.answerId;
        if (!conversationId || !answerId) return;
        void rateMessage(conversationId, answerId, type === "positive" ? "good" : "bad").catch(
          fail,
        );
      },
    }),
    [conversationId, fail],
  );
  const runtime = useExternalStoreRuntime<ThreadMessage>({
    messageRepository: repository,
    // The daemon owns the messages; this only lets the branch picker switch (see below).
    setMessages: () => {},
    unstable_onBranchChange: ({ headId }: ExternalStoreBranchChange) => {
      const head = headId ? heads.get(headId) : null;
      if (conversationId && head) void switchBranch(conversationId, head).catch(fail);
    },
    onEdit: async (message) => {
      const text = textOf(message);
      if (!conversationId || !message.sourceId || !text) return;
      setError(null);
      const original = items.find((item) => item.id === message.sourceId);
      const user = original?.block.user;
      const refs = user?.kind === "message" ? user.message.attachments : undefined;
      await editMessage(conversationId, storedIdOf(message.sourceId), text, reconcileImages(text, refs)).catch(fail);
    },
    onReload: async (parentId) => {
      if (!conversationId || !parentId) return;
      setError(null);
      await regenerate(conversationId, storedIdOf(parentId)).catch(fail);
    },
    isLoading: conversationId !== null && cliView === null,
    isRunning: running || waitingForQuota || answerWorking || Object.values(digest?.tasks ?? {}).some((task) => ["queued", "starting", "running", "blocked", "paused", "landing", "readyToLand"].includes(task.state)),
    isDisabled: archived,
    isSendDisabled: conversation === null && resolved.problem !== null,
    queue,
    adapters: { attachments, feedback },
    onNew: async (message) => submit(message, "auto"),
    onCancel: async () => {
      if (!conversationId) return;
      try {
        await interrupt(conversationId);
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
      }
    },
  });

  const onResume = useMemo(
    () =>
      conversationId && stopped && !running && !archived
        ? () => {
            setError(null);
            void resume(conversationId).catch(fail);
          }
        : null,
    [conversationId, stopped, running, archived, fail],
  );

  const target = useMemo<ComposerTarget>(
    () => ({
      conversation,
      resolved,
      targets,
      direct,
      mentions,
      running,
      onResume,
      queue: { pulled, attachments },
    }),
    [conversation, resolved, targets, direct, mentions, running, onResume, pulled, attachments],
  );

  // The same value while these stay: each change draws every message of the thread again.
  const view = useMemo(() => ({ selection, conversation, embedded }), [selection, conversation, embedded]);
  // Every standalone conversation and draft can show its context card.
  const summary = !embedded;
  // A session's own view has tabs; another tab in front covers the conversation (kept as it was).
  // A Chat has them once an agent opens a Browser tab in it, so the user can see and take it (#361).
  const standalone = !embedded && (conversation?.kind === "session" || conversation?.kind === "chat");
  const { tabs: openTabs, active: activeTab } = useSessionTabsOf(standalone ? conversationId : null);
  const tabbed = standalone && (conversation?.kind === "session" || openTabs.length > 0);
  // A pasted text a chip opened in a Chat or a draft, which have no tabs: over the thread (#427).
  const place = conversationId ?? "draft";
  const [pastedOver, setPastedOver] = useState<{ place: string; n: number | null; paste: AttachmentRef | null } | null>(null);
  const pasted = !tabbed && pastedOver?.place === place ? pastedOver : null;
  const openPasted = useCallback<OpenPastedText>(
    (n, paste) => {
      if (tabbed && conversationId) openPastedTab(conversationId, n, paste);
      else setPastedOver({ place, n, paste });
    },
    [tabbed, conversationId, place],
  );
  const covered = tabbed ? activeTab !== CHAT_TAB : pasted !== null;
  // A file link in an answer opens in a tab of its own when it is one of the session's files.
  const checkout =
    setup?.type === "session"
      ? setup.environment.type === "newWorktree"
        ? (setup.environment.path ?? setup.repo)
        : setup.repo
      : null;
  const openFileAt = useCallback(
    (path: string, line: number | null) => {
      const prefix = checkout ? `${checkout.replace(/\/$/, "")}/` : null;
      if (!prefix || !path.startsWith(prefix) || !conversationId) return false;
      openFileTab(conversationId, path.slice(prefix.length), { line });
      return true;
    },
    [checkout, conversationId],
  );
  return (
    <ViewContext.Provider value={view}>
      <ThreadHistoryContext.Provider value={earlier}>
      <ComposerTargetContext.Provider value={target}>
        <WorkspacePaneContext.Provider value={workspace}>
        <RightSidebarContext.Provider value={sidePanel}>
          <AgentsPanelContext.Provider value={agents}>
            <StatusCardContext.Provider value={statusCard}>
              <AssistantRuntimeProvider runtime={runtime}>
              <ThreadListContext.Provider value={list}>
                <AttachmentReaderContext.Provider value={reader}>
                  <OpenFileContext.Provider value={openFileAt}>
                  <PastedTextOpenerContext.Provider value={openPasted}>
                  <LinkConversationContext.Provider value={conversationId}>
                    {/* The summary's popover, where it floats: opened in the top bar, under it. */}
                    <SummaryFloat>
                    {/* The top bar spans the window to its end edge, over the right panels too, so
                        opening or closing one never moves its controls; the panels open below it. */}
                    <div className="flex h-full min-h-0 min-w-0 flex-col">
                    {tabbed && conversation ? (
                      <SessionTop conversation={conversation} onRename={rename}
                        kind={conversation.kind === "session" || workspace.enabled ? "session" : "chat"} />
                    ) : (
                      !embedded && (
                        <TopBar onRename={conversation && !archived ? () => setRenaming(true) : undefined}>
                          {conversation && (
                            <ChatActions conversation={conversation} onRename={() => setRenaming(true)} />
                          )}
                          <ConversationToolbar kind={workspace.enabled ? "session" : "chat"} />
                        </TopBar>
                      )
                    )}
                    <div className="flex min-h-0 min-w-0 flex-1">
                    <div
                      data-embedded-view={embedded || undefined}
                      data-slot="pane-workspace"
                      className="relative flex h-full min-h-0 min-w-0 flex-1 flex-col"
                    >
                      <div className="relative flex min-h-0 flex-1">
                      <div className="flex h-full min-w-0 flex-1 flex-col">
                          {error && (
                            <Banner tone="error" text={refusals.words(error)} onDismiss={() => setError(null)} />
                          )}
                          <div className="relative flex min-h-0 flex-1">
                          <div className="relative flex min-h-0 min-w-0 flex-1 flex-col">
                            {/* Its own stack, under the tabs: the composer's blurred layers stay under them too. */}
                            <div
                              inert={covered}
                              className={cn("isolate flex min-h-0 flex-1 flex-col", covered && "behind")}
                            >
                              <SummaryPane summary={summary}>
                                {summary && <SessionSummary conversation={conversation} />}
                                <Thread
                                  components={embedded ? EMBEDDED_THREAD_COMPONENTS : THREAD_COMPONENTS}
                                  // One placeholder, in every conversation.
                                  placeholder="Do anything"
                                  scrollKey={conversation?.id}
                                />
                              </SummaryPane>
                            </div>
                            {tabbed && conversationId && <SessionTabViews conversationId={conversationId} />}
                            {pasted && (
                              <PastedTextPane n={pasted.n} paste={pasted.paste} onClose={() => setPastedOver(null)} />
                            )}
                          </div>
                          </div>
                        {!embedded && <TerminalPane place={placeOf(selection)} />}
                      </div>
                      </div>
                    </div>
                    {!embedded && conversation && <RightSidebar conversationId={conversation.id} />}
                    <WorkspacePane />
                    </div>
                    </div>
                    </SummaryFloat>
                  </LinkConversationContext.Provider>
                  </PastedTextOpenerContext.Provider>
                  </OpenFileContext.Provider>
                  {conversation && (
                    <RenameDialog
                      conversation={conversation}
                      open={renaming}
                      onOpenChange={setRenaming}
                    />
                  )}
                </AttachmentReaderContext.Provider>
              </ThreadListContext.Provider>
              </AssistantRuntimeProvider>
            </StatusCardContext.Provider>
          </AgentsPanelContext.Provider>
        </RightSidebarContext.Provider>
        </WorkspacePaneContext.Provider>
      </ComposerTargetContext.Provider>
      </ThreadHistoryContext.Provider>
    </ViewContext.Provider>
  );
}

/** "Good morning" from five until noon, "Good afternoon" until six, else "Good evening". */
function greeting(now: number): string {
  const hour = new Date(now).getHours();
  return hour < 5 || hour >= 18 ? "Good evening" : hour < 12 ? "Good morning" : "Good afternoon";
}

/** The hero over a new chat: a faint mark, a greeting by name once it's known, over "What
 * should we build in {project}?". A side chat, or a tab's own chat, says what it is for instead. */
export const Welcome: FC = () => {
  const { selection, conversation, embedded } = useContext(ViewContext);
  const name = useApp((s) => s.settings.userName);
  const now = useNow(name ? 60_000 : null);
  const project = useApp((s) =>
    selection.type === "draft" && selection.kind === "session"
      ? (s.projects[selection.projectId] ?? null)
      : null,
  );
  if (embedded) {
    return (
      <p className="text-muted-foreground px-2 text-center text-sm">
        {conversation?.tabChat?.target.type === "file"
          ? "Ask about this file"
          : conversation?.tabChat
            ? "Ask about this page"
            : "Ask about this conversation without adding to it"}
      </p>
    );
  }
  return (
    <div className="flex flex-col items-center gap-6 select-none">
      <BrigadierGlyph
        spinOnHover
        className="text-foreground size-16 opacity-60 transition-opacity duration-300 hover:opacity-100 motion-reduce:transition-none"
      />
      {name && (
        <p className="text-muted-foreground -mb-3 px-2 text-center text-base">
          {greeting(now)}, {name}
        </p>
      )}
      <h1 className="font-display tracking-hero text-hero px-2 text-center font-normal">
        {project ? (
          <>
            What should we build in <ProjectCombobox project={project} inHeading />?
          </>
        ) : (
          "What should we build?"
        )}
      </h1>
    </div>
  );
};

const THREAD_COMPONENTS: ThreadComponents = {
  AssistantMessage: RequestBlock,
  Welcome,
  BeforeMessages: LoadEarlier,
  UserAttachments,
  AboveComposer,
  Capsule: ComposerCapsule,
  Composer: ConversationComposer,
  UserText: UserMessageText,
  PeerMessage: PeerUserMessage,
  Find: ThreadFind,
};

const EMBEDDED_THREAD_COMPONENTS: ThreadComponents = { ...THREAD_COMPONENTS, Find: undefined };
