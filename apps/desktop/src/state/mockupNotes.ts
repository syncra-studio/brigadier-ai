import { create } from "zustand";

/**
 * The user's notes on a mockup (SESSION-FIXES-PLAN.md Q10): a click drops a numbered marker on
 * a screenshot and a note says what to change there. They are kept until sent together, as one
 * message with the marked screenshots.
 */

/** A note at a point of one screen's screenshot, as fractions of its width and height. */
export type MockupNote = { id: string; screen: number; x: number; y: number; text: string };

const clamp = (value: number) => Math.min(1, Math.max(0, value));

/** A point in the image's displayed box of `size`, as fractions kept inside the image. */
export function pointIn(point: { x: number; y: number }, size: { width: number; height: number }): { x: number; y: number } {
  return { x: clamp(point.x / size.width), y: clamp(point.y / size.height) };
}

/** A fraction as a percent with at most one decimal: 0.3 → "30%", 1/3 → "33.3%". */
export function percent(fraction: number): string {
  return `${Math.round(clamp(fraction) * 1000) / 10}%`;
}

/** The file a screen's marked screenshot is sent as. */
export const markedName = (screen: number) => `mockup-${screen}-notes.png`;

/** Each screen with notes, in screen order, with its notes in the order they were made. */
export function notesByScreen(notes: readonly MockupNote[]): { screen: number; notes: MockupNote[] }[] {
  const screens = [...new Set(notes.map((note) => note.screen))].toSorted((a, b) => a - b);
  return screens.map((screen) => ({ screen, notes: notes.filter((note) => note.screen === screen) }));
}

/** A note's marker number: its place among its screen's notes. */
export function markerNumber(notes: readonly MockupNote[], note: MockupNote): number {
  return notes.filter((other) => other.screen === note.screen).indexOf(note) + 1;
}

/**
 * The one message the notes are sent to the mockup's worker as: one `Image N:` block per
 * marked screenshot, each note with its point and marker number.
 */
export function notesMessage(version: number, notes: readonly MockupNote[]): string {
  const blocks = notesByScreen(notes).map(({ screen, notes: own }, index) => [
    `Image ${index + 1} (${markedName(screen)}):`,
    ...own.map((note, at) => `${at + 1}. (x: ${percent(note.x)}, y: ${percent(note.y)}) ${note.text.trim()}`),
  ].join("\n"));
  return [`Notes on the mockup (version ${version}), numbered as the markers on the attached screenshots:`, ...blocks].join("\n\n");
}

type Box = { width: number; height: number };

/**
 * Where a note's box goes beside the point it is about, inside `surface` with `margin` to
 * spare: right of the marker (`gap` from the point), else left of it, else centred below,
 * else above.
 */
export function notePlacement(point: { x: number; y: number }, box: Box, surface: Box, gap: number, margin: number): { left: number; top: number } {
  const fitsX = (left: number) => left >= margin && left + box.width <= surface.width - margin;
  const fitsY = (top: number) => top >= margin && top + box.height <= surface.height - margin;
  const keep = (value: number, size: number, room: number) => Math.max(margin, Math.min(value, room - margin - size));
  const middle = point.y - box.height / 2;
  if (fitsX(point.x + gap)) return { left: point.x + gap, top: keep(middle, box.height, surface.height) };
  if (fitsX(point.x - gap - box.width)) return { left: point.x - gap - box.width, top: keep(middle, box.height, surface.height) };
  const left = keep(point.x - box.width / 2, box.width, surface.width);
  return fitsY(point.y + gap) ? { left, top: point.y + gap } : { left, top: keep(point.y - gap - box.height, box.height, surface.height) };
}

/**
 * A session's notes on its mockup (the task `task` built) while they are made, kept while the
 * tab closes and opens. `sent` holds the screenshots shown when notes went, each with its
 * screen, until the user moves to the revised ones (`dismissed`: not now, the next time the
 * mockup opens). `sending` and `failed` are the send in flight and the last one's failure,
 * kept with the notes so a tab opened again mid-send still has them frozen.
 */
type SessionNotes = {
  task: string | null; notes: MockupNote[]; commenting: boolean;
  sent: { version: number; shots: string[]; screens: (number | null)[]; dismissed: boolean } | null;
  sending: boolean; failed: boolean;
};

/** No notes, for no mockup yet. */
export const NO_NOTES: SessionNotes = { task: null, notes: [], commenting: false, sent: null, sending: false, failed: false };
const EMPTY = NO_NOTES;

export const useMockupNotes = create<{ sessions: Record<string, SessionNotes> }>(() => ({ sessions: {} }));

export function mockupNotesOf(conversationId: string): SessionNotes {
  return useMockupNotes.getState().sessions[conversationId] ?? EMPTY;
}

export function useSessionNotes(conversationId: string): SessionNotes {
  return useMockupNotes((s) => s.sessions[conversationId] ?? EMPTY);
}

export function changeNotes(conversationId: string, change: (current: SessionNotes) => Partial<SessionNotes>): void {
  useMockupNotes.setState(({ sessions }) => {
    const current = sessions[conversationId] ?? EMPTY;
    return { sessions: { ...sessions, [conversationId]: { ...current, ...change(current) } } };
  });
}

/**
 * Takes the session's notes to send: none when there are none or a send already runs, so it
 * is one send at a time per session, however often its tab closes and opens meanwhile.
 */
export function takeNotes(conversationId: string): MockupNote[] | null {
  const { notes, sending } = mockupNotesOf(conversationId);
  if (!notes.length || sending) return null;
  changeNotes(conversationId, () => ({ sending: true, failed: false }));
  return notes;
}

/**
 * Ends the send of the notes `taken`: sent (`sent`, the screenshots they went on), only those
 * notes leave; failed (`null`), they stay to try again.
 */
export function sendEnded(conversationId: string, taken: readonly MockupNote[], sent: SessionNotes["sent"]): void {
  changeNotes(conversationId, (current) => {
    if (!sent) return { sending: false, failed: true };
    const gone = new Set(taken.map((note) => note.id));
    const notes = current.notes.filter((note) => !gone.has(note.id));
    return { sending: false, notes, commenting: current.commenting && notes.length > 0, sent };
  });
}
