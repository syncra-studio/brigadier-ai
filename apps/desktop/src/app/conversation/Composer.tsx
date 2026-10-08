import { ComposerPrimitive, useAui, useAuiState } from "@assistant-ui/react";
import {
  ArrowUp,
  Mic,
  PlayTriangle,
  Stop,
  Warning,
  X,
} from "@openai/apps-sdk-ui/components/Icon";
import {
  type FC,
  lazy,
  type ReactNode,
  Suspense,
  useContext,
  useEffect,
  useRef,
  useState,
} from "react";

import { type ResolvedDraft, updateDraft } from "@/app/conversation/draftSetup";
import { BranchPopover, ProjectCombobox, WorkInMenu } from "@/app/conversation/RailPickers";
import {
  ConversationModelPicker,
  ConversationPermissionPicker,
  openPermissionsHelp,
  PermissionPicker,
  ProjectSettingsButton,
  ProvidersButton,
} from "@/app/conversation/SetupPickers";
import type { ComposerInputProps } from "@/app/conversation/ComposerEditor";
import { type MentionMemory, Mentions } from "@/app/conversation/Mentions";
import { SlashCommands } from "@/app/conversation/SlashCommands";
import {
  COMPOSER_EDITABLE,
  type ComposerTarget,
  ComposerTargetContext,
} from "@/app/conversation/composerTarget";
import {
  ApprovalSlot,
  PendingActionCard,
  setAsideCards,
  useAsideCards,
  usePendingActions,
  WaitingReminder,
} from "@/app/conversation/ActionCards";
import { QueueCard } from "@/app/conversation/QueueCard";
import { shortcutLabel } from "@/app/conversation/SidePanel";
import { useComposerDraft } from "@/app/conversation/composerDraft";
import type { BlobAttachmentAdapter } from "@/app/conversation/attachments";
import { StatusCard, StatusCardContext } from "@/app/conversation/StatusCard";
import { PLAN_PLACEHOLDER, PlanChip, PlusMenu, usePlanMode } from "@/app/conversation/PlusMenu";
import { ViewContext } from "@/app/conversation/viewContext";
import { ComposerRail, ComposerRailItem } from "@/components/assistant-ui/elements/composer-rail";
import { ComposerAttachments } from "@/components/assistant-ui/elements/attachment";
import { ModelSelector } from "@/components/assistant-ui/elements/model-selector";
import { ContextRing } from "@/components/assistant-ui/context-ring";
import { ShieldExclamation } from "@/components/glyphs/permission-glyphs";
import { Spinner } from "@/components/glyphs/spinner";
import { Button } from "@/components/ui/button";
import type { Conversation } from "@/ipc/generated";
import type { ComposerProps } from "@/components/assistant-ui/thread";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { cn } from "@/lib/utils";
import { setSetting } from "@/state/settings";
import { useBoard } from "@/state/board";
import {
  cancelDictation,
  type DictationPhase,
  startDictation,
  stopDictation,
  useDictation,
} from "@/state/dictation";
import { NEW_CHAT_SCOPE } from "@/state/drafts";
import { useApp } from "@/state/store";
import { folderTrust } from "@/state/trust";


const ComposerEditor = lazy(() => import("@/app/conversation/ComposerEditor"));

/**
 * The composer (assistant-ui composer elements, BB parity): attachments, @-mentions of
 * workers, files and conversations, `/` commands, and the setup pickers. A draft picks its
 * project (or none, for a Chat), environment, branch, permission level and model; a started
 * session can still change its model, effort and permission level, a Chat its model.
 */
export const ConversationComposer: FC<ComposerProps> = ({ autoFocus, placeholder }) => {
  const target = useContext(ComposerTargetContext);
  const [modelOpen, setModelOpen] = useState(false);
  const statusCard = useContext(StatusCardContext);
  const pending = usePendingActions(target?.conversation ?? null);
  const plan = usePlanMode(target);
  // Cards put aside with ×, for the conversation they belong to.
  const putAside = useAsideCards(target?.conversation?.id ?? null);
  // The conversation whose approvals the user stepped past with "Reply…", until none waits.
  const [replyingIn, setReplyingIn] = useState<string | null>(null);
  const shell = useRef<HTMLDivElement>(null);
  const anyApproval = pending.some((entry) => entry.type === "approval");
  // Once none waits, the next approval takes the composer's place again.
  if (replyingIn !== null && !anyApproval) setReplyingIn(null);
  if (!target) return null;
  const { conversation, resolved, targets } = target;
  const archived = conversation?.lifecycle === "archived";
  const dictationOwner = conversation?.id ?? NEW_CHAT_SCOPE;
  const waiting = pending.filter((entry) => !putAside.includes(entry.id));
  // Approvals take the composer's place, one at a time, until the user picks "Reply…"; then
  // they wait on the rail above it like any other decision. Questions and proposals always
  // sit above the full composer.
  const approvals = waiting.filter((entry) => entry.type === "approval").map((entry) => entry.id);
  const replying = conversation !== null && replyingIn === conversation.id;
  const slot = !archived && approvals.length > 0 && !replying;
  const current = archived || slot ? undefined : waiting[0];
  const setAsideIds = (ids: readonly string[]) => {
    if (conversation) setAsideCards(conversation.id, ids);
  };

  return (
    <ComposerPrimitive.Unstable_TriggerPopoverRoot>
      {!archived && (
        <ComposerDraft
          scope={conversation?.id ?? NEW_CHAT_SCOPE}
          attachments={target.queue.attachments}
          memory={target.mentions}
        />
      )}
      <div data-slot="composer" className="group/composer relative w-full">
        {conversation && (
          <Mentions conversation={conversation} targets={targets} />
        )}
        {conversation && !archived && <FullAccessNotice conversation={conversation} />}
        <SlashCommands
          conversation={conversation}
          groups={resolved.groups}
          onOpenModel={() => setModelOpen(true)}
        />
        {/* Hidden, not unmounted, while the slash or + menu is open over it. */}
        <ComposerRail className="transition-[opacity,visibility] group-has-[[data-slot=composer-commands]]/composer:invisible group-has-[[data-slot=composer-commands]]/composer:opacity-0 group-has-[[data-slot=plus-trigger][data-state=open]]/composer:invisible group-has-[[data-slot=plus-trigger][data-state=open]]/composer:opacity-0">
          {!conversation && <UtilityBar resolved={resolved} />}
          {/* /status sits on top, then the cards below it. */}
          {conversation && statusCard.open && (
            <StatusCard conversationId={conversation.id} onClose={() => statusCard.setOpen(false)} />
          )}
          {conversation && !archived && <QueueCard conversationId={conversation.id} />}
          {current && (
            <PendingActionCard
              action={current}
              more={waiting.length - 1}
              onDismiss={() => setAsideIds([...putAside, current.id])}
            />
          )}
          {pending.length > waiting.length && !archived && (
            <ComposerRailItem label="Waiting for you">
              <WaitingReminder count={pending.length - waiting.length} onShow={() => setAsideIds([])} />
            </ComposerRailItem>
          )}
        </ComposerRail>
        {slot && conversation && (
          <ApprovalSlot
            ids={approvals}
            onReply={() => {
              setReplyingIn(conversation.id);
              // The field comes back where it was, with its draft; the card waits above it.
              requestAnimationFrame(() =>
                shell.current?.querySelector<HTMLElement>("[contenteditable=true], textarea")?.focus(),
              );
            }}
          />
        )}
        {/* Kept while an approval has its place, so the draft and its caret stay. */}
        <ComposerPrimitive.Root className={cn("aui-composer-root relative flex w-full flex-col gap-1.5", slot && "hidden")}>
          {/* The composer's card: lifted, an inner hairline for an edge, and no focus ring. */}
          <div
            ref={shell}
            data-slot="aui_composer-shell"
            className="@container/composer bg-composer rounded-composer shadow-hairline relative flex w-full cursor-text flex-col gap-1 p-2 backdrop-blur-lg"
          >
            <ComposerAttachments />
            <ComposerInput
              placeholder={
                archived
                  ? "Restore this conversation to continue it."
                  : plan.on
                    ? PLAN_PLACEHOLDER
                    : placeholder
              }
              autoFocus={autoFocus}
            />
            <ComposerFooter owner={dictationOwner}>
              <div className="flex min-w-0 flex-1 flex-wrap items-center gap-0.5">
                <PlusMenu />
                {conversation ? (
                  <ConversationPermissionPicker conversation={conversation} />
                ) : (
                  resolved.kind === "session" && (
                    <PermissionPicker
                      value={resolved.permission}
                      untrusted={
                        resolved.repoPath !== null &&
                        folderTrust(resolved.project, resolved.repoPath) === false
                      }
                      onChange={(permission) =>
                        updateDraft(resolved.project?.id ?? null, { permission })
                      }
                    />
                  )
                )}
                <PlanChip />
              </div>
              <DictationNote owner={dictationOwner} />
              {conversation?.kind === "chat" && <ComposerContextRing />}
              {conversation ? (
                <ConversationModelPicker
                  conversation={conversation}
                  groups={resolved.groups}
                  open={modelOpen}
                  onOpenChange={setModelOpen}
                />
              ) : (
                <ModelSelector
                  groups={resolved.groups}
                  value={resolved.model}
                  defaultChoice={resolved.defaultModel}
                  label={resolved.kind === "session" ? "Orchestrator model" : "Model"}
                  onChange={(model) => updateDraft(resolved.project?.id ?? null, { model })}
                  open={modelOpen}
                  onOpenChange={setModelOpen}
                />
              )}
              <SendControls running={target.running} onResume={target.onResume} />
            </ComposerFooter>
          </div>
          <ComposerHint target={target} />
        </ComposerPrimitive.Root>
      </div>
    </ComposerPrimitive.Unstable_TriggerPopoverRoot>
  );
};

/** Keeps the composer's draft for its conversation (or the new chat). */
function ComposerDraft({
  scope,
  attachments,
  memory,
}: {
  scope: string;
  attachments: BlobAttachmentAdapter;
  memory: MentionMemory;
}) {
  useComposerDraft(scope, attachments, memory);
  return null;
}

/**
 * The text field: assistant-ui's Lexical input with mention/command chips (its own chunk, so
 * it doesn't hold up the first paint), a plain field on the same composer text until it loads.
 */
function ComposerInput(props: ComposerInputProps) {
  const { placeholder, autoFocus } = props;
  return (
    <Suspense
      fallback={
        <ComposerPrimitive.Input
          placeholder={placeholder}
          autoFocus={autoFocus}
          cancelOnEscape={false}
          rows={1}
          aria-label="Message input"
          className="aui-composer-input caret-primary placeholder:text-muted-foreground/60 min-h-composer max-h-composer-max w-full resize-none bg-transparent px-2 py-2.5 text-base outline-none"
        />
      }
    >
      <ComposerEditor {...props} />
    </Suspense>
  );
}

/** The context ring left of the model picker, once the model has said how full it is. */
function ComposerContextRing() {
  const show = useApp((s) => s.settings.showContextUsage);
  const usage = useBoard((s) => s.board?.context ?? null);
  if (!show || !usage) return null;
  return <ContextRing usage={usage} />;
}

/** Conversations whose Full access notice was closed, until the app restarts. */
const closedNotices = new Set<string>();

/**
 * The "Full access is on" card above the composer, while a session's workers run without the
 * OS sandbox. × closes it for this conversation; "Don't show again" turns it off in Settings,
 * where it can be turned back on.
 */
function FullAccessNotice({ conversation }: { conversation: Conversation }) {
  const show = useApp((s) => s.settings.showFullAccessNotice);
  const [, setClosed] = useState(0);
  const full = conversation.setup?.type === "session" && conversation.setup.permission === "fullAccess";
  if (!show || !full || closedNotices.has(conversation.id)) return null;
  const close = () => {
    closedNotices.add(conversation.id);
    setClosed((count) => count + 1);
  };
  return (
    <section
      aria-label="Full access is on"
      data-slot="full-access-notice"
      className="@container/notice animate-rail-open bg-rail border-foreground/10 rounded-surface mb-2 border px-4 py-3 text-sm motion-reduce:animate-none"
    >
      {/* Narrow, the buttons go under the text, so the text keeps the width to read. */}
      <div className="flex flex-col gap-2 @lg/notice:flex-row @lg/notice:items-center @lg/notice:gap-3">
        <div className="flex min-w-0 flex-1 items-center gap-3">
          <ShieldExclamation className="size-icon-md shrink-0" />
          <div className="min-w-0 flex-1">
            <p className="font-medium">Full access is on</p>
            <p className="text-muted-foreground">
              Brigadier’s workers can edit any file and run commands with internet access without
              your approval. This increases the risk of data loss, exposed information, and
              unexpected changes.{" "}
              <button type="button" className="underline underline-offset-2 hover:text-foreground" onClick={openPermissionsHelp}>
                Learn more
              </button>{" "}
              about elevated risks.
            </p>
          </div>
        </div>
        <div className="border-foreground/10 flex shrink-0 items-center justify-end gap-1 @lg/notice:self-stretch @lg/notice:border-s @lg/notice:ps-3">
          <Button
            size="xs"
            className="rounded-capsule"
            onClick={() =>
              void setSetting("showFullAccessNotice", false).catch(
                (error: unknown) => console.error("couldn't save the setting", error),
              )
            }
          >
            Don’t show again
          </Button>
          <TooltipIconButton tooltip="Close" side="bottom" className="rounded-capsule" onClick={close}>
            <X />
          </TooltipIconButton>
        </div>
      </div>
    </section>
  );
}

/** Whether a sideways-scrolling row has more past its right edge. */
function overflowsEnd(element: HTMLElement): boolean {
  return element.scrollLeft + element.clientWidth < element.scrollWidth - 1;
}

/**
 * The rail's utility bar on a new chat: project, where the session works and its branch.
 * It scrolls sideways, fading at the edge, when the composer is too narrow.
 */
function UtilityBar({ resolved }: { resolved: ResolvedDraft }) {
  const [overflowing, setOverflowing] = useState(false);
  const bar = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const element = bar.current;
    if (!element) return;
    const observer = new ResizeObserver(() => setOverflowing(overflowsEnd(element)));
    // The bar's own width and its row's, which grows with the pills' text.
    observer.observe(element);
    if (element.firstElementChild) observer.observe(element.firstElementChild);
    return () => observer.disconnect();
  }, []);
  return (
    <ComposerRailItem variant="controls" label="Composer utility bar">
      <div
        ref={bar}
        onScroll={(event) => setOverflowing(overflowsEnd(event.currentTarget))}
        data-overflowing={overflowing || undefined}
        className="data-overflowing:mask-fade-end hide-scrollbar overflow-x-auto px-1 py-1.25"
      >
        <div className="flex w-max items-center gap-2">
          <ProjectCombobox project={resolved.project} />
          {resolved.kind === "session" && resolved.repoPath && (
            <>
              <WorkInMenu resolved={resolved} />
              <BranchPopover resolved={resolved} />
            </>
          )}
        </div>
      </div>
    </ComposerRailItem>
  );
}

/** A second Esc within this long stops the model (the first one arms the button). */
const ESC_WINDOW_MS = 2000;

/**
 * A two-press stop: the first Esc outside menus and cards arms the send button (it reads
 * "Esc"), a second within two seconds stops the model. Esc in the composer also leaves the
 * field, keeping the text.
 */
function useEscToStop(canCancel: boolean): boolean {
  const aui = useAui();
  const { embedded } = useContext(ViewContext);
  const [armedAt, setArmedAt] = useState<number | null>(null);
  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented || event.isComposing) return;
      // A side chat's Esc is its own; the conversation beside it takes the rest.
      const inSideChat =
        event.target instanceof Element && event.target.closest("[data-embedded-view]") !== null;
      if (inSideChat !== embedded) return;
      // Menus, dialogs and the composer's own popovers take their Esc first.
      if (document.querySelector("[role=dialog], [role=menu], [role=listbox], [data-slot=composer-commands], [data-slot=composer-mentions]")) return;
      const active = document.activeElement;
      if (active instanceof HTMLElement && active.matches(COMPOSER_EDITABLE)) active.blur();
      if (!canCancel) return;
      const now = event.timeStamp;
      if (armedAt !== null && now - armedAt < ESC_WINDOW_MS) {
        setArmedAt(null);
        aui.composer().cancel();
      } else {
        setArmedAt(now);
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [aui, canCancel, armedAt, embedded]);
  useEffect(() => {
    if (armedAt === null) return;
    const timer = setTimeout(() => setArmedAt(null), ESC_WINDOW_MS);
    return () => clearTimeout(timer);
  }, [armedAt]);
  return armedAt !== null && canCancel;
}

type SendState = "starting" | "armed" | "stop" | "queue" | "resume" | "send";

const SEND_TIPS: Record<SendState, string> = {
  starting: "Starting your task…",
  armed: "Press Esc again to stop",
  stop: "Stop",
  queue: "Queue",
  resume: "Resume",
  send: "Send message",
};

/**
 * One send button, which changes with the state: ↑ to send (its tip "Queue" while the model
 * works: a Chat queues the message, a session's orchestrator sorts it), ■ to stop, "Esc" once
 * armed, ▶ to resume a stopped request, and a grey spinner while the conversation's model
 * starts.
 */
function SendControls({
  running,
  onResume,
}: {
  running: boolean;
  onResume: (() => void) | null;
}) {
  const target = useContext(ComposerTargetContext);
  const conversationId = target?.conversation?.id ?? null;
  const starting = useBoard(
    (s) => !!conversationId && s.board?.conversationId === conversationId && s.board.run === "starting",
  );
  const empty = useAuiState((s) => s.composer.isEmpty);
  const canCancel = useAuiState((s) => s.composer.canCancel);
  const armed = useEscToStop(canCancel);
  const dictationOwner = conversationId ?? NEW_CHAT_SCOPE;
  const state: SendState =
    armed
      ? "armed"
      : canCancel && empty
        ? starting
          ? "starting"
          : "stop"
        : running && !empty
          ? "queue"
          : !running && empty && onResume
            ? "resume"
            : "send";
  const button = (
    <TooltipIconButton
      tooltip={SEND_TIPS[state]}
      side="bottom"
      type="button"
      variant={state === "starting" ? "secondary" : "default"}
      size="icon-md"
      data-state={state}
      className={cn("aui-composer-send rounded-capsule", state === "armed" && "w-auto px-2 text-xs")}
      onClick={state === "resume" ? (onResume ?? undefined) : undefined}
    >
      {/* Keyed by state, so each glyph eases in as the button changes. */}
      <span key={state} className="animate-in fade-in zoom-in-75 flex items-center duration-200 motion-reduce:animate-none">
        {state === "starting" ? (
          <Spinner className="size-icon-sm animate-spin motion-reduce:animate-none" />
        ) : state === "armed" ? (
          "Esc"
        ) : state === "stop" ? (
          <Stop className="size-icon-sm" />
        ) : state === "resume" ? (
          <PlayTriangle />
        ) : (
          <ArrowUp />
        )}
      </span>
    </TooltipIconButton>
  );
  return (
    <div className="flex shrink-0 items-center gap-1">
      <DictateButton owner={dictationOwner} />
      {state === "resume" ? (
        button
      ) : state === "starting" || state === "armed" || state === "stop" ? (
        <ComposerPrimitive.Cancel asChild>{button}</ComposerPrimitive.Cancel>
      ) : (
        <ComposerPrimitive.Send asChild>{button}</ComposerPrimitive.Send>
      )}
    </div>
  );
}

/** What dictation is doing while it isn't recording: the model's download, starting, transcribing. */
function dictationNote(phase: DictationPhase): string | null {
  switch (phase.type) {
    case "downloading":
      return `Downloading the speech model${phase.total > 0 ? ` · ${Math.floor((phase.received / phase.total) * 100)}%` : "…"}`;
    case "starting":
      return "Starting dictation…";
    case "transcribing":
      return "Transcribing…";
    default:
      return null;
  }
}

/** The footer's line saying what dictation is doing, left of the model picker. */
function DictationNote({ owner }: { owner: string }) {
  const note = dictationNote(useDictation(owner).phase);
  if (!note) return null;
  return (
    <span data-slot="composer-dictation-note" className="text-muted-foreground min-w-0 truncate px-1 text-xs">
      {note}
    </span>
  );
}

/**
 * The Dictate button, its own beside the send button: the microphone (or ⌃⇧D) starts
 * dictating wherever the caret is, with or without text around it. It spins while the speech
 * model downloads, the microphone opens (a click cancels either) and the text is worked out,
 * and offers a retry after a failure. It keeps the field's focus, so the caret stays put.
 */
function DictateButton({ owner }: { owner: string }) {
  const { available, phase } = useDictation(owner);
  const mac = useApp((s) => s.info?.platform === "macos");
  if (!available) return null;
  const failed = phase.type === "failed" ? phase.message : null;
  const busy = phase.type === "downloading" || phase.type === "starting" || phase.type === "transcribing";
  const tooltip =
    phase.type === "downloading"
      ? "Cancel download"
      : phase.type === "starting"
        ? "Cancel dictation"
        : phase.type === "transcribing"
          ? "Transcribing…"
          : failed
            ? `Retry dictation: ${failed}`
            : "Dictate";
  return (
    <TooltipIconButton
      tooltip={tooltip}
      shortcut={busy ? undefined : shortcutLabel("⌃⇧D", mac)}
      aria-keyshortcuts="Control+Shift+D"
      side="top"
      type="button"
      size="icon-md"
      data-state={busy ? phase.type : failed ? "retry-dictation" : "dictate"}
      className="aui-composer-dictate rounded-capsule group/dictate"
      disabled={phase.type === "transcribing"}
      onMouseDown={(event) => event.preventDefault()}
      onClick={busy ? cancelDictation : () => void startDictation(owner)}
    >
      <span
        key={busy ? "busy" : failed ? "retry" : "dictate"}
        className="animate-in fade-in zoom-in-75 flex items-center duration-200 motion-reduce:animate-none"
      >
        {busy ? (
          <>
            <Spinner className="size-icon-sm animate-spin group-hover/dictate:hidden motion-reduce:animate-none" />
            <X className="size-icon-sm hidden group-hover/dictate:block" />
          </>
        ) : failed ? (
          <Warning className="size-icon-sm" />
        ) : (
          <Mic className="size-icon-sm" />
        )}
      </span>
    </TooltipIconButton>
  );
}

/** The composer's footer row; while the microphone records, the dictation controls take all of it. */
function ComposerFooter({ owner, children }: { owner: string; children: ReactNode }) {
  const { phase } = useDictation(owner);
  return (
    <div className="flex items-center gap-1">
      {phase.type === "recording" ? <DictationBar owner={owner} /> : children}
    </div>
  );
}

/**
 * The recording bar: Cancel, the microphone's waveform, Stop (the text goes to the caret) and
 * "Transcribe and send".
 */
function DictationBar({ owner }: { owner: string }) {
  const aui = useAui();
  const { levels } = useDictation(owner);
  return (
    <div
      data-slot="composer-dictation"
      data-phase="recording"
      className="flex min-w-0 flex-1 items-center gap-1"
    >
      <TooltipIconButton
        tooltip="Cancel dictation"
        side="bottom"
        type="button"
        size="icon-md"
        className="rounded-capsule"
        onClick={cancelDictation}
      >
        <X className="size-icon-sm" />
      </TooltipIconButton>
      <div
        aria-hidden
        className="flex h-7 min-w-0 flex-1 items-center justify-end gap-0.5 overflow-hidden px-1"
      >
        {levels.map((level, index) => (
          <span
            // Bars shift left as new ones arrive; their place is their identity.
            // oxlint-disable-next-line react/no-array-index-key
            key={index}
            className="bg-foreground w-0.5 shrink-0 rounded-full"
            style={{ height: `${Math.max(2, Math.round(level * 24))}px` }}
          />
        ))}
      </div>
      <TooltipIconButton
        tooltip="Stop dictation"
        side="bottom"
        type="button"
        size="icon-md"
        className="rounded-capsule"
        onClick={() => void stopDictation()}
      >
        <Stop className="size-icon-sm" />
      </TooltipIconButton>
      <TooltipIconButton
        tooltip="Transcribe and send"
        side="bottom"
        type="button"
        variant="default"
        size="icon-md"
        className="rounded-capsule"
        onClick={() => void stopDictation(() => aui.composer().send())}
      >
        <ArrowUp />
      </TooltipIconButton>
    </div>
  );
}

/** One line under the composer: what blocks sending, or what the setup implies. */
function ComposerHint({ target }: { target: ComposerTarget }) {
  const { conversation, resolved } = target;
  if (conversation) return null;
  const { project, problem, repo, environment, permission } = resolved;
  if (problem) {
    return (
      <p className="text-muted-foreground flex items-center gap-2 px-2 text-xs">
        {problem}
        {resolved.noModel && <ProvidersButton />}
        {project && !resolved.repoPath && <ProjectSettingsButton project={project} />}
        {repo.error && resolved.kind === "session" && (
          <span className="text-destructive">{repo.error}</span>
        )}
      </p>
    );
  }
  const dirty = environment === "localCheckout" && repo.info?.dirty;
  if (resolved.kind !== "session" || (permission !== "fullAccess" && !dirty)) return null;
  return (
    <p className="text-muted-foreground px-2 text-xs">
      {permission === "fullAccess" && (
        <span className="text-full-access">
          Full access: workers run without the OS sandbox.{" "}
        </span>
      )}
      {environment === "localCheckout" &&
        repo.info?.dirty &&
        "Your checkout has uncommitted changes; Brigadier will ask whether workers should see them."}
    </p>
  );
}
