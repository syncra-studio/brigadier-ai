import { Button, Kbd } from "@/components/controls";
import { BrigadierGlyph, Folders, MagnifyingGlassSearch, Mail } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { NavHeader } from "@/components/shared";
import { errorText } from "@/app/shell/dialogs/fields";
import { openSearch } from "@/app/shell/SearchDialog";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/** The sidebar panel's header on Home: the Brigadier mark and name, the Inbox | Projects toggle, and Search. */
export function SidebarHeader({ searchShortcut }: { searchShortcut: string }) {
  return (
    <NavHeader
      title={
        <span className="-ms-0.5 flex items-center gap-2">
          <BrigadierGlyph smallCut={20} className="size-5 shrink-0" />
          Brigadier
        </span>
      }
      actions={
        <>
          <ModeSwitch />
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
        </>
      }
    />
  );
}

/** Inbox | Projects (D8): one button showing the view you're in; the choice is a daemon setting every window shares. */
function ModeSwitch() {
  const mode = useApp((s) => (s.settings.sidebarMode === "projects" ? "projects" : "inbox"));
  const Icon = mode === "inbox" ? Mail : Folders;
  const other = mode === "inbox" ? "projects" : "inbox";
  const label = mode === "inbox" ? "Switch to Projects" : "Switch to Inbox";
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-md"
          aria-label={label}
          className="text-muted-foreground hover:text-foreground"
          onClick={() => void setSetting("sidebarMode", other).catch((error: unknown) => toast(errorText(error), { tone: "error" }))}
        >
          <Icon />
        </Button>
      </TooltipTrigger>
      <TooltipContent side="bottom">{label}</TooltipContent>
    </Tooltip>
  );
}
