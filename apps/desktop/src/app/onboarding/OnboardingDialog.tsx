import {
  ArrowLeft,
  Check,
  ExclamationMarkCircle,
  FolderPlus,
  Reload,
} from "@openai/apps-sdk-ui/components/Icon";
import { type ReactNode, useEffect, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { errorText } from "@/app/dialogs/fields";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import {
  type AgentState,
  agentState,
  checkAgents,
  SetupTerminal,
} from "@/app/onboarding/SetupTerminal";
import { BrigadierGlyph } from "@/components/glyphs/brand-glyph";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import { Spinner } from "@/components/glyphs/spinner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Kbd } from "@/components/ui/kbd";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { pickFolder } from "@/ipc/client";
import type { ProjectCandidate, ProviderKind } from "@/ipc/generated";
import { formatAgo, shortPath } from "@/lib/format";
import { providerStatusText } from "@/lib/setup";
import { useRevealed } from "@/lib/splash";
import { cn } from "@/lib/utils";
import { createProject, select } from "@/state/actions";
import { checkFolder } from "@/state/addProject";
import { findProjects, finishOnboarding, useOnboarding } from "@/state/onboarding";
import { useApp } from "@/state/store";

const PROVIDERS: readonly ProviderKind[] = ["claude", "codex"];

/** Suggested (preselected) projects: worked in at least this often within the last month. */
const SUGGESTED_SESSIONS = 3;
const SUGGESTED_WITHIN_MS = 30 * 86_400_000;

/** Cards fill as many columns as fit, each at least a setup card wide. */
const CARD_GRID =
  "grid grid-cols-[repeat(auto-fill,minmax(var(--spacing-setup-card),1fr))] gap-2.5";

type Step = "agents" | "projects";

function skip(): void {
  void finishOnboarding().catch((error: unknown) => console.error(error));
}

/**
 * The first-run setup: the coding agents Brigadier works through, then the projects to start
 * with, found in those agents' own session history. A sheet over the window, shown until
 * finished or skipped once; Settings can open it again. Escape asks before skipping.
 */
export function OnboardingDialog() {
  const catalogLoaded = useApp((s) => s.catalogLoaded);
  const onboarded = useApp((s) => s.settings.onboarded);
  const smoke = useApp((s) => s.info?.smoke ?? false);
  const reopened = useOnboarding((s) => s.reopened);
  // Not under the startup screen: it would take focus there, unseen.
  const revealed = useRevealed();
  const open = revealed && catalogLoaded && !smoke && (!onboarded || reopened);
  const [confirmSkip, setConfirmSkip] = useState(false);
  return (
    <Dialog open={open}>
      <DialogContent
        showCloseButton={false}
        className="max-w-setup h-setup bg-card flex flex-col gap-0 overflow-hidden p-0"
        onInteractOutside={(event) => event.preventDefault()}
        onEscapeKeyDown={(event) => {
          event.preventDefault();
          setConfirmSkip(true);
        }}
      >
        {open && <Onboarding />}
        <Dialog open={confirmSkip} onOpenChange={setConfirmSkip}>
          <DialogContent showCloseButton={false} className="max-w-xs">
            <DialogHeader>
              <DialogTitle>Skip setup?</DialogTitle>
              <DialogDescription>You can run it again from Settings.</DialogDescription>
            </DialogHeader>
            <DialogFooter>
              <Button
                variant="outline"
                onClick={() => {
                  setConfirmSkip(false);
                  skip();
                }}
              >
                Skip
              </Button>
              <Button autoFocus onClick={() => setConfirmSkip(false)}>
                Keep going
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </DialogContent>
    </Dialog>
  );
}

function Onboarding() {
  const [step, setStep] = useState<Step>("agents");
  const [reached, setReached] = useState<Step>("agents");
  const go = (next: Step) => {
    setStep(next);
    if (next === "projects") setReached("projects");
  };
  return (
    <div className="flex h-full min-h-0 min-w-0 flex-col px-8 pt-9 pb-8">
      <div className="flex items-center gap-2.5">
        <BrigadierGlyph className="text-foreground size-icon-lg" />
        <span className="font-display text-base font-semibold">Brigadier</span>
      </div>
      <Progress step={step} reached={reached} onStep={go} />
      {step === "agents" ? (
        <AgentsStep onContinue={() => go("projects")} />
      ) : (
        <ProjectsStep onBack={() => go("agents")} />
      )}
    </div>
  );
}

const STEPS: readonly { step: Step; label: string }[] = [
  { step: "agents", label: "Agents" },
  { step: "projects", label: "Projects" },
];

/** One bar per step, the current one longest; steps already reached can be revisited. */
function Progress({
  step,
  reached,
  onStep,
}: {
  step: Step;
  reached: Step;
  onStep: (step: Step) => void;
}) {
  const current = STEPS.findIndex((entry) => entry.step === step);
  const furthest = STEPS.findIndex((entry) => entry.step === reached);
  return (
    <TooltipProvider>
      <nav aria-label="Setup steps" className="mt-10 flex items-center gap-2">
        {STEPS.map((entry, index) => {
          const active = index === current;
          const done = index < current;
          return (
            <Tooltip key={entry.step}>
              <TooltipTrigger asChild>
                <button
                  type="button"
                  aria-label={`Step ${index + 1}: ${entry.label}`}
                  aria-current={active ? "step" : undefined}
                  disabled={index > furthest}
                  onClick={() => onStep(entry.step)}
                  className={cn(
                    "rounded-capsule relative h-1 transition-all before:absolute before:-inset-x-1 before:-inset-y-2",
                    active
                      ? "bg-foreground w-10"
                      : done
                        ? "bg-muted-foreground/70 hover:bg-foreground/80 w-6"
                        : "bg-muted-foreground/25 enabled:hover:bg-muted-foreground/45 w-6",
                  )}
                />
              </TooltipTrigger>
              <TooltipContent side="top">{entry.label}</TooltipContent>
            </Tooltip>
          );
        })}
        <span className="text-muted-foreground ms-3 text-xs font-medium">
          {current + 1} of {STEPS.length}
        </span>
      </nav>
    </TooltipProvider>
  );
}

/**
 * A step: its heading, a body filling the sheet, and the footer. The primary action also
 * runs on ⌘⏎ (Ctrl ⏎ elsewhere).
 */
function StepBody({
  eyebrow,
  title,
  description,
  children,
  start,
  onBack,
  primary,
}: {
  eyebrow?: string;
  title: string;
  description: string;
  children: ReactNode;
  start: ReactNode;
  onBack?: () => void;
  primary: { label: string; onClick: () => void; disabled?: boolean; busy?: boolean };
}) {
  const mac = useApp((s) => s.info?.platform === "macos");
  const { onClick, disabled = false, busy = false } = primary;

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Enter" || event.repeat || !(mac ? event.metaKey : event.ctrlKey)) return;
      event.preventDefault();
      if (!disabled && !busy) onClick();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [mac, onClick, disabled, busy]);

  return (
    <>
      <div className="mt-8 shrink-0">
        {eyebrow && (
          <p className="text-muted-foreground tracking-eyebrow mb-2 text-xs font-medium uppercase">
            {eyebrow}
          </p>
        )}
        <DialogTitle className="tracking-hero text-hero leading-tight font-semibold">
          {title}
        </DialogTitle>
        <DialogDescription className="mt-3 text-base">{description}</DialogDescription>
      </div>
      <div className="mt-10 flex min-h-0 flex-1 flex-col">{children}</div>
      <footer className="mt-6 flex shrink-0 items-center gap-2 border-t pt-5">
        {start}
        <div className="ms-auto flex items-center gap-2">
          {onBack && (
            <Button variant="outline" disabled={busy} onClick={onBack}>
              <ArrowLeft />
              Back
            </Button>
          )}
          <Button autoFocus disabled={disabled || busy} onClick={onClick}>
            {busy && <Spinner className="animate-spin" />}
            {primary.label}
            <Kbd className="bg-primary-foreground/10 border-primary-foreground/20 text-primary-foreground/70 ms-1">
              {mac ? "⌘⏎" : "Ctrl ⏎"}
            </Kbd>
          </Button>
        </div>
      </footer>
    </>
  );
}

function SkipButton({ label, disabled }: { label: string; disabled?: boolean }) {
  return (
    <Button
      variant="ghost"
      className="text-muted-foreground hover:text-foreground"
      disabled={disabled}
      onClick={skip}
    >
      {label}
    </Button>
  );
}

/** A small uppercase heading over a group of cards, with how many it holds. */
function SectionLabel({
  label,
  count,
  dot = false,
}: {
  label: string;
  count: number | undefined;
  dot?: boolean;
}) {
  return (
    <div className="text-muted-foreground tracking-eyebrow flex shrink-0 items-center gap-2 text-2xs font-medium uppercase">
      {dot && <span aria-hidden="true" className="bg-success size-1.5 shrink-0 rounded-full" />}
      <span>{label}</span>
      {count !== undefined && (
        <>
          <span aria-hidden="true" className="text-muted-foreground/60">
            ·
          </span>
          <span className="tabular-nums">{count}</span>
        </>
      )}
    </div>
  );
}

/** The corner check on a card that is set up or chosen. */
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

// ----- agents --------------------------------------------------------------------------

function AgentsStep({ onContinue }: { onContinue: () => void }) {
  const providers = useApp((s) => s.providers.view?.providers);
  const connected = useApp((s) => s.connection.status === "connected");
  // The agent being installed or signed in, and its state when that began.
  const [setup, setSetup] = useState<{
    provider: ProviderKind;
    install: boolean;
    from: AgentState;
  } | null>(null);
  const recheck = useAction();

  useEffect(() => {
    if (connected) void checkAgents();
  }, [connected]);

  const agents = PROVIDERS.map((provider) => {
    const overview = providers?.find((entry) => entry.provider === provider);
    return { provider, overview, state: agentState(overview) };
  });
  const anyReady = agents.some((agent) => agent.state === "ready");
  const allChecked = agents.every((agent) => agent.state !== "checking");
  // Its terminal closes by itself once the agent's state moves on (installed, signed in).
  const settingUp =
    setup && agents.find((agent) => agent.provider === setup.provider)?.state === setup.from
      ? setup
      : null;

  const installed = agents.filter((agent) => agent.state !== "install");
  const missing = agents.filter((agent) => agent.state === "install");
  const groups = allChecked
    ? [
        { label: "Detected on your system", dot: true, agents: installed },
        { label: "Not installed", dot: false, agents: missing },
      ]
    : [{ label: "Checking your system", dot: false, agents }];

  return (
    <StepBody
      eyebrow="Welcome to Brigadier"
      title="Connect your agents"
      description="Brigadier gets its work done through the coding agents on this computer, with your own subscriptions."
      start={<SkipButton label="Skip setup" />}
      primary={{ label: "Continue", onClick: onContinue }}
    >
      <div className="flex min-h-0 flex-1 flex-col gap-5">
        <div className="-m-1 flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto p-1">
          {groups
            .filter((group) => group.agents.length > 0)
            .map((group) => (
              <section key={group.label} className="flex flex-col gap-3">
                <SectionLabel
                  label={group.label}
                  dot={group.dot}
                  count={allChecked ? group.agents.length : undefined}
                />
                <ul className={CARD_GRID}>
                  {group.agents.map(({ provider, overview, state }) => (
                    <AgentCard
                      key={provider}
                      provider={provider}
                      path={overview?.status?.path ?? null}
                      state={state}
                      detail={providerStatusText(overview)}
                      settingUp={settingUp?.provider === provider}
                      onSetUp={(install) =>
                        setSetup({ provider, install, from: install ? "install" : "signIn" })
                      }
                    />
                  ))}
                </ul>
              </section>
            ))}
          {settingUp && (
            <SetupTerminal
              key={`${settingUp.provider}:${settingUp.install}`}
              provider={settingUp.provider}
              install={settingUp.install}
              onClose={() => {
                setSetup(null);
                void checkAgents();
              }}
            />
          )}
        </div>
        {!anyReady && allChecked && !settingUp && (
          <div className="bg-muted/25 rounded-surface flex shrink-0 items-center gap-3 border px-4 py-3">
            <ExclamationMarkCircle className="text-warning size-icon-md shrink-0" />
            <p className="min-w-0 flex-1 text-sm">
              Sessions need at least one agent that is signed in. You can also set this up later.
            </p>
            <Button
              size="sm"
              variant="ghost"
              disabled={recheck.busy}
              onClick={() => recheck.run(checkAgents)}
            >
              <Reload />
              Check again
            </Button>
          </div>
        )}
      </div>
    </StepBody>
  );
}

function AgentCard({
  provider,
  path,
  state,
  detail,
  settingUp,
  onSetUp,
}: {
  provider: ProviderKind;
  path: string | null;
  state: AgentState;
  detail: string;
  settingUp: boolean;
  onSetUp: (install: boolean) => void;
}) {
  const ready = state === "ready";
  return (
    <li
      className={cn(
        "rounded-surface relative flex min-w-0 items-start gap-2.5 border p-3.5 transition-colors",
        ready ? "border-success/40 bg-success/5" : "bg-muted/30",
      )}
    >
      {ready && <CardCheck className="bg-success text-success-foreground" label="Ready" />}
      <span className="bg-muted grid size-7 shrink-0 place-items-center rounded-md">
        <ProviderGlyph provider={provider} className="size-icon-md" />
      </span>
      <div className={cn("min-w-0 flex-1", ready && "pe-6")}>
        <p className="truncate text-sm font-medium">{PROVIDER_LABELS[provider]}</p>
        <p className="text-muted-foreground mt-0.5 truncate font-mono text-2xs">
          {path ? shortPath(path) : provider}
        </p>
        <p className="text-muted-foreground mt-2 truncate text-xs">{detail}</p>
      </div>
      {state === "checking" ? (
        <Spinner className="text-muted-foreground size-icon-sm shrink-0 animate-spin" />
      ) : (
        !ready && (
          <Button
            size="xs"
            variant="outline"
            className="shrink-0"
            disabled={settingUp}
            onClick={() => onSetUp(state === "install")}
          >
            {state === "install" ? "Install" : "Sign in"}
          </Button>
        )
      )}
    </li>
  );
}

// ----- projects ------------------------------------------------------------------------

function suggested(candidate: ProjectCandidate, nowMs: number): boolean {
  return (
    candidate.projectId === null &&
    candidate.sessions >= SUGGESTED_SESSIONS &&
    nowMs - candidate.lastActiveMs <= SUGGESTED_WITHIN_MS
  );
}

function ProjectsStep({ onBack }: { onBack: () => void }) {
  const [candidates, setCandidates] = useState<ProjectCandidate[] | null>(null);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [nowMs] = useState(Date.now);

  useEffect(() => {
    let live = true;
    findProjects()
      .then((found) => {
        if (!live) return;
        setCandidates(found);
        setSelected(new Set(found.filter((c) => suggested(c, nowMs)).map((c) => c.path)));
      })
      .catch((cause: unknown) => {
        if (!live) return;
        setCandidates([]);
        setError(errorText(cause));
      });
    return () => {
      live = false;
    };
  }, [nowMs]);

  const addable = (candidates ?? []).filter((candidate) => candidate.projectId === null);
  const chosen = addable.filter((candidate) => selected.has(candidate.path));

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
    <StepBody
      title="Choose your projects"
      description="These are the repositories you have worked in with Claude Code and Codex. Add the ones you want to use with Brigadier."
      start={<SkipButton label="Skip for now" disabled={busy} />}
      onBack={onBack}
      primary={{
        label: busy ? "Adding…" : `Add ${count} ${count === 1 ? "project" : "projects"}`,
        onClick: () => void add(),
        disabled: count === 0,
        busy,
      }}
    >
      <div className="flex min-h-0 flex-1 flex-col gap-3">
        <div className="flex shrink-0 flex-wrap items-center gap-1">
          <SectionLabel
            label="Found in your history"
            dot={addable.length > 0}
            count={candidates === null ? undefined : addable.length}
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
    </StepBody>
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
