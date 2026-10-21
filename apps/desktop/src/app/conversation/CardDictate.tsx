import { TooltipIconButton } from "@/components/controls";
import { Mic, Spinner } from "@/components/icons";
import { cancelDictation, startDictation, stopDictation, useDictation } from "@/state/dictation";

/**
 * The free-text row's microphone: dictates into the row, as the composer's does into the
 * composer; it stops (the words going into the row) or cancels while busy.
 */
export function CardDictate({ owner, disabled }: { owner: string; disabled: boolean }) {
  const { available, phase } = useDictation(owner);
  if (!available) return null;
  const recording = phase.type === "recording";
  const busy = phase.type === "downloading" || phase.type === "starting" || phase.type === "transcribing";
  return (
    <TooltipIconButton
      tooltip={recording ? "Stop dictation" : busy ? "Cancel dictation" : "Dictate"}
      side="top"
      type="button"
      size="icon-md"
      data-card-dictate={phase.type}
      className="rounded-capsule"
      disabled={disabled || phase.type === "transcribing"}
      onMouseDown={(event) => event.preventDefault()}
      onClick={() => (recording ? void stopDictation() : busy ? cancelDictation() : void startDictation(owner))}
    >
      {busy ? <Spinner className="size-icon-sm animate-spin motion-reduce:animate-none" /> : <Mic className={recording ? "size-icon-sm text-destructive" : "size-icon-sm"} />}
    </TooltipIconButton>
  );
}
