/** The recording bar's waveform: one bar per level the microphone heard, the newest at the right. */
export function DictationWaveform({ levels }: { levels: readonly number[] }) {
  return (
    <div
      aria-hidden
      className="flex h-7 min-w-0 flex-1 items-center justify-end gap-0.5 overflow-hidden px-1"
    >
      {levels.map((level, index) => (
        <span
          // Bars shift left as new ones arrive; their place is their identity.
          // oxlint-disable-next-line react/no-array-index-key
          key={index}
          className="bg-foreground w-0.5 shrink-0 rounded-full"
          style={{ height: `${Math.max(2, Math.round(level * 24))}px` }}
        />
      ))}
    </div>
  );
}
