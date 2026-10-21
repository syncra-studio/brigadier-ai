import { tokenPx } from "@/lib/tokens";

/* Where the sidebar's menus open. */

/**
 * How far a menu opened beside the collapsed strip sits from its icon: past the strip's edge
 * (the strip's inset), then the usual gap.
 */
export function besideStrip(): number {
  return tokenPx("--spacing") * 3;
}

/** Where a foot icon's menu opens: above it, or beside the strip while collapsed. */
export function footMenuPlacement(expanded: boolean) {
  return expanded
    ? ({ side: "top", align: "start" } as const)
    : ({ side: "right", align: "end", sideOffset: besideStrip() } as const);
}
