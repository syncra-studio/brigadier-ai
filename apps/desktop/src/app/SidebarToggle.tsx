import { Sidebar, SidebarFloatingLeft } from "@openai/apps-sdk-ui/components/Icon";

import { TITLEBAR_BUTTON } from "@/components/titlebar-button";
import { Button } from "@/components/ui/button";

/**
 * The sidebar toggle in the titlebar: one plain icon button. How the sidebar closes (to its
 * strip, or completely) is chosen in Settings › General › Appearance.
 */
export function SidebarToggle({ open, onToggle }: { open: boolean; onToggle: () => void }) {
  return (
    <Button
      variant="ghost"
      size="icon-md"
      data-slot="sidebar-toggle"
      aria-label={open ? "Hide sidebar" : "Show sidebar"}
      aria-expanded={open}
      className={TITLEBAR_BUTTON}
      onClick={onToggle}
    >
      {open ? <Sidebar /> : <SidebarFloatingLeft />}
    </Button>
  );
}
