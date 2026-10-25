import type { SendState } from "@/components/controls/send-button";

/** Queue/steer and the first Esc affect the action's tooltip, not its look. */
export function sendButtonState({ armed, canCancel, empty, running, resumable }: {
  armed: boolean;
  canCancel: boolean;
  empty: boolean;
  running: boolean;
  resumable: boolean;
}): SendState {
  if (armed || (canCancel && empty)) return "stop";
  if (!running && empty && resumable) return "resume";
  return "send";
}
