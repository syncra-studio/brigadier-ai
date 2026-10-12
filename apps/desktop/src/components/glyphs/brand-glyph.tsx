import { useEffect, useRef, useState, type FC, type SVGProps } from "react";

/**
 * Brigadier's mark, the striped disc: a solid core with bars through it and fifteen thin
 * strokes around it, on the 256-unit grid of its master (`src-tauri/icons/mark.svg`, which
 * also draws the app and menu-bar icons), in the current colour, sized by the caller. The new
 * chat's hero shows it above the heading and spins it while hovered: the columns drift
 * rightward through the disc's outline like a turning globe, each taking the height of the
 * column ahead as it goes, then coast to rest on the master's pose. The first launch's intro
 * turns it the same way once its strokes have gathered.
 */

/** The strokes and bars stand in columns this far apart, the first centred at x 27.2. */
const COLUMN_GAP = 14.4;
const FIRST_COLUMN_X = 27.2;

/** Each column's thin stroke height, left to right; all are 4.4 wide with round ends. */
const STRIPE_HEIGHTS = [
  38.4, 108.8, 145.6, 171.2, 185.6, 198.4, 204.8, 211.2, 204.8, 198.4, 185.6, 171.2, 145.6,
  108.8, 38.4,
];

/** The bars through the core stand in columns 2 to 12; all are 9.6 wide with round ends. */
const CORE_BAR_HEIGHTS = [
  0, 0, 57.6, 97.6, 124.8, 144, 153.6, 156.8, 153.6, 144, 124.8, 97.6, 57.6, 0, 0,
];

/** Columns drawn at any moment: one more than at rest, entering from the left while spinning. */
const COLUMNS = [-1, ...STRIPE_HEIGHTS.keys()];

/** Rounds away float noise (108.8 + 2.2) so the path data reads like the master's. */
const n = (value: number) => Math.round(value * 100) / 100;

/**
 * A vertical bar with fully rounded ends centred on (`cx`, 128), as path data (`<rect rx>` in
 * the master). One shorter than it is wide narrows to a dot, so columns fade in and out at the
 * disc's edges.
 */
const bar = (cx: number, width: number, height: number) => {
  const w = Math.min(width, height);
  if (w <= 0) return null;
  const r = n(w / 2);
  return `M${n(cx - w / 2)} ${n(128 - height / 2 + w / 2)}a${r} ${r} 0 0 1 ${n(w)} 0v${n(height - w)}a${r} ${r} 0 0 1 ${n(-w)} 0Z`;
};

/** Spin speed in grid units per second (a column every 0.4 s), reached this many seconds in. */
const SPIN_SPEED = 36;
const SPIN_RAMP_S = 0.3;
/** How hard a released spin brakes, in units per second², and the crawl it never drops below. */
const SPIN_BRAKE = 120;
const SPIN_CRAWL = 6;

/**
 * How far the columns have drifted past their rest, in `[0, COLUMN_GAP)`. While `spinning`
 * it ramps up to speed; once released it runs on to the next rest and brakes into it. Stays 0
 * when the system asks for reduced motion.
 */
const useSpinOffset = (spinning: boolean) => {
  const [offset, setOffset] = useState(0);
  const spinningRef = useRef(spinning);
  const frame = useRef<number | null>(null);

  useEffect(() => {
    spinningRef.current = spinning;
    if (!spinning || frame.current !== null) return;
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    let at = 0;
    let speed = 0;
    let last = performance.now();
    const step = (now: number) => {
      const dt = Math.min(now - last, 50) / 1000;
      last = now;
      if (spinningRef.current) {
        speed = Math.min(SPIN_SPEED, speed + (SPIN_SPEED / SPIN_RAMP_S) * dt);
      } else {
        const left = COLUMN_GAP - at;
        speed = Math.max(SPIN_CRAWL, Math.min(speed, Math.sqrt(2 * SPIN_BRAKE * left)));
      }
      at += speed * dt;
      if (!spinningRef.current && at >= COLUMN_GAP) {
        frame.current = null;
        setOffset(0);
        return;
      }
      at %= COLUMN_GAP;
      setOffset(at);
      frame.current = requestAnimationFrame(step);
    };
    frame.current = requestAnimationFrame(step);
  }, [spinning]);

  useEffect(
    () => () => {
      if (frame.current !== null) cancelAnimationFrame(frame.current);
    },
    [],
  );

  return offset;
};

/** Every column's stroke and bar with the columns `offset` units past their rest. */
const markPaths = (offset: number) => {
  const t = offset / COLUMN_GAP;
  const height = (heights: number[], column: number) =>
    (heights[column] ?? 0) * (1 - t) + (heights[column + 1] ?? 0) * t;
  const x = (column: number) => FIRST_COLUMN_X + column * COLUMN_GAP + offset;
  return {
    stripes: COLUMNS.map((column) => bar(x(column), 4.4, height(STRIPE_HEIGHTS, column))),
    coreBars: COLUMNS.map((column) => bar(x(column), 9.6, height(CORE_BAR_HEIGHTS, column))),
  };
};

type BrigadierGlyphProps = SVGProps<SVGSVGElement> & {
  /** Spin the disc while the pointer is over it. */
  spinOnHover?: boolean;
  /** Spin the disc while true (the first launch's intro turns it once). */
  spinning?: boolean;
};

export const BrigadierGlyph: FC<BrigadierGlyphProps> = ({
  spinOnHover = false,
  spinning = false,
  ...props
}) => {
  const [hovered, setHovered] = useState(false);
  const { stripes, coreBars } = markPaths(useSpinOffset(spinning || (spinOnHover && hovered)));
  return (
    <svg
      viewBox="0 0 256 256"
      fill="currentColor"
      aria-hidden
      onPointerEnter={spinOnHover ? () => setHovered(true) : undefined}
      onPointerLeave={spinOnHover ? () => setHovered(false) : undefined}
      {...props}
    >
      {stripes.map((d, i) => d && <path key={COLUMNS[i]} d={d} />)}
      <circle cx={128} cy={128} r={59} />
      {coreBars.map((d, i) => d && <path key={COLUMNS[i]} d={d} />)}
    </svg>
  );
};
