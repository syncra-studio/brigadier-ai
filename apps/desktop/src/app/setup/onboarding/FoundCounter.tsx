/** "Found in your history" and how many, with a green dot once there are any. */
export function FoundCounter({ found, loaded }: { found: number; loaded: boolean }) {
  return (
    <div className="text-muted-foreground/70 text-label flex items-center gap-2 px-1">
      {found > 0 && <span aria-hidden className="bg-success size-1.5 shrink-0 rounded-full" />}
      <span>Found in your history</span>
      {loaded && <span className="tabular-nums">{found}</span>}
    </div>
  );
}
