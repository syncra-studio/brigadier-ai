import type { FC, SVGProps } from "react";

/**
 * Brigadier's busy glyph: a faint ring with a quarter arc, drawn on the icon set's 24-unit
 * grid in the current colour, sized by the caller. Callers spin it (`animate-spin`).
 */

const TRACK =
  "M12 2.25a9.75 9.75 0 1 1 0 19.5 9.75 9.75 0 0 1 0-19.5Zm0 1.5a8.25 8.25 0 1 0 0 16.5 8.25 8.25 0 0 0 0-16.5Z";

const ARC =
  "M12 2.25A9.75 9.75 0 0 1 21.75 12a.75.75 0 0 1-1.5 0A8.25 8.25 0 0 0 12 3.75a.75.75 0 0 1 0-1.5Z";

export const Spinner: FC<SVGProps<SVGSVGElement>> = (props) => (
  <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden {...props}>
    <path fillRule="evenodd" clipRule="evenodd" d={TRACK} className="opacity-25" />
    <path d={ARC} />
  </svg>
);
