import { ChevronRight } from "@openai/apps-sdk-ui/components/Icon";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";

import { CheckboxRow, ErrorLine, errorText } from "@/app/dialogs/fields";
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
import { revealPath } from "@/ipc/client";
import type { CleanItem, CleanReport, SharedPart, StorageReport } from "@/ipc/generated";
import { formatBytes } from "@/lib/format";
import { openSettings } from "@/state/actions";
import { cleanStorage, openUninstall, scanStorage } from "@/state/storage";
import { useApp } from "@/state/store";

/** The Storage page's rows, for the page and for Settings search. */
export const STORAGE_ROWS = {
  computer: {
    label: "This computer",
    description: "What Brigadier keeps on this computer, and what of it can go.",
  },
  storage: {
    label: "Free up space",
    description: "Review what can go before anything is removed.",
  },
  archived: {
    label: "Archived chats",
    description: "Restore or delete archived sessions and chats.",
  },
  choose: {
    label: "Choose items yourself",
    description: "Anything that can be removed, including Brains, models and recordings.",
  },
  usage: {
    label: "Disk use per project",
    description: "What each project's work folders, Brain and conversations use.",
  },
  uninstall: {
    label: "Uninstall Brigadier",
    description:
      "Removes what Brigadier created on this computer, then the app. Your repositories, your own branches and your files are not touched.",
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
  onCancel,
  onRemove,
}: {
  open: boolean;
  report: StorageReport;
  onCancel: () => void;
  onRemove: (ids: Set<string>) => void;
}) {
  const [chosen, setChosen] = useState(
    () => new Set(report.items.filter((item) => item.checked && item.selectable).map((item) => item.id)),
  );
  const groups = grouped(
    report.items.filter((item) => item.selectable),
    ALL_GROUPS,
  );
  const plan = picked(report, chosen);
  const pick = (id: string, on: boolean) =>
    setChosen((set) => {
      const next = new Set(set);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });
  return (
    <Dialog open={open} onOpenChange={(next) => !next && onCancel()}>
      <DialogContent data-slot="storage-choose" className="flex max-h-full max-w-lg flex-col">
        <DialogHeader>
          <DialogTitle>{STORAGE_ROWS.choose.label}</DialogTitle>
          <DialogDescription>
            Everything that can be removed, including what Free up space leaves: work folders with
            unsaved changes (kept as a commit on their branch first), Brains, models and recordings.
          </DialogDescription>
        </DialogHeader>
        <div className="-mx-1 grid min-h-0 flex-1 gap-4 overflow-y-auto px-1">
          {groups.length === 0 && <p className="text-foreground/65 text-xs">Nothing can be removed.</p>}
          {groups.map((group) => (
            <div key={group.category} className="grid gap-2">
              <h3 className="text-label font-medium">{group.title}</h3>
              {group.items.map((item) => (
                <PickRow key={item.id} item={item} checked={chosen.has(item.id)} onCheckedChange={(on) => pick(item.id, on)} />
              ))}
            </div>
          ))}
        </div>
        <DialogFooter>
          <Button type="button" variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button type="button" disabled={plan.ids.length === 0} onClick={() => onRemove(chosen)}>
            {plan.ids.length === 0
              ? "Remove"
              : `Remove ${count(plan.ids.length, "item", "items")} (${formatBytes(plan.bytes)})`}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function PickRow({
  item,
  checked,
  onCheckedChange,
}: {
  item: CleanItem;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
}) {
  return (
    <div className="flex items-start gap-3">
      <div className="min-w-0 flex-1">
        <CheckboxRow
          label={item.label}
          checked={checked}
          onCheckedChange={onCheckedChange}
          note={
            <>
              {item.badges.map((badge) => (
                <Badge key={badge.type} variant="warning" className="me-1.5">
                  {badge.type === "hasChanges"
                    ? "Unsaved changes"
                    : badge.type === "notMerged"
                      ? `Not merged · ${badge.ahead} ahead`
                      : "Older Brigadier"}
                </Badge>
              ))}
              {item.reason}
              {item.toTrash && " Goes to the Trash."}
            </>
          }
        />
      </div>
      <span className="text-muted-foreground shrink-0 pt-0.5 text-xs tabular-nums">
        {formatBytes(item.bytes)}
      </span>
      {item.path && <RevealButton path={item.path} />}
    </div>
  );
}

/** Advanced: disk use per project. */
function UsageDialog({ open, report, onClose }: { open: boolean; report: StorageReport; onClose: () => void }) {
  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()}>
      <DialogContent data-slot="storage-usage" className="flex max-h-full max-w-lg flex-col">
        <DialogHeader>
          <DialogTitle>{STORAGE_ROWS.usage.label}</DialogTitle>
          <DialogDescription>{STORAGE_ROWS.usage.description}</DialogDescription>
        </DialogHeader>
        <div className="-mx-1 min-h-0 flex-1 overflow-y-auto px-1">
          <Usage report={report} />
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** Disk use per project and for what every project shares. */
function Usage({ report }: { report: StorageReport }) {
  const cell = "py-1 ps-3 text-end tabular-nums";
  return (
    <SettingsCard className="gap-1.5 px-4 py-3">
      <div className="overflow-x-auto">
        <table className="w-full text-xs">
          <thead className="text-muted-foreground">
            <tr className="border-border border-b">
              <th className="py-1 text-start font-normal">Project</th>
              <th className={`${cell} font-normal`}>Work folders</th>
              <th className={`${cell} font-normal`}>Brain + index</th>
              <th className={`${cell} font-normal`}>Conversations</th>
              <th className={`${cell} font-normal`}>Scratch</th>
              <th className={`${cell} font-normal`}>Total</th>
            </tr>
          </thead>
          <tbody>
            {report.projects.map((project) => (
              <tr key={project.projectId} className="border-border border-b">
                <td className="py-1">
                  {project.name}
                  {!project.repoFound && (
                    <span className="text-muted-foreground"> · repository not found</span>
                  )}
                </td>
                <td className={cell}>{formatBytes(project.worktreesBytes)}</td>
                <td className={cell}>{formatBytes(project.brainBytes)}</td>
                <td className={cell}>{formatBytes(project.blobsBytes)}</td>
                <td className={cell}>{formatBytes(project.scratchBytes)}</td>
                <td className={cell}>
                  {formatBytes(
                    project.worktreesBytes +
                      project.brainBytes +
                      project.blobsBytes +
                      project.scratchBytes,
                  )}
                </td>
              </tr>
            ))}
            {report.shared.map((shared) => (
              <tr key={shared.part} className="border-border border-b last:border-b-0">
                <td className="text-muted-foreground py-1" colSpan={5}>
                  {SHARED[shared.part]}
                </td>
                <td className={cell}>{formatBytes(shared.bytes)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="text-muted-foreground text-xs">
        Work folders start with copies of your project’s installed packages and build files. Where
        the disk allows, those copies share space with the originals, so they may use less than
        shown. {report.dataDir}
      </p>
    </SettingsCard>
  );
}
