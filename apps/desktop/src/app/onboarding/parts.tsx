import { ArrowLeft, ArrowRight } from "@openai/apps-sdk-ui/components/Icon";
import { type ReactNode, useEffect } from "react";

import { Spinner } from "@/components/glyphs/spinner";
import { Button } from "@/components/ui/button";
import { DialogTitle } from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import { useApp } from "@/state/store";

/*
 * What the setup's steps after the welcome share: a centred heading with a line under it, a
 * body, and a footer with Back and the step's one action.
 */

/** A step's heading: an optional small line over it, then its title and description. */
export function StepHeader({
  eyebrow,
  title,
  description,
}: {
  eyebrow?: string | null;
  title: string;
  description: ReactNode;
}) {
  return (
    <header className="mx-auto flex max-w-onboarding shrink-0 flex-col items-center px-6 text-center">
      {eyebrow && <p className="text-muted-foreground mb-3 text-sm">{eyebrow}</p>}
      <DialogTitle asChild>
        <h1 className="onboarding-heading text-balance">{title}</h1>
      </DialogTitle>
      <p aria-live="polite" className="text-muted-foreground mt-3.5 text-lg text-balance">
        {description}
      </p>
    </header>
  );
}

/**
 * The footer: Back, then the step's action (also on ⌘⏎, Ctrl ⏎ elsewhere), and a quiet line
 * under them.
 */
export function StepFooter({
  onBack,
  backLabel = "Back",
  primary,
  note,
}: {
  onBack?: () => void;
  backLabel?: string;
  primary?: { label: string; onClick: () => void; disabled?: boolean; busy?: boolean };
  note?: ReactNode;
}) {
  const mac = useApp((s) => s.info?.platform === "macos");
  const onClick = primary?.onClick;
  const blocked = !primary || primary.disabled === true || primary.busy === true;

  useEffect(() => {
    if (!onClick) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Enter" || event.repeat || !(mac ? event.metaKey : event.ctrlKey)) return;
      event.preventDefault();
      if (!blocked) onClick();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [mac, onClick, blocked]);

  return (
    <footer className="flex shrink-0 flex-col items-center gap-3 px-8 pt-4 pb-8">
      <div className="flex items-center gap-2.5">
        {onBack && (
          <Button size="lg" variant="outline" disabled={primary?.busy} onClick={onBack} className="gap-2">
            <ArrowLeft />
            {backLabel}
          </Button>
        )}
        {primary && (
          <Button
            size="lg"
            autoFocus
            aria-keyshortcuts={mac ? "Meta+Enter" : "Control+Enter"}
            disabled={primary.disabled || primary.busy}
            onClick={primary.onClick}
            className="min-w-32 gap-2"
          >
            {primary.busy && <Spinner className="animate-spin" />}
            {primary.label}
            <ArrowRight />
          </Button>
        )}
      </div>
      {note && <div className="text-muted-foreground min-h-4 text-xs">{note}</div>}
    </footer>
  );
}

/** A small uppercase heading over a group, with how many it holds. */
export function SectionLabel({
  label,
  count,
  dot = false,
  className,
}: {
  label: string;
  count?: number | undefined;
  dot?: boolean;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "text-muted-foreground tracking-eyebrow flex shrink-0 items-center gap-2 text-2xs font-medium uppercase",
        className,
      )}
    >
      {dot && <span aria-hidden="true" className="bg-success size-1.5 shrink-0 rounded-full" />}
      <span>{label}</span>
      {count !== undefined && (
        <>
          <span aria-hidden="true" className="text-muted-foreground/60">
            ·
          </span>
          <span className="tabular-nums">{count}</span>
        </>
      )}
    </div>
  );
}
