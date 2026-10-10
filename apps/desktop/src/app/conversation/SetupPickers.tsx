import {
  Check,
  Clock,
  Folder,
  Globe,
  HandRaised,
  Settings as SettingsIcon,
  Shuffle,
  Terminal,
  Warning,
} from "@openai/apps-sdk-ui/components/Icon";
import { type FC, useRef, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { ProjectDialog } from "@/app/dialogs/ProjectDialog";
import {
  ModelSelector,
  type ModelGroup,
} from "@/components/assistant-ui/elements/model-selector";
import { composerPill } from "@/components/assistant-ui/elements/surfaces";
import { ShieldExclamation, ShieldTerminal } from "@/components/glyphs/permission-glyphs";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useNow } from "@/hooks/use-now";
import type {
  Conversation,
  ModelChoice,
  ModelFallback,
  PermissionLevel,
  Project,
  QuotaWait,
} from "@/ipc/generated";
import { openUrl } from "@/ipc/client";
import {
  FULL_ACCESS_NOTE,
  PERMISSION_DETAILS,
  PERMISSION_LABELS,
  PERMISSION_LEVELS,
  PERMISSIONS_HELP_URL,
  resolveModel,
  UNTRUSTED_NOTE,
  useModelGroups,
  withChoice,
} from "@/lib/setup";
import { choiceName, formatResetAt, VENDOR_LABELS, withResetTime } from "@/lib/routing";
import { cn } from "@/lib/utils";
import { openSettings, updateSetup } from "@/state/actions";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";
import { folderTrust } from "@/state/trust";

/** Opens a project's settings, for a project that has no repository yet. */
export function ProjectSettingsButton({ project }: { project: Project }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button size="xs" variant="outline" onClick={() => setOpen(true)}>
        <SettingsIcon />
        Project settings
      </Button>
      <ProjectDialog open={open} onOpenChange={setOpen} project={project} />
    </>
  );
}

/** Opens Settings › Providers, for when no model is available. */
export function ProvidersButton() {
  return (
    <Button size="xs" variant="outline" onClick={() => openSettings("providers")}>
      <SettingsIcon />
      Open Providers
    </Button>
  );
}


// ----- permission ------------------------------------------------------------------------

const PERMISSION_ICONS: Record<PermissionLevel, FC<{ className?: string }>> = {
  askForApproval: HandRaised,
  approveForMe: ShieldTerminal,
  fullAccess: ShieldExclamation,
};

/** Opens the README's section on permission levels in the system browser. */
export function openPermissionsHelp(): void {
  openUrl(PERMISSIONS_HELP_URL).catch((error: unknown) =>
    toast(error instanceof Error ? error.message : String(error), { tone: "error" }),
  );
}

/**
 * The permission pill and menu, opening upward: "How should Brigadier's actions be approved?"
 * with "Learn more", each level with its icon and a one-line summary, a check on the one in
 * use, Full access in orange and confirmed before it turns on. In a narrow composer the pill
 * keeps only its icon. In a folder the user doesn't trust it shows Ask for approval, the only
 * level sessions there run at, and the others can't be picked.
 */
export function PermissionPicker({
  value: chosen,
  untrusted = false,
  onChange,
}: {
  value: PermissionLevel;
  untrusted?: boolean;
  onChange: (level: PermissionLevel) => void;
}) {
  const [open, setOpen] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const value: PermissionLevel = untrusted ? "askForApproval" : chosen;
  const full = value === "fullAccess";
  const Icon = PERMISSION_ICONS[value];
  return (
    <>
      <DropdownMenu modal={false} open={open} onOpenChange={setOpen}>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label="Permission level"
            data-slot="permission-picker"
            className={cn(
              composerPill,
              "text-muted-foreground max-w-xs",
              full && "text-full-access",
            )}
          >
            <Icon />
            <span className="truncate @max-md/composer:hidden">{PERMISSION_LABELS[value]}</span>
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent
          side="top"
          align="start"
          data-slot="permission-menu"
          className="max-w-(--radix-dropdown-menu-content-available-width) p-1.5"
        >
          <div className="flex items-baseline justify-between gap-6 px-2 pt-1 pb-1.5 text-sm">
            <p className="text-muted-foreground">How should Brigadier’s actions be approved?</p>
            <button
              type="button"
              className="text-muted-foreground hover:text-foreground shrink-0 underline underline-offset-2"
              onClick={() => {
                setOpen(false);
                openPermissionsHelp();
              }}
            >
              Learn more
            </button>
          </div>
          {untrusted && (
            // As wide as the menu, never widening it.
            <p className="text-muted-foreground w-0 min-w-full px-2 pb-1.5 text-xs">
              {UNTRUSTED_NOTE}
            </p>
          )}
          <DropdownMenuRadioGroup
            value={value}
            onValueChange={(next) => {
              const level = next as PermissionLevel;
              if (level === "fullAccess" && !full) setConfirming(true);
              else onChange(level);
            }}
          >
            {PERMISSION_LEVELS.map((level) => {
              const LevelIcon = PERMISSION_ICONS[level];
              const orange = level === "fullAccess";
              return (
                <DropdownMenuRadioItem
                  key={level}
                  value={level}
                  disabled={untrusted && level !== "askForApproval"}
                  indicator={<Check className="size-icon-md" />}
                  className={cn(
                    "h-auto gap-3 rounded-xl py-1.5 pe-9",
                    orange && "text-full-access focus:text-full-access",
                  )}
                >
                  <LevelIcon className="size-icon-md" />
                  <span className="flex min-w-0 flex-col">
                    <span>{PERMISSION_LABELS[level]}</span>
                    <span
                      className={cn(
                        "whitespace-nowrap",
                        orange ? "text-full-access" : "text-muted-foreground",
                      )}
                    >
                      {PERMISSION_DETAILS[level]}
                    </span>
                  </span>
                </DropdownMenuRadioItem>
              );
            })}
          </DropdownMenuRadioGroup>
        </DropdownMenuContent>
      </DropdownMenu>
      <FullAccessDialog
        open={confirming}
        onCancel={() => setConfirming(false)}
        onConfirm={() => {
          setConfirming(false);
          onChange("fullAccess");
        }}
      />
    </>
  );
}

/** What Full access lets workers do, in the confirmation's card. */
const FULL_ACCESS_POWERS: { icon: FC<{ className?: string }>; tone: string; title: string; line: string }[] = [
  {
    icon: Folder,
    tone: "text-link",
    title: "Files and folders",
    line: "Read, create, modify, or delete files anywhere on this computer",
  },
  {
    icon: Terminal,
    tone: "text-muted-foreground",
    title: "Terminal commands",
    line: "Run commands, install software, and change system settings",
  },
  { icon: Globe, tone: "text-link", title: "Internet", line: "Access websites and send data" },
];

/**
 * The "Turn on Full Access?" dialog: what workers could do without the sandbox, what still
 * asks, and the risks. Only Confirm changes the level; Esc or Cancel keeps it.
 */
export function FullAccessDialog({
  open,
  onCancel,
  onConfirm,
}: {
  open: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const cancel = useRef<HTMLButtonElement | null>(null);
  return (
    <Dialog open={open} onOpenChange={(next) => !next && onCancel()}>
      <DialogContent
        showCloseButton={false}
        className="max-w-lg"
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          cancel.current?.focus();
        }}
      >
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2 text-lg">
            <Warning className="size-icon-md" />
            Turn on Full Access?
          </DialogTitle>
          <DialogDescription>
            Brigadier’s workers will be able to run commands, use the internet, and create and
            edit files anywhere on this computer without your permission. This includes but is
            not limited to:
          </DialogDescription>
        </DialogHeader>
        <ul className="bg-foreground/5 rounded-surface divide-foreground/10 divide-y px-3">
          {FULL_ACCESS_POWERS.map(({ icon: PowerIcon, tone, title, line }) => (
            <li key={title} className="flex items-center gap-3 py-2.5">
              <PowerIcon className={cn("size-icon-lg shrink-0", tone)} />
              <span className="flex min-w-0 flex-col">
                <span className="font-medium">{title}</span>
                <span className="text-muted-foreground text-xs">{line}</span>
              </span>
            </li>
          ))}
        </ul>
        <p className="text-muted-foreground">
          {FULL_ACCESS_NOTE} This comes with risks like loss or exposure of sensitive data
          and prompt injection. You can turn this off.{" "}
          <button type="button" className="text-link hover:underline" onClick={openPermissionsHelp}>
            Learn more
          </button>
        </p>
        <DialogFooter>
          <Button ref={cancel} variant="secondary" className="rounded-capsule" onClick={onCancel}>
            Cancel
          </Button>
          <Button
            variant="ghost"
            className="rounded-capsule bg-destructive/15 text-destructive hover:bg-destructive/25 hover:text-destructive"
            onClick={onConfirm}
          >
            <Warning />
            Confirm
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ----- model -----------------------------------------------------------------------------

/**
 * Model and effort of a started conversation; changing them updates its setup. `groups` are
 * the available models; the one it runs on shows even when it isn't available any more.
 */
export function ConversationModelPicker({
  conversation,
  groups,
  open,
  onOpenChange,
}: {
  conversation: Conversation;
  groups: readonly ModelGroup[];
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}) {
  const settings = useApp((s) => s.settings);
  const project = useApp((s) =>
    conversation.projectId ? (s.projects[conversation.projectId] ?? null) : null,
  );
  const action = useAction();
  const setup = conversation.setup;
  const fallback = resolveModel(null, conversation.kind, project, settings, groups);
  const current: ModelChoice =
    setup?.type === "session"
      ? setup.orchestrator
      : setup?.type === "chat"
        ? setup.model
        : fallback;
  // A session created before setups existed gets one on first use; until then it can't change.
  const fixed = setup === null && conversation.kind === "session";
  const all = useModelGroups();
  const shown = withChoice(groups, all, current);

  return (
    <>
      {action.error && (
        <span role="alert" className="text-destructive max-w-xs truncate text-xs" title={action.error}>
          {action.error}
        </span>
      )}
      {conversation.fallback && (
        <StandInPill fallback={conversation.fallback} groups={all} />
      )}
      {conversation.quotaWait && <WaitingPill wait={conversation.quotaWait} />}
      <ModelSelector
        groups={shown}
        value={current}
        defaultChoice={fallback}
        disabled={fixed || action.busy}
        open={open}
        onOpenChange={onOpenChange}
        label={conversation.kind === "session" ? "Orchestrator model" : "Model"}
        onChange={(choice) =>
          action.run(() =>
            updateSetup(
              conversation.id,
              setup?.type === "session"
                ? { ...setup, orchestrator: choice }
                : { type: "chat", model: choice },
            ),
          )
        }
      />
    </>
  );
}

/**
 * The model standing in while the chosen one is at a limit: "On Codex gpt-6-sol until 21:10 ·
 * Claude limit", the reason on hover. The picker keeps showing the saved choice.
 */
function StandInPill({
  fallback,
  groups,
}: {
  fallback: ModelFallback;
  groups: readonly ModelGroup[];
}) {
  const now = useNow(60_000);
  const until = fallback.untilMs !== null ? ` until ${formatResetAt(fallback.untilMs, now)}` : "";
  const text = `On ${choiceName(groups, fallback.choice)}${until} · ${VENDOR_LABELS[fallback.replaces.provider]} limit`;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          data-slot="stand-in-model"
          aria-label={`${text}. ${fallback.reason} Open the Usage page.`}
          onClick={() => openSettings("usage")}
          className="h-pill px-pill rounded-capsule bg-warning/15 text-warning hover:bg-warning/25 inline-flex max-w-sm min-w-0 shrink items-center gap-1 text-xs transition-colors"
        >
          <Shuffle aria-hidden className="size-icon-xs shrink-0" />
          {/* A narrow composer keeps the icon; the rest is on hover. */}
          <span className="@lg/composer:inline hidden truncate">{text}</span>
        </button>
      </TooltipTrigger>
      <TooltipContent side="top" className="max-w-xs flex-col items-start gap-0.5">
        <span className="font-medium">{text}</span>
        <span>{fallback.reason}</span>
        <span className="text-muted-foreground text-xs">
          Your choice, {choiceName(groups, fallback.replaces)}, takes over again
          {fallback.untilMs !== null ? ` at ${formatResetAt(fallback.untilMs, now)}` : " when its limit resets"}.
        </span>
      </TooltipContent>
    </Tooltip>
  );
}

/**
 * Messages waiting for quota: the model is at its limit and no model the user allows can stand
 * in. "Waiting for quota · 21:10", why on hover. They go on their own; Stop drops them.
 */
function WaitingPill({ wait }: { wait: QuotaWait }) {
  const now = useNow(60_000);
  const until = wait.resetsAtMs !== null ? ` · ${formatResetAt(wait.resetsAtMs, now)}` : "";
  const text = `Waiting for quota${until}`;
  const reason = withResetTime(wait.reason, wait.resetsAtMs, now);
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          data-slot="quota-wait-pill"
          aria-label={`${text}. ${reason} Open Routing settings.`}
          onClick={() => openSettings("routing")}
          className="h-pill px-pill rounded-capsule bg-warning/15 text-warning hover:bg-warning/25 inline-flex max-w-sm min-w-0 shrink items-center gap-1 text-xs transition-colors"
        >
          <Clock aria-hidden className="size-icon-xs shrink-0" />
          {/* A narrow composer keeps the icon; the rest is on hover. */}
          <span className="@lg/composer:inline hidden truncate">{text}</span>
        </button>
      </TooltipTrigger>
      <TooltipContent side="top" className="max-w-xs flex-col items-start gap-0.5">
        <span className="font-medium">{text}</span>
        <span>{reason}</span>
        {wait.rule && <span>Your rule keeps it from other models: {wait.rule}</span>}
        {wait.ranking && <span>Kept for the models in {wait.ranking}.</span>}
        <span className="text-muted-foreground text-xs">
          Your messages go on their own once a model can take them; Stop drops them.
        </span>
      </TooltipContent>
    </Tooltip>
  );
}

/** Whether the user doesn't trust the folder `repo` of the project `projectId`. */
export function useUntrusted(projectId: string | null, repo: string | null): boolean {
  return useApp(
    (s) => projectId !== null && repo !== null && folderTrust(s.projects[projectId], repo) === false,
  );
}

/** Changes a started session's permission level: the composer's picker, and the thread's notice. */
export async function setSessionPermission(
  conversation: Conversation,
  permission: PermissionLevel,
): Promise<void> {
  const setup = conversation.setup;
  if (setup?.type !== "session") return;
  await updateSetup(conversation.id, { ...setup, permission });
}

/** Permission level of a started session. */
export function ConversationPermissionPicker({ conversation }: { conversation: Conversation }) {
  const action = useAction();
  const setup = conversation.setup;
  const untrusted = useUntrusted(
    conversation.projectId,
    setup?.type === "session" ? setup.repo : null,
  );
  if (setup?.type !== "session") return null;
  return (
    <>
      <PermissionPicker
        value={setup.permission}
        untrusted={untrusted}
        onChange={(permission) => action.run(() => setSessionPermission(conversation, permission))}
      />
      {action.error && (
        <span role="alert" className="text-destructive max-w-xs truncate text-xs" title={action.error}>
          {action.error}
        </span>
      )}
    </>
  );
}
