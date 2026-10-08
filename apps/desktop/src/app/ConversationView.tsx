import {
  type AppendMessage,
  AssistantRuntimeProvider,
  type ExternalStoreBranchChange,
  ExportedMessageRepository,
  type ExternalThreadQueueAdapter,
  type FeedbackAdapter,
  type QueueItemState,
  type ThreadMessage,
  type ThreadMessageLike,
  useAuiState,
  useExternalStoreRuntime,
} from "@assistant-ui/react";
import { Unarchive, X } from "@openai/apps-sdk-ui/components/Icon";
import {
  type FC,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";
import { useShallow } from "zustand/react/shallow";

import { PaneComposer, FloatingComposerSlot } from "@/app/conversation/PaneComposer";
import { TerminalPane } from "@/app/conversation/TerminalTab";
import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import { ChatActions, RenameDialog } from "@/app/conversation/ChatActions";
import { PinnedSummary, PinnedSummaryToggle, SummaryFloat, SummaryPane } from "@/app/conversation/PinnedSummary";
import { SessionTabBar, SessionTabViews } from "@/app/conversation/SessionTabBar";
import { WorkerDiffs } from "@/app/conversation/WorkerSummary";
import { ProjectCombobox } from "@/app/conversation/RailPickers";
import {
  SidePanel,
  PanelButtons,
  PanelButtonsRoom,
  SidePanelContext,
  useSidePanel,
} from "@/app/conversation/SidePanel";
import { queuedImageRefs, reconcileImages } from "@/lib/inlineImages";
import { BlobAttachmentAdapter } from "@/app/conversation/attachments";
import {
  type Block,
  type BoardDigest,
  buildThread,
  type ThreadNode,
} from "@/app/conversation/blocks";
import {
  type ComposerTarget,
  ComposerTargetContext,
  PulledSlot,
} from "@/app/conversation/composerTarget";
import { useResolvedDraft } from "@/app/conversation/draftSetup";
import { workerName } from "@/app/conversation/rowWords";
import { type BlockMeta, RequestBlock } from "@/app/conversation/RequestBlock";
import { StatusCardContext } from "@/app/conversation/StatusCard";
import { ComposerCapsule } from "@/app/conversation/ComposerCapsule";
import { useAction } from "@/app/conversation/useAction";
import { ViewContext } from "@/app/conversation/viewContext";
import {
  MentionMemory,
  mentionsIn,
  type MentionTarget,
  UserMessageText,
} from "@/app/conversation/Mentions";
import { TopBar } from "@/app/TopBar";
import { MessageAttachments } from "@/components/assistant-ui/elements/message-attachment";
import {
  type AttachmentReader,
  AttachmentReaderContext,
} from "@/components/assistant-ui/elements/attachment-tile";
import { OpenFileContext } from "@/components/assistant-ui/markdown-text";
import { Thread, type ThreadComponents } from "@/components/assistant-ui/thread";
import { BrigadierGlyph } from "@/components/glyphs/brand-glyph";
import { Button } from "@/components/ui/button";
import type {
  AttachmentRef,
  Mention,
  ModelChoice,
  Notice,
  Rating,
  UserRequest,
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
  restore,
  restoreQueued,
  resume,
  send,
  type SendLane,
  steerQueued,
  switchBranch,
} from "@/state/actions";
import { toast } from "@/state/toasts";
import { type Board, useBoard } from "@/state/board";
import { CHAT_TAB, openFileTab, selectTab, useSessionTabsOf } from "@/state/sessionTabs";
import { placeOf, setTerminalCover } from "@/state/terminalPlaces";
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
type Custom = {
  attachments?: AttachmentRef[];
  block?: BlockMeta;
  /** The message can be edited now (see `canRework`). */
  rework?: boolean;
  /** What a user message @-mentions, so ↑ recalls its chips. */
  mentions?: Mention[];
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
    block.quotaWait,
    block.texts.map((text) => [text.messageId, text.position, text.model]),
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
  nodes: readonly ThreadNode[],
  picked: ModelChoice | null,
  session: boolean,
  canRework: (requestId: string) => boolean,
  ratings: Partial<Record<string, Rating>>,
): Item[] {
  const [cache] = useState(() => new Map<string, { item: Item; sig: string; texts: string[] }>());
  return useMemo(() => {
    const items: Item[] = [];
    const seen = new Set<string>();
    for (const node of nodes) {
      seen.add(node.id);
      const { block } = node;
      const entry = cache.get(node.id);
      // A block that joined steered requests answers the last of them, not its own message.
      const rework =
        block.user?.kind === "message" &&
        canRework(block.key) &&
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
          });
        }
        items.push((cache.get(node.id) as { item: Item }).item);
        continue;
      }
      const answerId = block.texts.at(-1)?.messageId ?? null;
      const rating = (answerId && ratings[answerId]) || null;
      const sig = signature(block, picked, session, rework, rating);
      const texts = block.texts.map((text) => text.text);
      const changed =
        entry === undefined ||
        entry.item.parentId !== node.parentId ||
        entry.sig !== sig ||
        entry.texts.length !== texts.length ||
        entry.texts.some((text, index) => text !== texts[index]);
      if (changed) {
        const meta: BlockMeta = {
          texts: block.texts.map((text) => ({ position: text.position, model: text.model })),
          cards: block.cards,
          rows: block.rows,
          orchestratorSteps: block.orchestratorSteps,
          thinking: block.thinking,
          compactions: block.compactions,
          steers: block.steers.map((steer) => ({
            position: steer.position,
            text: steer.text,
            atMs: steer.message.createdAtMs,
            attachments: steer.message.attachments,
          })),
          state: block.state,
          startedAtMs: block.startedAtMs,
          endedAtMs: block.endedAtMs,
          worked: block.worked,
          quotaWait: block.quotaWait,
          picked,
          session,
          rework,
          requestId: block.key,
          requestIds: block.requestIds,
          answerId,
        };
        cache.set(node.id, {
          item: { id: node.id, parentId: node.parentId, kind: "block", block, meta, texts, rating },
          sig,
          texts,
        });
      }
      items.push((cache.get(node.id) as { item: Item }).item);
    }
    for (const key of cache.keys()) if (!seen.has(key)) cache.delete(key);
    return items;
  }, [nodes, picked, session, canRework, ratings, cache]);
}

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

function textOf(message: AppendMessage): string {
  return message.content
    .map((part) => (part.type === "text" ? part.text : ""))
    .join("")
    .trim();
}

const NO_PENDING: PendingMessage[] = [];
const NO_RATINGS: Partial<Record<string, Rating>> = {};

const EMPTY_DIGEST: BoardDigest & { head: string | null } = {
  tasks: {},
  approvals: {},
  questions: {},
  plans: {},
  requests: {},
  orchestratorSteps: [],
  machineSteps: [],
  decisions: [],
  compactions: {},
  runRequest: null,
  streaming: null,
  head: null,
};

/** The conversation's newest request (the daemon orders them the same way). */
function latestRequest(board: Board): UserRequest | null {
  let latest: UserRequest | null = null;
  for (const request of Object.values(board.requests)) {
    if (
      !latest ||
      request.startedAtMs > latest.startedAtMs ||
      (request.startedAtMs === latest.startedAtMs && request.id > latest.id)
    ) {
      latest = request;
    }
  }
  return latest;
}

/**
 * The session request the user may still edit or have answered again: the latest, until
 * anything it started has landed (or is landing with their approval).
 */
function reworkableRequest(board: Board): string | null {
  const latest = latestRequest(board);
  if (!latest) return null;
  const id = latest.id;
  const landed =
    Object.values(board.tasks).some((task) => task.requestId === id && task.state === "landed") ||
    Object.values(board.approvals).some(
      (approval) =>
        approval.requestId === id &&
        approval.state.type === "allowed" &&
        approval.subject.type === "landing",
    );
  return landed ? null : id;
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
        ? s.pending.filter((entry) => entry.conversationId === conversationId)
        : NO_PENDING,
    ),
  );
  const digest = useBoard(
    useShallow((s): (BoardDigest & { head: string | null }) | null =>
      s.board?.conversationId === conversationId
        ? {
            tasks: s.board.tasks,
            workerSteps: s.board.workerSteps,
            approvals: s.board.approvals,
            questions: s.board.questions,
            plans: s.board.plans,
            requests: s.board.requests,
            orchestratorSteps: s.board.orchestratorSteps,
            thinking: s.board.thinking,
            machineSteps: s.board.machineSteps,
            decisions: s.board.decisions,
            compactions: s.board.compactions,
            runRequest: s.board.runRequest,
            streaming: s.board.streaming,
            head: s.board.head,
          }
        : null,
    ),
  );
  const run = useBoard((s) =>
    s.board?.conversationId === conversationId ? s.board.run : "idle",
  );
  const queueItems = useBoard((s) =>
    s.board?.conversationId === conversationId ? s.board.queue.items : null,
  );
  const targets = useMentionTargets(conversationId);
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
  const tree = useMemo(
    () =>
      buildThread(
        thread.items,
        thread.fullText,
        thread.hasMore,
        digest ?? EMPTY_DIGEST,
        pending,
        session ? "edits" : "all",
      ),
    [thread.items, thread.fullText, thread.hasMore, digest, pending, session],
  );
  const setup = conversation?.setup;
  const picked =
    setup?.type === "chat" ? setup.model : setup?.type === "session" ? setup.orchestrator : null;
  const running = run === "running" || run === "starting";
  // Messages waiting for quota: nothing runs, but Stop (and Esc) drops them.
  const waitingForQuota = conversation?.quotaWait != null;
  const reworkable = useBoard((s) =>
    s.board?.conversationId === conversationId && s.board ? reworkableRequest(s.board) : null,
  );
  // The user stopped the latest request and nothing runs for it: the send button resumes it.
  const stopped = useBoard(
    (s) =>
      s.board?.conversationId === conversationId &&
      !!s.board &&
      latestRequest(s.board)?.state.type === "stopped",
  );
  // Not while the orchestrator's turn for the request runs (the composer can stop it); a
  // request whose workers still run can be redone.
  const runRequest = digest?.runRequest ?? null;
  const canRework = useCallback(
    (requestId: string) =>
      session ? requestId === reworkable && !(running && runRequest === requestId) : !running,
    [session, reworkable, running, runRequest],
  );
  const ratings = useBoard((s) =>
    s.board?.conversationId === conversationId ? s.board.ratings : NO_RATINGS,
  );
  const items = useItems(tree.nodes, picked, session, canRework, ratings);
  const repository = useMemo<ExportedMessageRepository>(
    () => ({
      headId: tree.headId,
      messages: items.map((item) => ({ message: toThreadMessage(item), parentId: item.parentId })),
    }),
    [items, tree.headId],
  );
  // What each item's branch ends at, for the branch picker.
  const heads = useMemo(
    () => new Map(tree.nodes.map((node) => [node.id, node.head])),
    [tree.nodes],
  );
  const { panel: sidePanel, agents } = useSidePanel(
    conversationId,
    embedded || conversation?.sideOf
      ? "sideChat"
      : session
        ? "session"
        : conversation
          ? "chat"
          : null,
  );
  const [renaming, setRenaming] = useState(false);

  // A new scope must not inherit uploads or refs from the previous composer.
  // oxlint-disable-next-line react-hooks/exhaustive-deps, react/memo-dependencies
  const attachments = useMemo(() => new BlobAttachmentAdapter(), [conversationId]);
  const reader = useMemo<AttachmentReader>(() => ({
    read: readAttachment,
    composerRef: (id) => attachments.refOf(id),
  }), [attachments]);
  const [mentions] = useState(() => new MentionMemory());
  const [pulled] = useState(() => new PulledSlot());
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
  const submit = useCallback(
    (message: AppendMessage, lane: SendLane) => {
      const text = textOf(message);
      const refs = attachments.messageRefs(text, message.attachments ?? []);
      // A queued message pulled out to edit goes back to its slot, unless steered in now.
      const slot = pulled.take(conversationId);
      if (!text && refs.length === 0) return;
      setError(null);
      const known = [...mentions.known(), ...(slot?.mentions ?? [])];
      // An edited queued message goes back to its slot; only an explicit steer sends it in now.
      const into = slot && lane === "auto" ? "queue" : lane;
      send(
        { text, attachments: refs, mentions: mentionsIn(text, targets, known) },
        draftTarget ?? undefined,
        {
          lane: into,
          queueIndex: into === "steer" ? null : (slot?.index ?? null),
          to: conversationId,
        },
      ).catch(fail);
    },
    [attachments, pulled, conversationId, targets, mentions, draftTarget, fail],
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
    return {
      items: states,
      steerItems: [],
      enqueue: (message) => submit(message, message.steer === false ? "queue" : "auto"),
      steer: (message) => submit(message, message.steer ? "steer" : "auto"),
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
          mentions: mentionsIn(text, targets, [...mentions.known(), ...(item?.mentions ?? [])]),
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
    isLoading: thread.loading && thread.items.length === 0,
    isRunning: running || waitingForQuota || Object.values(digest?.tasks ?? {}).some((task) => ["queued", "starting", "running", "blocked", "paused", "landing", "readyToLand"].includes(task.state)),
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
      mentions,
      running,
      onResume,
      queue: { pulled, attachments },
    }),
    [conversation, resolved, targets, mentions, running, onResume, pulled, attachments],
  );

  const fullscreen = sidePanel.visible && sidePanel.state.fullscreen;
  // Full view hides the thread column and its terminal; Terminal and ⌘J show it again.
  const { setFullscreen } = sidePanel;
  useEffect(() => {
    if (embedded) return;
    setTerminalCover(fullscreen ? () => setFullscreen(false) : null);
    return () => setTerminalCover(null);
  }, [embedded, fullscreen, setFullscreen]);
  // The pinned summary, in a session's own view.
  const summary = setup?.type === "session" && !embedded;
  // A session's own view has tabs; another tab in front covers the conversation (kept as it was).
  const tabbed = !embedded && conversation?.kind === "session";
  const { active: activeTab } = useSessionTabsOf(tabbed ? conversationId : null);
  const covered = tabbed && activeTab !== CHAT_TAB;
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
    <ViewContext.Provider value={{ selection, conversation, embedded }}>
      <ComposerTargetContext.Provider value={target}>
        <SidePanelContext.Provider value={sidePanel}>
          <AgentsPanelContext.Provider value={agents}>
            <StatusCardContext.Provider value={statusCard}>
              <AssistantRuntimeProvider runtime={runtime}>
                <AttachmentReaderContext.Provider value={reader}>
                  <OpenFileContext.Provider value={openFileAt}>
                    <div
                      ref={embedded ? undefined : sidePanel.workspace}
                      data-embedded-view={embedded || undefined}
                      data-slot="pane-workspace"
                      className="relative flex h-full min-h-0 flex-col"
                    >
                      <div className="relative flex min-h-0 flex-1">
                      <div className={cn("flex h-full min-w-0 flex-1 flex-col", fullscreen && "hidden")}>
                        {/* The summary's popover, where it floats: opened in the top bar, under it. */}
                        <SummaryFloat>
                          {tabbed && conversation ? (
                            <SessionTabBar conversation={conversation} onRename={() => setRenaming(true)}>
                              {summary && (
                                // The summary shows beside the conversation: it comes to the front.
                                <span
                                  className="contents"
                                  onClickCapture={() => conversationId && selectTab(conversationId, CHAT_TAB)}
                                >
                                  <PinnedSummaryToggle />
                                </span>
                              )}
                              <PanelButtonsRoom besidePanel />
                            </SessionTabBar>
                          ) : (
                            !embedded && (
                              <TopBar onRename={conversation && !archived ? () => setRenaming(true) : undefined}>
                                {conversation && (
                                  <ChatActions conversation={conversation} onRename={() => setRenaming(true)} />
                                )}
                                {conversation && summary && <PinnedSummaryToggle />}
                                <PanelButtonsRoom besidePanel />
                              </TopBar>
                            )
                          )}
                          {error && (
                            <p
                              role="alert"
                              className="bg-destructive/10 text-destructive border-destructive/20 flex items-center gap-2 border-b px-4 py-2 text-sm"
                            >
                              <span className="min-w-0 flex-1">{error}</span>
                              <Button
                                variant="ghost"
                                size="icon-sm"
                                aria-label="Dismiss"
                                onClick={() => setError(null)}
                              >
                                <X />
                              </Button>
                            </p>
                          )}
                          <div className="relative flex min-h-0 flex-1 flex-col">
                            {/* Its own stack, under the tabs: the composer's blurred layers stay under them too. */}
                            <div
                              inert={covered}
                              className={cn("isolate flex min-h-0 flex-1 flex-col", covered && "invisible")}
                            >
                              <SummaryPane summary={summary}>
                                {conversation && summary && <PinnedSummary conversation={conversation} />}
                                {conversation?.kind === "session" && <WorkerDiffs conversationId={conversation.id} />}
                                <Thread
                                  components={THREAD_COMPONENTS}
                                  // One placeholder, in every conversation.
                                  placeholder="Do anything"
                                  // A session's work comes before its answer, and is followed once it
                                  // reaches the composer; a Chat's answer fills the room made for it.
                                  scrollMode={conversation?.kind === "chat" ? "chat" : "session"}
                                  scrollKey={conversation?.id}
                                />
                              </SummaryPane>
                            </div>
                            {tabbed && conversationId && <SessionTabViews conversationId={conversationId} />}
                          </div>
                        </SummaryFloat>
                        {!embedded && <TerminalPane place={placeOf(selection)} />}
                      </div>
                      {!embedded && <SidePanel conversationId={conversationId} />}
                      {!embedded && <PanelButtons />}
                      {!embedded && <FloatingComposerSlot />}
                      </div>
                    </div>
                  </OpenFileContext.Provider>
                  {conversation && (
                    <RenameDialog
                      conversation={conversation}
                      open={renaming}
                      onOpenChange={setRenaming}
                    />
                  )}
                </AttachmentReaderContext.Provider>
              </AssistantRuntimeProvider>
            </StatusCardContext.Provider>
          </AgentsPanelContext.Provider>
        </SidePanelContext.Provider>
      </ComposerTargetContext.Provider>
    </ViewContext.Provider>
  );
}

/** The hero over a new chat: a faint mark over "What should we build in {project}?".
 * A side chat says what it is for instead. */
const Welcome: FC = () => {
  const { selection, embedded } = useContext(ViewContext);
  const project = useApp((s) =>
    selection.type === "draft" && selection.kind === "session"
      ? (s.projects[selection.projectId] ?? null)
      : null,
  );
  if (embedded) {
    return (
      <p className="text-muted-foreground px-2 text-center text-sm">
        Ask about this conversation without adding to it
      </p>
    );
  }
  return (
    <div className="flex flex-col items-center gap-6 select-none">
      <BrigadierGlyph
        spinOnHover
        className="text-foreground size-16 opacity-60 transition-opacity duration-300 hover:opacity-100 motion-reduce:transition-none"
      />
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

const LoadEarlier: FC = () => {
  const { selection } = useContext(ViewContext);
  const conversationId = selection.type === "conversation" ? selection.id : "";
  const hasMore = useApp((s) => s.threads[conversationId]?.hasMore ?? false);
  const loading = useApp((s) => s.threads[conversationId]?.loading ?? false);
  if (!hasMore) return null;
  return (
    <div className="mb-4 flex justify-center">
      <Button
        variant="ghost"
        size="sm"
        disabled={loading}
        onClick={() => void loadEarlier(conversationId)}
      >
        {loading ? "Loading…" : "Load earlier messages"}
      </Button>
    </div>
  );
};

/** Above a user message's text: its attachments. */
const UserAttachments: FC = () => {
  const role = useAuiState((s) => s.message.role);
  const custom = useAuiState((s) => s.message.metadata.custom) as Custom;
  if (role !== "user" || !custom.attachments || custom.attachments.length === 0) return null;
  return <MessageAttachments attachments={custom.attachments} className="justify-end" />;
};

/** Notices (environment problems, fallbacks), newest last; each can be dismissed. */
function Notices({ notices }: { notices: readonly Notice[] }) {
  const [dismissed, setDismissed] = useState<ReadonlySet<number>>(new Set());
  const shown = notices.filter((notice) => !dismissed.has(notice.atMs)).slice(-3);
  if (shown.length === 0) return null;
  return (
    <ul aria-label="Notices" className="flex flex-col gap-1">
      {shown.map((notice) => (
        <li
          key={`${notice.atMs}:${notice.text}`}
          className={cn(
            "rounded-control flex items-start gap-2 px-3 py-1.5 text-xs",
            notice.level === "warning" ? "bg-warning/10 text-warning" : "bg-muted text-muted-foreground",
          )}
        >
          <span className="min-w-0 flex-1 whitespace-pre-wrap">{notice.text}</span>
          <button
            type="button"
            aria-label="Dismiss notice"
            className="shrink-0 opacity-70 hover:opacity-100"
            onClick={() => setDismissed((current) => new Set(current).add(notice.atMs))}
          >
            <X className="size-icon-xs" />
          </button>
        </li>
      ))}
    </ul>
  );
}

const NO_NOTICES: Notice[] = [];

/** Between the thread and the composer: notices, a failed run, the archived state. */
const AboveComposer: FC = () => {
  const { conversation } = useContext(ViewContext);
  const conversationId = conversation?.id ?? null;
  const notices = useBoard((s) =>
    s.board?.conversationId === conversationId ? s.board.notices : NO_NOTICES,
  );
  const runError = useBoard((s) =>
    s.board?.conversationId === conversationId && s.board.run === "failed"
      ? (s.board.runError ?? "The model stopped with an error.")
      : null,
  );
  const action = useAction();
  const archived = conversation?.lifecycle === "archived";
  // Nothing at all when there is nothing to say, so the footer adds no gap for it.
  if (!conversation || (notices.length === 0 && !runError && !archived)) return null;
  return (
    <div className="flex flex-col gap-1.5">
      <Notices notices={notices} />
      {runError && (
        <p role="alert" className="bg-destructive/10 text-destructive rounded-control px-3 py-1.5 text-xs">
          {runError}
        </p>
      )}
      {archived ? (
        <div className="bg-muted rounded-control flex items-center gap-2 px-3 py-2 text-sm">
          <span className="min-w-0 flex-1">
            Archived. Restore it to continue; the orchestrator restarts from its transcript.
          </span>
          {action.error && (
            <span role="alert" className="text-destructive text-xs">
              {action.error}
            </span>
          )}
          <Button size="sm" disabled={action.busy} onClick={() => action.run(() => restore(conversation.id))}>
            <Unarchive />
            Restore
          </Button>
        </div>
      ) : null}
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
  Composer: PaneComposer,
  UserText: UserMessageText,
};
