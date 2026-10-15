import { createContext } from "react";

/**
 * The user's Stop ended the thread around these rows (a turn, a worker): when, if known. A step
 * still running then reads "Stopped npm test after 4s" with a stop mark, and its Shell box ends
 * "Stopped"; `null` when nothing was stopped.
 */
export const StoppedBy = createContext<{ atMs: number | null } | null>(null);
