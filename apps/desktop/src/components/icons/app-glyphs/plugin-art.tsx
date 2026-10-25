import type { SVGProps } from "react";

import { cn } from "@/lib/utils";

/*
 * The Plugins page's two illustrations, for an item with no icon of its own: an MCP server (a
 * server with a link going out, on a blue wash) and a skill (a page of instructions with a spark,
 * on a pink wash). Each fills its own rounded tile; sized by the caller (36 in a row, 44 beside a
 * details page's title).
 */

type ArtProps = SVGProps<SVGSVGElement>;

/** The tile, and its top half a shade lighter. */
const TILE = "M6.67 0h10.66A6.67 6.67 0 0 1 24 6.67v10.66A6.67 6.67 0 0 1 17.33 24H6.67A6.67 6.67 0 0 1 0 17.33V6.67A6.67 6.67 0 0 1 6.67 0Z";
const TILE_TOP = "M6.67 0h10.66A6.67 6.67 0 0 1 24 6.67V12H0V6.67A6.67 6.67 0 0 1 6.67 0Z";

export function ServerArt({ className, ...props }: ArtProps) {
  return (
    <svg viewBox="0 0 24 24" aria-hidden className={cn("shrink-0", className)} {...props}>
      <path d={TILE} className="fill-toggle-on/20" />
      <path d={TILE_TOP} className="fill-toggle-on/10" />
      <g fill="none" strokeLinecap="round" className="stroke-toggle-on stroke-1">
        <path d="M6.47 6h7.06a1.47 1.47 0 0 1 1.47 1.47v2.06A1.47 1.47 0 0 1 13.53 11H6.47A1.47 1.47 0 0 1 5 9.53V7.47A1.47 1.47 0 0 1 6.47 6Z" />
        <path d="M6.47 13h7.06a1.47 1.47 0 0 1 1.47 1.47v2.06A1.47 1.47 0 0 1 13.53 18H6.47A1.47 1.47 0 0 1 5 16.53v-2.06A1.47 1.47 0 0 1 6.47 13Z" />
        <path d="M15 8.5h2a1.33 1.33 0 0 1 1.33 1.33v4.34A1.33 1.33 0 0 1 17 15.5h-2" />
        <path d="M9.67 8.5h3M9.67 15.5h3" className="stroke-toggle-on/60" />
      </g>
      <path d="M7.33 7.77a.73.73 0 1 1 0 1.46a.73.73 0 1 1 0-1.46ZM7.33 14.77a.73.73 0 1 1 0 1.46a.73.73 0 1 1 0-1.46Z" className="fill-success" />
      <path d="M19.33 10.53a1.47 1.47 0 1 1 0 2.94a1.47 1.47 0 1 1 0-2.94Z" className="fill-toggle-on" />
    </svg>
  );
}

export function SkillArt({ className, ...props }: ArtProps) {
  return (
    <svg viewBox="0 0 24 24" aria-hidden className={cn("shrink-0", className)} {...props}>
      <path d={TILE} className="fill-file-pink/20" />
      <path d={TILE_TOP} className="fill-file-pink/10" />
      <g fill="none" strokeLinejoin="round" strokeLinecap="round" className="stroke-file-pink stroke-1">
        <path d="M6.67 5.67h7l3.33 3.33v8.33a1.33 1.33 0 0 1-1.33 1.34H6.67a1.33 1.33 0 0 1-1.34-1.34V7a1.33 1.33 0 0 1 1.34-1.33Z" />
        <path d="M13.67 5.67V8a1 1 0 0 0 1 1H17" />
        <path d="M7.67 10.33h4M7.67 12.67h2.66" className="stroke-file-pink/60" />
      </g>
      <path d="M16 12.33l.83 2.17 2.17.83-2.17.84L16 18.33l-.83-2.16-2.17-.84 2.17-.83Z" className="fill-foreground" />
    </svg>
  );
}
