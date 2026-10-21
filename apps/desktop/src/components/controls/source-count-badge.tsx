/** How many files Source lists, on its view's icon: up to 99, then "99+". */
export function SourceCountBadge({ count }: { count: number }) {
  return (
    <span aria-hidden data-slot="source-count"
      className="bg-link text-background absolute -end-0.5 -top-0.5 flex h-4 min-w-4 items-center justify-center rounded-full px-1 text-2xs leading-none tabular-nums">
      {count > 99 ? "99+" : count}
    </span>
  );
}
