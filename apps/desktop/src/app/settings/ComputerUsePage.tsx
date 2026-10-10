import {
  ArrowRotateCw,
  CheckCircleFilled,
  Click,
  ExclamationMarkCircle,
  Eye,
  HandRaised,
  InfoCircle,
  Reload,
} from "@openai/apps-sdk-ui/components/Icon";
import { useState, type ReactNode } from "react";

import { useAction } from "@/app/conversation/useAction";
import { ErrorLine } from "@/app/dialogs/fields";
import { SettingsButton, SettingsCard, SettingsPage } from "@/app/settings/parts";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import type { ComputerAccess, ComputerGrant } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import {
  allowComputerAccess,
  checkComputerAccess,
  openComputerSettings,
  useLiveComputerAccess,
} from "@/state/computerAccess";

/** The Computer use page's rows, for the page and for Settings search. */
export const COMPUTER_USE_ROWS = {
  accessibility: {
    label: "Control apps",
    description: "Lets workers click, type and read app windows.",
  },
  screenRecording: {
    label: "See the screen",
    description: "Lets workers look at app windows to check their work.",
  },
} as const;

const ICONS: Record<ComputerGrant, ReactNode> = {
  accessibility: <Click />,
  screenRecording: <Eye />,
};

const INTRO =
  "Lets workers see and use apps on this Mac in the background. Your cursor and keyboard stay yours.";

const GRANTS: readonly ComputerGrant[] = ["accessibility", "screenRecording"];

/** What to do in System Settings after an Allow (the conversation's "Waiting on you" item says it too). */
export const GRANT_STEPS =
  "System Settings opens on the right list: Device Control and Data Access (called Accessibility before macOS 27) for Control apps, Screen & System Audio Recording for See the screen. Turn on Brigadier Computer Use there; this page updates by itself.";
/** Where the helper is listed, on the footnote's tip. */
export const WHERE_LISTED =
  "Control apps is under Privacy & Security → Device Control and Data Access (Accessibility before macOS 27). See the screen is under Screen & System Audio Recording.";
export const STALE_ENTRY = "Turned it on, but it still says Not allowed?";
export const RESTARTING = "Brigadier Computer Use is restarting to use the permission you just gave.";

/** Asks for a permission; `startOver` first forgets Brigadier Computer Use's old entry for it. */
export type OnAllow = (grant: ComputerGrant, startOver: boolean) => Promise<void>;

type Tone = "ready" | "needed" | "restarting" | "problem";

/** What the status card says: a heading and one line, in plain words. */
export function computerUseStatus(access: ComputerAccess): { tone: Tone; title: string; detail: string } {
  if (access.problem) return { tone: "problem", title: "Couldn't check permissions", detail: access.problem };
  if (access.restarting) return { tone: "restarting", title: "Almost ready", detail: RESTARTING };
  const missing = GRANTS.filter((grant) => !access[grant]).length;
  if (missing === 0) return { tone: "ready", title: "Ready", detail: "Workers can see and use apps on this Mac." };
  return {
    tone: "needed",
    title: "Finish setup",
    detail: missing === 1 ? "1 permission needed" : `${missing} permissions needed`,
  };
}

// Missing permissions raise the hand the conversation's "Waiting on you" item does.
const TONE_ICON: Record<Tone, ReactNode> = {
  ready: <CheckCircleFilled />,
  needed: <HandRaised />,
  restarting: <ArrowRotateCw className="animate-spin [animation-duration:2s] motion-reduce:animate-none" />,
  problem: <ExclamationMarkCircle />,
};

const TONE_TILE: Record<Tone, string> = {
  ready: "bg-success/15 text-success",
  needed: "bg-warning/15 text-warning",
  restarting: "bg-foreground/5 text-foreground/80",
  problem: "bg-destructive/15 text-destructive",
};

/** A row's icon on its square tile. */
function Tile({ className, children }: { className?: string; children: ReactNode }) {
  return (
    <span
      aria-hidden
      className={cn(
        "rounded-nav [&_svg]:size-icon-md flex size-8 shrink-0 items-center justify-center",
        className,
      )}
    >
      {children}
    </span>
  );
}

function StatusCard({ access, onRefresh }: { access: ComputerAccess; onRefresh: () => Promise<void> }) {
  const refresh = useAction();
  const status = computerUseStatus(access);
  return (
    <SettingsCard>
      <div data-slot="computer-use-status" className="flex items-center gap-3 px-4 py-3">
        <Tile className={TONE_TILE[status.tone]}>{TONE_ICON[status.tone]}</Tile>
        {/* Announced as it changes: a switch turned on in System Settings shows here by itself. */}
        <div role="status" className="flex min-w-0 flex-1 flex-col gap-0.5">
          <div className="text-label font-medium">{status.title}</div>
          <div className="text-foreground/65 text-xs break-words">{status.detail}</div>
          <ErrorLine error={refresh.error} />
        </div>
        <TooltipIconButton
          tooltip="Check again"
          side="left"
          disabled={refresh.busy}
          onClick={() => refresh.run(onRefresh)}
          className="text-foreground/65 hover:text-foreground"
        >
          <Reload className={cn(refresh.busy && "animate-spin motion-reduce:animate-none")} />
        </TooltipIconButton>
      </div>
    </SettingsCard>
  );
}

function GrantRow({
  grant,
  allowed,
  unknown,
  asked,
  onAllow,
  onOpen,
}: {
  grant: ComputerGrant;
  allowed: boolean;
  /** The permissions couldn't be read: neither allowed nor not is known. */
  unknown: boolean;
  asked: boolean;
  onAllow: OnAllow;
  onOpen: (grant: ComputerGrant) => Promise<void>;
}) {
  const allow = useAction();
  const open = useAction();
  const row = COMPUTER_USE_ROWS[grant];
  return (
    <div
      data-slot="settings-row"
      // Settings search scrolls to the row it found by this.
      data-setting={row.label}
      className="flex items-center gap-3 px-4 py-3"
    >
      <Tile className="bg-foreground/5 text-foreground/80">{ICONS[grant]}</Tile>
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
          <span className="text-label font-medium">{row.label}</span>
          {allowed ? (
            <Badge variant="success">
              <span aria-hidden className="bg-success size-1.5 rounded-full" />
              Allowed
            </Badge>
          ) : (
            <Badge variant="outline" className="border-divider text-foreground/65">
              <span aria-hidden className="bg-foreground/35 size-1.5 rounded-full" />
              {unknown ? "Not checked" : "Not allowed"}
            </Badge>
          )}
        </div>
        <div className="text-foreground/65 text-xs break-words">{row.description}</div>
        <ErrorLine error={allow.error ?? open.error} />
      </div>
      <div className="flex shrink-0 items-center gap-1">
        {(allowed || asked) && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={open.busy}
            onClick={() => open.run(() => onOpen(grant))}
            className="rounded-nav text-label text-foreground/65 hover:bg-foreground/8 hover:text-foreground font-normal"
          >
            Open System Settings
          </Button>
        )}
        {!allowed && (
          <SettingsButton
            disabled={allow.busy}
            onClick={() => allow.run(() => onAllow(grant, false))}
            className="bg-primary text-primary-foreground hover:bg-primary/90"
          >
            Allow…
          </SettingsButton>
        )}
      </div>
    </div>
  );
}

/** Start over, for a permission that was asked for and is still missing: a quiet line under the rows. */
function StartOver({ grants, onAllow }: { grants: readonly ComputerGrant[]; onAllow: OnAllow }) {
  const start = useAction();
  return (
    <div className="text-foreground/50 text-label flex flex-col gap-1 px-1">
      <p>
        {STALE_ENTRY}{" "}
        {grants.map((grant, index) => (
          <span key={grant}>
            {index > 0 && " · "}
            <button
              type="button"
              disabled={start.busy}
              onClick={() => start.run(() => onAllow(grant, true))}
              className="text-foreground/65 hover:text-foreground rounded-xs underline underline-offset-2 disabled:opacity-50"
            >
              {grants.length === 1 ? "Start over" : `Start over for ${COMPUTER_USE_ROWS[grant].label}`}
            </button>
          </span>
        ))}
        : Brigadier Computer Use forgets its old entry, and macOS asks again.
      </p>
      <ErrorLine error={start.error} />
    </div>
  );
}

/** The page's content for the permissions `access` reports; nothing where there is no computer use. */
export function ComputerUseBody({
  access,
  asked,
  onAllow,
  onOpen,
  onRefresh,
}: {
  access: ComputerAccess | null;
  /** The permissions whose Allow was clicked: the user went to System Settings for them. */
  asked: ReadonlySet<ComputerGrant>;
  onAllow: OnAllow;
  onOpen: (grant: ComputerGrant) => Promise<void>;
  onRefresh: () => Promise<void>;
}) {
  if (!access?.available) return null;
  const stale = access.problem ? [] : GRANTS.filter((grant) => asked.has(grant) && !access[grant]);
  return (
    <SettingsPage title="Computer use" description={INTRO}>
      <div className="flex flex-col gap-3">
        <StatusCard access={access} onRefresh={onRefresh} />
        <SettingsCard>
          {GRANTS.map((grant) => (
            <GrantRow
              key={grant}
              grant={grant}
              allowed={access[grant]}
              unknown={access.problem !== null}
              asked={asked.has(grant)}
              onAllow={onAllow}
              onOpen={onOpen}
            />
          ))}
        </SettingsCard>
        {stale.length > 0 && <StartOver grants={stale} onAllow={onAllow} />}
        <p className="text-foreground/50 text-label px-1">
          Computer use runs in a separate helper,{" "}
          <span className="text-foreground/65 font-medium">Brigadier Computer Use</span>. That's the name to look
          for in System Settings.{" "}
          <TooltipProvider delayDuration={0}>
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  type="button"
                  aria-label="Where to find it"
                  className="text-foreground/50 hover:text-foreground [&_svg]:size-icon-sm inline-flex rounded-xs align-text-bottom"
                >
                  <InfoCircle />
                </button>
              </TooltipTrigger>
              <TooltipContent side="top" className="text-start">
                {WHERE_LISTED}
              </TooltipContent>
            </Tooltip>
          </TooltipProvider>
        </p>
      </div>
    </SettingsPage>
  );
}

export function ComputerUsePage() {
  // Kept current while shown: a switch turned on in System Settings shows here by itself.
  const access = useLiveComputerAccess();
  const [asked, setAsked] = useState<ReadonlySet<ComputerGrant>>(new Set());

  return (
    <ComputerUseBody
      access={access}
      asked={asked}
      onAllow={(grant, startOver) => {
        setAsked((was) => new Set(was).add(grant));
        return allowComputerAccess(grant, startOver);
      }}
      onOpen={openComputerSettings}
      onRefresh={checkComputerAccess}
    />
  );
}
