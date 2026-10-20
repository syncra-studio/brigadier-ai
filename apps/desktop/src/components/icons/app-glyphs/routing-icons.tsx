import type { SVGProps } from "react";

import { cn } from "@/lib/utils";

/** A stroked 24px line icon, for the design's glyphs the app's icon set lacks. */
function LineIcon({ paths, className, ...rest }: SVGProps<SVGSVGElement> & { paths: string[] }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={cn("stroke-2", className)}
      {...rest}
    >
      {paths.map((path) => (
        <path key={path} d={path} />
      ))}
    </svg>
  );
}

/** A commit on a line: Write commit messages. */
export function CommitIcon(props: SVGProps<SVGSVGElement>) {
  return <LineIcon paths={["M15 12a3 3 0 1 1-6 0a3 3 0 1 1 6 0", "M3 12h6", "M15 12h6"]} {...props} />;
}

/** A numbered list: How a model is picked. */
export function ListOrderedIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <LineIcon paths={["M10 6h11", "M10 12h11", "M10 18h11", "M4 6h1v4", "M4 10h2", "M6 18H4c0-1 2-2 2-3s-1-1.5-2-1"]} {...props} />
  );
}

/** The handle a row is dragged by: two columns of three dots. */
export function GripIcon({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden className={cn("size-3.5 shrink-0", className)}>
      {[5, 12, 19].map((y) => (
        <g key={y}>
          <circle cx="9" cy={y} r="1.6" />
          <circle cx="15" cy={y} r="1.6" />
        </g>
      ))}
    </svg>
  );
}
