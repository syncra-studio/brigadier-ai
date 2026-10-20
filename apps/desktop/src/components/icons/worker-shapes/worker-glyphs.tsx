import { useId } from "react";

import { cn } from "@/lib/utils";

/**
 * Workers' avatars: small two-tone shapes on a 16-unit grid, each drawn in one glyph colour, a
 * light body (`base`) under a full-tone mark. A worker keeps the same one wherever it appears.
 */
interface Glyph {
  base: string;
  mark: string;
  color: string;
}

const COLORS = [
  "text-glyph-1",
  "text-glyph-2",
  "text-glyph-3",
  "text-glyph-4",
  "text-glyph-5",
  "text-glyph-6",
  "text-glyph-7",
  "text-glyph-8",
] as const;

const SHAPES: readonly Omit<Glyph, "color">[] = [
  // Four petals.
  {
    base: "M8 1a3 3 0 1 1 0 6a3 3 0 1 1 0-6ZM15 8a3 3 0 1 1-6 0a3 3 0 1 1 6 0ZM8 9a3 3 0 1 1 0 6a3 3 0 1 1 0-6ZM7 8a3 3 0 1 1-6 0a3 3 0 1 1 6 0Z",
    mark: "M8 6a2 2 0 1 1 0 4a2 2 0 1 1 0-4Z",
  },
  // Hourglass.
  { base: "M2 2H14L8 8Z", mark: "M8 8L14 14H2Z" },
  // Sparkle.
  {
    base: "M8 .5C8.8 5 11 7.2 15.5 8C11 8.8 8.8 11 8 15.5C7.2 11 5 8.8.5 8C5 7.2 7.2 5 8 .5Z",
    mark: "M8 5C8.3 6.8 9.2 7.7 11 8C9.2 8.3 8.3 9.2 8 11C7.7 9.2 6.8 8.3 5 8C6.8 7.7 7.7 6.8 8 5Z",
  },
  // Crossed disc.
  {
    base: "M8 1.5a6.5 6.5 0 1 1 0 13a6.5 6.5 0 1 1 0-13Z",
    mark: "M7 1.5h2v13H7ZM1.5 7h13v2h-13Z",
  },
  // Clover.
  {
    base: "M8 1a3.5 3.5 0 1 1 0 7a3.5 3.5 0 1 1 0-7ZM4.5 6.5a3.5 3.5 0 1 1 0 7a3.5 3.5 0 1 1 0-7ZM11.5 6.5a3.5 3.5 0 1 1 0 7a3.5 3.5 0 1 1 0-7Z",
    mark: "M8 7.5a1.5 1.5 0 1 1 0 3a1.5 1.5 0 1 1 0-3Z",
  },
  // Sun.
  {
    base: "M8 1.5a6.5 6.5 0 1 1 0 13a6.5 6.5 0 1 1 0-13Z",
    mark: "M8 4.5a3.5 3.5 0 1 1 0 7a3.5 3.5 0 1 1 0-7Z",
  },
  // Diamond.
  { base: "M8 1L15 8L8 15L1 8Z", mark: "M8 4.5L11.5 8L8 11.5L4.5 8Z" },
  // Leaf.
  {
    base: "M2 14C2 7 6 2 14 2C14 10 9 14 2 14Z",
    mark: "M2 14L10 6L11 7L3 15Z",
  },
  // Hexagon.
  { base: "M8 1l6 3.5v7L8 15l-6-3.5v-7Z", mark: "M8 5l3 1.75v3.5L8 12l-3-1.75v-3.5Z" },
  // Crescent.
  {
    base: "M8 1.5a6.5 6.5 0 1 1 0 13a6.5 6.5 0 1 1 0-13Z",
    mark: "M8 1.5a6.5 6.5 0 0 0 0 13a4.5 6.5 0 0 1 0-13Z",
  },
  // Peak.
  { base: "M8 1.5L15 14H1Z", mark: "M8 7L11.2 12.5H4.8Z" },
  // Tile.
  {
    base: "M3 1.5h10A1.5 1.5 0 0 1 14.5 3v10a1.5 1.5 0 0 1-1.5 1.5H3A1.5 1.5 0 0 1 1.5 13V3A1.5 1.5 0 0 1 3 1.5Z",
    mark: "M5 5h6v6H5Z",
  },
  // Drop.
  {
    base: "M8 1C8 1 13.5 7 13.5 10.5a5.5 5.5 0 0 1-11 0C2.5 7 8 1 8 1Z",
    mark: "M8 6C8 6 11 9.2 11 11a3 3 0 0 1-6 0C5 9.2 8 6 8 6Z",
  },
  // Heart.
  {
    base: "M8 14.5S1.5 10.5 1.5 5.8A3.3 3.3 0 0 1 8 4.2A3.3 3.3 0 0 1 14.5 5.8C14.5 10.5 8 14.5 8 14.5Z",
    mark: "M8 7.5a1.5 1.5 0 1 1 0 3a1.5 1.5 0 1 1 0-3Z",
  },
  // Bolt.
  {
    base: "M8 1.5a6.5 6.5 0 1 1 0 13a6.5 6.5 0 1 1 0-13Z",
    mark: "M9 3L5 8.5h3L7 13l4-5.5H8Z",
  },
  // Six petals.
  {
    base: "M12.5 5.5a2.5 2.5 0 1 1 0 5a2.5 2.5 0 1 1 0-5ZM10.25 9.4a2.5 2.5 0 1 1 0 5a2.5 2.5 0 1 1 0-5ZM5.75 9.4a2.5 2.5 0 1 1 0 5a2.5 2.5 0 1 1 0-5ZM3.5 5.5a2.5 2.5 0 1 1 0 5a2.5 2.5 0 1 1 0-5ZM5.75 1.6a2.5 2.5 0 1 1 0 5a2.5 2.5 0 1 1 0-5ZM10.25 1.6a2.5 2.5 0 1 1 0 5a2.5 2.5 0 1 1 0-5Z",
    mark: "M8 5.5a2.5 2.5 0 1 1 0 5a2.5 2.5 0 1 1 0-5Z",
  },
  // Halo.
  {
    base: "M8 1.5a6.5 6.5 0 1 1 0 13a6.5 6.5 0 1 1 0-13ZM8 4.5a3.5 3.5 0 1 0 0 7a3.5 3.5 0 1 0 0-7Z",
    mark: "M8 6.5a1.5 1.5 0 1 1 0 3a1.5 1.5 0 1 1 0-3Z",
  },
  // Pinwheel.
  {
    base: "M8 8V1.5A6.5 6.5 0 0 1 14.5 8ZM8 8V14.5A6.5 6.5 0 0 1 1.5 8Z",
    mark: "M8 8H1.5A6.5 6.5 0 0 1 8 1.5ZM8 8H14.5A6.5 6.5 0 0 1 8 14.5Z",
  },
  // Arch.
  {
    base: "M1.5 11.25a6.5 6.5 0 0 1 13 0h-3a3.5 3.5 0 0 0-7 0Z",
    mark: "M5.5 11.25a2.5 2.5 0 0 1 5 0Z",
  },
  // Two capsules.
  {
    base: "M5 1.5h6a3 3 0 0 1 0 6H5a3 3 0 0 1 0-6Z",
    mark: "M5 8.5h6a3 3 0 0 1 0 6H5a3 3 0 0 1 0-6Z",
  },
  // Cross.
  { base: "M6 1.5h4V6h4.5v4H10v4.5H6V10H1.5V6H6Z", mark: "M6 6h4v4H6Z" },
  // Octagon.
  {
    base: "M5.3 1.5h5.4l3.8 3.8v5.4l-3.8 3.8H5.3l-3.8-3.8V5.3Z",
    mark: "M4.5 7h7v2h-7Z",
  },
  // Shield.
  {
    base: "M8 1.5L14 4v4.5c0 3-2.7 5.2-6 6c-3.3-.8-6-3-6-6V4Z",
    mark: "M8 4.5l3 1.25V8.5c0 1.6-1.3 2.8-3 3.3Z",
  },
  // Seed.
  {
    base: "M1.5 8C4 3.5 12 3.5 14.5 8C12 12.5 4 12.5 1.5 8Z",
    mark: "M8 6a2 2 0 1 1 0 4a2 2 0 1 1 0-4Z",
  },
  // Three dots.
  {
    base: "M8 1a2.5 2.5 0 1 1 0 5a2.5 2.5 0 1 1 0-5ZM3.5 9.5a2.5 2.5 0 1 1 0 5a2.5 2.5 0 1 1 0-5ZM12.5 9.5a2.5 2.5 0 1 1 0 5a2.5 2.5 0 1 1 0-5Z",
    mark: "M8 7.5l2 3.5H6Z",
  },
  // Checker.
  {
    base: "M1.5 1.5H7V7H1.5ZM9 9h5.5v5.5H9Z",
    mark: "M11.75 1.5a2.75 2.75 0 1 1 0 5.5a2.75 2.75 0 1 1 0-5.5ZM4.25 9a2.75 2.75 0 1 1 0 5.5a2.75 2.75 0 1 1 0-5.5Z",
  },
  // Flag.
  { base: "M4.5 2h9L11 5.5L13.5 9h-9Z", mark: "M3 1.5h1.5v13H3Z" },
];

const GLYPHS: readonly Glyph[] = SHAPES.map((shape, index) => ({
  ...shape,
  color: COLORS[index % COLORS.length] ?? COLORS[0],
}));

/** The glyph for an id: `h = (h * 31 + c) % 2147483647` over its characters, then `h % N`. */
function glyphFor(id: string): Glyph {
  let hash = 0;
  for (const char of id) hash = (hash * 31 + (char.codePointAt(0) ?? 0)) % 2147483647;
  return GLYPHS[hash % GLYPHS.length] ?? (GLYPHS[0] as Glyph);
}

/** The glyph's square less a ring around its status dot. */
const DOT_CUTOUT = "M0 0H16V16H0ZM13 8.75a4.25 4.25 0 1 0 0 8.5a4.25 4.25 0 1 0 0-8.5Z";

/**
 * A two-tone avatar for `id` (a worker), 1em square in its own colour unless sized or coloured
 * by the caller. With `dot`, a small status dot sits in its lower corner, cut out of the glyph
 * so it reads on any background.
 */
export function IdGlyph({
  id,
  dot,
  className,
}: {
  id: string;
  dot?: boolean | undefined;
  className?: string | undefined;
}) {
  const glyph = glyphFor(id);
  const clip = useId();
  const paths = (
    <>
      <path d={glyph.base} className="opacity-45" />
      <path d={glyph.mark} />
    </>
  );
  return (
    <svg
      viewBox="0 0 16 16"
      aria-hidden
      fill="currentColor"
      className={cn("size-icon-md shrink-0", glyph.color, className)}
    >
      {dot ? (
        <>
          <clipPath id={clip}>
            <path clipRule="evenodd" d={DOT_CUTOUT} />
          </clipPath>
          <g clipPath={`url(#${clip})`}>{paths}</g>
          <circle cx="13" cy="13" r="2.75" className="fill-success" />
        </>
      ) : (
        paths
      )}
    </svg>
  );
}
