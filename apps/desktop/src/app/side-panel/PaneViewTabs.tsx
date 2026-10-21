import { shortcutKeys } from "@/app/shortcutCatalog";
import { SourceCountBadge, TooltipIconButton } from "@/components/controls";
import { FilesGlyph, SearchGlyph, SourceControlGlyph } from "@/components/icons";
import { TitlebarTips } from "@/components/shared";
import type { WorkspacePaneTab } from "@/state/workspacePane";

/** The right sidebar's views, in its icon row's order. */
const VIEWS = [
  { id: "files", title: "Files", shortcut: "show-files", Icon: FilesGlyph },
  { id: "search", title: "Search", shortcut: "show-search", Icon: SearchGlyph },
  { id: "source", title: "Source", shortcut: "show-source", Icon: SourceControlGlyph },
] as const satisfies readonly { id: WorkspacePaneTab; title: string; shortcut: string; Icon: unknown }[];

/** The right sidebar's row of view icons: Files, Search and Source (with its count), as tabs. */
export function PaneViewTabs({
  active,
  changed,
  mac,
  onSelect,
}: {
  active: WorkspacePaneTab;
  changed: number;
  mac: boolean;
  onSelect: (tab: WorkspacePaneTab) => void;
}) {
  return (
    <div role="tablist" aria-label="Right sidebar" className="h-page-toolbar bg-background flex shrink-0 items-center justify-center gap-1 px-2">
      <TitlebarTips>
        {VIEWS.map(({ id, title, shortcut, Icon }, index) => (
          <TooltipIconButton look="toolbar" key={id} role="tab" id={`workspace-pane-${id}`} aria-controls="workspace-pane-content"
            aria-selected={active === id} tabIndex={active === id ? 0 : -1}
            className="aria-selected:bg-toolbar-pressed aria-selected:text-foreground relative"
            tooltip={title} shortcut={shortcutKeys(shortcut, mac)}
            onClick={() => onSelect(id)}
            onKeyDown={(event) => {
              const next = event.key === "ArrowRight" ? (index + 1) % VIEWS.length
                : event.key === "ArrowLeft" ? (index + VIEWS.length - 1) % VIEWS.length
                  : event.key === "Home" ? 0 : event.key === "End" ? VIEWS.length - 1 : null;
              if (next === null) return;
              event.preventDefault();
              const view = VIEWS[next]!;
              onSelect(view.id);
              document.getElementById(`workspace-pane-${view.id}`)?.focus();
            }}>
            <Icon />
            {id === "source" && changed > 0 && <SourceCountBadge count={changed} />}
          </TooltipIconButton>
        ))}
      </TitlebarTips>
    </div>
  );
}
