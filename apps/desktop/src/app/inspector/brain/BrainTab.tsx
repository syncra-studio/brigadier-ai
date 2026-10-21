import { useEffect, useMemo, useState } from "react";

import { BrainGraphPanel, preloadGraphCanvas } from "@/app/inspector/brain/BrainGraphPanel";
import { BrainOverviewView } from "@/app/inspector/brain/BrainOverview";
import { BrainSearch } from "@/app/inspector/brain/BrainSearch";
import { type BrainView, BrainToolbar } from "@/app/inspector/brain/BrainToolbar";
import type { BrainOverview } from "@/ipc/generated";
import { loadBrain, PERSONAL, selectBrain, useBrain } from "@/state/brain";
import { inspectedConversation } from "@/state/actions";
import { useApp } from "@/state/store";

/** How long after the tab opens the graph's libraries start loading. */
const GRAPH_PRELOAD_MS = 500;

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Something in the overview moves on its own (a scan, a download, a job), so read it often. */
function busy(overview: BrainOverview | undefined): boolean {
  if (!overview) return false;
  const embedder = overview.embedder.state.type;
  return (
    overview.index?.state.type === "scanning" ||
    embedder === "downloading" ||
    embedder === "loading" ||
    overview.jobs.some((job) => job.state.type === "running")
  );
}

/**
 * The Brain the tab shows: the one picked, else the inspected conversation's project's (the one
 * Settings was opened from), else the Personal Brain.
 */
function useBrainKey(): string {
  const picked = useBrain((s) => s.selected);
  const openProject = useApp((s) => inspectedConversation(s)?.projectId ?? null);
  const exists = useApp((s) => (picked === null ? false : picked === PERSONAL || !!s.projects[picked]));
  if (picked !== null && exists) return picked;
  return openProject ?? PERSONAL;
}

/**
 * The Inspector's Brain tab: a project's Brain (or the Personal Brain) at a glance, a search
 * through the real retrieval, and the graph of what it knows.
 */
export function BrainTab() {
  const key = useBrainKey();
  const projects = useApp((s) => s.projects);
  const overview = useBrain((s) => s.overviews[key]);
  const [view, setView] = useState<BrainView>("overview");
  const [failure, setFailure] = useState<{ key: string; message: string } | null>(null);
  const moving = busy(overview);

  const options = useMemo(
    () => [
      { value: PERSONAL, label: "Personal Brain", hint: "Your preferences, across projects" },
      ...Object.values(projects)
        .toSorted((a, b) => a.name.localeCompare(b.name))
        .map((project) => {
          const repo = project.repos[0]?.path;
          return { value: project.id, label: project.name, ...(repo ? { hint: repo } : {}) };
        }),
    ],
    [projects],
  );

  // The graph's libraries load in the background once the tab has painted.
  useEffect(() => {
    const timer = window.setTimeout(preloadGraphCanvas, GRAPH_PRELOAD_MS);
    return () => window.clearTimeout(timer);
  }, []);

  // Read the overview now and keep it current while the tab shows: often while something
  // moves (index progress and downloads have no events), rarely otherwise.
  useEffect(() => {
    let current = true;
    let timer: number | undefined;
    const read = () => {
      loadBrain(key).then(
        (fresh) => {
          if (!current) return;
          setFailure(null);
          timer = window.setTimeout(read, busy(fresh) ? 1000 : 5000);
        },
        (cause: unknown) => {
          if (!current) return;
          setFailure({ key, message: errorText(cause) });
          timer = window.setTimeout(read, 5000);
        },
      );
    };
    read();
    return () => {
      current = false;
      window.clearTimeout(timer);
    };
  }, [key]);

  const error = failure?.key === key ? failure.message : null;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <BrainToolbar
        brainKey={key}
        options={options}
        onBrainChange={selectBrain}
        view={view}
        onViewChange={setView}
      />
      {error && (
        <p role="alert" className="text-destructive shrink-0 border-b px-3 py-2 text-xs">
          {error}
        </p>
      )}
      {view === "overview" && (
        <div data-selectable className="min-h-0 flex-1 overflow-y-auto">
          {overview ? (
            <BrainOverviewView brainKey={key} overview={overview} moving={moving} />
          ) : (
            !error && <p className="text-muted-foreground p-4 text-xs">Loading…</p>
          )}
        </div>
      )}
      {view === "search" && <BrainSearch key={key} brainKey={key} />}
      {view === "graph" && <BrainGraphPanel key={key} brainKey={key} overview={overview ?? null} />}
    </div>
  );
}
