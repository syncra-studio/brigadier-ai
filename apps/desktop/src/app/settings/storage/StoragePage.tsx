import { useEffect, useState } from "react";

import { create } from "zustand";

import { errorText } from "@/app/shell/dialogs/fields";

import { SettingsPage, SettingsSection } from "@/app/settings/parts";

import { FreeUpCard, type FreeUp } from "@/app/settings/storage/FreeUpCard";

import { checkedWords, count, size, startTicked, usageParts } from "@/app/settings/storage/model";

import { UsageBar } from "@/app/settings/storage/parts";

import { ProjectsCard, type UnusedFreeUp } from "@/app/settings/storage/ProjectsCard";

import { FolderOpen, Reload } from "@/components/icons";

import { Button } from "@/components/controls";

import { RowText, SettingsCard, StateDot } from "@/components/shared";

import { revealPath } from "@/ipc/client";

import type { StorageReport } from "@/ipc/generated";

import { shortPath } from "@/lib/format";

import { cn } from "@/lib/utils";

import { openSettings } from "@/state/actions";

import { cleanStorage, freeUpProjects, onCleanProgress, openUninstall, scanStorage } from "@/state/storage";

import { useApp } from "@/state/store";

/** The Storage page's rows, for the page and for Settings search. */
export const STORAGE_ROWS = {
  computer: {
    label: "This computer",
    description: "What Brigadier keeps on this computer, and what of it can go",
  },
  storage: {
    label: "Free up space",
    description: "Temporary files, and files left by sessions that are gone",
  },
  projects: {
    label: "Disk use by project",
    description: "What each project's agents keep. Open one to see what's in it or clean it up.",
  },
  archived: {
    label: "Archived chats",
    description: "Restore or delete archived sessions and chats",
  },
  uninstall: {
    label: "Uninstall Brigadier",
    description:
      "Removes what Brigadier created on this computer, then the app. Your repositories, your own branches and your files stay.",
  },
} as const;

/**
 * Where the page stands, kept while the app runs: leaving the page while space is freed up
 * keeps its progress, and the last scan shows at once when it opens again.
 */
type Storage = {
  /** The latest scan, and when it was made. */
  report: StorageReport | null;
  checkedAtMs: number;
  scanning: boolean;
  scanError: string | null;
  freeUp: FreeUp;
  /** The scan a clean under way (or done) came from. */
  cleanScan: StorageReport | null;
  unused: UnusedFreeUp;
};

export const useStorage = create<Storage>(() => ({
  report: null,
  checkedAtMs: 0,
  scanning: false,
  scanError: null,
  freeUp: { phase: "rest" },
  cleanScan: null,
  unused: { phase: "idle" },
}));

// Only the latest scan's answer counts.
let latestScan = 0;

/** Looks at what Brigadier keeps now. */
export function look(): void {
  const id = ++latestScan;
  useStorage.setState({ scanning: true, scanError: null });
  scanStorage().then(
    (report) => {
      if (id !== latestScan) return;
      useStorage.setState({ report, checkedAtMs: Date.now(), scanning: false });
    },
    (cause: unknown) => {
      if (id !== latestScan) return;
      useStorage.setState({ scanning: false, scanError: `Couldn't look: ${errorText(cause)}` });
    },
  );
}

onCleanProgress((progress) => {
  const { freeUp } = useStorage.getState();
  if (freeUp.phase !== "removing" || freeUp.scanId !== progress.scanId) return;
  useStorage.setState({ freeUp: { ...freeUp, done: progress.done, total: progress.total } });
});

/** Removes the items picked in a review; the card says how it goes, then what went. */
function remove(scanId: string, ids: string[], bytes: number): void {
  const { report } = useStorage.getState();
  useStorage.setState({
    freeUp: { phase: "removing", scanId, ids, bytes, done: 0, total: ids.length },
    cleanScan: report,
  });
  cleanStorage(scanId, ids).then(
    (cleaned) => {
      useStorage.setState({ freeUp: { phase: "done", scanId, ids, cleaned } });
      look();
    },
    (cause: unknown) => {
      useStorage.setState({ freeUp: { phase: "review", scanId, ticked: new Set(ids), error: errorText(cause) } });
    },
  );
}

/** Frees up the projects not in use (confirmed in their row), then looks again. */
function freeUpUnused(ids: string[]): void {
  useStorage.setState({ unused: { phase: "freeing" } });
  freeUpProjects(ids).then(
    (freed) => {
      // What went to the Trash takes its room until the Trash is emptied.
      const back = [
        `${size(freed.reclaimedBytes)} back`,
        ...(freed.trashedBytes > 0 ? [`${size(freed.trashedBytes)} in the Trash`] : []),
      ].join(", ");
      if (freed.failures.length > 0) {
        const line = `${count(freed.freed, "project", "projects")} freed up, ${back}. Not all of it: ${freed.failures[0]}`;
        useStorage.setState({ unused: { phase: "failed", line } });
      } else {
        useStorage.setState({
          unused: {
            phase: "done",
            title: `${count(freed.freed, "project", "projects")} freed up`,
            line: `${back.charAt(0).toUpperCase()}${back.slice(1)}. Their repositories and files stayed.`,
          },
        });
      }
      look();
    },
    (cause: unknown) => useStorage.setState({ unused: { phase: "failed", line: errorText(cause) } }),
  );
}

/**
 * Settings › Storage, drawn to the user's design: what Brigadier uses on this computer, Free up
 * space (its review, removing and done right in the card), each project's disk use (opened
 * with a cleanup for just that project, or all of them with search, sort and the projects not
 * in use), Archived chats and Uninstall.
 */
export function StoragePage() {
  const { report, checkedAtMs, scanning, scanError, freeUp, cleanScan, unused } = useStorage();
  const mac = useApp((s) => s.info?.platform === "macos");
  const reveal = mac ? "Reveal in Finder" : "Show in folder";
  const now = useNow();
  // It looks again each time it opens.
  useEffect(look, []);

  const refresh = () => {
    if (freeUp.phase === "done") useStorage.setState({ freeUp: { phase: "rest" }, cleanScan: null });
    useStorage.setState({ unused: { phase: "idle" } });
    look();
  };
  const freeUpProject = (projectId: string) => {
    // A clean under way keeps its card until it ends.
    if (!report || useStorage.getState().freeUp.phase === "removing") return;
    useStorage.setState({ freeUp: { phase: "review", scanId: report.scanId, ticked: startTicked(report, projectId) } });
    requestAnimationFrame(() =>
      document
        .querySelector(`[data-setting="${STORAGE_ROWS.storage.label}"]`)
        ?.scrollIntoView({ block: "start", behavior: "smooth" }),
    );
  };
  const busy = freeUp.phase === "removing" || unused.phase === "freeing";

  return (
    <SettingsPage
      title="Storage"
      description="What Brigadier keeps on this computer, and what of it can go."
      actions={
        <>
          {checkedAtMs > 0 && (
            <span className="text-muted-foreground pe-1 text-xs">
              {scanning ? "Checking…" : checkedWords(checkedAtMs, now)}
            </span>
          )}
          <Button variant="soft" size="sm" onClick={refresh} disabled={scanning || busy}>
            <Reload aria-hidden className={cn(scanning && "animate-spin motion-reduce:animate-none")} />
            Refresh
          </Button>
        </>
      }
    >
      <SettingsSection title={STORAGE_ROWS.computer.label} setting={STORAGE_ROWS.computer.label}>
        <UsageCard report={report} error={scanError} />
      </SettingsSection>
      <SettingsSection title={STORAGE_ROWS.storage.label} setting={STORAGE_ROWS.storage.label}>
        <FreeUpCard
          report={freeUp.phase === "removing" || freeUp.phase === "done" ? cleanScan : report}
          scanned={report}
          state={freeUp}
          onState={(next) => useStorage.setState({ freeUp: next })}
          onRemove={remove}
        />
      </SettingsSection>
      {report && report.projects.length > 0 && (
        <SettingsSection
          title={STORAGE_ROWS.projects.label}
          setting={STORAGE_ROWS.projects.label}
          description={STORAGE_ROWS.projects.description}
          aside="Largest first · shared items count once"
        >
          <ProjectsCard
            report={report}
            nowMs={now}
            reveal={reveal}
            unused={unused}
            onUnused={(next) => useStorage.setState({ unused: next })}
            onFreeUpUnused={freeUpUnused}
            onFreeUpProject={freeUpProject}
            cleaning={freeUp.phase === "removing"}
          />
        </SettingsSection>
      )}
      <SettingsSection title={STORAGE_ROWS.archived.label} setting={STORAGE_ROWS.archived.label}>
        <SettingsCard inset split={false}>
          <div className="flex items-center gap-4 py-4">
            <RowText title={STORAGE_ROWS.archived.label} line={STORAGE_ROWS.archived.description} />
            <Button variant="soft" size="sm" onClick={() => openSettings("archived")}>Manage</Button>
          </div>
        </SettingsCard>
      </SettingsSection>
      <SettingsSection title="Uninstall" setting={STORAGE_ROWS.uninstall.label}>
        <SettingsCard inset split={false}>
          <div className="flex items-center gap-4 py-4">
            <RowText title={STORAGE_ROWS.uninstall.label} line={STORAGE_ROWS.uninstall.description} />
            <Button variant="soft-destructive" size="sm" onClick={() => openUninstall()}>Uninstall…</Button>
          </div>
        </SettingsCard>
      </SettingsSection>
    </SettingsPage>
  );
}

/** What Brigadier uses in all, where, and what it is made of. */
function UsageCard({ report, error }: { report: StorageReport | null; error: string | null }) {
  const parts = report ? usageParts(report) : [];
  return (
    <SettingsCard padded data-usage>
      <div className="flex items-end gap-4">
        <div className="flex min-w-0 flex-1 items-end gap-2.5">
          <span className="text-page-title font-medium">
            {report ? size(report.totalBytes) : "…"}
          </span>
          <span className="text-foreground/65 text-label leading-none">
            {report ? "used by Brigadier" : "Looking…"}
          </span>
        </div>
        {report?.dataDir && (
          <button
            type="button"
            title={report.dataDir}
            onClick={() => void revealPath(report.dataDir).catch(() => {})}
            className="text-muted-foreground hover:text-foreground/65 flex min-w-0 items-center gap-2 font-mono text-xs"
          >
            <span className="truncate">{shortPath(report.dataDir)}</span>
            <FolderOpen aria-hidden className="text-foreground/65 size-3.5 shrink-0" />
          </button>
        )}
      </div>
      {error && <span className="text-destructive text-xs">{error}</span>}
      <UsageBar parts={parts} pending={!report} />
      <div className="flex justify-between gap-4">
        {parts.map((part) => (
          <div key={part.key} data-legend={part.key} className="flex flex-col gap-1.25">
            <span className="text-foreground/65 flex items-center gap-1.75 text-xs">
              <StateDot color={part.color} />
              {part.label}
            </span>
            <span
              className={cn(
                "text-xs font-medium",
                part.bytes > 0 ? "text-foreground" : "text-muted-foreground",
              )}
            >
              {size(part.bytes)}
            </span>
          </div>
        ))}
      </div>
    </SettingsCard>
  );
}

/** The time now, again every half minute ("Checked 3m ago", "Used today"). */
function useNow(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(timer);
  }, []);
  return now;
}
