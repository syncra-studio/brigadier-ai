import type { FC } from "react";

/** A 12 pt progress ring, filling clockwise from the top as phases finish. */
export const Donut: FC<{ done: number; total: number }> = ({ done, total }) => (
  <svg aria-hidden viewBox="0 0 16 16" className="text-link size-icon-xs shrink-0 -rotate-90">
    <circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" className="stroke-2 opacity-25" />
    <circle
      cx="8"
      cy="8"
      r="6"
      fill="none"
      stroke="currentColor"
      pathLength={100}
      strokeDasharray={`${total > 0 ? (done / total) * 100 : 0} 100`}
      className="stroke-2 transition-[stroke-dasharray] duration-200 motion-reduce:transition-none"
    />
  </svg>
);
