import { create } from "zustand";

import type {
  AccountsView,
  AppInfo,
  AttachmentRef,
  Conversation,
  DaemonInfo,
  DaemonMetrics,
  Diagnostics,
  EnvironmentKind,
  EventEnvelope,
  Message,
  ModelChoice,
  PermissionLevel,
  Project,
  ProvidersView,
  RawEntry,
  RawSession,
  Settings,
} from "@/ipc/generated";
import { applyDensity, cachedDensity } from "@/lib/density";
import { cachedPinnedSummary } from "@/lib/pinnedSummary";
import { confirmPending } from "@/state/shownIds";

/** A page of Settings. */
export type SettingsPageId =
  | "general"
  | "conversations"
  | "personalization"
  | "usage"
  | "providers"
  | "accounts"
  | "routing"
  | "git"
  | "computerUse"
  | "storage"
  | "inspector"
  | "archived";

/**
 * What the main area shows. Drafts become real conversations on their first message. Settings
 * takes the sidebar panel (its navigation) and the main area (the page).
 */
export type Selection =
  | { type: "none" }
  | { type: "settings"; page: SettingsPageId }
  | { type: "conversation"; id: string }
  | { type: "draft"; kind: "chat" }
  | { type: "draft"; kind: "session"; projectId: string };

/**
 * The composer's choices for a conversation that does not exist yet. `null` fields follow
 * the resolution order: the project's remembered choice, then the global default.
 */
export type DraftSetup = {
  /** The project these choices were made for; they reset when the project changes. */
  projectId: string | null;
  environment: EnvironmentKind | null;
  /** Local checkout: the branch commits land on. Absent: the checked-out branch. */
  branch: string | null;
  /** Local checkout, "New branch…": the branch to create from `branch`. */
  newBranch: string | null;
  /** New worktree: the base branch. Absent: the checked-out branch. */
  base: string | null;
  /** New worktree: the session branch's name. Empty: Brigadier names it. */
  sessionBranch: string;
  permission: PermissionLevel | null;
  model: ModelChoice | null;
  /** The session starts in plan mode. */
  planMode: boolean;
};

export function emptyDraft(projectId: string | null): DraftSetup {
  return {
    projectId,
    environment: null,
    branch: null,
    newBranch: null,
    base: null,
    sessionBranch: "",
    permission: null,
    model: null,
    planMode: false,
  };
}

export type ConnectionState = {
  status: "connecting" | "connected" | "disconnected";
  daemon: DaemonInfo | null;
  reason: string | null;
};

/** A sent message not yet confirmed by the daemon. */
export type PendingMessage = {
  localId: string;
  conversationId: string;
  text: string;
  attachments: AttachmentRef[];
  createdAtMs: number;
  /** Queue items already seen before this send could be admitted. */
  queuedIds?: readonly string[];
};

export { shownIdOf, storedIdOf } from "@/state/shownIds";

/**
 * Confirms a pending message from the daemon's answer to sending it, unless its event came
 * first and the thread already shows it (then under its stored id, if the event did not match
 * it to this pending message).
 */
export function confirmSent(conversationId: string, message: Message, localId: string): void {
  const shown = useApp.getState().threads[conversationId]?.items.some((item) => item.id === message.id);
  if (!shown) confirmPending(message.id, localId);
}

export type Thread = {
  items: Message[];
  hasMore: boolean;
  loading: boolean;
  /** Full text of blob-backed messages, by message id. */
  fullText: Record<string, string>;
};

export type InspectorTab =
  | "events"
  | "orchestrator"
  | "brain"
  | "processes"
  | "performance"
  | "providers"
  | "routing";

/** Newest inspector events kept in memory. */
export const INSPECTOR_EVENTS = 500;

/** Newest entries kept per open raw-session transcript; older ones load on demand. */
export const RAW_ENTRIES = 5_000;

/** The loaded part of a raw session's transcript, oldest first. */
export type RawTranscript = {
  entries: RawEntry[];
  hasMore: boolean;
  loading: boolean;
};

/** The Inspector's Providers tab: provider overviews, raw sessions and their transcripts. */
export type ProvidersState = {
  /** Absent until first loaded. */
  view: ProvidersView | null;
  /** The raw session shown, if any. */
  selected: string | null;
  /** Transcripts of the raw sessions opened so far, by session id. */
  transcripts: Record<string, RawTranscript>;
};

export type AppState = {
  info: AppInfo | null;
  connection: ConnectionState;
  catalogLoaded: boolean;
  projects: Record<string, Project>;
  conversations: Record<string, Conversation>;
  settings: Settings;
  threads: Record<string, Thread>;
  pending: PendingMessage[];
  selection: Selection;
  draft: DraftSetup;
  expandedProjects: Record<string, boolean>;
  windowVisible: boolean;
  coldStartMs: number | null;
  /** The summary card pinned at the top end of a session's thread is shown: one pin for every
      conversation, kept across launches. */
  pinnedSummary: boolean;
  inspector: {
    tab: InspectorTab;
    /** Newest first. */
    events: EventEnvelope[];
    metrics: DaemonMetrics | null;
    diagnostics: Diagnostics | null;
  };
  providers: ProvidersState;
  /** Settings → Accounts, once read (`getAccounts`), kept current by `accountsChecked`. */
  accounts: AccountsView | null;
  /** Counts `rankingsChanged` events: views showing model ratings read them again on a change. */
  rankingsRevision: number;
};

export const useApp = create<AppState>()(() => ({
  info: null,
  connection: { status: "connecting", daemon: null, reason: null },
  catalogLoaded: false,
  projects: {},
  conversations: {},
  settings: {
    density: cachedDensity(),
    defaultPermission: "fullAccess",
    defaultOrchestrator: null,
    defaultChatModel: null,
    hibernateAfterMinutes: 30,
    showContextUsage: true,
    showFullAccessNotice: true,
    enrichBrain: true,
    shortReplies: true,
    onboarded: false,
    keepAwake: "agents",
    keepAwakeLidClosed: false,
    routingOverrides: [],
    routingRankings: [],
    disabledProviders: [],
    hiddenModels: [],
    knownModels: [],
    omitAiCoauthors: true,
    accounts: [],
    switchAccounts: true,
    settingsVersion: 1,
  },
  threads: {},
  pending: [],
  selection: { type: "draft", kind: "chat" },
  draft: emptyDraft(null),
  expandedProjects: {},
  windowVisible: true,
  coldStartMs: null,
  pinnedSummary: cachedPinnedSummary(),
  inspector: {
    tab: "events",
    events: [],
    metrics: null,
    diagnostics: null,
  },
  providers: { view: null, selected: null, transcripts: {} },
  accounts: null,
  rankingsRevision: 0,
}));

export const emptyThread: Thread = {
  items: [],
  hasMore: false,
  loading: false,
  fullText: {},
};

function byId<T extends { id: string }>(items: readonly T[]): Record<string, T> {
  return Object.fromEntries(items.map((item) => [item.id, item]));
}

/** A change to the settings, made on the latest ones (see `editSettings`). */
export type SettingsEdit = (settings: Settings) => Settings;

/**
 * The settings as the daemon last reported them, and the edits made since that it has not
 * answered yet, oldest first. What the app shows is those edits applied on top, so a newer
 * edit is never undone by an older answer or a `settingsChanged` event arriving meanwhile.
 */
let confirmedSettings: Settings | null = null;
const pendingEdits: SettingsEdit[] = [];

/** The settings to show: the confirmed ones with every pending edit applied, in order. */
function shownSettings(): Settings {
  const base = confirmedSettings ?? useApp.getState().settings;
  return pendingEdits.reduce((settings, edit) => edit(settings), base);
}

/** Shows `shownSettings()`, switching the density tokens with it. */
function showSettings(): Settings {
  const settings = shownSettings();
  applyDensity(settings.density);
  useApp.setState({ settings });
  return settings;
}

/** Records settings the daemon reported (a load, a write's answer or an event). */
function confirmSettings(settings: Settings): Settings {
  confirmedSettings = settings;
  const shown = pendingEdits.reduce((current, edit) => edit(current), settings);
  applyDensity(shown.density);
  return shown;
}

/** Starts showing `edit`; the settings writer sends it in its turn (state/settings.ts). */
export function beginSettingsEdit(edit: SettingsEdit): void {
  // Before the first load the shown settings are the base; keep them unedited, so a failed
  // edit goes back to them.
  confirmedSettings ??= useApp.getState().settings;
  pendingEdits.push(edit);
  showSettings();
}

/** The settings `edit` should be sent as: the confirmed ones with the edits up to it applied. */
export function settingsThrough(edit: SettingsEdit): Settings {
  const upTo = pendingEdits.indexOf(edit);
  const base = confirmedSettings ?? useApp.getState().settings;
  return pendingEdits.slice(0, upTo + 1).reduce((settings, next) => next(settings), base);
}

/** Ends `edit`: saved (`saved` is the daemon's answer) or failed (`saved` is null). */
export function endSettingsEdit(edit: SettingsEdit, saved: Settings | null): Settings {
  const at = pendingEdits.indexOf(edit);
  if (at !== -1) pendingEdits.splice(at, 1);
  if (saved) confirmedSettings = saved;
  return showSettings();
}

export function replaceCatalog(
  projects: readonly Project[],
  conversations: readonly Conversation[],
  settings: Settings,
): void {
  useApp.setState({
    catalogLoaded: true,
    projects: byId(projects),
    conversations: byId(conversations),
    settings: confirmSettings(settings),
  });
}

/** Inserts messages into a thread, ordered by seq, without duplicates. */
export function mergeMessages(
  existing: readonly Message[],
  incoming: readonly Message[],
): Message[] {
  const known = new Set(existing.map((message) => message.id));
  const added = incoming.filter((message) => !known.has(message.id));
  if (added.length === 0) return existing as Message[];
  return [...existing, ...added].toSorted((a, b) => a.seq - b.seq);
}

/**
 * Folds a batch of committed events into the state in one update, so a burst of events
 * costs one render.
 */
export function applyEvents(envelopes: readonly EventEnvelope[]): void {
  if (envelopes.length === 0) return;
  useApp.setState((state) => {
    let { projects, conversations, threads, pending, settings, providers, accounts, rankingsRevision } =
      state;
    for (const envelope of envelopes) {
      ({ projects, conversations, threads, pending, settings, providers, accounts, rankingsRevision } =
        applyEvent(envelope, {
          projects,
          conversations,
          threads,
          pending,
          settings,
          providers,
          accounts,
          rankingsRevision,
        }));
    }
    const newest = envelopes.toReversed();
    const events = [...newest, ...state.inspector.events].slice(
      0,
      INSPECTOR_EVENTS,
    );
    return {
      projects,
      conversations,
      threads,
      pending,
      settings,
      providers,
      accounts,
      rankingsRevision,
      inspector: { ...state.inspector, events },
    };
  });
}

type Slice = Pick<
  AppState,
  | "projects"
  | "conversations"
  | "threads"
  | "pending"
  | "settings"
  | "providers"
  | "accounts"
  | "rankingsRevision"
>;

/** Adds or replaces a raw session in the list, newest first. */
export function upsertRawSession(
  providers: ProvidersState,
  session: RawSession,
): ProvidersState {
  const view = providers.view;
  if (!view) return providers;
  const index = view.sessions.findIndex((existing) => existing.id === session.id);
  const sessions =
    index === -1
      ? [session, ...view.sessions]
      : view.sessions.map((existing, i) => (i === index ? session : existing));
  return { ...providers, view: { ...view, sessions } };
}

function applyProviderEvent(
  { event, streamSeq, atMs }: EventEnvelope,
  providers: ProvidersState,
): ProvidersState {
  switch (event.type) {
    case "providerChecked": {
      const view = providers.view;
      if (!view) return providers;
      const others = view.providers.filter(
        (overview) => overview.provider !== event.overview.provider,
      );
      const order = ["claude", "codex"];
      const overviews = [...others, event.overview].toSorted(
        (a, b) => order.indexOf(a.provider) - order.indexOf(b.provider),
      );
      return { ...providers, view: { ...view, providers: overviews } };
    }
    case "rawSessionCreated":
      return upsertRawSession(providers, event.session);
    case "rawSessionUpdated": {
      const current = providers.view?.sessions.find((session) => session.id === event.id);
      if (!current) return providers;
      return upsertRawSession(providers, {
        ...current,
        state: event.state,
        nativeId: event.nativeId ?? current.nativeId,
        error: event.error,
        updatedAtMs: atMs,
      });
    }
    case "rawEvent": {
      const transcript = providers.transcripts[event.sessionId];
      const last = transcript?.entries.at(-1);
      if (!transcript || (last && last.streamSeq >= streamSeq)) return providers;
      const entries = [...transcript.entries, { streamSeq, atMs, event: event.event }];
      const trimmed = entries.length > RAW_ENTRIES;
      return {
        ...providers,
        transcripts: {
          ...providers.transcripts,
          [event.sessionId]: {
            ...transcript,
            entries: trimmed ? entries.slice(-RAW_ENTRIES) : entries,
            hasMore: transcript.hasMore || trimmed,
          },
        },
      };
    }
    default:
      return providers;
  }
}

function applyEvent(envelope: EventEnvelope, slice: Slice): Slice {
  const { event, streamSeq } = envelope;
  switch (event.type) {
    case "projectCreated":
      return {
        ...slice,
        projects: { ...slice.projects, [event.project.id]: event.project },
      };
    case "conversationCreated":
      return {
        ...slice,
        conversations: {
          ...slice.conversations,
          [event.conversation.id]: event.conversation,
        },
      };
    case "conversationRenamed":
    case "conversationPinned":
    case "conversationFallback":
    case "conversationWaiting": {
      const current = slice.conversations[event.id];
      if (!current) return slice;
      const next =
        event.type === "conversationRenamed"
          ? { ...current, title: event.title }
          : event.type === "conversationPinned"
            ? { ...current, pinnedAtMs: event.pinnedAtMs }
            : event.type === "conversationFallback"
              ? { ...current, fallback: event.fallback }
              : { ...current, quotaWait: event.wait };
      return {
        ...slice,
        conversations: { ...slice.conversations, [event.id]: next },
      };
    }
    case "queueChanged": {
      let pending = slice.pending;
      for (const item of event.queue.items) {
        const echo = pending.findIndex(
          (entry) =>
            entry.conversationId === event.conversationId &&
            !entry.queuedIds?.includes(item.id) &&
            entry.text === item.text &&
            entry.attachments.length === item.attachments.length &&
            entry.attachments.every((attachment, index) => {
              const queued = item.attachments[index];
              return attachment.id === queued?.id &&
                attachment.inline === queued.inline && attachment.pasted === queued.pasted;
            }),
        );
        if (echo !== -1) pending = pending.filter((_, index) => index !== echo);
      }
      // Replayed queue snapshots must not consume another identical send's echo.
      const queuedIds = event.queue.items.map((item) => item.id);
      return {
        ...slice,
        pending: pending.map((entry) =>
          entry.conversationId === event.conversationId
            ? { ...entry, queuedIds: [...new Set([...(entry.queuedIds ?? []), ...queuedIds])] }
            : entry,
        ),
      };
    }
    case "messageAppended": {
      // The store assigns a message's position: its sequence in the conversation stream.
      const message = { ...event.message, seq: streamSeq };
      const id = message.conversationId;
      let { conversations, threads } = slice;
      const conversation = conversations[id];
      if (conversation && conversation.updatedAtMs < message.createdAtMs) {
        conversations = {
          ...conversations,
          [id]: { ...conversation, updatedAtMs: message.createdAtMs },
        };
      }
      const thread = threads[id];
      if (thread) {
        const items = mergeMessages(thread.items, [message]);
        if (items !== thread.items) {
          threads = { ...threads, [id]: { ...thread, items } };
        }
      }
      // The confirmed message replaces its optimistic copy (the event can beat the response).
      const echo = slice.pending.findIndex(
        (entry) =>
          message.role === "user" && entry.conversationId === id && entry.text === message.text,
      );
      if (echo !== -1) confirmPending(message.id, (slice.pending[echo] as PendingMessage).localId);
      const pending =
        echo === -1
          ? slice.pending
          : slice.pending.filter((_, index) => index !== echo);
      return { ...slice, conversations, threads, pending };
    }
    case "projectUpdated":
      return {
        ...slice,
        projects: { ...slice.projects, [event.project.id]: event.project },
      };
    case "conversationSetUp":
    case "conversationLifecycleChanged": {
      const current = slice.conversations[event.id];
      if (!current) return slice;
      const next =
        event.type === "conversationSetUp"
          ? { ...current, setup: event.setup }
          : { ...current, lifecycle: event.lifecycle };
      return {
        ...slice,
        conversations: { ...slice.conversations, [event.id]: next },
      };
    }
    // Being deleted is gone already, for the user.
    case "conversationDeleting":
    case "conversationDeleted": {
      if (!slice.conversations[event.id]) return slice;
      const { [event.id]: _deleted, ...conversations } = slice.conversations;
      return { ...slice, conversations };
    }
    case "projectRemoved": {
      if (!slice.projects[event.id]) return slice;
      const { [event.id]: _removed, ...projects } = slice.projects;
      return { ...slice, projects };
    }
    case "settingsChanged":
      return { ...slice, settings: confirmSettings(event.settings) };
    case "rawSessionCreated":
    case "rawSessionUpdated":
    case "rawEvent":
    case "providerChecked":
      return { ...slice, providers: applyProviderEvent(envelope, slice.providers) };
    case "accountsChecked":
      return { ...slice, accounts: event.accounts };
    // A refresh ended, the rankings were reset or a newer registry was installed: the ratings,
    // route previews and refresh state are read again (getUsage in state/usage).
    case "rankingsChanged":
      return { ...slice, rankingsRevision: slice.rankingsRevision + 1 };
    case "probe":
    // A draft's pinned attachments only matter to blob collection.
    case "draftPinned":
    // The cleanup ledger shows in the event list only.
    case "cleanupRecorded":
    case "cleanupRemoved":
    case "branchesKept":
    case "cleanupRequested":
    case "cleanupCompleted":
    case "conversationCleanup":
    // The daemon's own record of the thread engine's first start; its deletes arrive as
    // conversationDeleting.
    case "engineSwitching":
    case "engineSwitched":
    // Conversation views (tasks, cards, queue, streaming) and the Inspector's orchestrator
    // log read these themselves.
    case "thinkingDelta":
    case "messageDelta":
    case "runStateChanged":
    case "requestUpdated":
    case "branchSwitched":
    case "workerStepped":
    case "orchestratorStepped":
    case "machineStepped":
    case "compactionUpdated":
    case "messageRated":
    case "conversationNotice":
    case "taskUpdated":
    case "approvalUpdated":
    case "questionUpdated":
    case "planUpdated":
    case "reviewUpdated":
    case "threadCommitsSeen":
    case "outputStored":
    case "computerActed":
    case "checkRan":
    case "threadLooked":
    case "previewUpdated":
    case "overnightUpdated":
    case "workerEvent":
    case "orchestratorLogged":
    // A Chat's Memory chips live on its board; Brain jobs in the Inspector's Brain tab.
    case "memoryUpdated":
    case "brainJobUpdated":
    // "Decided for you" and "Waiting on you" live on the board.
    case "decidedForYou":
    case "waitingOnYou":
    case "waitingResolved":
    // The rail's update button reads these (state/updates).
    case "updatesChanged":
      return slice;
  }
}

/** Resolves the conversation a selection points at, if any. */
export function selectedConversation(state: AppState): Conversation | null {
  return state.selection.type === "conversation"
    ? (state.conversations[state.selection.id] ?? null)
    : null;
}
