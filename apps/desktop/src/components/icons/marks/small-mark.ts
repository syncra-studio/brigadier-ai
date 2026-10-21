/**
 * Brigadier's mark cut for a small pixel grid: the same disc, core and upright stripes as the
 * master (`src-tauri/icons/mark.svg`), with fewer, wider stripes. At 14–32 px the master's 4.4-unit
 * strokes are under a pixel wide and blur into a haze round the core wherever a screen draws a
 * point with one pixel (Windows at 100–150%); here every stripe and every gap is a whole pixel,
 * the middle stripe two, and the stripes' tips trace the master's outline.
 *
 * Plain geometry in pixels, so the app's glyph (brand-glyph.tsx) and the icon script
 * (scripts/generate-icons.mjs) draw the same cut.
 */

/** The master's stripes: the first centred at x 27.2, then one every 14.4 units of 256. */
const COLUMN_GAP = 14.4;
const FIRST_COLUMN_X = 27.2;
const STRIPE_HEIGHTS = [
  38.4, 108.8, 145.6, 171.2, 185.6, 198.4, 204.8, 211.2, 204.8, 198.4, 185.6, 171.2, 145.6,
  108.8, 38.4,
];
/** The core's diameter as a share of the mark, the master's core with the bars through it. */
const CORE = 0.5;

export interface SmallMark {
  /** The stripes, each `[x, y, width, height]` in whole pixels. */
  stripes: [number, number, number, number][];
  /** The core: its centre and radius, in pixels. */
  core: { cx: number; cy: number; r: number };
}

/** The master's stripe height (of 256) at `x` (of 256), between its columns. */
function outline(x: number): number {
  const at = (x - FIRST_COLUMN_X) / COLUMN_GAP;
  const i = Math.max(0, Math.min(STRIPE_HEIGHTS.length - 2, Math.floor(at)));
  const t = at - i;
  return Math.max(0, STRIPE_HEIGHTS[i]! * (1 - t) + STRIPE_HEIGHTS[i + 1]! * t);
}

/** An even number of pixels near `value`, at least 2, so a centred bar keeps whole-pixel ends. */
const even = (value: number) => Math.max(2, Math.round(value / 2) * 2);

/** The mark on a `size` × `size` grid (an even size: 14, 16, 20, 24, 32…). */
export function smallMark(size: number): SmallMark {
  const centre = size / 2;
  const unit = size / 256;
  const bar = (x: number, width: number) => {
    const height = Math.min(size, even(outline((x + width / 2) / unit) * unit));
    return [x, centre - height / 2, width, height] as [number, number, number, number];
  };
  // The middle stripe takes the two pixels either side of the centre; the others stand a
  // pixel apart out to the edges, mirrored.
  const stripes = [bar(centre - 1, 2)];
  for (let x = centre - 3; x >= 0; x -= 2) stripes.push(bar(x, 1), bar(size - 1 - x, 1));
  return { stripes, core: { cx: centre, cy: centre, r: (size * CORE) / 2 } };
}
