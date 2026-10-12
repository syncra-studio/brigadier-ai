import { preloadable } from "@/lib/preloadable";
import { prefetchUsage } from "@/state/usage";

// Settings (with its pages and their search) loads off the cold-start path: at idle after
// startup, when the pointer comes to the Settings button, or when first opened.
const nav = preloadable(() => import("@/app/settings/SettingsNav").then((module) => module.SettingsNav));
const view = preloadable(() => import("@/app/settings/SettingsView").then((module) => module.SettingsView));

export const SettingsNav = nav.View;
export const SettingsView = view.View;

/** Loads Settings' code ahead of opening it, so it shows on the first frame after the click. */
export function preloadSettings(): void {
  Promise.all([nav.preload(), view.preload()]).catch((error: unknown) =>
    console.error("loading Settings failed", error),
  );
}

/**
 * At idle after startup: Settings' code (its pages' too), and the usage read its Usage and
 * Providers pages show (kept, and read again when one of them opens), so each shows filled in
 * from the start.
 */
export function warmSettings(): void {
  preloadSettings();
  prefetchUsage();
  // Then the pages that load apart (the Inspector, which its shortcut opens straight away).
  view
    .preload()
    .then(() => import("@/app/settings/pages"))
    .then((pages) => pages.prefetchSettingsPages())
    .catch(() => {});
}
