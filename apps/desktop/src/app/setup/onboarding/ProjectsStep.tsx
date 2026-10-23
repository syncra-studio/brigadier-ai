import { useEffect, useState } from "react";

import { errorText } from "@/app/shell/dialogs/fields";
import { FoundCounter } from "@/app/setup/onboarding/FoundCounter";
import { RAISED_BUTTON, StepFooter, StepHeader } from "@/app/setup/onboarding/parts";
import { ProjectCard } from "@/app/setup/onboarding/ProjectCard";
import { SelectAllToggle } from "@/app/setup/onboarding/SelectAllToggle";
import { FolderOpen, Spinner } from "@/components/icons";
import { Button } from "@/components/controls";
import { pickFolder, warmFolderPicker } from "@/ipc/client";
import type { ProjectCandidate } from "@/ipc/generated";
import { shortPath } from "@/lib/format";
import { cn } from "@/lib/utils";
import { createProject, select } from "@/state/actions";
import { checkFolder } from "@/state/addProject";
import { findProjects, finishOnboarding } from "@/state/onboarding";

/**
 * The projects to start with, found in the agents' own session history, none ticked to begin
 * with (the history holds every folder an agent ever ran in): three cards to a row (fewer in a narrow window), and only the cards scroll. One button
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

  // Add folder…'s picker opens at once on the first click.
  useEffect(() => void warmFolderPicker().catch(() => {}), []);

  useEffect(() => {
    let live = true;
    findProjects()
      .then((found) => {
        if (!live) return;
        setCandidates(found);
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
        description="Pick the projects you want to work on."
      />
      <div className="mt-12 flex min-h-0 flex-1 flex-col gap-3">
        <div className="shrink-0 px-6">
        <div className="max-w-onboarding mx-auto flex w-full flex-wrap items-center gap-x-1 gap-y-2">
          <FoundCounter found={found} loaded={candidates !== null} />
          <div className="ms-auto flex items-center gap-1">
            {addable.length > 0 && (
              // Words alone, as designed: hovered they brighten, with no pill behind them.
              <SelectAllToggle
                allChosen={allChosen}
                disabled={busy}
                onToggle={() =>
                  setSelected(allChosen ? new Set() : new Set(addable.map((candidate) => candidate.path)))
                }
              />
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
                Looking for projects in your history…
              </div>
            ) : candidates.length === 0 ? (
              <p className="text-muted-foreground bg-card rounded-settings border px-4 py-6 text-center text-sm">
                No repositories found in your history. Add a folder to start with one.
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
