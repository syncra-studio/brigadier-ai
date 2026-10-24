import { ComposerPrimitive, ComposerRail, ComposerRailItem, ComposerAttachments, type ComposerProps } from "@/components/conversation";
import {
  type FC,
  useContext,
  useEffect,
  useRef,
  useState,
} from "react";

import { updateDraft } from "@/app/conversation/draftSetup";
import {
  ConversationModelPicker,
  ConversationPermissionPicker,
  PermissionPicker,
  useShownPermission,
} from "@/app/shell/SetupPickers";
import { type MentionMemory, Mentions } from "@/app/conversation/Mentions";
import { SlashCommands } from "@/app/conversation/SlashCommands";
import { ComposerTargetContext } from "@/app/conversation/composerTarget";
import {
  ApprovalSlot,
  PendingActionCard,
  setAsideCards,
  useAsideCards,
  usePendingActions,
  WaitingReminder,
} from "@/app/conversation/ActionCards";
import { ComposerCard } from "@/app/conversation/ComposerCard";
import { ComposerContextRing } from "@/app/conversation/ComposerContextRing";
import { ComposerFooter } from "@/app/conversation/ComposerFooter";
import { ComposerHint } from "@/app/conversation/ComposerHint";
import { ComposerInput } from "@/app/conversation/ComposerInput";
import { ComposerStatusSlot } from "@/app/conversation/ComposerStatusSlot";
import { Notices } from "@/app/conversation/Notices";
import { DictationNote } from "@/app/conversation/DictationNote";
import { FullAccessNotice } from "@/app/conversation/FullAccessNotice";
import { QueueCard } from "@/app/conversation/QueueCard";
import { SendControls } from "@/app/conversation/SendControls";
import { UtilityBar } from "@/app/conversation/UtilityBar";
import { useComposerDraft } from "@/app/conversation/composerDraft";
import { useRuleRefusals } from "@/app/conversation/ruleRefusals";
import type { BlobAttachmentAdapter } from "@/app/conversation/attachments";
import { StatusCard, StatusCardContext } from "@/app/conversation/StatusCard";
import { PausedSendDialog } from "@/app/conversation/PausedSend";
import { NewSessionChip, PLAN_PLACEHOLDER, PlanChip, PlusMenu, usePlanMode } from "@/app/conversation/PlusMenu";
import { SelectionPill } from "@/app/conversation/SelectionPill";
import { type ModelGroup, ModelSelector } from "@/components/shared";
import { formatDuration } from "@/lib/format";
import { cn } from "@/lib/utils";
import { useActivity } from "@/state/activity";
import { useBoard } from "@/state/board";
import { useThreadView } from "@/state/cliThread";
import { NEW_CHAT_SCOPE } from "@/state/drafts";
import { loadModelRules, useModelRulesRead } from "@/state/modelRules";
import { folderTrust } from "@/state/trust";

const NO_GROUPS: ModelGroup[] = [];

/**
 * The composer (assistant-ui composer elements): attachments, @-mentions of
 * workers, files and conversations, `/` commands, and the setup pickers. A draft picks its
 * project (or none, for a Chat), environment, branch, permission level and model; a started
 * session can still change its model, effort and permission level, a Chat its model.
 */
export const ConversationComposer: FC<ComposerProps> = ({ autoFocus, placeholder }) => {
  const target = useContext(ComposerTargetContext);
  const [modelOpen, setModelOpen] = useState(false);
  // The user's rules file, read as the composer opens and again as the model picker does, so a
  // hand edit shows: what it keeps out the picker shows but can't pick.
  useModelRulesRead();
  const refusals = useRuleRefusals(target?.resolved.groups ?? NO_GROUPS);
  const openModel = (open: boolean) => {
    if (open) void loadModelRules();
    setModelOpen(open);
  };
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
  // A session draft's pill: Ask in a folder the user doesn't trust.
  const draftUntrusted = !conversation && resolved.kind === "session" && resolved.repoPath !== null && folderTrust(resolved.project, resolved.repoPath) === false;
  const permissionLevel = conversation ? sessionPermission : draftUntrusted ? "ask" : resolved.permission;

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
          onOpenModel={() => requestAnimationFrame(() => openModel(true))}
          permission={permissionLevel && { level: permissionLevel, onOpen: () => requestAnimationFrame(() => setPermissionOpen(true)) }}
          draft={conversation ? null : {
            provider: resolved.model.provider,
            projectId: resolved.kind === "session" ? (resolved.project?.id ?? null) : null,
            repo: resolved.kind === "session" ? resolved.repoPath : null,
          }}
        />
        {/* Hidden, not unmounted, while the slash or + menu is open over it. */}
        <ComposerRail className="transition-[opacity,visibility] group-has-[[data-slot=composer-commands]]/composer:invisible group-has-[[data-slot=composer-commands]]/composer:opacity-0 group-has-[[data-slot=plus-trigger][data-state=open]]/composer:invisible group-has-[[data-slot=plus-trigger][data-state=open]]/composer:opacity-0">
          {!conversation && <UtilityBar resolved={resolved} />}
          {/* /status sits on top, then the cards below it. */}
          {conversation && statusCard.open && (
            <StatusCard conversationId={conversation.id} onClose={() => statusCard.setOpen(false)} />
          )}
          {conversation && !archived && <ComposerStatusSlot conversationId={conversation.id} />}
          {conversation && <Notices conversationId={conversation.id} />}
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
          <ComposerCard ref={shell}>
            <ComposerAttachments />
            <ComposerInput
              placeholder={
                archived
                  ? "Restore this conversation to continue it."
                  : plan.on
                    ? PLAN_PLACEHOLDER
                    : (working ?? placeholder)
              }
              autoFocus={autoFocus}
            />
            <ComposerFooter owner={dictationOwner}>
              <div className="flex min-w-0 flex-1 flex-wrap items-center gap-0.5">
                <PlusMenu />
                {conversation ? (
                  <ConversationPermissionPicker conversation={conversation} open={permissionOpen} onOpenChange={setPermissionOpen} />
                ) : (
                  <PermissionPicker
                    value={resolved.permission}
                    untrusted={draftUntrusted}
                    open={permissionOpen}
                    onOpenChange={setPermissionOpen}
                    onChange={(permission) =>
                      updateDraft(resolved.project?.id ?? null, { permission })
                    }
                  />
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
                  onOpenChange={openModel}
                />
              ) : (
                <ModelSelector
                  groups={resolved.groups}
                  value={resolved.model}
                  defaultChoice={resolved.defaultModel}
                  label="Model"
                  onChange={(model) => updateDraft(resolved.project?.id ?? null, { model })}
                  open={modelOpen}
                  onOpenChange={openModel}
                  refused={refusals.refused}
                />
              )}
              <SendControls running={target.running} onResume={target.onResume} />
            </ComposerFooter>
          </ComposerCard>
          <ComposerHint target={target} />
        </ComposerPrimitive.Root>
        {conversation && !archived && <PausedSendDialog conversationId={conversation.id} />}
      </div>
    </ComposerPrimitive.Unstable_TriggerPopoverRoot>
  );
};

/**
 * "Working for 2m 5s" while the conversation's CLI turn is open, from its first second on,
 * ticking each second; null otherwise, and while it only waits for the user's answer to its
 * question card (#366): nothing works then. The turn's start is its CLI's own timestamp.
 */
function useWorkingFor(conversationId: string | null): string | null {
  const view = useThreadView(conversationId);
  const open = useActivity((s) => (conversationId ? (s.byConversation[conversationId]?.turnOpen ?? false) : false));
  const asking = useBoard((s) => {
    const board = s.board;
    if (!conversationId || board?.conversationId !== conversationId) return false;
    return Object.values(board.questions).some((question) => question.answer === null);
  });
  const last = view?.turns.at(-1);
  const from = open && !asking && last && last.end === null ? last.startedAtMs : null;
  // The time, read as a turn starts (or the composer opens on one), then on each whole second of
  // it; none shows until it was read for this turn.
  const [clock, setClock] = useState<{ from: number | null; now: number }>(() => ({ from, now: Date.now() }));
  useEffect(() => {
    if (from === null) return;
    let timer = 0;
    const tick = () => {
      const now = Date.now();
      setClock({ from, now });
      timer = window.setTimeout(tick, 1000 - (Math.max(0, now - from) % 1000));
    };
    timer = window.setTimeout(tick, 0);
    return () => window.clearTimeout(timer);
  }, [from]);
  if (from === null || clock.from !== from) return null;
  const elapsed = clock.now - from;
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
