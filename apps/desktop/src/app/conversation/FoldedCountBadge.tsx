/** The folded status slot's count: how many rows the one line stands for. */
export function FoldedCountBadge({ count }: { count: number }) {
  return <span className="bg-foreground/10 text-foreground/70 rounded-full px-1.5 text-xs tabular-nums">{count}</span>;
}
