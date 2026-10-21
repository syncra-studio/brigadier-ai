export function BrainProgress({ value, label }: { value: number | null; label: string }) {
  const share = value === null ? null : Math.min(100, Math.max(0, value * 100));
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={share === null ? undefined : Math.round(share)}
      className="bg-muted h-1 overflow-hidden rounded-full"
    >
      {share !== null && <div className="bg-primary h-full" style={{ width: `${share}%` }} />}
    </div>
  );
}
