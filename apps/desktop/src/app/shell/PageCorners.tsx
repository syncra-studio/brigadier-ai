/** Keeps the page surface's corners round over the columns that fill it (see page-corner). */
export function PageCorners() {
  return (
    <div
      aria-hidden
      className="top-titlebar start-surface-inset end-surface-inset bottom-surface-inset pointer-events-none absolute z-20"
    >
      <span className="page-corner page-corner-top-start" />
      <span className="page-corner page-corner-top-end" />
      <span className="page-corner page-corner-bottom-start" />
      <span className="page-corner page-corner-bottom-end" />
    </div>
  );
}
