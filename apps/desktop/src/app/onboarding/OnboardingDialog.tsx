import { X } from "@openai/apps-sdk-ui/components/Icon";
import { useCallback, useEffect, useRef, useState } from "react";

import { AgentsStep } from "@/app/onboarding/AgentsStep";
import { ProjectsStep } from "@/app/onboarding/ProjectsStep";
import { Welcome } from "@/app/onboarding/Welcome";
import { preloadWelcomeSound } from "@/app/onboarding/welcomeSound";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useRevealed } from "@/lib/splash";
import { cn } from "@/lib/utils";
import { finishOnboarding, useOnboarding } from "@/state/onboarding";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

type Step = "intro" | "name" | "agents" | "projects";

function skip(): void {
  void finishOnboarding().catch((error: unknown) => console.error(error));
}

/**
 * The first launch: a welcome intro with its sound, the name to call the user by, the coding
 * agents Brigadier works through, then the projects to start with. It fills the window and
 * shows until finished or skipped once; Settings can open it again, at the agents. Escape
 * (once the intro has played, and outside an agent's own page) asks before skipping.
 */
export function OnboardingDialog() {
  const catalogLoaded = useApp((s) => s.catalogLoaded);
  const onboarded = useApp((s) => s.settings.onboarded);
  const introSeen = useApp((s) => s.settings.introSeen);
  const smoke = useApp((s) => s.info?.smoke ?? false);
  const reopened = useOnboarding((s) => s.reopened);
  // Not under the startup screen: it would take focus there, unseen.
  const revealed = useRevealed();
  const open = revealed && catalogLoaded && !smoke && (!onboarded || reopened);
  const [confirmSkip, setConfirmSkip] = useState(false);
  // Takes Escape first when a step has something of its own to close.
  const escape = useRef<(() => boolean) | null>(null);
  const onEscape = useCallback((handler: (() => boolean) | null) => {
    escape.current = handler;
  }, []);

  // The sound loads under the startup screen, so the intro can start with it.
  const intro = catalogLoaded && !smoke && !onboarded && !introSeen;
  useEffect(() => {
    if (intro) preloadWelcomeSound().catch((error: unknown) => console.error(error));
  }, [intro]);

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
            if (escape.current?.()) return;
            setConfirmSkip(true);
          }}
        >
          {open && <Onboarding onEscape={onEscape} onSkip={() => setConfirmSkip(true)} />}
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
    </>
  );
}

/** Where the setup starts: a run from Settings at the agents, otherwise where the last one left off. */
function firstStep(reopened: boolean, introSeen: boolean, userName: string): Step {
  if (reopened) return "agents";
  if (!introSeen) return "intro";
  return userName ? "agents" : "name";
}

function Onboarding({
  onEscape,
  onSkip,
}: {
  onEscape: (handler: (() => boolean) | null) => void;
  onSkip: () => void;
}) {
  const userName = useApp((s) => s.settings.userName);
  // A run from Settings: they've met already.
  const reopened = useOnboarding((s) => s.reopened);
  const [step, setStep] = useState<Step>(() => {
    const { settings } = useApp.getState();
    return firstStep(useOnboarding.getState().reopened, settings.introSeen, settings.userName);
  });
  const [reached, setReached] = useState(() => Math.max(STEPS.indexOf(step), 0));
  const stopSound = useRef<(() => void) | null>(null);

  // Closing the setup before the sound has ended fades it out.
  useEffect(() => () => stopSound.current?.(), []);

  const go = (next: Step) => {
    setStep(next);
    setReached((current) => Math.max(current, STEPS.indexOf(next)));
  };

  return (
    <div className="relative flex h-full min-h-0 flex-col">
      {step !== "intro" && <TopBar step={step} reached={reached} onStep={go} onSkip={onSkip} />}
      {step === "intro" || step === "name" ? (
        <Welcome
          step={step}
          name={userName}
          onSound={(stop) => {
            stopSound.current = stop;
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
          name={reopened ? "" : userName}
          onEscape={onEscape}
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
 * The strip along the top: the window drags by it, a dot per step (the current one longest;
 * steps already reached can be revisited) and Skip at the end.
 */
function TopBar({
  step,
  reached,
  onStep,
  onSkip,
}: {
  step: Step;
  reached: number;
  onStep: (step: Step) => void;
  onSkip: () => void;
}) {
  const current = STEPS.indexOf(step);
  return (
    <div
      data-tauri-drag-region
      className="h-titlebar relative z-10 grid shrink-0 grid-cols-[1fr_auto_1fr] items-center px-2"
    >
      <TooltipProvider>
        <nav aria-label="Setup steps" className="col-start-2 flex items-center gap-2">
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
                      "rounded-capsule relative h-1 transition-all before:absolute before:-inset-x-1 before:-inset-y-2",
                      active
                        ? "bg-foreground w-8"
                        : index < current
                          ? "bg-muted-foreground/70 hover:bg-foreground/80 w-4"
                          : "bg-muted-foreground/25 enabled:hover:bg-muted-foreground/45 w-4",
                    )}
                  />
                </TooltipTrigger>
                <TooltipContent side="bottom">{STEP_LABELS[entry]}</TooltipContent>
              </Tooltip>
            );
          })}
        </nav>
      </TooltipProvider>
      <Button
        size="icon-sm"
        variant="ghost"
        aria-label="Skip setup"
        onClick={onSkip}
        className="text-muted-foreground hover:text-foreground justify-self-end"
      >
        <X />
      </Button>
    </div>
  );
}
