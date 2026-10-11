import { Check, ChevronRight, FolderOpen, Minus } from "@openai/apps-sdk-ui/components/Icon";
import { Checkbox as CheckboxPrimitive } from "radix-ui";
import { useCallback, useEffect, useId, useRef, useState, type ReactNode } from "react";

import { ErrorLine, errorText } from "@/app/dialogs/fields";
import {
  ALL_GROUPS,
  action,
  confirmation,
  count,
  failure,
  freeLine,
  grouped,
  picked,
  result,
  sweep,
  usageParts,
  type Sweep,
} from "@/app/settings/freeUpSpace";
import {
  SettingsAdvanced,
  SettingsButton,
  SettingsCard,
  SettingsPage,
  SettingsRow,
  SettingsSection,
} from "@/app/settings/parts";
import { Spinner } from "@/components/glyphs/spinner";
import { Badge } from "@/components/ui/badge";
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
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { revealPath } from "@/ipc/client";
import type { CleanItem, CleanReport, SharedPart, StorageReport } from "@/ipc/generated";
import { formatBytes, shortPath } from "@/lib/format";
import { cn } from "@/lib/utils";
import { openSettings } from "@/state/actions";
import { cleanStorage, openUninstall, scanStorage } from "@/state/storage";
import { useApp } from "@/state/store";

/** The Storage page's rows, for the page and for Settings search. */
export const STORAGE_ROWS = {
  computer: {
    label: "This computer",
    description: "What Brigadier keeps on this computer, and what of it can go",
  },
  storage: {
    label: "Free up space",
    description: "Review what can go before anything is removed",
  },
  archived: {
    label: "Archived chats",
    description: "Restore or delete archived sessions and chats",
  },
  choose: {
    label: "Choose items yourself",
    description: "Anything that can be removed, including Brains, models and recordings",
  },
  usage: {
    label: "Disk use per project",
    description: "What each project's work folders, Brain and conversations use",
  },
  uninstall: {
    label: "Uninstall Brigadier",
    description:
      "Removes what Brigadier created on this computer, then the app; your repositories, your own branches and your files stay",
  },
} as const;

const SHARED: Record<SharedPart, string> = {
  database: "Database",
  otherBlobs: "Other stored files",
  models: "Downloaded models",
  logs: "Logs",
  recordings: "Recordings",
  personalBrain: "Personal Brain",
  other: "Everything else",
};

/**
 * Settings → Storage. What Brigadier uses on this computer (a bar of its parts, scanned when the
 * page opens and again on Refresh), then a row to free up space: Review opens what can go, in a
 * few plain groups, and what is kept on purpose with why; its one button removes the safe part
 * and the dialog then says what it freed. Choosing items by hand and disk use per project are
 * under Advanced.
 */
export function StoragePage() {
  const [report, setReport] = useState<StorageReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [scanning, setScanning] = useState(true);
  const [cleaning, setCleaning] = useState(false);
  const [cleaned, setCleaned] = useState<CleanReport | null>(null);
  const [cleanError, setCleanError] = useState<string | null>(null);
  // Only the latest scan's answer counts.
  const latest = useRef(0);

  // Asks the daemon; the state says "looking" already (on open) or is set so by `scan`.
  const look = useCallback(() => {
    const id = ++latest.current;
    scanStorage().then(
      (scanned) => {
        if (id !== latest.current) return;
        setReport(scanned);
        setScanning(false);
      },
      (cause: unknown) => {
        if (id !== latest.current) return;
        setError(`Couldn't look: ${errorText(cause)}`);
        setScanning(false);
      },
    );
  }, []);
  useEffect(look, [look]);
  const scan = () => {
    setScanning(true);
    setError(null);
    look();
  };

  const clean = (scanId: string, ids: string[]) => {
    setCleaning(true);
    setCleanError(null);
    cleanStorage(scanId, ids).then(
      (done) => {
        setCleaning(false);
        setCleaned(done);
      },
      (cause: unknown) => {
        setCleaning(false);
        setCleanError(errorText(cause));
      },
    );
  };

  return (
    <SettingsPage title="Storage">
      <StorageBody
        report={report}
        error={error}
        scanning={scanning}
        cleaning={cleaning}
        cleaned={cleaned}
        cleanError={cleanError}
        onRefresh={scan}
        onClean={clean}
        onDone={() => {
          // A finished clean changed the disk: look again.
          setCleaned(null);
          setCleanError(null);
          if (cleaned) scan();
        }}
      />
      <SettingsSection>
        <SettingsCard>
          <SettingsRow
            label={STORAGE_ROWS.uninstall.label}
            description={STORAGE_ROWS.uninstall.description}
          >
            <SettingsButton destructive onClick={() => openUninstall()}>
              Uninstall…
            </SettingsButton>
          </SettingsRow>
        </SettingsCard>
      </SettingsSection>
    </SettingsPage>
  );
}

/**
 * What a dialog asks to remove: the sweep (Review) or items chosen by hand, with the scan they
 * came from (a Refresh that lands while the dialog is open gives the items other ids).
 */
type Ask = { title: string; plan: Sweep; scanId: string };

/** The page for one scan: what is used, freeing up space, Advanced. */
export function StorageBody({
  report,
  error,
  scanning,
  cleaning,
  cleaned,
  cleanError,
  onRefresh,
  onClean,
  onDone,
}: {
  report: StorageReport | null;
  error: string | null;
  scanning: boolean;
  cleaning: boolean;
  cleaned: CleanReport | null;
  cleanError: string | null;
  onRefresh: () => void;
  onClean: (scanId: string, ids: string[]) => void;
  /** The dialog closed after a clean or a failed one. */
  onDone: () => void;
}) {
  const [ask, setAsk] = useState<Ask | null>(null);
  const [choosing, setChoosing] = useState(false);
  const [viewingUsage, setViewingUsage] = useState(false);
  const plan = report ? sweep(report) : null;

  return (
    <>
      <SettingsSection title={STORAGE_ROWS.computer.label} description={STORAGE_ROWS.computer.description}>
        <div data-setting={STORAGE_ROWS.computer.label} className="flex flex-col gap-3 pt-2">
          <div className="flex min-h-7 items-center justify-between gap-4">
            <span role="status" className="text-label flex min-w-0 items-center gap-2 font-semibold">
              {!report && !error && <Spinner className="size-icon-sm animate-spin" />}
              {report
                ? `${formatBytes(report.totalBytes)} used by Brigadier`
                : (error ?? "Looking at what Brigadier keeps on this computer…")}
            </span>
            <SettingsButton onClick={onRefresh} disabled={scanning}>
              {scanning && report ? (
                <>
                  <Spinner className="size-icon-sm animate-spin" />
                  Updating…
                </>
              ) : report || !error ? (
                "Refresh"
              ) : (
                "Try again"
              )}
            </SettingsButton>
          </div>
          {report && <UsageBar report={report} />}
          {report && error && <ErrorLine error={error} />}
        </div>
      </SettingsSection>

      <div className="flex flex-col gap-4">
        <SettingsCard>
          <SettingsRow
            label={STORAGE_ROWS.storage.label}
            description={plan ? freeLine(plan) : STORAGE_ROWS.storage.description}
          >
            <SettingsButton
              disabled={!report || cleaning}
              onClick={() => report && plan && setAsk({ title: "Free up space", plan, scanId: report.scanId })}
            >
              Review
            </SettingsButton>
          </SettingsRow>
        </SettingsCard>
        <SettingsCard>
          <SettingsRow label={STORAGE_ROWS.archived.label} description={STORAGE_ROWS.archived.description}>
            <SettingsButton onClick={() => openSettings("archived")}>Manage</SettingsButton>
          </SettingsRow>
        </SettingsCard>
      </div>

      <SettingsAdvanced
        description="Choose items yourself, and see disk use per project."
        labels={[STORAGE_ROWS.choose.label, STORAGE_ROWS.usage.label]}
      >
        <SettingsCard>
          <SettingsRow label={STORAGE_ROWS.choose.label} description={STORAGE_ROWS.choose.description}>
            <SettingsButton disabled={!report || cleaning} onClick={() => setChoosing(true)}>
              Choose…
            </SettingsButton>
          </SettingsRow>
          <SettingsRow label={STORAGE_ROWS.usage.label} description={STORAGE_ROWS.usage.description}>
            <SettingsButton disabled={!report} onClick={() => setViewingUsage(true)}>
              View
            </SettingsButton>
          </SettingsRow>
        </SettingsCard>
      </SettingsAdvanced>

      {report && (
        <ChooseDialog
          // Each opening starts from the safe items picked.
          key={choosing ? report.scanId : "closed"}
          open={choosing}
          report={report}
          scanning={scanning}
          onCancel={() => setChoosing(false)}
          onRemove={(ids) => {
            setChoosing(false);
            setAsk({ title: "Remove chosen items", plan: picked(report, ids), scanId: report.scanId });
          }}
        />
      )}
      {report && <UsageDialog open={viewingUsage} report={report} onClose={() => setViewingUsage(false)} />}
      <CleanDialog
        ask={ask}
        cleaning={cleaning}
        cleaned={cleaned}
        error={cleanError}
        onConfirm={() => ask && onClean(ask.scanId, ask.plan.ids)}
        onClose={() => {
          if (cleaning) return;
          setAsk(null);
          if (cleaned || cleanError) onDone();
        }}
      />
    </>
  );
}

/** The bar of what Brigadier uses, its legend and the data folder. */
function UsageBar({ report }: { report: StorageReport }) {
  const parts = usageParts(report);
  const total = Math.max(1, parts.reduce((sum, part) => sum + part.bytes, 0));
  const mac = useApp((s) => s.info?.platform === "macos");
  return (
    <>
      <div
        data-slot="storage-bar"
        role="img"
        aria-label={parts.map((part) => `${part.label} ${formatBytes(part.bytes)}`).join(", ")}
        className="bg-foreground/8 flex h-6 overflow-hidden rounded-full"
      >
        {parts
          .filter((part) => part.bytes > 0)
          .map((part) => (
            <span
              key={part.key}
              className={`${part.color} h-full min-w-1 shrink-0`}
              style={{ width: `${(part.bytes / total) * 100}%` }}
            />
          ))}
      </div>
      <ul className="flex flex-wrap gap-x-4 gap-y-1.5">
        {parts.map((part) => (
          <li key={part.key} className="grid grid-cols-[auto_auto] items-center gap-x-1.5 gap-y-0.5">
            <span aria-hidden className={`${part.color} size-2 rounded-full`} />
            <span className="text-foreground/65 text-label">{part.label}</span>
            <span className="text-foreground/65 col-start-1 col-end-3 text-xs tabular-nums">
              {formatBytes(part.bytes)}
            </span>
          </li>
        ))}
      </ul>
      {report.dataDir && (
        <button
          type="button"
          title={mac ? "Reveal in Finder" : "Show in folder"}
          className="text-foreground/65 hover:text-foreground w-fit text-start text-xs break-all"
          onClick={() => void revealPath(report.dataDir).catch(() => {})}
        >
          {report.dataDir}
        </button>
      )}
    </>
  );
}

/**
 * Review: what goes, group by group, and what is kept on purpose. The dialog is the
 * confirmation; after the clean it says what it freed and what stayed.
 */
function CleanDialog({
  ask,
  cleaning,
  cleaned,
  error,
  onConfirm,
  onClose,
}: {
  ask: Ask | null;
  cleaning: boolean;
  cleaned: CleanReport | null;
  error: string | null;
  onConfirm: () => void;
  onClose: () => void;
}) {
  return (
    <Dialog open={ask !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        data-slot="storage-dialog"
        className="flex max-h-full max-w-lg flex-col"
        showCloseButton={!cleaning}
      >
        {ask && cleaned ? (
          <Result cleaned={cleaned} onDone={onClose} />
        ) : (
          ask && (
            <Review ask={ask} cleaning={cleaning} error={error} onConfirm={onConfirm} onCancel={onClose} />
          )
        )}
      </DialogContent>
    </Dialog>
  );
}

function Review({
  ask,
  cleaning,
  error,
  onConfirm,
  onCancel,
}: {
  ask: Ask;
  cleaning: boolean;
  error: string | null;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const { plan } = ask;
  const empty = plan.ids.length === 0;
  return (
    <>
      <DialogHeader>
        <DialogTitle>{ask.title}</DialogTitle>
        <DialogDescription>
          {empty ? "Nothing to free up: Brigadier is tidy." : "These go. Open a group to see each item and why."}
        </DialogDescription>
      </DialogHeader>
      <div className="-mx-1 min-h-0 flex-1 overflow-y-auto px-1">
        <SettingsCard>
          {plan.groups.map((group) => (
            <GroupRow
              key={group.category}
              title={group.title}
              detail={count(group.items.length, "item", "items")}
              bytes={group.bytes}
            >
              {group.items.map((item) => (
                <ItemLine key={item.id} label={item.label} bytes={item.bytes} reason={item.reason} path={item.path} />
              ))}
            </GroupRow>
          ))}
          {plan.kept.length > 0 && (
            <GroupRow
              title="Kept on purpose"
              detail={count(plan.kept.length, "thing", "things")}
              bytes={plan.keptBytes}
              muted
            >
              {plan.kept.map((kept) => (
                <ItemLine key={kept.key} label={kept.label} bytes={kept.bytes} reason={kept.reason} path={kept.path} />
              ))}
            </GroupRow>
          )}
        </SettingsCard>
      </div>
      {!empty && <p className="text-foreground/65 text-xs">{confirmation(plan).footer}</p>}
      <ErrorLine error={error} />
      <DialogFooter>
        <Button type="button" variant="ghost" disabled={cleaning} onClick={onCancel}>
          Cancel
        </Button>
        <Button type="button" disabled={cleaning || empty} onClick={onConfirm}>
          {cleaning ? "Freeing up…" : action(plan)}
        </Button>
      </DialogFooter>
    </>
  );
}

/** A group's line, opening to its items. */
function GroupRow({
  title,
  detail,
  bytes,
  muted = false,
  children,
}: {
  title: string;
  detail: string;
  bytes: number;
  muted?: boolean;
  children: ReactNode;
}) {
  return (
    <Collapsible data-slot="storage-group">
      <CollapsibleTrigger className="group flex w-full items-center gap-3 px-4 py-3 text-start">
        <ChevronRight
          aria-hidden
          className="text-muted-foreground size-icon-sm shrink-0 transition-transform group-data-[state=open]:rotate-90 motion-reduce:transition-none"
        />
        <span className={muted ? "text-foreground/65 text-label min-w-0 flex-1" : "text-label min-w-0 flex-1 font-medium"}>
          {title}
        </span>
        <span className="text-foreground/65 shrink-0 text-xs">{detail}</span>
        <span className="text-label w-20 shrink-0 text-end tabular-nums">{formatBytes(bytes)}</span>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <ul className="grid gap-3 pe-4 ps-11 pb-3">{children}</ul>
      </CollapsibleContent>
    </Collapsible>
  );
}

/** One item of a group: what it is, its size, and why it goes (or stays). */
function ItemLine({
  label,
  bytes,
  reason,
  path,
}: {
  label: string;
  bytes: number;
  reason: string;
  path: string | null;
}) {
  return (
    <li className="flex items-start gap-3">
      <div className="grid min-w-0 flex-1 gap-0.5">
        <span className="text-label break-words">{label}</span>
        <span className="text-foreground/65 text-xs break-words">{reason}</span>
      </div>
      {path && <RevealButton path={path} />}
      <span className="text-foreground/65 w-20 shrink-0 pt-0.5 text-end text-xs tabular-nums">
        {bytes > 0 ? formatBytes(bytes) : ""}
      </span>
    </li>
  );
}

function RevealButton({ path }: { path: string }) {
  const mac = useApp((s) => s.info?.platform === "macos");
  return (
    <Button
      type="button"
      variant="ghost"
      size="xs"
      className="shrink-0"
      onClick={() => void revealPath(path).catch(() => {})}
    >
      {mac ? "Reveal in Finder" : "Show in folder"}
    </Button>
  );
}

/** What a clean freed, and what stayed with why. */
function Result({ cleaned, onDone }: { cleaned: CleanReport; onDone: () => void }) {
  const words = result(cleaned);
  return (
    <>
      <DialogHeader>
        <DialogTitle>{words.title}</DialogTitle>
        {words.lines.length > 0 && (
          <DialogDescription asChild>
            <div role="status" className="grid gap-1">
              {words.lines.map((line) => (
                <span key={line}>{line}</span>
              ))}
            </div>
          </DialogDescription>
        )}
      </DialogHeader>
      {cleaned.failures.length > 0 && (
        <ul className="-mx-1 grid min-h-0 flex-1 gap-2 overflow-y-auto px-1">
          {cleaned.failures.map((failed, index) => {
            const why = failure(failed);
            return (
              <li key={index} className="grid gap-0.5">
                <span className="text-label">{failed.label}</span>
                <span className="text-xs">{why.plain}</span>
                <span className="text-foreground/50 text-xs break-all">{why.detail}</span>
              </li>
            );
          })}
        </ul>
      )}
      <DialogFooter>
        <Button type="button" onClick={onDone}>
          Done
        </Button>
      </DialogFooter>
    </>
  );
}

/** Advanced: every item that can go, picked by hand (the safe ones start picked). */
function ChooseDialog({
  open,
  report,
  scanning,
  onCancel,
  onRemove,
}: {
  open: boolean;
  report: StorageReport;
  /** A new scan is on its way: the list starts over when it lands. */
  scanning: boolean;
  onCancel: () => void;
  onRemove: (ids: Set<string>) => void;
}) {
  const [chosen, setChosen] = useState(
    () => new Set(report.items.filter((item) => item.checked && item.selectable).map((item) => item.id)),
  );
  const selectable = report.items.filter((item) => item.selectable);
  const groups = grouped(selectable, ALL_GROUPS);
  const plan = picked(report, chosen);
  const pick = (ids: readonly string[], on: boolean) =>
    setChosen((set) => {
      const next = new Set(set);
      for (const id of ids) {
        if (on) next.add(id);
        else next.delete(id);
      }
      return next;
    });
  return (
    <Dialog open={open} onOpenChange={(next) => !next && onCancel()}>
      <DialogContent data-slot="storage-choose" className="flex max-h-full max-w-lg flex-col">
        <DialogHeader>
          <DialogTitle>Choose what to remove</DialogTitle>
          <DialogDescription>Pick what Brigadier may remove. Nothing goes until you confirm.</DialogDescription>
        </DialogHeader>
        <div className="-mx-1 grid min-h-0 flex-1 content-start gap-3 overflow-y-auto px-1">
          {groups.length === 0 ? (
            <p data-slot="storage-empty" className="text-foreground/65 py-6 text-center text-sm">
              Nothing can be removed right now.
            </p>
          ) : (
            groups.map((group) => (
              <PickGroup
                key={group.category}
                title={group.title}
                bytes={group.bytes}
                items={group.items}
                chosen={chosen}
                onPick={pick}
              />
            ))
          )}
        </div>
        <DialogFooter className="items-center">
          <span role="status" className="text-foreground/65 me-auto flex items-center gap-2 text-xs tabular-nums">
            {scanning ? (
              <>
                <Spinner className="size-icon-sm animate-spin" />
                Updating…
              </>
            ) : (
              selectable.length > 0 &&
              `${plan.ids.length} of ${selectable.length} selected · ${formatBytes(plan.bytes)}`
            )}
          </span>
          <Button type="button" variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button type="button" disabled={plan.ids.length === 0 || scanning} onClick={() => onRemove(chosen)}>
            {plan.ids.length === 0 ? "Review" : `Review ${count(plan.ids.length, "item", "items")}`}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** A group of the items to choose from: its own box picks or leaves all of them. */
function PickGroup({
  title,
  bytes,
  items,
  chosen,
  onPick,
}: {
  title: string;
  bytes: number;
  items: readonly CleanItem[];
  chosen: ReadonlySet<string>;
  onPick: (ids: readonly string[], on: boolean) => void;
}) {
  const ids = items.map((item) => item.id);
  const on = ids.filter((id) => chosen.has(id)).length;
  const state = on === 0 ? false : on === ids.length ? true : "indeterminate";
  return (
    <SettingsCard>
      <PickLine
        checked={state}
        label={title}
        labelClass="font-medium"
        bytes={bytes}
        onCheckedChange={(next) => onPick(ids, next)}
      />
      {items.map((item) => (
        <PickLine
          key={item.id}
          checked={chosen.has(item.id)}
          label={item.label}
          badges={item.badges}
          note={plainWords(item)}
          bytes={item.bytes}
          path={item.path}
          inset
          onCheckedChange={(next) => onPick([item.id], next)}
        />
      ))}
    </SettingsCard>
  );
}

/** What happens to an item, in plain words: why it can go, and where it goes. */
function plainWords(item: CleanItem): string {
  const words = [item.reason.trim()];
  if (item.badges.some((badge) => badge.type === "hasChanges")) words.push("Its changes are kept as a commit on its branch first.");
  if (item.toTrash) words.push("Goes to the Trash.");
  return words.filter(Boolean).join(" ");
}

const BADGE_WORDS = {
  hasChanges: () => "Unsaved changes",
  notMerged: (ahead: number) => `Not merged · ${ahead} ahead`,
  legacy: () => "Older Brigadier",
};

/** One line to pick: its box, name and warnings, what it is, its size and where it is. */
function PickLine({
  checked,
  label,
  labelClass,
  badges = [],
  note,
  bytes,
  path = null,
  inset = false,
  onCheckedChange,
}: {
  checked: boolean | "indeterminate";
  label: string;
  labelClass?: string;
  badges?: readonly CleanItem["badges"][number][];
  note?: string;
  bytes: number;
  path?: string | null;
  /** An item under its group's line. */
  inset?: boolean;
  onCheckedChange: (checked: boolean) => void;
}) {
  const id = useId();
  return (
    <div data-slot="storage-pick" className={cn("flex items-start gap-3 py-3 pe-2", inset ? "ps-11" : "ps-4")}>
      <CheckboxPrimitive.Root
        id={id}
        checked={checked}
        // A group partly picked picks all of it.
        onCheckedChange={(state) => onCheckedChange(state !== false)}
        className="border-input data-[state=checked]:bg-primary data-[state=checked]:border-primary data-[state=checked]:text-primary-foreground data-[state=indeterminate]:bg-primary data-[state=indeterminate]:border-primary data-[state=indeterminate]:text-primary-foreground rounded-xs mt-0.5 flex size-icon-md shrink-0 items-center justify-center border"
      >
        <CheckboxPrimitive.Indicator>
          {checked === "indeterminate" ? <Minus className="size-icon-xs" /> : <Check className="size-icon-xs" />}
        </CheckboxPrimitive.Indicator>
      </CheckboxPrimitive.Root>
      <label htmlFor={id} className="grid min-w-0 flex-1 gap-0.5">
        <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
          <span className={cn("text-label break-words", labelClass)}>{label}</span>
          {badges.map((badge) => (
            <Badge key={badge.type} variant="warning">
              {badge.type === "notMerged" ? BADGE_WORDS.notMerged(badge.ahead) : BADGE_WORDS[badge.type]()}
            </Badge>
          ))}
        </span>
        {note && <span className="text-foreground/65 text-xs">{note}</span>}
      </label>
      <span className="text-label shrink-0 pt-px text-end tabular-nums">{formatBytes(bytes)}</span>
      <span className="flex w-icon-button-sm shrink-0 justify-center">{path && <RevealIconButton path={path} />}</span>
    </div>
  );
}

/** Reveals a path in Finder (or its folder elsewhere), as a small icon with its name on hover. */
function RevealIconButton({ path }: { path: string }) {
  const mac = useApp((s) => s.info?.platform === "macos");
  const name = mac ? "Reveal in Finder" : "Show in folder";
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          aria-label={name}
          className="text-foreground/65 hover:text-foreground -my-1"
          onClick={() => void revealPath(path).catch(() => {})}
        >
          <FolderOpen />
        </Button>
      </TooltipTrigger>
      <TooltipContent>{name}</TooltipContent>
    </Tooltip>
  );
}

/** Advanced: disk use per project, then what every project shares. */
function UsageDialog({ open, report, onClose }: { open: boolean; report: StorageReport; onClose: () => void }) {
  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()}>
      <DialogContent data-slot="storage-usage" className="flex max-h-full max-w-lg flex-col">
        <DialogHeader>
          <DialogTitle>{STORAGE_ROWS.usage.label}</DialogTitle>
          <DialogDescription>What each project takes up on this computer, largest first.</DialogDescription>
        </DialogHeader>
        <div className="-mx-1 grid min-h-0 flex-1 content-start gap-5 overflow-y-auto px-1">
          <Usage report={report} />
        </div>
        <DialogFooter>
          <Button type="button" onClick={onClose}>
            Done
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** A project's parts, in the page bar's colours. */
function projectParts(project: StorageReport["projects"][number]) {
  return [
    { key: "work", label: "Work folders", bytes: project.worktreesBytes, color: "bg-glyph-5" },
    { key: "conversations", label: "Conversations", bytes: project.blobsBytes, color: "bg-glyph-1" },
    { key: "brain", label: "Brain", bytes: project.brainBytes, color: "bg-glyph-7" },
    { key: "scratch", label: "Scratch", bytes: project.scratchBytes, color: "bg-foreground/50" },
  ].filter((part) => part.bytes > 0);
}

/** Each project with its total, a bar of its parts and their sizes; then what all share. */
function Usage({ report }: { report: StorageReport }) {
  const projects = report.projects
    .map((project) => ({ project, parts: projectParts(project) }))
    .map((entry) => ({ ...entry, total: entry.parts.reduce((sum, part) => sum + part.bytes, 0) }))
    .toSorted((a, b) => b.total - a.total);
  const largest = Math.max(1, ...projects.map((entry) => entry.total));
  return (
    <>
      <SettingsSection title="Projects">
        {projects.length === 0 ? (
          <SettingsCard>
            <p data-slot="storage-empty" className="text-foreground/65 px-4 py-6 text-center text-sm">
              No projects yet.
            </p>
          </SettingsCard>
        ) : (
          <SettingsCard>
            {projects.map(({ project, parts, total }) => (
              <div key={project.projectId} data-slot="storage-project" className="grid gap-2 px-4 py-3">
                <div className="flex items-center gap-2">
                  <span className="text-label min-w-0 truncate font-medium">{project.name}</span>
                  {!project.repoFound && <Badge variant="secondary">Repository not found</Badge>}
                  <span className="text-label ms-auto shrink-0 tabular-nums">{formatBytes(total)}</span>
                </div>
                <div aria-hidden className="bg-foreground/8 flex h-1.5 overflow-hidden rounded-full">
                  <div className="flex h-full" style={{ width: `${(total / largest) * 100}%` }}>
                    {parts.map((part) => (
                      <span
                        key={part.key}
                        className={`${part.color} h-full min-w-0.5`}
                        style={{ width: `${(part.bytes / Math.max(1, total)) * 100}%` }}
                      />
                    ))}
                  </div>
                </div>
                <ul className="text-foreground/65 flex flex-wrap gap-x-3 gap-y-1 text-xs">
                  {parts.map((part) => (
                    <li key={part.key} className="flex items-center gap-1.5 tabular-nums">
                      <span aria-hidden className={`${part.color} size-2 rounded-full`} />
                      {part.label} {formatBytes(part.bytes)}
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </SettingsCard>
        )}
      </SettingsSection>
      {report.shared.length > 0 && (
        <SettingsSection title="Shared by all projects">
          <SettingsCard>
            {report.shared.map((shared) => (
              <div key={shared.part} data-slot="storage-shared" className="flex items-center gap-3 px-4 py-2.5">
                <span className="text-label min-w-0 flex-1">{SHARED[shared.part]}</span>
                <span className="text-label shrink-0 tabular-nums">{formatBytes(shared.bytes)}</span>
              </div>
            ))}
          </SettingsCard>
        </SettingsSection>
      )}
      <div className="grid gap-2">
        <p className="text-foreground/65 text-xs">
          Work folders start as copies of a project’s installed packages and build files. Where the disk
          allows, the copies share space with the originals, so they may use less than shown.
        </p>
        {report.dataDir && (
          <div className="flex items-center gap-2">
            <span className="text-foreground/65 min-w-0 flex-1 truncate text-xs" title={report.dataDir}>
              {shortPath(report.dataDir)}
            </span>
            <RevealButton path={report.dataDir} />
          </div>
        )}
      </div>
    </>
  );
}
