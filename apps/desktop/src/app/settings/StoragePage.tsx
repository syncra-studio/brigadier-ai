import { ChevronRight } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useState, type ReactNode } from "react";

import { CheckboxRow, ErrorLine, errorText } from "@/app/dialogs/fields";
import {
  ALL_GROUPS,
  action,
  confirmation,
  count,
  failure,
  grouped,
  picked,
  result,
  summary,
  sweep,
  type Kept,
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
import { cleanStorage, openUninstall, scanStorage } from "@/state/storage";
import { useApp } from "@/state/store";

/** The Storage page's rows, for the page and for Settings search. */
export const STORAGE_ROWS = {
  storage: {
    label: "Free up space",
    description: "What Brigadier keeps on this computer, and what of it can go.",
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
 * Settings → Storage: Free up space. A scan when the page opens shows what can go, in a few
 * plain groups, and what is kept on purpose with why; one button removes the safe part after
 * a confirmation, and the page then says what it freed. Choosing items by hand and disk use per
 * project are under Advanced.
 */
export function StoragePage() {
  // A new scan starts afresh.
  const [scans, setScans] = useState(0);
  return (
    <SettingsPage title="Storage" wide>
      <FreeUpSpace key={scans} onScanAgain={() => setScans((n) => n + 1)} />
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

/** One scan, and cleaning what it found. */
function FreeUpSpace({ onScanAgain }: { onScanAgain: () => void }) {
  const [report, setReport] = useState<StorageReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [cleaning, setCleaning] = useState(false);
  const [cleaned, setCleaned] = useState<CleanReport | null>(null);

  useEffect(() => {
    let current = true;
    scanStorage().then(
      (scanned) => current && setReport(scanned),
      (cause: unknown) => current && setError(`Couldn't look: ${errorText(cause)}`),
    );
    return () => {
      current = false;
    };
  }, []);

  const clean = (ids: string[]) => {
    if (!report) return;
    setCleaning(true);
    setError(null);
    cleanStorage(report.scanId, ids).then(
      (done) => {
        setCleaning(false);
        setCleaned(done);
      },
      (cause: unknown) => {
        setCleaning(false);
        setError(errorText(cause));
      },
    );
  };

  return (
    <FreeUpSpaceBody
      report={report}
      error={error}
      cleaning={cleaning}
      cleaned={cleaned}
      onClean={clean}
      onScanAgain={onScanAgain}
    />
  );
}

/** Free up space for one scan: looking, what can go, the result. */
export function FreeUpSpaceBody({
  report,
  error,
  cleaning,
  cleaned,
  onClean,
  onScanAgain,
}: {
  report: StorageReport | null;
  error: string | null;
  cleaning: boolean;
  cleaned: CleanReport | null;
  onClean: (ids: string[]) => void;
  onScanAgain: () => void;
}) {
  const [confirming, setConfirming] = useState<Sweep | null>(null);

  if (!report) {
    return (
      <SettingsSection title={STORAGE_ROWS.storage.label}>
        <SettingsCard>
          <div role="status" className="text-foreground/65 flex items-center gap-2 px-4 py-3 text-xs">
            {!error && <Spinner className="size-icon-sm animate-spin" />}
            {error ?? "Looking at what Brigadier keeps on this computer…"}
          </div>
        </SettingsCard>
        {error && (
          <div className="flex justify-end">
            <SettingsButton onClick={onScanAgain}>Try again</SettingsButton>
          </div>
        )}
      </SettingsSection>
    );
  }

  const plan = sweep(report);
  if (cleaned) {
    return <Result cleaned={cleaned} onScanAgain={onScanAgain} />;
  }

  return (
    <>
      <SettingsSection title={STORAGE_ROWS.storage.label} description={summary(report, plan)}>
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
              detail={keptDetail(plan.kept)}
              bytes={plan.keptBytes}
              muted
            >
              {plan.kept.map((kept) => (
                <ItemLine key={kept.key} label={kept.label} bytes={kept.bytes} reason={kept.reason} path={kept.path} />
              ))}
            </GroupRow>
          )}
          <div className="flex items-center justify-end gap-3 px-4 py-3">
            <ErrorLine error={error} />
            <Button
              type="button"
              size="sm"
              disabled={cleaning || plan.ids.length === 0}
              onClick={() => setConfirming(plan)}
            >
              {cleaning ? "Freeing up…" : action(plan)}
            </Button>
          </div>
        </SettingsCard>
      </SettingsSection>

      <SettingsAdvanced description="Choose items yourself, and see disk use per project.">
        <ChooseItems report={report} cleaning={cleaning} onRemove={(ids) => setConfirming(picked(report, ids))} />
        <SettingsSection title="Disk use">
          <Usage report={report} />
        </SettingsSection>
      </SettingsAdvanced>

      <Confirm
        plan={confirming}
        onCancel={() => setConfirming(null)}
        onConfirm={() => {
          if (confirming) onClean(confirming.ids);
          setConfirming(null);
        }}
      />
    </>
  );
}

function keptDetail(kept: Kept[]): string {
  return count(kept.length, "thing", "things");
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

/** "Free up 12.6 GB?": what goes, group by group, and that nothing else is touched. */
function Confirm({
  plan,
  onCancel,
  onConfirm,
}: {
  plan: Sweep | null;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const words = plan ? confirmation(plan) : null;
  return (
    <Dialog open={plan !== null} onOpenChange={(open) => !open && onCancel()}>
      <DialogContent className="max-w-md">
        {plan && words && (
          <>
            <DialogHeader>
              <DialogTitle>{words.title}</DialogTitle>
              <DialogDescription>These go:</DialogDescription>
            </DialogHeader>
            <ul className="grid gap-1 text-sm">
              {words.lines.map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
            <p className="text-muted-foreground text-xs">{words.footer}</p>
            <DialogFooter>
              <Button type="button" variant="ghost" onClick={onCancel}>
                Cancel
              </Button>
              <Button type="button" onClick={onConfirm}>
                {action(plan)}
              </Button>
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}

/** What a clean freed, and what stayed. */
function Result({ cleaned, onScanAgain }: { cleaned: CleanReport; onScanAgain: () => void }) {
  const words = result(cleaned);
  return (
    <SettingsSection title={STORAGE_ROWS.storage.label}>
      <SettingsCard>
        <div role="status" className="grid gap-1 px-4 py-3">
          <span className="text-label font-medium">{words.title}</span>
          {words.lines.map((line) => (
            <span key={line} className="text-foreground/65 text-xs">
              {line}
            </span>
          ))}
        </div>
        {cleaned.failures.length > 0 && (
          <ul className="grid gap-2 px-4 py-3">
            {cleaned.failures.map((failed, index) => {
              const words = failure(failed);
              return (
                <li key={index} className="grid gap-0.5">
                  <span className="text-label">{failed.label}</span>
                  <span className="text-xs">{words.plain}</span>
                  <span className="text-foreground/50 text-xs break-all">{words.detail}</span>
                </li>
              );
            })}
          </ul>
        )}
        <div className="flex justify-end px-4 py-3">
          <SettingsButton onClick={onScanAgain}>Scan again</SettingsButton>
        </div>
      </SettingsCard>
    </SettingsSection>
  );
}

/** Advanced: every item that can go, picked by hand (the safe ones start picked). */
function ChooseItems({
  report,
  cleaning,
  onRemove,
}: {
  report: StorageReport;
  cleaning: boolean;
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
    <SettingsSection
      title="Choose items"
      description="Everything that can be removed, including what the sweep leaves: work folders with unsaved changes (kept as a commit on their branch first), Brains, models and recordings."
    >
      <SettingsCard>
        {groups.length === 0 && (
          <p className="text-foreground/65 px-4 py-3 text-xs">Nothing can be removed.</p>
        )}
        {groups.map((group) => (
          <div key={group.category} className="grid gap-2 px-4 py-3">
            <h3 className="text-label font-medium">{group.title}</h3>
            {group.items.map((item) => (
              <PickRow key={item.id} item={item} checked={chosen.has(item.id)} onCheckedChange={(on) => pick(item.id, on)} />
            ))}
          </div>
        ))}
        <div className="flex justify-end px-4 py-3">
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={cleaning || plan.ids.length === 0}
            onClick={() => onRemove(chosen)}
          >
            {plan.ids.length === 0
              ? "Remove"
              : `Remove ${count(plan.ids.length, "item", "items")} (${formatBytes(plan.bytes)})`}
          </Button>
        </div>
      </SettingsCard>
    </SettingsSection>
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
