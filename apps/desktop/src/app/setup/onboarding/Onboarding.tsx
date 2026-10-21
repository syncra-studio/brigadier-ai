import { useEffect, useRef, useState } from "react";

import { AgentsStep } from "@/app/setup/onboarding/AgentsStep";
import { OnboardingTopBar } from "@/app/setup/onboarding/OnboardingTopBar";
import { ProjectsStep } from "@/app/setup/onboarding/ProjectsStep";
import { type Step, STEPS } from "@/app/setup/onboarding/steps";
import { Welcome } from "@/app/setup/onboarding/Welcome";
import { useOnboarding } from "@/state/onboarding";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

/** Where the setup starts: a run from Settings at the agents, otherwise where the last one left off. */
function firstStep(reopened: boolean, introSeen: boolean, userName: string): Step {
  if (reopened) return "agents";
  if (!introSeen) return "intro";
  return userName ? "agents" : "name";
}

export function Onboarding() {
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
        <OnboardingTopBar step={step} reached={reached} onStep={go} closable={reopened && step !== "projects"} />
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
