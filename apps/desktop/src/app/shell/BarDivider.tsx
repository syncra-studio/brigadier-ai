/** The rule between the bar's own controls and the toolbar's; `hidden` fades it but keeps its place. */
export function BarDivider({ hidden = false }: { hidden?: boolean }) {
  return (
    <span aria-hidden data-bar-divider data-hidden={hidden || undefined}
      className="bg-session-tab-separator mx-0.5 h-4 w-px shrink-0 transition-opacity duration-150 data-hidden:opacity-0 motion-reduce:transition-none" />
  );
}
