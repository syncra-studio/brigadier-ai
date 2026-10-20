import { type ReactNode, type RefObject, useEffect, useRef } from "react";

import { ArrowLeft, Spinner } from "@/components/icons";
import { Button } from "@/components/controls/button";
import { Kbd } from "@/components/controls/kbd";
import { DialogTitle } from "@/components/overlays/dialog";
import { cn } from "@/lib/utils";
import { useApp } from "@/state/store";

/*
 * What the setup's steps after the name share: a centred heading with a line under it that stays
 * put, a body that scrolls on its own, and a footer with Back, a count and the step's one action.
 */

/**
 * Focuses what it's given a frame after it mounts, where autoFocus would focus it at once: the
 * setup's focus trap only keeps a focus it saw arrive, and the chat under the setup can take the
 * focus as it mounts on launch.
 */
export function useFocusOnMount<T extends HTMLElement>(): RefObject<T | null> {
  const ref = useRef<T>(null);
  useEffect(() => {
    const frame = requestAnimationFrame(() => ref.current?.focus());
    return () => cancelAnimationFrame(frame);
  }, []);
  return ref;
}

/** A step's heading: its title and the line under it, alike on the agents and the projects. */
export function StepHeader({ title, description }: { title: string; description: ReactNode }) {
  return (
    <header className="mx-auto flex w-full max-w-onboarding shrink-0 flex-col items-center px-6 pt-18 text-center">
      <DialogTitle asChild>
        <h1 className="onboarding-heading text-balance">{title}</h1>
      </DialogTitle>
      <p aria-live="polite" className="text-muted-foreground mt-3.5 text-lg text-balance">
        {description}
      </p>
    </header>
  );
}

/** The setup's buttons, as the design draws them: 30 high, round-ended, 13px. */
export const PILL_BUTTON = "h-7.5 rounded-full text-label";

/** Each step's main button, as the design draws the name's Continue: 36 high, 16 before the words,
 * 8 after its key, which sits in a chip. */
export const PRIMARY_BUTTON = "text-label h-9 gap-2.5 rounded-full ps-4 pe-2";
export const PRIMARY_KEY = "in-data-[slot=button]:text-2xs px-1.5";

/** Its quieter ones (Sign in, Install, Add folder…) on `variant="outline"`: a raised fill in a
 * hairline, 12 to a side. */
export const RAISED_BUTTON = cn(PILL_BUTTON, "border-input border px-3 gap-1.5");

/** Where Enter belongs to what's open (a menu, a list) or a field that sends, not to the step. */
const OWN_ENTER = "[role=menu], [role=listbox], [role=combobox], [data-own-enter]";

/**
 * The footer, across the window under a rule: Back (also on ⌘[, Alt ← elsewhere) at the start
 * of the step's column; at its end how many are chosen, then the step's action. Enter is the
 * action's alone on these steps (⌘⏎, Ctrl ⏎ elsewhere, too): wherever the focus is, it goes on
 * and never presses the focused card or button, which Space still does.
 */
export function StepFooter({
  onBack,
  count,
  primary,
}: {
  onBack?: () => void;
  /** "2 of 5 connected". */
  count?: ReactNode;
  primary?: { label: string; onClick: () => void; disabled?: boolean; busy?: boolean };
}) {
  const mac = useApp((s) => s.info?.platform === "macos");
  const onClick = primary?.onClick;
  const blocked = !primary || primary.disabled === true || primary.busy === true;
  const primaryRef = useFocusOnMount<HTMLButtonElement>();

  const backBlocked = primary?.busy === true;

  useEffect(() => {
    if (!onClick) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Enter" || event.isComposing) return;
      if (event.target instanceof Element && event.target.closest(OWN_ENTER)) return;
      // Ahead of the focused control, so it isn't pressed.
      event.preventDefault();
      event.stopPropagation();
      const others = event.altKey || event.shiftKey || (mac ? event.ctrlKey : event.metaKey);
      // Plain, or with the platform's own modifier; never a held one.
      if (!event.repeat && !others && !blocked) onClick();
    };
    window.addEventListener("keydown", onKeyDown, { capture: true });
    return () => window.removeEventListener("keydown", onKeyDown, { capture: true });
  }, [mac, onClick, blocked]);

  useEffect(() => {
    if (!onBack) return;
    const onKeyDown = (event: KeyboardEvent) => {
      const back = mac
        ? event.metaKey && event.key === "["
        : event.altKey && event.key === "ArrowLeft";
      if (!back || event.repeat) return;
      event.preventDefault();
      if (!backBlocked) onBack();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [mac, onBack, backBlocked]);

  return (
    <footer className="border-divider shrink-0 border-t px-6 py-4">
      <div className="max-w-onboarding mx-auto flex w-full items-center gap-2">
        {onBack && (
          <Button
            size="sm"
            variant="ghost"
            aria-keyshortcuts={mac ? "Meta+BracketLeft" : "Alt+ArrowLeft"}
            disabled={backBlocked}
            onClick={onBack}
            className={cn(PILL_BUTTON, "text-muted-foreground gap-1.75 ps-2.5 pe-3")}
          >
            <ArrowLeft className="size-3.25" />
            Back
          </Button>
        )}
        <span className="flex-1" />
        {count !== undefined && (
          <span role="status" className="text-muted-foreground/70 truncate text-xs tabular-nums">
            {count}
          </span>
        )}
        {primary && (
          <Button
            ref={primaryRef}
            size="sm"
            aria-keyshortcuts={mac ? "Enter Meta+Enter" : "Enter Control+Enter"}
            disabled={primary.disabled || primary.busy}
            onClick={primary.onClick}
            className={cn(PRIMARY_BUTTON, "ms-3.5")}
          >
            {primary.busy && <Spinner className="animate-spin" />}
            {primary.label}
            <Kbd className={PRIMARY_KEY}>↩</Kbd>
          </Button>
        )}
      </div>
    </footer>
  );
}
