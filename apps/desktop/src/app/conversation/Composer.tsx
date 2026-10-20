import { ComposerPrimitive, useAui, useAuiState } from "@assistant-ui/react";
import {
  type FC,
  lazy,
  type MouseEvent,
  type ReactNode,
  Suspense,
  useContext,
  useEffect,
  useMemo,
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
  useShownPermission,
} from "@/app/shell/SetupPickers";
import { SuggestionContext, useSuggestion } from "@/app/conversation/suggestion";
import type { ComposerInputProps } from "@/components/conversation/ComposerEditor";
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
import { ComposerStatusSlot } from "@/app/conversation/ComposerStatusSlot";
import { QueueCard } from "@/app/conversation/QueueCard";
import { shortcutLabel } from "@/app/side-panel/SidePanel";
import { useComposerDraft } from "@/app/conversation/composerDraft";
import {
  type BlobAttachmentAdapter,
  FILES_UPLOADING,
  uploading as anyUploading,
  useReadingFiles,
} from "@/app/conversation/attachments";
import { useImagesBlocked, useImagesUnsupportedToast } from "@/app/conversation/imageInput";
import { IMAGES_BLOCKED } from "@/app/conversation/imageModels";
import { StatusCard, StatusCardContext } from "@/app/conversation/StatusCard";
import { askPausedSend, PausedSendDialog } from "@/app/conversation/PausedSend";
import { NewSessionChip, PLAN_PLACEHOLDER, PlanChip, PlusMenu, usePlanMode } from "@/app/conversation/PlusMenu";
import { ViewContext } from "@/app/conversation/viewContext";
import { requestWaitsOnCard, turnSpans, workedMs } from "@/app/conversation/blocks";
import { SelectionPill } from "@/app/conversation/SelectionPill";
import { ComposerRail, ComposerRailItem, ComposerAttachments, ContextRing, type ComposerProps } from "@/components/conversation";
import { ModelSelector } from "@/components/shared";
import {
  ArrowUp,
  ArrowUpRight,
  Mic,
  PlayTriangle,
  ShieldExclamation,
  Spinner,
  Stop,
  Warning,
  X,
} from "@/components/icons";
import { Button, TooltipIconButton } from "@/components/controls";
import type { Conversation, RunState } from "@/ipc/generated";
import { whenRevealed } from "@/lib/splash";
import { formatDuration } from "@/lib/format";
import { cn } from "@/lib/utils";
import { setSetting } from "@/state/settings";
import { liveRequest, useBoard } from "@/state/board";
import {
  cancelDictation,
  type DictationPhase,
  startDictation,
  stopDictation,
  useDictation,
} from "@/state/dictation";
import { NEW_CHAT_SCOPE } from "@/state/drafts";
import { clearBackgroundSend, markBackgroundSend, useBackgroundArmed } from "@/state/backgroundSend";
import { isTaskTab } from "@/state/tabStates";
import { shortcutKeys } from "@/app/shortcutCatalog";
import { useApp } from "@/state/store";
import { folderTrust } from "@/state/trust";


// Loaded once the startup screen has gone: the plain field stands in until then, and on a slow
// machine loading and mounting the editor would hold up startup.
const ComposerEditor = lazy(() =>
  whenRevealed().then(() => import("@/components/conversation/ComposerEditor")),
);

/**
 * The composer (assistant-ui composer elements): attachments, @-mentions of
 * workers, files and conversations, `/` commands, and the setup pickers. A draft picks its
 * project (or none, for a Chat), environment, branch, permission level and model; a started
 * session can still change its model, effort and permission level, a Chat its model.
 */
export const ConversationComposer: FC<ComposerProps> = ({ autoFocus, placeholder }) => {
  const target = useContext(ComposerTargetContext);
  // The guess at the next message, in the empty field (Tab, → or Enter takes it).
  const suggestion = useSuggestion(target?.conversation?.id ?? null);
  const attached = useAuiState((s) => s.composer.attachments.length > 0);
  const [modelOpen, setModelOpen] = useState(false);
  const [permissionOpen, setPermissionOpen] = useState(false);
  const sessionPermission = useShownPermission(target?.conversation ?? null);
  const statusCard = useContext(StatusCardContext);
  const pending = usePendingActions(target?.conversation ?? null);
  const plan = usePlanMode(target);
  // Cards put aside with ×, for the conversation they belong to.
  const putAside = useAsideCards(target?.conversation?.id ?? null);
  // The conversation whose approvals the user stepped past with "Reply…", until none waits.
  const [replyingIn, setReplyingIn] = useState<string | null>(null);
  const shell = useRef<HTMLDivElement>(null);
  const working = useWorkingFor(target?.conversation?.id ?? null);
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
  const guess = !archived && !plan.on && !attached ? (suggestion?.text ?? null) : null;
  // A session draft's pill: Ask for approval in a folder the user doesn't trust.
  const draftUntrusted = !conversation && resolved.kind === "session" && resolved.repoPath !== null && folderTrust(resolved.project, resolved.repoPath) === false;
  const permissionLevel = conversation ? sessionPermission : resolved.kind === "session" ? (draftUntrusted ? "askForApproval" : resolved.permission) : null;

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
        <Mentions
          conversation={conversation}
          targets={targets}
          projectId={!conversation && resolved.kind === "session" ? (resolved.project?.id ?? null) : null}
        />
        {/* Out of the way while a decision waits above the composer (see FullAccessNotice). */}
        {conversation && !archived && !current && !slot && <FullAccessNotice conversation={conversation} />}
        <SlashCommands
          conversation={conversation}
          groups={resolved.groups}
          // Each picker opens once the / menu has closed and the composer took the command out,
          // or it would close as it opens.
          onOpenModel={() => requestAnimationFrame(() => setModelOpen(true))}
          permission={permissionLevel && { level: permissionLevel, onOpen: () => requestAnimationFrame(() => setPermissionOpen(true)) }}
        />
        {/* Hidden, not unmounted, while the slash or + menu is open over it. */}
        <ComposerRail className="transition-[opacity,visibility] group-has-[[data-slot=composer-commands]]/composer:invisible group-has-[[data-slot=composer-commands]]/composer:opacity-0 group-has-[[data-slot=plus-trigger][data-state=open]]/composer:invisible group-has-[[data-slot=plus-trigger][data-state=open]]/composer:opacity-0">
          {!conversation && <UtilityBar resolved={resolved} />}
          {/* /status sits on top, then the cards below it. */}
          {conversation && statusCard.open && (
            <StatusCard conversationId={conversation.id} onClose={() => statusCard.setOpen(false)} />
          )}
          {conversation && !archived && <ComposerStatusSlot conversationId={conversation.id} />}
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
          <SelectionPill />
          {/* The composer's card: lifted, an inner hairline for an edge, and no focus ring. */}
          <div
            ref={shell}
            data-slot="aui_composer-shell"
            className="@container/composer bg-composer rounded-composer shadow-hairline relative flex w-full cursor-text flex-col gap-1 p-2 backdrop-blur-lg"
          >
            <ComposerAttachments />
            <SuggestionContext.Provider value={guess}>
              <ComposerInput
                placeholder={
                  archived
                    ? "Restore this conversation to continue it."
                    : plan.on
                      ? PLAN_PLACEHOLDER
                      : guess
                        ? ""
                        : (working ?? placeholder)
                }
                autoFocus={autoFocus}
                guess={guess}
              />
            </SuggestionContext.Provider>
            <ComposerFooter owner={dictationOwner}>
              <div className="flex min-w-0 flex-1 flex-wrap items-center gap-0.5">
                <PlusMenu />
                {conversation ? (
                  <ConversationPermissionPicker conversation={conversation} open={permissionOpen} onOpenChange={setPermissionOpen} />
                ) : (
                  resolved.kind === "session" && (
                    <PermissionPicker
                      value={resolved.permission}
                      untrusted={draftUntrusted}
                      open={permissionOpen}
                      onOpenChange={setPermissionOpen}
                      onChange={(permission) =>
                        updateDraft(resolved.project?.id ?? null, { permission })
                      }
                    />
                  )
                )}
                <PlanChip />
                <NewSessionChip />
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
                  label="Model"
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
        {conversation && !archived && <PausedSendDialog conversationId={conversation.id} />}
      </div>
    </ComposerPrimitive.Unstable_TriggerPopoverRoot>
  );
};

/**
 * "Working for 2m 5s" while the conversation's request works, from its first second on, ticking
 * each second; null otherwise, and while it only waits for the user's answer to its question card
 * (#366): nothing works then. It times the turn as its work header does, the follow-ups that
 * joined it counted with it (#446).
 */
function useWorkingFor(conversationId: string | null): string | null {
  const newest = useBoard((s) => {
    const board = s.board;
    if (!conversationId || board?.conversationId !== conversationId) return null;
    const live = liveRequest(board);
    if (!live || requestWaitsOnCard(live.live)) return null;
    return live.newest;
  });
  const requests = useBoard((s) => s.board?.requests);
  const spans = useMemo(() => (newest && requests ? turnSpans(requests, newest) : null), [newest, requests]);
  // Which turn it times, as it started: a request answered again starts over.
  const turn = spans ? `${newest}@${Math.min(...spans.map((span) => span.fromMs))}` : null;
  // The time, read as a request starts working (or the composer opens on one), then on each whole
  // second of its work; none shows until it was read for this turn.
  const [clock, setClock] = useState<{ from: string | null; now: number }>(() => ({ from: turn, now: Date.now() }));
  useEffect(() => {
    if (!spans) return;
    let timer = 0;
    const tick = () => {
      const now = Date.now();
      setClock({ from: turn, now });
      timer = window.setTimeout(tick, 1000 - (workedMs(spans, now) % 1000));
    };
    timer = window.setTimeout(tick, 0);
    return () => window.clearTimeout(timer);
  }, [spans, turn]);
  if (!spans || clock.from !== turn) return null;
  const elapsed = workedMs(spans, clock.now);
  if (elapsed < 1000) return null;
  return `Working for ${formatDuration(elapsed)}`;
}

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
 * it doesn't hold up startup), a plain field on the same composer text until it loads.
 */
function ComposerInput({ guess, ...props }: ComposerInputProps & { guess: string | null }) {
  const { placeholder, autoFocus } = props;
  const empty = useAuiState((s) => s.composer.isEmpty);
  return (
    <div className="relative min-w-0">
      <ComposerField placeholder={placeholder} autoFocus={autoFocus} />
      {/* The guess at the next message, where the placeholder would be. */}
      {guess && empty && (
        <p
          data-slot="composer-suggestion"
          className="text-muted-foreground/60 pointer-events-none absolute inset-x-0 top-0 px-2 py-2.5 text-base"
        >
          {guess}
        </p>
      )}
    </div>
  );
}

function ComposerField(props: ComposerInputProps) {
  const { placeholder, autoFocus } = props;
  return (
    <Suspense fallback={<ComposerStandIn placeholder={placeholder} autoFocus={autoFocus} />}>
      <ComposerEditor {...props} />
    </Suspense>
  );
}

/**
 * The plain field that stands in for the composer while its editor loads: what's typed is what's
 * sent here too, with none of macOS's text substitutions (smart quotes and dashes, auto-correct,
 * capitals, the double-space period), as in the editor (#188).
 */
export function ComposerStandIn({ placeholder, autoFocus }: { placeholder?: string | undefined; autoFocus?: boolean | undefined }) {
  const field = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    const element = field.current;
    element?.addEventListener("beforeinput", keepTypedSpace, true);
    return () => element?.removeEventListener("beforeinput", keepTypedSpace, true);
  }, []);
  return (
    <ComposerPrimitive.Input
      ref={field}
      placeholder={placeholder}
      autoFocus={autoFocus}
      cancelOnEscape={false}
      rows={1}
      aria-label="Message input"
      spellCheck={false}
      autoCorrect="off"
      autoCapitalize="off"
      className="aui-composer-input caret-primary placeholder:text-muted-foreground/60 min-h-composer max-h-composer-max w-full resize-none bg-transparent px-2 py-2.5 text-base outline-none"
    />
  );
}

/**
 * macOS's "Add period with double-space" in a plain field: its input method puts ". " in place
 * of the space before the caret. The two spaces typed stay instead.
 */
export function keepTypedSpace(event: InputEvent) {
  if (event.data !== ". " || !/^insert(Replacement)?Text$/.test(event.inputType)) return;
  const field = event.currentTarget as HTMLTextAreaElement;
  const { selectionStart: start, selectionEnd: end, value } = field;
  // A textarea gives no target ranges: what it replaces is the space before the caret, or that
  // space selected.
  const selected = end - start === 1 && value[start] === " ";
  if (!selected && (start !== end || value[start - 1] !== " ")) return;
  event.preventDefault();
  event.stopImmediatePropagation();
  field.ownerDocument.execCommand("insertText", false, selected ? "  " : " ");
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
 * OS sandbox: the full warning at nine tenths of a card's usual size (its padding, type, icon
 * and buttons; heights rounded to whole pixels, so the thread's footer stays on the pixel grid),
 * rounded like the composer. It steps aside while a question, proposal or approval
 * waits above the composer, so it never pushes that card or the thread's last lines out of
 * view, and while messages wait in the queue, so they stay right under the conversation (#448). × closes it for this conversation; "Don't show again" turns it off in Settings, where it
 * can be turned back on.
 */
function FullAccessNotice({ conversation }: { conversation: Conversation }) {
  const show = useApp((s) => s.settings.showFullAccessNotice);
  const [, setClosed] = useState(0);
  const full = conversation.setup?.type === "session" && conversation.setup.permission === "fullAccess";
  const queued = useBoard((s) => {
    const queue = s.board?.conversationId === conversation.id ? s.board.queue : null;
    return !!queue && (queue.items.length > 0 || (queue.moved?.length ?? 0) > 0);
  });
  if (!show || !full || queued || closedNotices.has(conversation.id)) return null;
  const close = () => {
    closedNotices.add(conversation.id);
    setClosed((count) => count + 1);
  };
  return (
    <section
      aria-label="Full access is on"
      data-slot="full-access-notice"
      className="@container/notice animate-rail-open bg-rail border-foreground/10 rounded-composer mb-2 border px-notice-x py-notice-y text-notice motion-reduce:animate-none"
    >
      {/* Narrow, the buttons go under the text, so the text keeps the width to read. */}
      <div className="flex flex-col gap-notice-gap @lg/notice:flex-row @lg/notice:items-center @lg/notice:gap-notice-gap-wide">
        <div className="flex min-w-0 flex-1 items-center gap-notice-gap-wide">
          <ShieldExclamation className="size-notice-icon shrink-0" />
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
        <div className="border-foreground/10 flex shrink-0 items-center justify-end gap-1 @lg/notice:self-stretch @lg/notice:border-s @lg/notice:ps-notice-gap-wide">
          <Button
            size="xs"
            className="rounded-capsule h-notice-button text-notice-button"
            onClick={() =>
              void setSetting("showFullAccessNotice", false).catch(
                (error: unknown) => console.error("couldn't save the setting", error),
              )
            }
          >
            Don’t show again
          </Button>
          <TooltipIconButton
            tooltip="Close"
            side="bottom"
            className="rounded-capsule size-notice-close"
            onClick={close}
          >
            <X className="size-notice-close-icon" />
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

/** Other fields that keep the keyboard on Esc. */
const TEXT_FIELD = "input, textarea, select, [contenteditable=true], [contenteditable='']";

/**
 * A two-press stop: the first Esc outside menus and cards arms the send button (it reads
 * "Esc"), a second within two seconds stops the model; Esc in the composer then also leaves the
 * field, keeping the text. With nothing running, Esc puts the cursor in the composer instead.
 * Esc while dictation (`dictationOwner`'s) downloads, starts or records cancels it, and only that.
 */
function useEscToStop(canCancel: boolean, dictationOwner: string): boolean {
  const aui = useAui();
  const { embedded } = useContext(ViewContext);
  const [armedAt, setArmedAt] = useState<number | null>(null);
  const dictation = useDictation(dictationOwner).phase.type;
  const dictating = dictation === "downloading" || dictation === "starting" || dictation === "recording";
  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented || event.isComposing) return;
      // A side chat's Esc is its own; the conversation beside it takes the rest.
      const inSideChat =
        event.target instanceof Element && event.target.closest("[data-embedded-view]") !== null;
      if (inSideChat !== embedded) return;
      // Menus, dialogs and the composer's own popovers take their Esc first.
      if (document.querySelector("[role=dialog], [role=menu], [role=listbox], [data-slot=composer-commands], [data-slot=composer-mentions]")) return;
      if (dictating) {
        cancelDictation();
        return;
      }
      const active = document.activeElement;
      const inComposer = active instanceof HTMLElement && active.matches(COMPOSER_EDITABLE);
      if (!canCancel) {
        if (inComposer || (active instanceof HTMLElement && active.matches(TEXT_FIELD))) return;
        const mine = [...document.querySelectorAll<HTMLElement>(COMPOSER_EDITABLE)].find(
          (field) => (field.closest("[data-embedded-view]") !== null) === embedded,
        );
        mine?.focus();
        return;
      }
      if (inComposer) active.blur();
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
  }, [aui, canCancel, armedAt, embedded, dictating]);
  useEffect(() => {
    if (armedAt === null) return;
    const timer = setTimeout(() => setArmedAt(null), ESC_WINDOW_MS);
    return () => clearTimeout(timer);
  }, [armedAt]);
  return armedAt !== null && canCancel;
}

type SendState = "starting" | "armed" | "stop" | "queue" | "resume" | "send";

/**
 * How long Stop stays after the model stops being cancellable. A merge lands its workers one by
 * one and the conversation stops and starts working between them; without this the button
 * flipped Stop ↔ Send several times a second, each flip easing its glyph in again from nothing,
 * so it showed as a blank disc.
 */
const STOP_HOLD_MS = 700;

/** `on`, staying true for `ms` after it goes false (and false at once if it comes back on). */
function useHeldOn(on: boolean, ms: number): boolean {
  const [held, setHeld] = useState(on);
  // Set while rendering (React's way to adjust state to a prop), not in an effect.
  if (on && !held) setHeld(true);
  useEffect(() => {
    if (on) return;
    const timer = setTimeout(() => setHeld(false), ms);
    return () => clearTimeout(timer);
  }, [on, ms]);
  return on || held;
}

/**
 * Whether the model is starting for the first time in this stretch of work (`working`). A
 * session's model starts again for each worker report and landing; those starts keep Stop, so
 * the button doesn't dim to the spinner and back mid-work (#422).
 */
function useFirstStart(run: RunState | null, working: boolean): boolean {
  const [ran, setRan] = useState(false);
  // Set while rendering (React's way to adjust state to a prop), not in an effect.
  if (working && run === "running" && !ran) setRan(true);
  if (!working && ran) setRan(false);
  return run === "starting" && !ran;
}

/** What ⌘⏎ and ⌘-click on Send do in a project's session. */
const START_TIP = "Start as a new session";

const SEND_TIPS: Record<SendState, string> = {
  starting: "Starting your task…",
  armed: "Press Esc again to stop",
  stop: "Stop",
  queue: "Queue",
  resume: "Resume",
  send: "Send message",
};

/** Whether ⌘ (Ctrl off macOS) is held down, while `on`. */
function useModHeld(mac: boolean, on: boolean): boolean {
  const [held, setHeld] = useState(false);
  useEffect(() => {
    if (!on) return;
    const update = (event: KeyboardEvent) => setHeld(mac ? event.metaKey : event.ctrlKey);
    // Its key-up goes to another app once the window loses focus (⌘Tab).
    const release = () => setHeld(false);
    window.addEventListener("keydown", update);
    window.addEventListener("keyup", update);
    window.addEventListener("blur", release);
    return () => {
      window.removeEventListener("keydown", update);
      window.removeEventListener("keyup", update);
      window.removeEventListener("blur", release);
    };
  }, [mac, on]);
  return on && held;
}

/**
 * One send button, which changes with the state: ↑ to send (its tip "Queue" while the model
 * works: a Chat queues the message, a session's orchestrator sorts it; "Steer" when steering is
 * the default follow-up, Settings › General), ■ to stop, "Esc" once
 * armed, ▶ to resume a stopped request, and a grey spinner while the conversation's model
 * starts. In a project's session ⌘-click starts the message as a new session in the
 * background, as ⌘⏎ does; its arrow turns while ⌘ is held (or the New session chip is on).
 */
function SendControls({
  running,
  onResume,
}: {
  running: boolean;
  onResume: (() => void) | null;
}) {
  const aui = useAui();
  const target = useContext(ComposerTargetContext);
  const conversationId = target?.conversation?.id ?? null;
  const run = useBoard((s) =>
    !!conversationId && s.board?.conversationId === conversationId ? s.board.run : null,
  );
  const empty = useAuiState((s) => s.composer.isEmpty);
  const cancellable = useAuiState((s) => s.composer.canCancel);
  const canCancel = useHeldOn(cancellable, STOP_HOLD_MS);
  const starting = useFirstStart(run, canCancel);
  const dictationOwner = conversationId ?? NEW_CHAT_SCOPE;
  const armed = useEscToStop(canCancel, dictationOwner);
  const steers = useApp((s) => s.settings.followUp === "steer");
  const mac = useApp((s) => s.info?.platform === "macos");
  // A project's session: ⌘⏎ and ⌘-click start a new session in the background.
  const projectSession = target?.conversation && isTaskTab(target.conversation) ? target.conversation.id : null;
  const modHeld = useModHeld(mac, projectSession !== null);
  const chip = useBackgroundArmed(projectSession);
  // Images for a model that takes none hold the message until they go or the model changes;
  // files still being read or stored hold it until they are.
  const imagesBlocked = useImagesBlocked();
  useImagesUnsupportedToast();
  const stored = useAuiState((s) => anyUploading(s.composer.attachments));
  const uploading = useReadingFiles(dictationOwner) || stored;
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
  const sending = state === "send" || state === "queue";
  // Stop is a plain button, not the Cancel primitive: held Stop (nothing to cancel) and live Stop
  // are then the same element, so a flip between them never remounts it.
  const cancelling = state === "starting" || state === "armed" || state === "stop";
  const background = sending && projectSession !== null && !empty && (modHeld || chip);
  const tip = state === "queue" && steers ? "Steer" : SEND_TIPS[state];
  const tooltip =
    sending && imagesBlocked
      ? IMAGES_BLOCKED
      : sending && uploading
        ? FILES_UPLOADING
        : background
        ? START_TIP
        : sending && projectSession
          ? `${state === "send" ? "Send" : tip} ${shortcutKeys("send", mac)} · ${START_TIP} ${shortcutKeys("start-in-tab", mac)}`
          : tip;
  const button = (
    <TooltipIconButton
      tooltip={tooltip}
      side="bottom"
      type="button"
      disabled={sending && (imagesBlocked || uploading)}
      variant={state === "starting" ? "secondary" : "default"}
      size="icon-md"
      data-state={state}
      className={cn("aui-composer-send rounded-capsule", state === "armed" && "w-auto px-2 text-xs")}
      onClick={
        state === "resume"
          ? (onResume ?? undefined)
          : cancelling
            ? () => {
                if (cancellable) aui.composer().cancel();
              }
          : (event: MouseEvent<HTMLButtonElement>) => {
              const toNewSession = sending && !empty && (mac ? event.metaKey : event.ctrlKey);
              // Into a paused queue it asks first; the Send primitive's own click doesn't send.
              if (sending && !empty && !(projectSession && toNewSession) && askPausedSend(conversationId, () => aui.composer().send())) {
                event.preventDefault();
                return;
              }
              if (!projectSession) return;
              // A ⌘-click whose send never came is not this send's.
              clearBackgroundSend(projectSession);
              // Before the Send primitive's own click sends: this send goes to a new session.
              if (toNewSession) markBackgroundSend(projectSession);
            }
      }
    >
      {/* Keyed by state, so each glyph eases in as the button changes. */}
      <span key={background ? "background" : state} className="animate-in fade-in zoom-in-75 flex items-center duration-200 motion-reduce:animate-none">
        {state === "starting" ? (
          <Spinner className="size-icon-sm animate-spin motion-reduce:animate-none" />
        ) : state === "armed" ? (
          "Esc"
        ) : state === "stop" ? (
          <Stop className="size-icon-sm" />
        ) : state === "resume" ? (
          <PlayTriangle />
        ) : background ? (
          <ArrowUpRight data-slot="send-new-session" />
        ) : (
          <ArrowUp />
        )}
      </span>
    </TooltipIconButton>
  );
  return (
    <div className="flex shrink-0 items-center gap-1">
      <DictateButton owner={dictationOwner} />
      {state === "resume" || cancelling ? button : <ComposerPrimitive.Send asChild>{button}</ComposerPrimitive.Send>}
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
  const { project, problem, repo, permission } = resolved;
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
  // A new worktree starts from its base's last commit: what isn't committed stays out of it.
  const dirty = repo.info?.dirty;
  if (resolved.kind !== "session" || (permission !== "fullAccess" && !dirty)) return null;
  return (
    <p className="text-muted-foreground px-2 text-xs">
      {permission === "fullAccess" && (
        <span className="text-full-access">
          Full access: workers run without the OS sandbox.{" "}
        </span>
      )}
      {dirty &&
        "Every tab starts clean on a worktree of its own: your checkout's uncommitted changes stay in your checkout."}
    </p>
  );
}
