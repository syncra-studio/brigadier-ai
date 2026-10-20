import { useEffect, useRef, useState } from "react";

import { AgentsStep } from "@/app/onboarding/AgentsStep";
import { ProjectsStep } from "@/app/onboarding/ProjectsStep";
import { Welcome } from "@/app/onboarding/Welcome";
import { preloadWelcomeSound } from "@/app/onboarding/welcomeSound";
import { Button } from "@/components/controls/button";
import { Dialog, DialogContent } from "@/components/overlays/dialog";
import { X } from "@/components/icons";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/overlays/tooltip";
import { introRelease } from "@/ipc/client";
import { useRevealed } from "@/lib/splash";
import { cn } from "@/lib/utils";
import { finishOnboarding, useOnboarding } from "@/state/onboarding";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

type Step = "intro" | "name" | "agents" | "projects";

/** Closes a run of the setup opened from Settings. */
function close(): void {
  void finishOnboarding().catch((error: unknown) => console.error(error));
}

/**
 * The first launch: a welcome intro with its sound, the name to call the user by, the coding
 * agents Brigadier works through, then the projects to start with. It fills the window and
 * shows until finished; it can't be skipped. Settings can open it again, at the agents, and
 * that run can be closed (its X on the name and the agents, or Escape).
 */
export function OnboardingDialog() {
  const catalogLoaded = useApp((s) => s.catalogLoaded);
  const onboarded = useApp((s) => s.settings.onboarded);
  const introSeen = useApp((s) => s.settings.introSeen);
  const smoke = useApp((s) => s.info?.smoke ?? false);
  const reopened = useOnboarding((s) => s.reopened);
  const finishing = useOnboarding((s) => s.finishing);
  // Not under the startup screen: it would take focus there, unseen.
  const revealed = useRevealed();
  const open = revealed && catalogLoaded && !smoke && (!onboarded || reopened || finishing);

  // The sound loads under the startup screen, so the intro can start with it.
  const intro = catalogLoaded && !smoke && !onboarded && !introSeen;
  useEffect(() => {
    if (intro) preloadWelcomeSound().catch((error: unknown) => console.error(error));
  }, [intro]);

  // A window held for the intro (src-tauri/src/intro.rs) shows normally when there's none.
  useEffect(() => {
    if (revealed && catalogLoaded && !intro) introRelease().catch((error: unknown) => console.error(error));
  }, [revealed, catalogLoaded, intro]);

  // A first launch's setup fills the window, so the startup screen fades onto its backdrop
  // rather than the app it hasn't been seen over yet; the setup opens over this once revealed.
  const cover = catalogLoaded && !smoke && !onboarded;

  return (
    <>
      {cover && <div aria-hidden className="bg-background fixed inset-0 z-50" />}
      <Dialog open={open}>
        <DialogContent
          showCloseButton={false}
          aria-describedby={undefined}
          className="bg-background start-0 top-0 flex h-full w-full max-w-none translate-x-0 translate-y-0 flex-col gap-0 overflow-hidden rounded-none p-0 ring-0 rtl:translate-x-0"
          onInteractOutside={(event) => event.preventDefault()}
          // The setup holds the focus itself until a step focuses its own control (parts.tsx
          // useFocusOnMount): the first step dot would take it and flash its tooltip.
          onOpenAutoFocus={(event) => {
            event.preventDefault();
            (event.currentTarget as HTMLElement | null)?.focus();
          }}
          onEscapeKeyDown={(event) => {
            event.preventDefault();
            if (reopened) close();
          }}
        >
          {open && <Onboarding />}
        </DialogContent>
      </Dialog>
    </>
  );
}

/** Where the setup starts: a run from Settings at the agents, otherwise where the last one left off. */
function firstStep(reopened: boolean, introSeen: boolean, userName: string): Step {
  if (reopened) return "agents";
  if (!introSeen) return "intro";
  return userName ? "agents" : "name";
}

function Onboarding() {
  const userName = useApp((s) => s.settings.userName);
  // A run from Settings: they've met already.
  const reopened = useOnboarding((s) => s.reopened);
  const [step, setStep] = useState<Step>(() => {
    const { settings } = useApp.getState();
    return firstStep(useOnboarding.getState().reopened, settings.introSeen, settings.userName);
  });
  const [reached, setReached] = useState(() => Math.max(STEPS.indexOf(step), 0));
  const stopSound = useRef<(() => void) | null>(null);

  // Closing the setup before the sound has ended fades it out. So does closing the window,
  // which only hides it (the setup stays, behind it).
  useEffect(() => () => stopSound.current?.(), []);
  const windowVisible = useApp((s) => s.windowVisible);
  useEffect(() => {
    if (!windowVisible) stopSound.current?.();
  }, [windowVisible]);

  const go = (next: Step) => {
    setStep(next);
    setReached((current) => Math.max(current, STEPS.indexOf(next)));
  };

  return (
    <div className="relative flex h-full min-h-0 flex-col">
      {step !== "intro" && (
        // As designed, the projects' strip has no Close: Back and the step's action lead on.
        <TopBar step={step} reached={reached} onStep={go} closable={reopened && step !== "projects"} />
      )}
      {step === "intro" || step === "name" ? (
        <Welcome
          step={step}
          name={userName}
          onSound={(stop) => {
            stopSound.current = stop;
            if (!useApp.getState().windowVisible) stop();
          }}
          onContinue={() => {
            go("name");
            setSetting("introSeen", true).catch((error: unknown) => console.error(error));
          }}
          onName={async (name) => {
            await setSetting("userName", name);
            go("agents");
          }}
        />
      ) : step === "agents" ? (
        <AgentsStep
          onBack={() => go("name")}
          onContinue={() => go("projects")}
        />
      ) : (
        <ProjectsStep onBack={() => go("agents")} />
      )}
    </div>
  );
}

/** The steps after the intro, in the dots at the top. */
const STEPS: readonly Step[] = ["name", "agents", "projects"];
const STEP_LABELS: Record<Step, string> = {
  intro: "Welcome",
  name: "Name",
  agents: "Agents",
  projects: "Projects",
};

/**
 * The strip along the top: the window drags by it, a segment per step (the current one longer
 * and bright; steps already reached can be revisited) and, on a run from Settings, Close at the
 * end.
 */
function TopBar({
  step,
  reached,
  onStep,
  closable,
}: {
  step: Step;
  reached: number;
  onStep: (step: Step) => void;
  closable: boolean;
}) {
  const current = STEPS.indexOf(step);
  return (
    <div
      data-tauri-drag-region
      className="h-titlebar relative z-10 grid shrink-0 grid-cols-[1fr_auto_1fr] items-center px-2"
    >
      <TooltipProvider>
        <nav aria-label="Setup steps" className="col-start-2 flex items-center gap-1">
          {STEPS.map((entry, index) => {
            const active = index === current;
            return (
              <Tooltip key={entry}>
                <TooltipTrigger asChild>
                  <button
                    type="button"
                    aria-label={`Step ${index + 1}: ${STEP_LABELS[entry]}`}
                    aria-current={active ? "step" : undefined}
                    disabled={index > reached}
                    onClick={() => onStep(entry)}
                    className={cn(
                      "rounded-capsule relative h-0.75 transition-all before:absolute before:-inset-x-0.5 before:-inset-y-2",
                      active ? "bg-foreground w-7.5" : "bg-muted enabled:hover:bg-muted-foreground/45 w-3.75",
                    )}
                  />
                </TooltipTrigger>
                <TooltipContent side="bottom">{STEP_LABELS[entry]}</TooltipContent>
              </Tooltip>
            );
          })}
        </nav>
      </TooltipProvider>
      {closable && (
        <Button
          size="icon-sm"
          variant="ghost"
          aria-label="Close setup"
          onClick={close}
          // As designed: round, 28 across, 20 in from the window's edge.
          className="text-muted-foreground hover:text-foreground me-3 size-7 justify-self-end rounded-full"
        >
          <X className="size-3.75" />
        </Button>
      )}
    </div>
  );
}
