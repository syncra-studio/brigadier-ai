import type { BackgroundState } from "@/app/conversation/backgroundRows";
import { Pause, Spinner } from "@/components/icons";

/**
 * A row's icon for its state: a spinner while the process is on its way somewhere (building,
 * starting, a smoke test or a check of it) and while an event runs or waits (checks, a review, a
 * merge), a steady green dot once a process is up, a pause sign while it is held. With less
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
  return <Spinner data-icon="spinner" className="motion-safe:animate-spin" />;
}
