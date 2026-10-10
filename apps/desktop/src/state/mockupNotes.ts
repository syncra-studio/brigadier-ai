/**
 * The user's notes on a mockup (SESSION-FIXES-PLAN.md Q10): each marks a region of one
 * screenshot and says what to change there; they are sent together, as one message with the
 * marked screenshots attached.
 */

/** A region as fractions of the image's width and height, so it holds at any zoom. */
export type Region = { x: number; y: number; width: number; height: number };

export type MockupNote = { id: string; screen: number; region: Region; text: string };

const clamp = (value: number) => Math.min(1, Math.max(0, value));

/**
 * The region dragged from `start` to `end` (points in the image's displayed box of `size`),
 * normalised whatever the drag's direction and kept inside the image. A drag too small to be
 * meant (`min` px either way) is a click: null.
 */
export function regionFrom(
  start: { x: number; y: number },
  end: { x: number; y: number },
  size: { width: number; height: number },
  min = 6,
): Region | null {
  if (Math.abs(end.x - start.x) < min && Math.abs(end.y - start.y) < min) return null;
  const x0 = clamp(Math.min(start.x, end.x) / size.width);
  const y0 = clamp(Math.min(start.y, end.y) / size.height);
  const x1 = clamp(Math.max(start.x, end.x) / size.width);
  const y1 = clamp(Math.max(start.y, end.y) / size.height);
  return { x: x0, y: y0, width: x1 - x0, height: y1 - y0 };
}

/** The file a screen's marked screenshot is sent as. */
export const markedName = (screen: number) => `mockup-${screen}-notes.png`;

/**
 * The one message the notes are sent as: numbered as the markers on the screenshots, each with
 * its screen, so the thread can pass them to the mockup's worker with the marked files.
 */
export function notesMessage(title: string, version: number, notes: readonly MockupNote[]): string {
  const screens = new Set(notes.map((note) => note.screen));
  const lines = notes.map((note, index) =>
    `${index + 1}. ${screens.size > 1 ? `Screen ${note.screen}: ` : ""}${note.text.trim()}`);
  const files = [...screens].toSorted((a, b) => a - b).map(markedName).join(", ");
  return `Changes to the mockup "${title}" (version ${version}):\n${lines.join("\n")}\n\nThe numbered areas are marked on ${files}.`;
}
