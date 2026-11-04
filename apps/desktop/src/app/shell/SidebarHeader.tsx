import { Button, Kbd, ToggleGroup, ToggleGroupItem } from "@/components/controls";
import { BrigadierGlyph, Folders, MagnifyingGlassSearch, Mail } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { NavHeader } from "@/components/shared";
import { errorText } from "@/app/shell/dialogs/fields";
import { openSearch } from "@/app/shell/SearchDialog";
import type { SidebarMode } from "@/ipc/generated";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/** The sidebar panel's header on Home: the Brigadier mark and name, the Inbox | Projects switch, and Search. */
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

const MODES: { mode: SidebarMode; label: string; Icon: typeof Mail }[] = [
  { mode: "inbox", label: "Inbox", Icon: Mail },
  { mode: "projects", label: "Projects", Icon: Folders },
];

/** Inbox | Projects (D8): two icons, Inbox first; the choice is a daemon setting every window shares. */
function ModeSwitch() {
  const mode = useApp((s) => (s.settings.sidebarMode === "projects" ? "projects" : "inbox"));
  return (
    <ToggleGroup
      type="single"
      size="sm"
      spacing="tight"
      aria-label="Sidebar view"
      value={mode}
      onValueChange={(value) => {
        if (value === "inbox" || value === "projects") {
          void setSetting("sidebarMode", value).catch((error: unknown) => toast(errorText(error), { tone: "error" }));
        }
      }}
    >
      {MODES.map(({ mode: value, label, Icon }) => (
        <Tooltip key={value}>
          <TooltipTrigger asChild>
            <ToggleGroupItem value={value} aria-label={`${label} view`} className="text-muted-foreground data-[state=on]:text-foreground">
              <Icon />
            </ToggleGroupItem>
          </TooltipTrigger>
          <TooltipContent side="bottom">{label}</TooltipContent>
        </Tooltip>
      ))}
    </ToggleGroup>
  );
}
