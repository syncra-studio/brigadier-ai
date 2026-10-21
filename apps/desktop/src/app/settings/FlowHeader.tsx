import { BrowserIcon } from "@/app/settings/BrowserIcon";
import { DialogDescription, DialogHeader, DialogTitle } from "@/components/overlays";
import type { ImportSource } from "@/ipc/generated";

/** The heading every step of the dialog shares: the browser's icon and what is going on. */
export function FlowHeader({ source, title, description }: { source: ImportSource; title: string; description: string }) {
  return (
    <DialogHeader>
      <DialogTitle className="flex items-center gap-2.5">
        <BrowserIcon source={source} className="size-6" />
        {title}
      </DialogTitle>
      <DialogDescription>{description}</DialogDescription>
    </DialogHeader>
  );
}
