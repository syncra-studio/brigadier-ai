import { DropdownMenuContent, DropdownMenuItem } from "@/components/overlays";
import { Check, ImageSquare } from "@/components/icons";
import type { ArtifactRef } from "@/ipc/generated";
import { cn } from "@/lib/utils";

const nameOf = (ref: ArtifactRef) => ref.fileName ?? ref.title;

/** The mockup's file pill menu: every image to show, a tick at the one shown. */
export function ImagesMenu({ items, current, onPick }: {
  items: readonly { key: string; ref: ArtifactRef }[]; current: { key: string } | null; onPick: (key: string) => void;
}) {
  return (
    <DropdownMenuContent align="start">
      {items.map((item) => (
        <DropdownMenuItem key={item.key} onSelect={() => onPick(item.key)}>
          <ImageSquare className="size-icon-sm" />
          <span className="flex-1 truncate">{nameOf(item.ref)}</span>
          <Check className={cn("size-icon-sm", item.key !== current?.key && "invisible")} />
        </DropdownMenuItem>
      ))}
    </DropdownMenuContent>
  );
}
