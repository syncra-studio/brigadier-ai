import { create } from "zustand";

/** Something the open view lets the bottom bar do: whether it is on, and turning it over. */
export type BarAction = { on: boolean; toggle: () => void };

/**
 * What the bottom bar offers only where the open view can do it. The view publishes it while
 * it is mounted and takes it back when it goes. `terminalCover` is set while the view's full
 * view hides its terminal: calling it leaves full view.
 */
export const useBarActions = create<{
  sideChat: BarAction | null;
  terminalCover: (() => void) | null;
}>(() => ({ sideChat: null, terminalCover: null }));
