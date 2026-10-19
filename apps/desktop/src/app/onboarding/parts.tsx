import { ArrowLeft, Check } from "@openai/apps-sdk-ui/components/Icon";
import { type ReactNode, type RefObject, useEffect, useRef } from "react";

import { Spinner } from "@/components/glyphs/spinner";
import { Button } from "@/components/ui/button";
import { DialogTitle } from "@/components/ui/dialog";
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

/** A step's heading: its title and the line under it, sized as the step's design has it. */
export function StepHeader({
  title,
  description,
  size = "large",
}: {
  title: string;
  description: ReactNode;
  /** The agents' large heading, or the projects' smaller one with a narrower line. */
  size?: "large" | "small";
}) {
  const small = size === "small";
  return (
    <header
      className={cn(
        "mx-auto flex w-full max-w-onboarding shrink-0 flex-col items-center px-6 text-center",
        small ? "pt-20" : "pt-18",
      )}
    >
      <DialogTitle asChild>
        <h1 className={cn("text-balance", small ? "onboarding-heading-small" : "onboarding-heading")}>{title}</h1>
      </DialogTitle>
      <p
        aria-live="polite"
        className={cn(
          "text-muted-foreground",
          small ? "max-w-onboarding-subline mt-2.5 text-sm leading-normal" : "mt-3.5 text-lg text-balance",
        )}
      >
        {description}
      </p>
    </header>
  );
}

/** The footer's buttons, as the design draws them: 30 high, round-ended, 13px. */
const FOOTER_BUTTON = "h-7.5 rounded-full text-label";

/** What takes a plain Enter itself, so the footer leaves it be. */
const FOCUS_TAKERS = "button, a[href], input, textarea, select, [contenteditable], [role=menuitem], [role=option]";

/**
 * The footer, across the window under a rule: Back (also on ⌘[, Alt ← elsewhere) at the start
 * of the step's column; at its end how many are chosen, then the step's action (also on ⌘⏎,
 * Ctrl ⏎ elsewhere, and on Enter while no control has the focus).
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
      if (event.key !== "Enter" || event.repeat || event.isComposing) return;
      // A plain Enter counts too when no control has the focus: one let go of it.
      const unfocused = event.target instanceof Element && event.target.closest(FOCUS_TAKERS) === null;
      if (!(mac ? event.metaKey : event.ctrlKey) && !unfocused) return;
      event.preventDefault();
      if (!blocked) onClick();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
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
            className={cn(FOOTER_BUTTON, "text-muted-foreground gap-1.75 ps-2.5 pe-3")}
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
            aria-keyshortcuts={mac ? "Meta+Enter" : "Control+Enter"}
            disabled={primary.disabled || primary.busy}
            onClick={primary.onClick}
            className={cn(FOOTER_BUTTON, "ms-3.5 gap-1.75 px-3.5")}
          >
            {primary.busy && <Spinner className="animate-spin" />}
            {primary.label}
          </Button>
        )}
      </div>
    </footer>
  );
}

/** A tick box's look (Radix's in dialogs/fields.tsx), for a control that is a whole card; ticked, it
 * takes the links' blue, as the design has it. */
export function TickMark({ checked }: { checked: boolean }) {
  return (
    <span
      aria-hidden
      className={cn(
        "border-input rounded-xs flex size-icon-md shrink-0 items-center justify-center border",
        checked && "bg-link border-link text-foreground",
      )}
    >
      {checked && <Check className="size-icon-xs" />}
    </span>
  );
}
