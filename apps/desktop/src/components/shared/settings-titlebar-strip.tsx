import type { ReactNode } from "react";

import { TitlebarTips } from "@/components/shared/titlebar-button";

/**
 * Settings' strip of the titlebar: only its buttons (Terminal), at the end. The page scrolls
 * below it, clipped at its bottom edge.
 */
export function SettingsTitlebarStrip({ children }: { children: ReactNode }) {
  return (
    <div data-tauri-drag-region className="h-titlebar flex shrink-0 items-center justify-end pe-1">
      <TitlebarTips>{children}</TitlebarTips>
    </div>
  );
}
