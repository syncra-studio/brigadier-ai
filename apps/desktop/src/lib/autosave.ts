/*
 * The state of a text box that saves itself: what is saved, the edit waiting to be saved, the
 * save on its way, and a short "Saved" after one lands. A pure model; `useAutosave` runs its
 * timers.
 */

/** How long typing must pause before an edit saves itself, and how long "Saved" shows. */
export const AUTOSAVE_MS = 3000;
export const SAVED_MS = 3000;

export type Autosave = {
  /** What was last saved. */
  saved: string;
  /** An edit not saved yet, or `null`. */
  draft: string | null;
  /** The value being saved now, or `null`. */
  saving: string | null;
  /** A save just landed: "Saved" shows until the next edit or `SAVED_MS` later. */
  justSaved: boolean;
};

/** What the box shows at its end: a Save button while there's an edit, "Saved" after. */
export type AutosaveStatus = "idle" | "dirty" | "saving" | "saved";

export const autosave = (saved: string): Autosave => ({
  saved,
  draft: null,
  saving: null,
  justSaved: false,
});

/** The text in the box. */
export const shown = (state: Autosave): string => state.draft ?? state.saving ?? state.saved;

export function status(state: Autosave): AutosaveStatus {
  if (state.saving !== null) return "saving";
  if (state.draft !== null) return "dirty";
  return state.justSaved ? "saved" : "idle";
}

/** The user typed `value`; typing back what's saved leaves nothing to save. */
export function edited(state: Autosave, value: string): Autosave {
  const settled = state.saving ?? state.saved;
  return { ...state, draft: value === settled ? null : value, justSaved: false };
}

/** Starts saving the edit, if there is one and no save is on its way: the value to save. */
export function begin(state: Autosave): [Autosave, string | null] {
  if (state.draft === null || state.saving !== null) return [state, null];
  return [{ ...state, saving: state.draft, draft: null }, state.draft];
}

/** The save landed. An edit typed meanwhile stays waiting. */
export function landed(state: Autosave): Autosave {
  if (state.saving === null) return state;
  return { ...state, saved: state.saving, saving: null, justSaved: state.draft === null };
}

/** The save failed: its value is an edit again, unless the user typed on. */
export function failed(state: Autosave): Autosave {
  if (state.saving === null) return state;
  return { ...state, draft: state.draft ?? state.saving, saving: null };
}

/** "Saved" has shown long enough. */
export const quiet = (state: Autosave): Autosave => ({ ...state, justSaved: false });

/** What was saved changed elsewhere (another window): taken when the box has no edit. */
export function external(state: Autosave, saved: string): Autosave {
  return state.draft === null && state.saving === null ? { ...state, saved } : state;
}
