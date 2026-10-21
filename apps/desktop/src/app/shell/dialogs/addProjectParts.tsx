import type { ReactNode } from "react";

import { errorText, PROJECT_DIALOG_TITLE } from "@/app/shell/dialogs/fields";
import { ArrowLeft, Folder, X } from "@/components/icons";
import { Button } from "@/components/controls";
import { DialogClose, DialogDescription, DialogTitle } from "@/components/overlays";
import { pickFolder } from "@/ipc/client";
import { shortPath } from "@/lib/format";
import { cn } from "@/lib/utils";
import { closeAddProject } from "@/state/addProject";

/* The parts Add project's views share: header, footer, field label and box, and Save to. */

/** Where clones and new repositories go unless the user changes it (made when missing). */
export const DEFAULT_PARENT = "~/code/";

/** The title, its line and the close button; with `onBack`, a back arrow before them. */
export function Header({ title, line, onBack }: { title: string; line: string; onBack?: (() => void) | undefined }) {
  return (
    <div className={cn("flex gap-3.5 pt-6 pr-5 pb-5", onBack ? "pl-6" : "pl-7")}>
      {onBack && (
        <Button variant="ghost" size="icon-sm" className="text-toolbar-foreground" aria-label="Back" onClick={onBack}>
          <ArrowLeft />
        </Button>
      )}
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <DialogTitle className={PROJECT_DIALOG_TITLE}>{title}</DialogTitle>
        <DialogDescription className="leading-4.25">{line}</DialogDescription>
      </div>
      <DialogClose asChild>
        <Button variant="ghost" size="icon-sm" className="text-toolbar-foreground" aria-label="Close">
          <X />
        </Button>
      </DialogClose>
    </div>
  );
}

/** The bar along the bottom: `start` on the left, Cancel and the main button on the right. */
export function Footer({ start, children }: { start?: ReactNode; children: ReactNode }) {
  return (
    // The top line sits inside the bar's 60px, as drawn: it takes a pixel of the top padding.
    <div className="border-divider flex items-center gap-2 border-t pt-3.75 pr-6 pb-4 pl-5">
      <div className="flex min-w-0 flex-1 items-center gap-1">{start}</div>
      {children}
    </div>
  );
}

export function CancelButton({ label = "Cancel" }: { label?: string }) {
  return (
    <Button variant="ghost" size="sm" onClick={closeAddProject}>
      {label}
    </Button>
  );
}

/** A field's label, with an optional note on the right. */
export function FieldLabel({ htmlFor, label, aside }: { htmlFor?: string; label: string; aside?: string | undefined }) {
  return (
    <div className="flex items-center justify-between gap-4">
      <label htmlFor={htmlFor} className="text-foreground text-xs leading-3.75 font-medium">
        {label}
      </label>
      {aside && <span className="text-muted-foreground text-xs leading-3.75">{aside}</span>}
    </div>
  );
}

/** A 42-high box for a field, in the app's field look (and the Input's when invalid): its icon, then its content. */
export const BOX =
  "bg-muted/60 focus-within:bg-background focus-within:border-ring rounded-control flex h-10.5 items-center gap-2.5 border border-transparent transition-colors [&>svg]:size-icon-sm [&>svg]:shrink-0 [&>svg]:text-muted-foreground has-aria-invalid:border-destructive/60 has-[input:disabled]:opacity-50";

/** A field's own input inside its box. */
export const BOX_INPUT = "text-foreground placeholder:text-muted-foreground min-w-0 flex-1 bg-transparent font-mono text-xs outline-none";

/**
 * Save to: the folder it goes in (`parent`, dimmer) and its own name a step brighter, with
 * Change… to choose another parent folder.
 */
export function SaveTo({
  parent,
  name,
  aside,
  disabled,
  onParent,
  onError,
}: {
  parent: string;
  name: string;
  aside?: string;
  disabled?: boolean;
  onParent: (parent: string) => void;
  onError: (error: string) => void;
}) {
  const change = () => {
    pickFolder(parent)
      .then((picked) => {
        if (picked) onParent(`${shortPath(picked).replace(/[/\\]+$/, "")}/`);
      })
      .catch((cause: unknown) => onError(errorText(cause)));
  };
  return (
    <div className="flex flex-col gap-2">
      <FieldLabel label="Save to" aside={aside} />
      <div className={cn(BOX, "pr-1.5 pl-3")}>
        <Folder />
        <p data-slot="save-to" className="flex min-w-0 flex-1 font-mono text-xs">
          <span className="text-muted-foreground min-w-0 truncate">{parent}</span>
          <span className="text-foreground shrink-0">{name}</span>
        </p>
        <Button variant="outline" size="sm" disabled={disabled} onClick={change}>
          Change…
        </Button>
      </div>
    </div>
  );
}
