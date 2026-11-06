// The record a dev run's checks read: every renderer exit, focus change and page load. Nothing without
// BRIGADIER_EVENTS.
import fs from "node:fs";

export type Note = (kind: string, detail?: Record<string, unknown>) => void;

const eventsFile = process.env.BRIGADIER_EVENTS ?? null;

export const note: Note = (kind, detail = {}) => {
  if (!eventsFile) return;
  // The event's own name goes last: a detail with a `kind` of its own must not replace it.
  fs.appendFileSync(eventsFile, `${JSON.stringify({ at: new Date().toISOString(), ...detail, kind })}\n`);
};
