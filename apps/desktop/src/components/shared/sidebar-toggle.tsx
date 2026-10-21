import { TITLEBAR_BUTTON } from "@/components/shared/titlebar-button";
import { Button } from "@/components/controls/button";
import { Sidebar, SidebarFloatingLeft } from "@/components/icons";

/**
 * A sidebar toggle's icon: a window with its sidebar shown, or floating while it is hidden. The
 * right sidebar's is the left one's, mirrored.
 */
export function SidebarGlyph({ open, side = "left" }: { open: boolean; side?: "left" | "right" }) {
  const mirror = side === "right" ? "-scale-x-100" : undefined;
  return open ? <Sidebar className={mirror} /> : <SidebarFloatingLeft className={mirror} />;
}

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
      <SidebarGlyph open={open} />
    </Button>
  );
}
