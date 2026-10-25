import type { BackgroundState } from "@/app/conversation/backgroundRows";
import { Check, Pause, Spinner, StopCircle, Warning } from "@/components/icons";

/**
 * A row's icon for its state: a spinner while the process is on its way somewhere (building,
 * starting, a smoke test or a check of it) and while an event runs or waits (a review, CI, a
 * merge), a steady green dot once a process is up, a pause sign while it is held, a tick once a
 * CLI's background task is done or finished, a warning once it failed, a stop sign once stopped. With less
 * motion, nothing spins or pulses.
 */
export function StateIcon({ state }: { state: BackgroundState }) {
  if (state === "running") {
    return (
      <span data-icon="live" aria-hidden className="flex size-icon-md items-center justify-center">
        <span className="size-2 rounded-full bg-success motion-safe:animate-pulse" />
      </span>
    );
  }
  if (state === "paused") return <Pause data-icon="paused" />;
  if (state === "done" || state === "finished") return <Check data-icon="done" />;
  if (state === "failed") return <Warning data-icon="failed" />;
  if (state === "stopped") return <StopCircle data-icon="stopped" />;
  return <Spinner data-icon="spinner" className="motion-safe:animate-spin" />;
}
