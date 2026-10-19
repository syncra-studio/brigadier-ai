import { FolderOpen } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useState } from "react";

import { errorText } from "@/app/dialogs/fields";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { PILL_BUTTON, RAISED_BUTTON, StepFooter, StepHeader, TickMark } from "@/app/onboarding/parts";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import { Spinner } from "@/components/glyphs/spinner";
import { Button } from "@/components/ui/button";
import { pickFolder } from "@/ipc/client";
import type { ProjectCandidate } from "@/ipc/generated";
import { formatAgo, shortPath } from "@/lib/format";
import { cn } from "@/lib/utils";
import { createProject, select } from "@/state/actions";
import { checkFolder } from "@/state/addProject";
import { findProjects, finishOnboarding } from "@/state/onboarding";

/**
 * The projects to start with, found in the agents' own session history, all ticked to begin
 * with: three cards to a row (fewer in a narrow window), and only the cards scroll. One button
 * ticks the rest or clears them all; Add folder… adds one more, ticked. Adding them finishes the
 * setup and opens the first; with none ticked the setup finishes without projects.
 */
export function ProjectsStep({ onBack }: { onBack: () => void }) {
  const [candidates, setCandidates] = useState<ProjectCandidate[] | null>(null);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  // Folders added with Add folder…, which the history count leaves out.
  const [byHand, setByHand] = useState<ReadonlySet<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [nowMs] = useState(Date.now);

  useEffect(() => {
    let live = true;
    findProjects()
      .then((found) => {
        if (!live) return;
        setCandidates(found);
        setSelected(new Set(found.filter((candidate) => candidate.projectId === null).map((candidate) => candidate.path)));
      })
      .catch((cause: unknown) => {
        if (!live) return;
        setCandidates([]);
        setError(errorText(cause));
      });
    return () => {
      live = false;
    };
  }, []);

  const addable = (candidates ?? []).filter((candidate) => candidate.projectId === null);
  const chosen = addable.filter((candidate) => selected.has(candidate.path));
  const found = (candidates ?? []).filter((candidate) => !byHand.has(candidate.path)).length;
  const allChosen = addable.length > 0 && chosen.length === addable.length;

  const toggle = (path: string, on: boolean) =>
    setSelected((current) => {
      const next = new Set(current);
      if (on) next.add(path);
      else next.delete(path);
      return next;
    });

  const addFolder = async () => {
    setError(null);
    try {
      const picked = await pickFolder();
      if (!picked) return;
      const check = await checkFolder(picked);
      if (check.kind === "invalid") throw new Error(check.reason);
      if (check.kind !== "repo") {
        throw new Error(
          `${shortPath(picked)} is not in a git repository. Add it with Add project, which can make one.`,
        );
      }
      const repo = { path: check.root, name: check.name };
      const project = check.projectId ? { id: check.projectId } : null;
      if (!candidates?.some((candidate) => candidate.path === repo.path)) {
        setByHand((current) => new Set(current).add(repo.path));
      }
      setCandidates((current) => {
        const list = current ?? [];
        if (list.some((candidate) => candidate.path === repo.path)) return list;
        return [
          {
            path: repo.path,
            name: repo.name,
            providers: [],
            sessions: 0,
            lastActiveMs: 0,
            projectId: project?.id ?? null,
          },
          ...list,
        ];
      });
      if (!project) toggle(repo.path, true);
    } catch (cause) {
      setError(errorText(cause));
    }
  };

  const add = async () => {
    setBusy(true);
    setError(null);
    const failed: string[] = [];
    let first: string | null = null;
    for (const candidate of chosen) {
      try {
        const project = await createProject("", candidate.path);
        first ??= project.id;
        setCandidates((current) =>
          (current ?? []).map((entry) =>
            entry.path === candidate.path ? { ...entry, projectId: project.id } : entry,
          ),
        );
      } catch (cause) {
        failed.push(`${candidate.name}: ${errorText(cause)}`);
      }
    }
    setBusy(false);
    if (failed.length > 0) {
      setError(`Some projects could not be added.\n${failed.join("\n")}`);
      return;
    }
    if (first) select({ type: "draft", kind: "session", projectId: first });
    await finishOnboarding().catch((cause: unknown) => setError(errorText(cause)));
  };

  const count = chosen.length;
  return (
    <div className="onboarding-step flex min-h-0 flex-1 flex-col">
      <StepHeader
        title="Choose your projects"
        description="These are the repositories you have worked in with Claude Code and Codex. Add the ones you want to use with Brigadier."
      />
      <div className="mt-12 flex min-h-0 flex-1 flex-col gap-3">
        <div className="shrink-0 px-6">
        <div className="max-w-onboarding mx-auto flex w-full flex-wrap items-center gap-x-1 gap-y-2">
          <div className="text-muted-foreground/70 text-label flex items-center gap-2 px-1">
            {found > 0 && <span aria-hidden className="bg-success size-1.5 shrink-0 rounded-full" />}
            <span>Found in your history</span>
            {candidates !== null && <span className="tabular-nums">{found}</span>}
          </div>
          <div className="ms-auto flex items-center gap-1">
            {addable.length > 0 && (
              // Words alone, as designed: hovered they brighten, with no pill behind them.
              <button
                type="button"
                disabled={busy}
                onClick={() =>
                  setSelected(allChosen ? new Set() : new Set(addable.map((candidate) => candidate.path)))
                }
                className={cn(
                  PILL_BUTTON,
                  "text-muted-foreground enabled:hover:text-foreground grid items-center px-2.5 font-medium transition-colors disabled:opacity-50",
                )}
              >
                {/* Both labels hold the room, so the button never moves as it switches. */}
                <span className="col-start-1 row-start-1">{allChosen ? "Select none" : "Select all"}</span>
                <span aria-hidden className="invisible col-start-1 row-start-1">
                  Select none
                </span>
              </button>
            )}
            <Button
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => void addFolder()}
              className={cn(RAISED_BUTTON, "gap-1.75")}
            >
              <FolderOpen className="size-3.25" />
              Add folder…
            </Button>
          </div>
        </div>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-6">
          <div className="max-w-onboarding mx-auto flex w-full flex-col gap-3">
            {candidates === null ? (
              <div className="text-muted-foreground flex flex-col items-center justify-center gap-3 py-16 text-sm">
                <Spinner className="size-icon-md animate-spin" />
                Looking for projects in your Claude Code and Codex history…
              </div>
            ) : candidates.length === 0 ? (
              <p className="text-muted-foreground bg-card rounded-surface border px-4 py-6 text-center text-sm">
                No repositories found in your Claude Code or Codex history. Add a folder to start
                with one.
              </p>
            ) : (
              <ul className="onboarding-project-grid">
                {candidates.map((candidate) => (
                  <ProjectCard
                    key={candidate.path}
                    candidate={candidate}
                    nowMs={nowMs}
                    selected={selected.has(candidate.path)}
                    disabled={busy}
                    onToggle={() => toggle(candidate.path, !selected.has(candidate.path))}
                  />
                ))}
              </ul>
            )}
            {error && (
              <p role="alert" className="text-destructive text-xs whitespace-pre-wrap">
                {error}
              </p>
            )}
          </div>
        </div>
      </div>
      <StepFooter
        onBack={onBack}
        count={`${count} of ${addable.length} selected`}
        primary={{
          label: busy
            ? "Adding…"
            : count === 0
              ? "Continue without projects"
              : `Add ${count} ${count === 1 ? "project" : "projects"}`,
          onClick: () => void add(),
          busy,
        }}
      />
    </div>
  );
}

function ProjectCard({
  candidate,
  nowMs,
  selected,
  disabled,
  onToggle,
}: {
  candidate: ProjectCandidate;
  nowMs: number;
  selected: boolean;
  disabled: boolean;
  onToggle: () => void;
}) {
  const added = candidate.projectId !== null;
  const on = added || selected;
  const activity =
    candidate.sessions > 0
      ? `${candidate.sessions} ${candidate.sessions === 1 ? "session" : "sessions"} · ${formatAgo(candidate.lastActiveMs, nowMs)}`
      : null;
  return (
    <li className="min-w-0">
      <button
        type="button"
        role="checkbox"
        aria-checked={on}
        aria-label={candidate.name}
        disabled={disabled || added}
        onClick={onToggle}
        className={cn(
          "bg-card rounded-surface flex h-full w-full min-w-0 flex-col gap-1 border p-4 text-start transition-colors",
          "enabled:hover:bg-muted/60 disabled:cursor-default",
          added && "opacity-60",
        )}
      >
        <span className="flex w-full min-w-0 items-center gap-2">
          <span className="min-w-0 flex-1 truncate text-sm font-medium">{candidate.name}</span>
          <TickMark checked={on} />
        </span>
        <span className="text-muted-foreground/70 w-full truncate font-mono text-xs">{shortPath(candidate.path)}</span>
        <span className="text-muted-foreground/70 flex min-w-0 items-center gap-1.5 pt-2.5 text-xs">
          {candidate.providers.map((provider) => (
            <span key={provider} title={PROVIDER_LABELS[provider]} className="flex shrink-0">
              <ProviderGlyph provider={provider} className="size-icon-sm" />
              <span className="sr-only">{PROVIDER_LABELS[provider]}</span>
            </span>
          ))}
          <span className="truncate">{added ? "Already added" : activity}</span>
        </span>
      </button>
    </li>
  );
}
