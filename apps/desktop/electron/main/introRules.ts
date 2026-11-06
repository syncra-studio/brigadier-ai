// The intro's rules that need no window.

/** Whether this launch holds the window for the intro: the first on macOS, when it opens for the user. */
export const shouldHold = (platform: NodeJS.Platform, firstLaunch: boolean, smoke: boolean): boolean => platform === "darwin" && firstLaunch && !smoke;

/** Eases in and out, as the Tauri build's animation group: the opacity at `progress` of a fade. */
export function fadeValue(from: number, to: number, progress: number): number {
  const t = Math.min(1, Math.max(0, progress));
  const eased = t < 0.5 ? 2 * t * t : 1 - 2 * (1 - t) * (1 - t);
  return from + (to - from) * eased;
}
