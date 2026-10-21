/**
 * The page surface the sidebar panel and the content sit on; their headers stay above it, in the
 * titlebar strip.
 */
export function PageSurface() {
  return (
    <div
      aria-hidden
      data-slot="page-surface"
      className="bg-background rounded-page shadow-page top-titlebar start-surface-inset end-surface-inset bottom-surface-inset pointer-events-none absolute"
    />
  );
}
