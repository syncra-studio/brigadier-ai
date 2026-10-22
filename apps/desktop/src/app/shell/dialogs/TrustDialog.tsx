import { type SyntheticEvent, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { typeInto } from "@/app/conversation/composerDraft";
import { COMPOSER_EDITABLE } from "@/app/conversation/composerTarget";
import { useAction } from "@/app/conversation/useAction";
import { PROJECT_DIALOG_CARD, PROJECT_DIALOG_TITLE } from "@/app/shell/dialogs/fields";
import { Button, ErrorLine } from "@/components/controls";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/overlays";
import type { Project } from "@/ipc/generated";
import { shortPath } from "@/lib/format";
import { useRevealed } from "@/lib/splash";
import { cn } from "@/lib/utils";
import { NEW_CHAT_SCOPE } from "@/state/drafts";
import { useOnboarding } from "@/state/onboarding";
import { useApp } from "@/state/store";
import { projectFolder, setFolderTrust, toAsk } from "@/state/trust";

/**
 * "Trust this folder?", for the project the user is in when its folder has no answer: a new
 * project, one from before the question, or a folder changed in its settings, each asked once
 * when first used. One of the two answers must be picked; both are safe.
 */
export function TrustDialog() {
  const catalogLoaded = useApp((s) => s.catalogLoaded);
  const onboarding = useApp((s) => !s.settings.onboarded);
  const smoke = useApp((s) => s.info?.smoke ?? false);
  const reopened = useOnboarding((s) => s.reopened);
  const project = useApp(toAsk);
  // Not under the startup screen, nor during the first-run setup.
  const revealed = useRevealed();
  const folder = project && projectFolder(project);
  const open = revealed && catalogLoaded && !smoke && !onboarding && !reopened && folder !== null;
  // The composer behind it: the conversation's, or the new chat's.
  const scope = useApp((s) => (s.selection.type === "conversation" ? s.selection.id : NEW_CHAT_SCOPE));
  // Whether text typed while it asked went into the composer, which then takes the keyboard back.
  const typed = useRef(false);
  const type = (edit: ((text: string) => string) | null, event: SyntheticEvent) => {
    if (!edit || event.target !== event.currentTarget || !typeInto(scope, edit)) return;
    event.preventDefault();
    typed.current = true;
  };
  return (
    <Dialog open={open}>
      <DialogContent
        data-testid="trust-dialog"
        showCloseButton={false}
        className={cn(PROJECT_DIALOG_CARD, "max-w-trust gap-0 p-0")}
        onInteractOutside={(event) => event.preventDefault()}
        onEscapeKeyDown={(event) => event.preventDefault()}
        // The card itself takes focus, not an answer: Enter never trusts a folder by accident.
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          (event.currentTarget as HTMLElement).focus();
        }}
        // What's typed or pasted meanwhile goes into the message box behind it, not nowhere (#477).
        onKeyDown={(event) => type(typedEdit(event), event)}
        onPaste={(event) => {
          const pasted = event.clipboardData.getData("text/plain");
          type(pasted ? (text) => text + pasted : null, event);
        }}
        onCloseAutoFocus={(event) => {
          if (!typed.current) return;
          typed.current = false;
          event.preventDefault();
          // At the text's end, to go on typing.
          const field = document.querySelector<HTMLElement>(COMPOSER_EDITABLE);
          field?.focus();
          if (field) getSelection()?.selectAllChildren(field);
          getSelection()?.collapseToEnd();
        }}
      >
        {open && project && folder && (
          <TrustForm key={`${project.id}\n${folder}`} project={project} folder={folder} />
        )}
      </DialogContent>
    </Dialog>
  );
}

/**
 * What a key typed on the dialog's card does to the composer's text: a character goes on its
 * end and Backspace takes the last one off; `null` for a shortcut or any other key.
 */
function typedEdit(event: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "altKey">): ((text: string) => string) | null {
  // Ctrl with Alt is AltGr on Windows, which types characters.
  if (event.metaKey || (event.ctrlKey && !event.altKey)) return null;
  if (event.key === "Backspace") return (text) => [...text].slice(0, -1).join("");
  return [...event.key].length === 1 ? (text) => text + event.key : null;
}

/** The folder's path split at its last part: where it lives, and the folder's own name. */
export function splitFolder(path: string): { parent: string; name: string } {
  const trimmed = path.length > 1 ? path.replace(/\/+$/, "") : path;
  const cut = trimmed.lastIndexOf("/") + 1;
  return { parent: trimmed.slice(0, cut), name: trimmed.slice(cut) };
}

/**
 * The folder's path as the dialog shows it in a line `fits` says it fits in: whole when it
 * fits; else the home folder becomes `~`, then the path is cut from the middle, never the ends.
 * Its first folder and the folder's own name stay, the parts nearest the name kept longest.
 */
export function fitFolder(path: string, fits: (text: string) => boolean): { parent: string; name: string } {
  const whole = splitFolder(path);
  if (fits(whole.parent + whole.name)) return whole;
  const { parent, name } = splitFolder(shortPath(path));
  const parts = parent.split("/").filter(Boolean);
  // "~/" or "/": what the path starts from.
  const root = parent.startsWith("~") ? "~/" : "/";
  if (root === "~/") parts.shift();
  const shortened = [root + parts.join("/") + (parts.length > 0 ? "/" : "")];
  for (let tail = parts.length - 2; tail >= 0; tail--) {
    shortened.push([root + parts[0], "…", ...parts.slice(parts.length - tail)].join("/") + "/");
  }
  shortened.push(`${root}…/`);
  const fitting = shortened.find((candidate) => fits(candidate + name));
  return { parent: fitting ?? shortened[shortened.length - 1]!, name };
}

/** How long "Copied" stays in the path's tooltip. */
const COPIED_MS = 1500;

/**
 * The folder's path: at rest dimmer with its name a step brighter, shortened to its line;
 * hovered (or focused) a step brighter, with the whole path above "Click to copy".
 */
function FolderPath({ folder }: { folder: string }) {
  const line = useRef<HTMLButtonElement>(null);
  // The line's width and font, once laid out.
  const [measured, setLine] = useState<{ width: number; font: string } | null>(null);
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  useLayoutEffect(() => {
    const element = line.current;
    if (!element) return;
    const measure = () => setLine({ width: element.clientWidth, font: getComputedStyle(element).font });
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    measure();
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), COPIED_MS);
    return () => clearTimeout(timer);
  }, [copied]);
  const shown = useMemo(() => {
    const context = measured ? document.createElement("canvas").getContext("2d") : null;
    if (!measured || !context) return splitFolder(folder);
    context.font = measured.font;
    return fitFolder(folder, (text) => context.measureText(text).width <= measured.width);
  }, [folder, measured]);
  const copy = () => {
    void navigator.clipboard.writeText(folder).then(
      () => setCopied(true),
      (error: unknown) => console.error("copying the folder's path failed", error),
    );
  };
  return (
    <div className="relative">
      <button
        ref={line}
        type="button"
        data-slot="trust-path"
        aria-label={`${folder}, click to copy`}
        className="flex w-full min-w-0 cursor-default text-left font-mono text-xs outline-none"
        onMouseEnter={() => setOpen(true)}
        onMouseLeave={() => setOpen(false)}
        onFocus={() => setOpen(true)}
        onBlur={() => setOpen(false)}
        onClick={copy}
      >
        <span className={cn("shrink-0 whitespace-pre", open ? "text-foreground/65" : "text-muted-foreground")}>
          {shown.parent}
        </span>
        <span className={cn("min-w-0 break-all", open ? "text-foreground" : "text-foreground/65")}>
          {shown.name}
        </span>
      </button>
      {open && (
        <div
          role="tooltip"
          data-slot="trust-path-tooltip"
          className="bg-popover text-popover-foreground border-border rounded-tooltip shadow-tooltip pointer-events-none absolute top-full -right-2 -left-2 z-10 mt-6.25 flex flex-col gap-1.5 border px-3 py-2.5"
        >
          {/* Lines break after a slash: each part with its slash is one piece, split only when wider than a line. */}
          <span className="font-mono text-xs leading-normal wrap-anywhere">
            {folder.split(/(?<=\/)/).map((part, index) => (
              <span key={index} className="inline-block max-w-full">
                {part}
              </span>
            ))}
          </span>
          <span className="text-muted-foreground text-2xs leading-3.25">{copied ? "Copied" : "Click to copy"}</span>
        </div>
      )}
    </div>
  );
}

/** The question for one project's folder, and its two answers. */
export function TrustForm({ project, folder }: { project: Project; folder: string }) {
  const action = useAction();
  const answer = (trusted: boolean) => action.run(() => setFolderTrust(project.id, folder, trusted));
  return (
    <div className="flex flex-col">
      <div className="flex flex-col gap-3 px-6 pt-6">
        <DialogHeader className="gap-1.5">
          <DialogTitle className={PROJECT_DIALOG_TITLE}>Trust this folder?</DialogTitle>
          <FolderPath folder={folder} />
        </DialogHeader>
        <DialogDescription>
          Agents can edit and run files here without asking, and the folder's own settings can run code.
        </DialogDescription>
        <ErrorLine error={action.error} className="leading-4.25" />
      </div>
      <div className="flex items-center justify-end gap-2 px-6 pt-6 pb-5">
        <Button variant="outline" size="sm" disabled={action.busy} onClick={() => answer(false)}>
          Don't trust
        </Button>
        <Button size="sm" disabled={action.busy} onClick={() => answer(true)}>
          Trust folder
        </Button>
      </div>
    </div>
  );
}
