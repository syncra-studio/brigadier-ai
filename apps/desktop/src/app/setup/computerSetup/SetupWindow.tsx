import { useEffect } from "react";

import { useAction } from "@/app/conversation/useAction";
import { CaptureStep } from "@/app/setup/computerSetup/CaptureStep";
import { PermissionStep } from "@/app/setup/computerSetup/PermissionStep";
import { SetupPanel } from "@/app/setup/computerSetup/SetupPanel";
import { StepRailItem } from "@/app/setup/computerSetup/StepRailItem";
import { Button, ErrorLine } from "@/components/controls";
import type { ComputerUseStatus } from "@/ipc/generated";
import { checkComputerUse, refreshComputerUse, useComputerUse } from "@/state/computerUse";

/** How often the window reads computer use while it is open (it only reads). */
const LOOK_EVERY_MS = 1500;

type Step = "accessibility" | "screenRecording" | "capture";

const STEPS: readonly { id: Step; title: string }[] = [
  { id: "accessibility", title: "Accessibility" },
  { id: "screenRecording", title: "Screen Recording" },
  { id: "capture", title: "Check screen capture" },
];

/** Which steps are done. */
export function stepsDone(status: ComputerUseStatus | null): Record<Step, boolean> {
  return {
    accessibility: status?.accessibility === "granted",
    screenRecording: status?.screenRecording === "granted",
    capture: status?.setup.state === "ready",
  };
}

/**
 * Computer use's setup window: Accessibility, then Screen Recording, then one test picture.
 * Each step ticks as soon as macOS reports it; coming back from System Settings checks again.
 */
export function SetupWindow({ onClose }: { onClose: () => void }) {
  const status = useComputerUse();
  const check = useAction();
  useEffect(() => {
    // The helper starts here if it isn't running: the user is setting it up.
    check.run(() => checkComputerUse());
    let looking: number | undefined;
    const look = () => {
      window.clearInterval(looking);
      looking = undefined;
      if (document.visibilityState === "visible") {
        looking = window.setInterval(() => void refreshComputerUse().catch(() => {}), LOOK_EVERY_MS);
      }
    };
    look();
    // Closing the window only hides it; shown again, it checks afresh.
    const shown = () => {
      if (document.visibilityState === "visible") check.run(() => checkComputerUse());
      look();
    };
    // Back from System Settings: a new Screen Recording grant needs the helper refreshed.
    const back = () => check.run(() => checkComputerUse());
    document.addEventListener("visibilitychange", shown);
    window.addEventListener("focus", back);
    return () => {
      window.clearInterval(looking);
      document.removeEventListener("visibilitychange", shown);
      window.removeEventListener("focus", back);
    };
    // Once, when the window opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const done = stepsDone(status);
  const current = STEPS.find((step) => !done[step.id])?.id ?? null;
  return (
    <div className="bg-background text-foreground flex h-screen select-none">
      <nav aria-label="Steps" className="border-divider flex w-48 shrink-0 flex-col gap-1 border-r p-4 pt-6">
        {STEPS.map((step, index) => (
          <StepRailItem
            key={step.id}
            title={step.title}
            number={index + 1}
            done={done[step.id]}
            current={current === step.id}
          />
        ))}
      </nav>
      <main className="flex min-w-0 flex-1 flex-col gap-4 overflow-y-auto p-6">
        {status?.setup.state === "off" ? (
          <SetupPanel title="Computer use is off">
            <p>Turn it on in Settings › Computer use, then come back here.</p>
          </SetupPanel>
        ) : current === "accessibility" || current === "screenRecording" ? (
          <PermissionStep permission={current} busy={check.busy} onCheck={() => check.run(() => checkComputerUse())} />
        ) : current === "capture" ? (
          <CaptureStep />
        ) : status ? (
          <SetupPanel title="You're all set">
            <p>Brigadier can see app windows, and agents can use apps on this Mac.</p>
            <div>
              <Button onClick={onClose}>Done</Button>
            </div>
          </SetupPanel>
        ) : (
          <p className="text-foreground/60 text-sm">Checking…</p>
        )}
        <ErrorLine error={check.error} />
      </main>
    </div>
  );
}
