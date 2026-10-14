import { Suspense, useEffect } from "react";

import { prefetchSettingsPages, settingsPage } from "@/app/settings/pages";
import { whenIdle } from "@/lib/preloadable";
import { closeSettings } from "@/state/actions";
import type { SettingsPageId } from "@/state/store";

/** Something open over or inside the page that Esc belongs to first. */
const ESC_OWNERS = '[role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"]';

/** The main area while Settings is open: the chosen page. Esc goes back to what was shown. */
export function SettingsView({ page }: { page: SettingsPageId }) {
  const Page = settingsPage(page).component;

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      if (event.target instanceof Element && event.target.closest(ESC_OWNERS)) return;
      if (document.querySelector(ESC_OWNERS)) return;
      closeSettings();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
  // Once it shows and the app is idle, the other pages get ready to show at once too.
  useEffect(() => whenIdle(prefetchSettingsPages, 300), []);

  return (
    <Suspense fallback={null}>
      <Page key={page} />
    </Suspense>
  );
}
