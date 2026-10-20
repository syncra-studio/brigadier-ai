import type { FC, SVGProps } from "react";

/** A plan step done in part: a ring with its left half filled (16-unit grid). */
export const PlanPartial: FC<SVGProps<SVGSVGElement>> = (props) => (
  <svg viewBox="0 0 16 16" {...props}>
    <circle cx="8" cy="8" r="5.5" fill="none" stroke="currentColor" />
    <path d="M8 2.5a5.5 5.5 0 0 0 0 11z" fill="currentColor" />
  </svg>
);
