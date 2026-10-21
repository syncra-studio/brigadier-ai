import type { BackgroundState } from "@/app/conversation/backgroundRows";
import { Check, Pause, Spinner, X } from "@/components/icons";

/**
 * A row's icon for its state: a spinner while the process is on its way somewhere (building,
 * starting, a smoke test or a check of it), a steady green dot once it is up, a pause sign while
 * it is held. With less motion, nothing spins or pulses.
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
  if (state === "failed") return <X data-icon="failed" className="text-destructive" />;
  if (state === "finished") return <Check data-icon="finished" />;
  return <Spinner data-icon="spinner" className="motion-safe:animate-spin" />;
}
