import { Picker, type PickerOption } from "@/app/inspector/providers/Picker";

/** The Orchestrator tab's top bar: which session's orchestrator it shows. */
export function SessionBar({
  sessionId,
  options,
  onChange,
}: {
  sessionId: string;
  options: readonly PickerOption[];
  onChange: (value: string) => void;
}) {
  return (
    <div className="flex shrink-0 items-center gap-2 border-b px-3 py-2 text-xs">
      <span className="text-muted-foreground">Session</span>
      <Picker label="Session" value={sessionId} options={options} onChange={onChange} />
    </div>
  );
}
