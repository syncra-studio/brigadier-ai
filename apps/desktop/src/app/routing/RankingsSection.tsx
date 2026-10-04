import { ChevronRight, Reload } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { SourceLink } from "@/app/routing/models";
import {
  changedModels,
  fieldWords,
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
import type { MergedModel, RankingsRefresh, RatingChange } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useRankingsRefresh } from "@/state/rankings";

/** The section's row, for Settings search; the section renders this copy. */
export const RANKINGS_ROW = {
  label: "Model ratings",
  description:
    "Refresh rankings researches the web with one of your enabled models and updates the models' ratings. Your manual rankings are not changed.",
} as const;

const STATUS_TONES: Record<RefreshStatus["tone"], string> = {
  running: "text-foreground/80",
  done: "text-foreground",
  warning: "text-warning",
  error: "text-destructive",
};

/**
 * Model ratings: "Refresh rankings" (a web research run that re-rates the models), what it is
 * doing or how it ended, and which ratings are in use, with a way back to the curated ones.
 */
export function RankingsSection({ merged }: { merged: readonly MergedModel[] | null }) {
  const { refresh, error, start, reset } = useRankingsRefresh();
  const starting = useAction();
  const [resetting, setResetting] = useState(false);
  const running = refresh !== null && refreshRunning(refresh.state);
  const status = refresh && refreshStatus(refresh);
  return (
    <SettingsSection
      title={RANKINGS_ROW.label}
      description={RANKINGS_ROW.description}
      actions={
        <SettingsButton
          disabled={running || starting.busy || refresh === null}
          onClick={() => starting.run(start)}
        >
          <Reload
            className={running || starting.busy ? "animate-spin motion-reduce:animate-none" : undefined}
          />
          {running ? "Refreshing…" : "Refresh rankings"}
        </SettingsButton>
      }
    >
      {(starting.error ?? error) && (
        <p role="alert" className="text-destructive text-xs">
          {starting.error ?? error}
        </p>
      )}
      {/* Always there, so screen readers hear each change of it. */}
      <p role="status" aria-live="polite" className={cn("px-1 text-xs", status && STATUS_TONES[status.tone])}>
        {status?.text}
      </p>
      {refresh && (
        <SettingsCard>
          {refresh.state === "done" && <ChangedModels changes={refresh.changes} merged={merged} />}
          {refresh.errors.length > 0 && !running && <RefreshErrors refresh={refresh} />}
          <SettingsRow label="Ratings in use" description={overlayStatus(refresh)}>
            {refresh.overlayApplied && (
              <SettingsButton onClick={() => setResetting(true)}>Reset to curated ratings</SettingsButton>
            )}
          </SettingsRow>
        </SettingsCard>
      )}
      <ResetRatingsDialog
        open={resetting}
        running={running}
        onOpenChange={setResetting}
        onConfirm={reset}
      />
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
          <DialogTitle>Reset to curated ratings?</DialogTitle>
          <DialogDescription>
            The researched ratings are discarded and every model goes back to its curated rating.
            {running && " The refresh that is running stops."} Your manual rankings stay as they
            are.
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
            Reset to curated ratings
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
