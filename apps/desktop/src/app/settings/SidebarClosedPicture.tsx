/** A tiny window: the strip down its left side, or nothing, beside a few lines of chat. */
export function SidebarClosedPicture({ strip }: { strip: boolean }) {
  return (
    <div className="border-divider bg-background flex h-14 w-24 gap-1 rounded-md border p-1">
      {strip && (
        <div className="bg-foreground/15 flex w-2.5 shrink-0 flex-col items-center gap-1 rounded-sm pt-1">
          <div className="bg-foreground/40 size-1 rounded-full" />
          <div className="bg-foreground/40 size-1 rounded-full" />
          <div className="bg-foreground/40 size-1 rounded-full" />
        </div>
      )}
      <div className="bg-foreground/5 flex flex-1 flex-col justify-end gap-1 rounded-sm p-1.5">
        <div className="bg-foreground/20 h-1 w-3/4 rounded-full" />
        <div className="bg-foreground/20 h-1 w-1/2 rounded-full" />
      </div>
    </div>
  );
}
