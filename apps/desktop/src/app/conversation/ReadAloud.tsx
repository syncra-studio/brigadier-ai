import type { FC } from "react";

import { TooltipIconButton } from "@/components/controls";
import { useAuiState } from "@/components/conversation";
import { SoundOnReadOutLoudSpeaker, Stop } from "@/components/icons";
import { readAloud, stopReading, useReading } from "@/state/readAloud";

/** "Read aloud": the answer in the system's voice; "Stop" while it reads. */
export const ReadAloud: FC<{ text: string }> = ({ text }) => {
  const id = useAuiState((s) => s.message.id);
  const reading = useReading(id);
  if (!text) return null;
  return reading ? (
    <TooltipIconButton tooltip="Stop" onClick={stopReading}>
      <Stop />
    </TooltipIconButton>
  ) : (
    <TooltipIconButton tooltip="Read aloud" onClick={() => readAloud(id, text)}>
      <SoundOnReadOutLoudSpeaker />
    </TooltipIconButton>
  );
};
