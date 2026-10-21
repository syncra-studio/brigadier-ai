import type { ReactNode } from "react";

import { PROJECT_DIALOG_TITLE } from "@/app/shell/dialogs/fields";
import { X } from "@/components/icons";
import { Button } from "@/components/controls";
import { DialogClose, DialogDescription, DialogTitle } from "@/components/overlays";
import { cn } from "@/lib/utils";

/* The parts Remove project's steps share: the head and the actions row. */

/**
 * The title with the close button, and the line under it across the dialog; `beside` (while
 * looking, and once removed) keeps the line under the title, beside the close button.
 */
export function Head({ title, beside, children }: { title: string; beside?: boolean; children: ReactNode }) {
  const close = (
    <DialogClose asChild>
      <Button type="button" variant="ghost" size="icon-sm" className="text-muted-foreground" aria-label="Close">
        <X />
      </Button>
    </DialogClose>
  );
  if (beside) {
    return (
      <div className="flex items-start gap-3">
        <div className="flex min-w-0 flex-1 flex-col gap-1.5">
          <DialogTitle className={PROJECT_DIALOG_TITLE}>{title}</DialogTitle>
          <DialogDescription>{children}</DialogDescription>
        </div>
        {close}
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-3">
        <DialogTitle className={cn(PROJECT_DIALOG_TITLE, "min-w-0 flex-1")}>{title}</DialogTitle>
        {close}
      </div>
      <DialogDescription>{children}</DialogDescription>
    </div>
  );
}

export function Actions({ children }: { children: ReactNode }) {
  return <div className="flex justify-end gap-2">{children}</div>;
}
