import { type DictationPhase, useDictation } from "@/state/dictation";

/** What dictation is doing while it isn't recording: the model's download, starting, transcribing. */
function dictationNote(phase: DictationPhase): string | null {
  switch (phase.type) {
    case "downloading":
      return `Downloading the speech model${phase.total > 0 ? ` · ${Math.floor((phase.received / phase.total) * 100)}%` : "…"}`;
    case "starting":
      return "Starting dictation…";
    case "transcribing":
      return "Transcribing…";
    default:
      return null;
  }
}

/** The footer's line saying what dictation is doing, left of the model picker. */
export function DictationNote({ owner }: { owner: string }) {
  const note = dictationNote(useDictation(owner).phase);
  if (!note) return null;
  return (
    <span data-slot="composer-dictation-note" className="text-muted-foreground min-w-0 truncate px-1 text-xs">
      {note}
    </span>
  );
}
