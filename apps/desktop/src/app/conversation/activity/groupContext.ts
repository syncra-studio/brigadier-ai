import { createContext } from "react";

/** A row inside an opened group: a running command there reads "Running command for 12s". */
export const InGroup = createContext(false);

/** A row inside a fold of one tool's repeats: it says what the call was given ("Used search issues checkout tax"). */
export const InFold = createContext(false);

/** The lead is suspended for quota; its stale steps stay readable without running animation. */
export const LeadPaused = createContext(false);

/**
 * A live turn's: the user opened a row's output or a group in it, so its work stays open when its
 * answer comes rather than folding away under the user (D44, D50). `null` once the turn is done.
 */
export const OpensWork = createContext<(() => void) | null>(null);
