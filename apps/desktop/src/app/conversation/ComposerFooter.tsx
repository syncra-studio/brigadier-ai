import type { ReactNode } from "react";

import { DictationBar } from "@/app/conversation/DictationBar";
import { useDictation } from "@/state/dictation";

/** The composer's footer row; while the microphone records, the dictation controls take all of it. */
export function ComposerFooter({ owner, children }: { owner: string; children: ReactNode }) {
  const { phase } = useDictation(owner);
  return (
    <div className="flex items-center gap-1">
      {phase.type === "recording" ? <DictationBar owner={owner} /> : children}
    </div>
  );
}
