const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const WEEK = 7 * DAY;
const YEAR = 365 * DAY;

/** How long ago `atMs` was, as a row says it: now, 4m, 2h, 3d, 1w, 2y. */
export function shortAge(atMs: number, nowMs: number): string {
  const age = nowMs - atMs;
  if (age < MINUTE) return "now";
  if (age < HOUR) return `${Math.floor(age / MINUTE)}m`;
  if (age < DAY) return `${Math.floor(age / HOUR)}h`;
  if (age < WEEK) return `${Math.floor(age / DAY)}d`;
  if (age < YEAR) return `${Math.floor(age / WEEK)}w`;
  return `${Math.floor(age / YEAR)}y`;
}

/** A project's badge: the first letters of its first two words, or a lone word's first two letters. */
export function projectInitials(name: string): string {
  const words = name.split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  const [first = "", second] = words;
  const letters = second === undefined ? [...first].slice(0, 2) : [[...first][0], [...second][0]];
  return letters.join("").toUpperCase();
}

/** `path` with the user's home folder as ~. */
export function tildePath(path: string): string {
  return path.replace(/^(?:\/Users\/[^/]+|\/home\/[^/]+|[A-Za-z]:\\Users\\[^\\]+)/, "~");
}
