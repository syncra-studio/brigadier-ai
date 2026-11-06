// The record a dev run's checks read: every renderer exit, focus change and page load. Nothing without
// BRIGADIER_EVENTS.
import fs from "node:fs";

export type Note = (kind: string, detail?: Record<string, unknown>) => void;

const eventsFile = process.env.BRIGADIER_EVENTS ?? null;

export const note: Note = (kind, detail = {}) => {
  if (!eventsFile) return;
  fs.appendFileSync(eventsFile, `${JSON.stringify({ at: new Date().toISOString(), kind, ...detail })}\n`);
};
