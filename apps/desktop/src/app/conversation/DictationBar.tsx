import { DictationWaveform } from "@/app/conversation/DictationWaveform";
import { TooltipIconButton } from "@/components/controls";
import { useAui } from "@/components/conversation";
import { ArrowUp, Stop, X } from "@/components/icons";
import { cancelDictation, stopDictation, useDictation } from "@/state/dictation";

/**
 * The recording bar: Cancel, the microphone's waveform, Stop (the text goes to the caret) and
 * "Transcribe and send".
 */
export function DictationBar({ owner }: { owner: string }) {
  const aui = useAui();
  const { levels } = useDictation(owner);
  return (
    <div
      data-slot="composer-dictation"
      data-phase="recording"
      className="flex min-w-0 flex-1 items-center gap-1"
    >
      <TooltipIconButton
        tooltip="Cancel dictation"
        side="bottom"
        type="button"
        size="icon-md"
        className="rounded-capsule"
        onClick={cancelDictation}
      >
        <X className="size-icon-sm" />
      </TooltipIconButton>
      <DictationWaveform levels={levels} />
      <TooltipIconButton
        tooltip="Stop dictation"
        side="bottom"
        type="button"
        size="icon-md"
        className="rounded-capsule"
        onClick={() => void stopDictation()}
      >
        <Stop className="size-icon-sm" />
      </TooltipIconButton>
      <TooltipIconButton
        tooltip="Transcribe and send"
        side="bottom"
        type="button"
        variant="default"
        size="icon-md"
        className="rounded-capsule"
        onClick={() => void stopDictation(() => aui.composer().send())}
      >
        <ArrowUp />
      </TooltipIconButton>
    </div>
  );
}
