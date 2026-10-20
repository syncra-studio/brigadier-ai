import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState, type ReactNode } from "react";

import { useAction } from "@/app/conversation/useAction";
import { ErrorLine } from "@/app/dialogs/fields";
import { Button } from "@/components/controls";
import { Check, Cursor } from "@/components/icons";
import type { ComputerPermission, ComputerUseStatus } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import {
  checkComputerUse,
  confirmComputerCapture,
  openPrivacySettings,
  refreshComputerUse,
  useComputerUse,
} from "@/state/computerUse";

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
          <div
            key={step.id}
            data-slot="setup-step"
            data-done={done[step.id] || undefined}
            aria-current={current === step.id ? "step" : undefined}
            className={cn(
              "text-foreground/60 flex items-center gap-2 rounded-nav px-2 py-1.5 text-sm",
              current === step.id && "bg-foreground/5 text-foreground",
            )}
          >
            <span
              className={cn(
                "flex size-5 shrink-0 items-center justify-center rounded-full border text-xs",
                done[step.id] ? "bg-success/20 text-success border-transparent" : "border-divider",
              )}
            >
              {done[step.id] ? <Check aria-label="Done" className="size-icon-xs" /> : index + 1}
            </span>
            {step.title}
          </div>
        ))}
      </nav>
      <main className="flex min-w-0 flex-1 flex-col gap-4 overflow-y-auto p-6">
        {status?.setup.state === "off" ? (
          <Panel title="Computer use is off">
            <p>Turn it on in Settings › Computer use, then come back here.</p>
          </Panel>
        ) : current === "accessibility" || current === "screenRecording" ? (
          <PermissionStep permission={current} busy={check.busy} onCheck={() => check.run(() => checkComputerUse())} />
        ) : current === "capture" ? (
          <CaptureStep />
        ) : status ? (
          <Panel title="You're all set">
            <p>Brigadier can see app windows, and agents can use apps on this Mac.</p>
            <div>
              <Button onClick={onClose}>Done</Button>
            </div>
          </Panel>
        ) : (
          <p className="text-foreground/60 text-sm">Checking…</p>
        )}
        <ErrorLine error={check.error} />
      </main>
    </div>
  );
}

function Panel({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section data-slot="setup-panel" className="flex flex-col gap-3 text-sm">
      <h1 className="text-lg font-medium">{title}</h1>
      {children}
    </section>
  );
}

const PERMISSION_TEXT: Record<ComputerPermission, { title: string; why: string }> = {
  accessibility: {
    title: "Allow Accessibility",
    why: "Lets Brigadier read buttons and text in other apps, and press them for agents.",
  },
  screenRecording: {
    title: "Allow Screen Recording",
    why: "Lets Brigadier see app windows, so agents can check what happened.",
  },
};

function PermissionStep({
  permission,
  busy,
  onCheck,
}: {
  permission: ComputerPermission;
  busy: boolean;
  onCheck: () => void;
}) {
  const open = useAction();
  const text = PERMISSION_TEXT[permission];
  return (
    <Panel title={text.title}>
      <p className="text-foreground/80">{text.why}</p>
      <ol className="text-foreground/80 list-decimal space-y-1 pl-5">
        <li>Open System Settings.</li>
        <li>Drag Brigadier Computer Use into the list, or turn it on if it's already there.</li>
      </ol>
      {permission === "screenRecording" && (
        <p className="text-foreground/60">
          If macOS offers to quit and reopen Brigadier Computer Use, either answer is fine: Brigadier restarts it by
          itself and stays open.
        </p>
      )}
      <div className="flex gap-2">
        <Button onClick={() => open.run(() => openPrivacySettings(permission))}>Open System Settings</Button>
        <Button variant="outline" disabled={busy} onClick={onCheck}>
          {busy ? "Checking…" : "Check again"}
        </Button>
      </div>
      <HelperCard />
      <ErrorLine error={open.error} />
    </Panel>
  );
}

/** The helper app, to drag into the list in System Settings. */
function HelperCard() {
  const drag = useAction();
  const reveal = useAction();
  return (
    <div className="flex flex-col gap-1.5">
      <div
        data-slot="helper-card"
        role="button"
        tabIndex={-1}
        aria-label="Brigadier Computer Use: drag this into the list in System Settings"
        onPointerDown={(event) => {
          if (event.button === 0) drag.run(() => invoke("computer_setup_drag"));
        }}
        className="border-divider bg-foreground/3 hover:bg-foreground/6 flex cursor-grab items-center gap-3 rounded-lg border p-3"
      >
        <span className="bg-foreground/10 flex size-10 shrink-0 items-center justify-center rounded-lg">
          <Cursor aria-hidden className="size-icon-lg" />
        </span>
        <div className="flex flex-col">
          <span className="font-medium">Brigadier Computer Use</span>
          <span className="text-foreground/60 text-xs">Drag this into the list in System Settings</span>
        </div>
      </div>
      <button
        type="button"
        className="text-foreground/60 hover:text-foreground self-start text-xs underline-offset-2 hover:underline"
        onClick={() => reveal.run(() => invoke("reveal_computer_helper"))}
      >
        Show in Finder
      </button>
      <ErrorLine error={drag.error ?? reveal.error} />
    </div>
  );
}

function CaptureStep() {
  const capture = useAction();
  const [problem, setProblem] = useState<string | null>(null);
  return (
    <Panel title="Check screen capture">
      <p className="text-foreground/80">Brigadier takes one test picture of a window to make sure everything works.</p>
      <p className="text-foreground/60">
        macOS may ask whether Brigadier Computer Use may “bypass the system private window picker”. That lets it look
        at app windows without asking you each time. Choose Allow.
      </p>
      <div>
        <Button
          disabled={capture.busy}
          onClick={() =>
            capture.run(async () => {
              setProblem(await confirmComputerCapture());
            })
          }
        >
          {capture.busy ? "Taking a picture…" : "Take a test picture"}
        </Button>
      </div>
      {problem && <p className="text-warning">The test picture didn't work: {problem}. Try again.</p>}
      <ErrorLine error={capture.error} />
    </Panel>
  );
}
