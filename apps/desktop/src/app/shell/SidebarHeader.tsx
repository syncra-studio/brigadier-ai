import { Button, Kbd, Segmented } from "@/components/controls";
import { BrigadierGlyph, Folders, MagnifyingGlassSearch, Mail } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { NavHeader } from "@/components/shared";
import { errorText } from "@/app/shell/dialogs/fields";
import { openSearch } from "@/app/shell/SearchDialog";
import type { SidebarMode } from "@/ipc/generated";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/** The sidebar panel's header on Home: the Brigadier mark and name, Search, and under them the Inbox | Projects tabs. */
export function SidebarHeader({ searchShortcut }: { searchShortcut: string }) {
  return (
    <>
      <NavHeader
        title={
          <span className="-ms-0.5 flex items-center gap-2">
            <BrigadierGlyph smallCut={20} className="size-5 shrink-0" />
            Brigadier
          </span>
        }
        actions={
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-md"
                aria-label="Search"
                className="text-muted-foreground hover:text-foreground"
                onClick={() => openSearch()}
              >
                <MagnifyingGlassSearch />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom">
              Search
              <Kbd>{searchShortcut}</Kbd>
            </TooltipContent>
          </Tooltip>
        }
      />
      <ModeTabs />
    </>
  );
}

const tab = (Icon: typeof Mail, name: string) => (
  <span className="flex items-center justify-center gap-1.5">
    <Icon aria-hidden className="size-icon-sm shrink-0" />
    {name}
  </span>
);

const MODES = [
  { value: "inbox", label: tab(Mail, "Inbox") },
  { value: "projects", label: tab(Folders, "Projects") },
] as const;

/** Inbox | Projects (D8): which view the sidebar shows; the choice is a daemon setting every window shares. */
function ModeTabs() {
  const mode = useApp((s) => (s.settings.sidebarMode === "projects" ? "projects" : "inbox"));
  return (
    <div className="shrink-0 px-2 pb-2">
      <Segmented<SidebarMode>
        fill
        label="Sidebar view"
        value={mode}
        options={MODES}
        // The two tabs take equal widths, whatever their words.
        className="*:basis-0"
        onChange={(next) => void setSetting("sidebarMode", next).catch((error: unknown) => toast(errorText(error), { tone: "error" }))}
      />
    </div>
  );
}
