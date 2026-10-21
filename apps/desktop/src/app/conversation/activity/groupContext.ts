import { createContext } from "react";

/** A row inside an opened group: a running command there reads "Running command for 12s". */
export const InGroup = createContext(false);

/** A row inside a fold of one tool's repeats: it says what the call was given ("Used search issues checkout tax"). */
export const InFold = createContext(false);
