import { ChevronRight, Reload } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { RegistryRow, SourceLink } from "@/app/routing/models";
import {
  changedModels,
  fieldWords,
  lastRefresh,
  overlayStatus,
  refreshRunning,
  refreshStatus,
  type RefreshStatus,
} from "@/app/routing/rankings";
import { SettingsButton, SettingsCard, SettingsRow, SettingsSection } from "@/app/settings/parts";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { MergedModel, RankingsRefresh, RatingChange, RegistryInfo } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import type { RankingsRefreshView } from "@/state/rankings";

/** The rankings rows, for Settings search; the page renders this copy. */
export const RANKINGS_ROWS = {
  refresh: {
    label: "Refresh rankings",
    description:
      "Looks up the latest on each model on the web with one of your models, then updates how Brigadier rates them. Your own choices stay as they are.",
  },
  ratings: {
    label: "Model ratings",
    description:
      "Which ratings are in use, what the last refresh changed and where it read it, and going back to the published ratings.",
  },
} as const;

const STATUS_TONES: Record<RefreshStatus["tone"], string> = {
  running: "text-foreground/80",
  done: "text-foreground/65",
  warning: "text-warning",
  error: "text-destructive",
};

/** The display name of the model a refresh researched with, when the models list has it. */
function researcherName(refresh: RankingsRefresh, merged: readonly MergedModel[] | null): string | null {
  if (refresh.model === null) return null;
  const model = merged?.find(
    (entry) =>
      entry.provider === refresh.provider && (entry.id === refresh.model || entry.resolved === refresh.model),
  );
  return model?.displayName ?? refresh.model;
}

/** "Refresh rankings": starts a refresh, or shows that one is running. */
export function RefreshRankingsButton({ rankings }: { rankings: RankingsRefreshView }) {
  const { refresh, starting, start } = rankings;
  const running = refresh !== null && refreshRunning(refresh.state);
  return (
    <SettingsButton disabled={running || starting.busy || refresh === null} onClick={() => starting.run(start)}>
      <Reload className={running || starting.busy ? "animate-spin motion-reduce:animate-none" : undefined} />
      {running ? "Refreshing…" : RANKINGS_ROWS.refresh.label}
    </SettingsButton>
  );
}

/** One line under the button: what the refresh is doing, or how the last one ended. */
export function RefreshStatusLine({
  rankings,
  merged,
}: {
  rankings: RankingsRefreshView;
  merged: readonly MergedModel[] | null;
}) {
  const { refresh, error, starting } = rankings;
  const status = refresh && refreshStatus(refresh, researcherName(refresh, merged));
  const failure = starting.error ?? error;
  return (
    <>
      {failure && (
        <p role="alert" className="text-destructive px-1 text-xs">
          {failure}
        </p>
      )}
      {/* Always there, so screen readers hear each change of it. */}
      <p role="status" aria-live="polite" className={cn("px-1 text-xs empty:hidden", status && STATUS_TONES[status.tone])}>
        {status?.text}
      </p>
    </>
  );
}

/**
 * Under Advanced: which ratings are in use (with a way back to the published ones), the last
 * refresh with what it changed and its sources, and the model registry they build on.
 */
export function RatingsSection({
  rankings,
  merged,
  registry,
  now,
}: {
  rankings: RankingsRefreshView;
  merged: readonly MergedModel[] | null;
  registry: RegistryInfo | null;
  now: number;
}) {
  const { refresh, reset } = rankings;
  const [resetting, setResetting] = useState(false);
  const running = refresh !== null && refreshRunning(refresh.state);
  const last = refresh && lastRefresh(refresh, researcherName(refresh, merged));
  return (
    <SettingsSection title={RANKINGS_ROWS.ratings.label} description={RANKINGS_ROWS.ratings.description}>
      <SettingsCard>
        {refresh && (
          <SettingsRow label="Ratings in use" description={overlayStatus(refresh)}>
            {refresh.overlayApplied && (
              <SettingsButton onClick={() => setResetting(true)}>Use published ratings</SettingsButton>
            )}
          </SettingsRow>
        )}
        {last && <SettingsRow label="Last refresh" description={last} />}
        {refresh?.state === "done" && <ChangedModels changes={refresh.changes} merged={merged} />}
        {refresh && refresh.errors.length > 0 && !running && <RefreshErrors refresh={refresh} />}
        {registry && <RegistryRow registry={registry} now={now} />}
      </SettingsCard>
      <ResetRatingsDialog open={resetting} running={running} onOpenChange={setResetting} onConfirm={reset} />
    </SettingsSection>
  );
}

/** The model a change names, by its display name when the models list has it. */
function changeName(change: RatingChange, merged: readonly MergedModel[] | null): string {
  const model = merged?.find(
    (entry) =>
      entry.provider === change.provider &&
      (entry.id === change.model || entry.resolved === change.model),
  );
  return model?.displayName ?? change.model;
}

/** The models a done refresh changed, folded away: each with its changed fields and sources. */
function ChangedModels({
  changes,
  merged,
}: {
  changes: readonly RatingChange[];
  merged: readonly MergedModel[] | null;
}) {
  const changed = changedModels(changes);
  if (changed.length === 0) return null;
  return (
    <Collapsible>
      <CollapsibleTrigger className="text-muted-foreground hover:text-foreground group flex w-full items-center gap-1.5 px-4 py-2.5 text-xs">
        <ChevronRight
          aria-hidden
          className="size-icon-xs transition-transform group-data-[state=open]:rotate-90 motion-reduce:transition-none"
        />
        What changed
      </CollapsibleTrigger>
      <CollapsibleContent>
        <ul className="flex flex-col gap-3 px-4 pb-3 ps-9 text-xs">
          {changed.map((change) => (
            <li key={`${change.provider}/${change.model}`} className="flex min-w-0 flex-col gap-0.5">
              <span>
                <span className="font-medium">{changeName(change, merged)}</span>
                <span className="text-muted-foreground">: {change.fields.map(fieldWords).join(", ")}</span>
              </span>
              {change.sources.length > 0 && (
                <ul className="flex flex-col gap-0.5">
                  {change.sources.map((source) => (
                    <li key={source} className="min-w-0">
                      <SourceLink url={source} />
                    </li>
                  ))}
                </ul>
              )}
            </li>
          ))}
        </ul>
      </CollapsibleContent>
    </Collapsible>
  );
}

/** Why a refresh failed, or what a finished one left out. */
function RefreshErrors({ refresh }: { refresh: RankingsRefresh }) {
  const failed = refresh.state === "failed";
  return (
    <div className="flex flex-col gap-1 px-4 py-3 text-xs">
      <p className={failed ? "text-destructive" : "text-warning"}>
        {failed ? "What went wrong:" : "Some findings were left out:"}
      </p>
      <ul className="text-muted-foreground flex list-disc flex-col gap-0.5 ps-4">
        {refresh.errors.map((error, index) => (
          // oxlint-disable-next-line react/no-array-index-key -- the same message may repeat.
          <li key={index} className="break-words">
            {error}
          </li>
        ))}
      </ul>
    </div>
  );
}

function ResetRatingsDialog({
  open,
  running,
  onOpenChange,
  onConfirm,
}: {
  open: boolean;
  /** A refresh is running: resetting stops it. */
  running: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => Promise<void>;
}) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const confirm = async () => {
    setBusy(true);
    setError(null);
    try {
      await onConfirm();
      onOpenChange(false);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>Use the published ratings?</DialogTitle>
          <DialogDescription>
            The researched ratings are discarded and every model goes back to its published rating.
            {running && " The refresh that is running stops."} Your own choices stay as they are.
          </DialogDescription>
        </DialogHeader>
        {error && (
          <p role="alert" className="text-destructive text-xs">
            {error}
          </p>
        )}
        <DialogFooter>
          <Button type="button" variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="button" size="sm" disabled={busy} onClick={() => void confirm()}>
            Use published ratings
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
