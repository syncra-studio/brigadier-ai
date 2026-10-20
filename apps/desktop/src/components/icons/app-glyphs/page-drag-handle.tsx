import type { FC, SVGProps } from "react";

/** A page block's drag handle: two columns of three dots (10 × 16). */
export const PageDragHandle: FC<SVGProps<SVGSVGElement>> = (props) => (
  <svg viewBox="0 0 10 16" fill="currentColor" {...props}>
    {[3, 8, 13].flatMap((y) => [2.5, 7.5].map((x) => <circle key={`${x}-${y}`} cx={x} cy={y} r="1.25" />))}
  </svg>
);
