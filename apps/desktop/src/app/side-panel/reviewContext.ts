import { createContext } from "react";

/** Whose checkout a file's diff is of. */
export const ReviewContext = createContext("");

/** The checkout a file's diff was read in: the worktree it names if any, and the folder it is in. */
export const ReviewPlaceContext = createContext<{ checkout: string | undefined; root: string | null }>({ checkout: undefined, root: null });
