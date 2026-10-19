import { Check, FolderPlus } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useState } from "react";

import { errorText } from "@/app/dialogs/fields";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { SectionLabel, StepFooter, StepHeader } from "@/app/onboarding/parts";
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

/** Cards fill as many columns as fit, each at least a setup card wide. */
const CARD_GRID =
  "grid grid-cols-[repeat(auto-fill,minmax(var(--spacing-setup-card),1fr))] gap-2.5";

/** The corner check on a card that is chosen. */
function CardCheck({ className, label }: { className: string; label: string }) {
  return (
    <span
      className={cn(
        "absolute end-2 top-2 grid size-5 place-items-center rounded-full shadow-hairline",
        className,
      )}
    >
      <Check className="size-icon-xs" />
      <span className="sr-only">{label}</span>
    </span>
  );
}

/**
 * The projects to start with, found in the agents' own session history; none come chosen.
 * Adding them finishes the setup and opens the first.
 */
export function ProjectsStep({ onBack }: { onBack: () => void }) {
  const [candidates, setCandidates] = useState<ProjectCandidate[] | null>(null);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  // Folders added with Add a folder…, which the history count leaves out.
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
  const found = addable.filter((candidate) => !byHand.has(candidate.path)).length;

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
      <div className="max-w-onboarding-detail mx-auto mt-10 flex min-h-0 w-full flex-1 flex-col px-6">
      <div className="flex min-h-0 flex-1 flex-col gap-3">
        <div className="flex shrink-0 flex-wrap items-center gap-1">
          <SectionLabel
            label="Found in your history"
            dot={found > 0}
            count={candidates === null ? undefined : found}
          />
          <div className="text-muted-foreground ms-auto flex items-center gap-1 text-xs">
            {addable.length > 0 && (
              <>
                <span role="status" className="me-1">
                  {count} selected
                </span>
                <Button
                  size="xs"
                  variant="ghost"
                  disabled={busy || count === addable.length}
                  onClick={() => setSelected(new Set(addable.map((c) => c.path)))}
                >
                  Select all
                </Button>
                <Button
                  size="xs"
                  variant="ghost"
                  disabled={busy || count === 0}
                  onClick={() => setSelected(new Set())}
                >
                  Select none
                </Button>
              </>
            )}
            <Button size="xs" variant="outline" disabled={busy} onClick={() => void addFolder()}>
              <FolderPlus />
              Add a folder…
            </Button>
          </div>
        </div>
        {candidates === null ? (
          <div className="text-muted-foreground flex flex-1 flex-col items-center justify-center gap-3 text-sm">
            <Spinner className="size-icon-md animate-spin" />
            Looking for projects in your Claude Code and Codex history…
          </div>
        ) : candidates.length === 0 ? (
          <p className="text-muted-foreground bg-muted/25 rounded-surface border px-4 py-6 text-center text-sm">
            No repositories found in your Claude Code or Codex history. Add a folder to start
            with one.
          </p>
        ) : (
          <ul className={cn(CARD_GRID, "-m-1 min-h-0 flex-1 content-start overflow-y-auto p-1")}>
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
          <p role="alert" className="text-destructive shrink-0 text-xs whitespace-pre-wrap">
            {error}
          </p>
        )}
      </div>
      </div>
      <StepFooter
        onBack={onBack}
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
        aria-pressed={on}
        disabled={disabled || added}
        onClick={onToggle}
        className={cn(
          "rounded-surface relative flex w-full min-w-0 flex-col gap-2 border p-3.5 text-start transition-colors",
          on
            ? "border-primary/50 bg-primary/5 ring-primary/20 ring-2"
            : "bg-muted/30 hover:bg-muted/60",
          added ? "cursor-default opacity-60" : "disabled:cursor-default",
        )}
      >
        {on && (
          <CardCheck
            className="bg-primary text-primary-foreground"
            label={added ? "Added" : "Selected"}
          />
        )}
        <div className="min-w-0 pe-6">
          <p className="truncate text-sm font-medium">{candidate.name}</p>
          <p className="text-muted-foreground mt-0.5 truncate font-mono text-2xs">
            {shortPath(candidate.path)}
          </p>
        </div>
        <div className="text-muted-foreground flex min-w-0 items-center gap-2 text-xs">
          {candidate.providers.map((provider) => (
            <span key={provider} title={PROVIDER_LABELS[provider]} className="flex shrink-0">
              <ProviderGlyph provider={provider} className="size-icon-sm" />
              <span className="sr-only">{PROVIDER_LABELS[provider]}</span>
            </span>
          ))}
          <span className="truncate">{added ? "Already added" : activity}</span>
        </div>
      </button>
    </li>
  );
}
