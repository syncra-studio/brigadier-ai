import { Fragment, useState } from "react";

import { CheckboxMark } from "@/components/controls";
import { Check, ChevronDown, Folder, Folders } from "@/components/icons";
import { Popover, PopoverContent, PopoverTrigger, Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import type { PluginItemView, PluginsView } from "@/ipc/generated";
import { shortPath } from "@/lib/format";
import { cn } from "@/lib/utils";
import { placesLabel, type Places } from "./model";

const ROW = "rounded-control hover:bg-foreground/5 flex w-full shrink-0 items-center gap-2.5 px-2.5 py-1.5 text-start";

/**
 * A row's project tag as the place picker: Global at the top, then the projects to check, several
 * at a time. Each pick applies at once; the last project checked stays (its switch turns it off).
 * Only a Library item has places; any other item is added to every agent first.
 */
export function PlacesPicker({ view, item, places, busy, onPlaces, onEveryAgent }: {
  view: PluginsView;
  item: PluginItemView;
  places: Places;
  busy: boolean;
  /** Global (null), or exactly these projects, by id. */
  onPlaces: (projectIds: string[] | null) => void;
  onEveryAgent: () => void;
}) {
  const [open, setOpen] = useState(false);
  const global = places === "global";
  const ids = global ? [] : places.flatMap((path) => view.projects.find((project) => project.path === path)?.id ?? []);
  const Icon = global ? Folders : Folder;
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          data-slot="places-picker"
          className={cn("bg-foreground/5 text-foreground/65 hover:bg-foreground/8 rounded-capsule flex h-pill shrink-0 items-center gap-1 ps-2 pe-1.5 text-xs", open && "bg-foreground/10 text-foreground")}
        >
          <Icon aria-hidden className="size-3" />
          {placesLabel(view, places)}
          <ChevronDown aria-hidden className="size-3 opacity-70" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="end" aria-label={`Where ${item.name} is on`} data-slot="places-picker-menu" className="flex max-h-(--radix-popover-content-available-height) w-75 flex-col gap-0.5 p-1.5">
        {!item.key.startsWith("lib:") ? (
          <button
            type="button"
            disabled={busy}
            className={ROW}
            onClick={() => {
              setOpen(false);
              onEveryAgent();
            }}
          >
            <span className="min-w-0 flex-1 truncate text-label">Add to every agent first</span>
          </button>
        ) : (
          <>
            <button type="button" aria-current={global || undefined} disabled={busy} onClick={() => !global && onPlaces(null)} className={cn(ROW, global && "bg-foreground/5")}>
              <Folders aria-hidden className="text-foreground/65 size-icon-sm shrink-0" />
              <span className="flex min-w-0 flex-1 flex-col">
                <span className="truncate text-label">Global</span>
                <span className="text-foreground/50 truncate text-xs">On in every project</span>
              </span>
              {global && <Check aria-hidden className="size-icon-sm shrink-0" />}
            </button>
            <div className="border-divider mx-1 my-1 border-t" />
            <div className="flex min-h-0 flex-col gap-0.5 overflow-y-auto">
              {view.projects.map((project) => {
                const on = ids.includes(project.id);
                const last = on && ids.length === 1;
                const row = (
                  <button
                    type="button"
                    role="checkbox"
                    aria-checked={on}
                    aria-disabled={last || undefined}
                    disabled={busy}
                    onClick={() => {
                      if (last) return;
                      onPlaces(global ? [project.id] : on ? ids.filter((id) => id !== project.id) : [...ids, project.id]);
                    }}
                    className={cn(ROW, last && "cursor-not-allowed")}
                  >
                    <CheckboxMark checked={on} className={cn(last && "opacity-50")} />
                    <span className="flex min-w-0 flex-1 flex-col">
                      <span className="truncate text-label">{project.name}</span>
                      <span className="text-foreground/50 truncate text-xs">{shortPath(project.path)}</span>
                    </span>
                  </button>
                );
                return last ? (
                  <Tooltip key={project.id}>
                    <TooltipTrigger asChild>{row}</TooltipTrigger>
                    <TooltipContent side="left">Turn it off with its switch</TooltipContent>
                  </Tooltip>
                ) : (
                  <Fragment key={project.id}>{row}</Fragment>
                );
              })}
            </div>
          </>
        )}
      </PopoverContent>
    </Popover>
  );
}
