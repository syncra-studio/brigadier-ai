import { Button, Kbd } from "@/components/controls";
import { BrigadierGlyph, MagnifyingGlassSearch } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { NavHeader } from "@/components/shared";
import { openSearch } from "@/app/shell/SearchDialog";

/** The sidebar panel's header on Home: the Brigadier mark and name, and Search. */
export function SidebarHeader({ searchShortcut }: { searchShortcut: string }) {
  return (
    <NavHeader
      title={
        <span className="-ms-0.5 flex items-center gap-2">
          <BrigadierGlyph className="size-5 shrink-0" />
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
  );
}
