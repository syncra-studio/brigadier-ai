import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { ErrorLine } from "@/app/dialogs/fields";
import { DialogPill } from "@/app/dialogs/pills";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { Project } from "@/ipc/generated";
import { shortPath } from "@/lib/format";
import { useRevealed } from "@/lib/splash";
import { cn } from "@/lib/utils";
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
  return (
    <Dialog open={open}>
      <DialogContent
        data-testid="trust-dialog"
        showCloseButton={false}
        className="bg-settings-surface border-settings-border rounded-design-dialog max-w-trust shadow-design-dialog text-settings-text gap-0 border p-0 ring-0"
        onInteractOutside={(event) => event.preventDefault()}
        onEscapeKeyDown={(event) => event.preventDefault()}
        // The card itself takes focus, not an answer: Enter never trusts a folder by accident.
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          (event.currentTarget as HTMLElement).focus();
        }}
      >
        {open && project && folder && (
          <TrustForm key={`${project.id}\n${folder}`} project={project} folder={folder} />
        )}
      </DialogContent>
    </Dialog>
  );
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
        className="text-settings-meta flex w-full min-w-0 cursor-default text-left font-mono outline-none"
        onMouseEnter={() => setOpen(true)}
        onMouseLeave={() => setOpen(false)}
        onFocus={() => setOpen(true)}
        onBlur={() => setOpen(false)}
        onClick={copy}
      >
        <span className={cn("shrink-0 whitespace-pre", open ? "text-settings-text-secondary" : "text-settings-text-tertiary")}>
          {shown.parent}
        </span>
        <span className={cn("min-w-0 break-all", open ? "text-settings-text" : "text-settings-text-secondary")}>
          {shown.name}
        </span>
      </button>
      {open && (
        <div
          role="tooltip"
          data-slot="trust-path-tooltip"
          className="bg-settings-chosen border-settings-ring rounded-design-tooltip shadow-design-tooltip pointer-events-none absolute top-full -right-2 -left-2 z-10 mt-1.75 flex flex-col gap-1.5 border px-3 py-2.5"
        >
          {/* Lines break after a slash: each part with its slash is one piece, split only when wider than a line. */}
          <span className="text-settings-text text-settings-meta leading-[1.5] font-mono wrap-anywhere">
            {folder.split(/(?<=\/)/).map((part, index) => (
              <span key={index} className="inline-block max-w-full">
                {part}
              </span>
            ))}
          </span>
          <span className="text-settings-text-tertiary text-settings-badge">{copied ? "Copied" : "Click to copy"}</span>
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
          <DialogTitle className="text-dialog-title text-settings-text font-medium">Trust this folder?</DialogTitle>
          <FolderPath folder={folder} />
        </DialogHeader>
        <DialogDescription className="text-settings-text-secondary text-settings-small leading-[1.5]">
          Agents can edit and run files here without asking, and the folder's own settings can run code.
        </DialogDescription>
        <ErrorLine error={action.error} />
      </div>
      <div className="flex items-center justify-end gap-2 px-6 pt-6 pb-5">
        <DialogPill look="secondary" disabled={action.busy} onClick={() => answer(false)}>
          Don't trust
        </DialogPill>
        <DialogPill look="primary" disabled={action.busy} onClick={() => answer(true)}>
          Trust folder
        </DialogPill>
      </div>
    </div>
  );
}
