import type { ReactNode } from "react";

import { field } from "@/components/conversation";
import { cn } from "@/lib/utils";

/**
 * A card the thread puts to the user (a session to start, feedback to send, a setting to
 * change): its icon in a small square, a question and a grey line under it, what it is about,
 * and its buttons. Without a title it holds only its children (a line once it is answered).
 */
export function OfferCard({
  label,
  kind,
  icon,
  title,
  line,
  actions,
  children,
}: {
  label: string;
  /** The thread step it shows, for `data-kind`. */
  kind: string;
  icon?: ReactNode;
  title?: ReactNode;
  line?: ReactNode;
  actions?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <section
      aria-label={label}
      data-slot="orchestrator-step"
      data-kind={kind}
      className="bg-rail border-foreground/10 rounded-question my-1 flex flex-col gap-3 border p-4 text-sm"
    >
      {title && (
        <div className="flex min-w-0 items-center gap-2.5">
          <span className="bg-foreground/5 text-muted-foreground rounded-control size-icon-button-md flex shrink-0 items-center justify-center [&_svg]:size-icon-sm">
            {icon}
          </span>
          <div className="min-w-0 flex-1">
            <p className="font-medium">{title}</p>
            {line && <p className="text-muted-foreground text-xs wrap-break-word">{line}</p>}
          </div>
        </div>
      )}
      {children}
      {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
    </section>
  );
}

/** What an offer is about: a title over up to four lines of text, in a soft box. */
export function OfferBody({ title, text }: { title: string; text: string }) {
  return (
    <div className={cn(field, "rounded-settings flex flex-col gap-1 px-3 py-2.5")}>
      <p className="font-medium wrap-break-word">{title}</p>
      <p className="text-muted-foreground line-clamp-4 whitespace-pre-line wrap-break-word">{text}</p>
    </div>
  );
}

/** An offer's error, under what it is about. */
export function OfferError({ children }: { children: ReactNode }) {
  return (
    <p role="alert" className="text-destructive text-xs">
      {children}
    </p>
  );
}

/** An offer's buttons: the answer filled, another soft, a quiet one in grey. */
export function offerButton(look: "primary" | "soft" | "quiet"): string {
  return look === "primary"
    ? "bg-foreground text-background rounded-capsule h-control-sm flex items-center gap-1.5 px-3.5 text-sm font-medium transition-opacity hover:opacity-90 active:scale-95 disabled:pointer-events-none disabled:opacity-40 motion-reduce:transition-none"
    : look === "soft"
      ? cn(field, "rounded-capsule h-control-sm hover:bg-foreground/10 px-3.5 text-sm transition-colors disabled:opacity-40")
      : "text-muted-foreground hover:text-foreground rounded-capsule h-control-sm px-3 text-sm transition-colors disabled:opacity-40";
}
