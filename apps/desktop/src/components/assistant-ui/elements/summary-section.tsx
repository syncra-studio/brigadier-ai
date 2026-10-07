import { ChevronDown } from "@openai/apps-sdk-ui/components/Icon";
import { type ComponentProps, type ReactNode, useId, useState } from "react";

import { cn } from "@/lib/utils";

/**
 * The pinned summary's sections and rows. A section is a quiet 13px title over its rows, inset
 * from the card's edges, with a hairline under it (not under the last). Its title folds it: the
 * chevron after the title shows on hover while the section is open, and stays, turned, while it
 * is folded, when a count follows the title ("Plan · 4").
 */

/** Sections folded or opened by hand, by key, for the app's lifetime: the card keeps them. */
const folds = new Map<string, boolean>();

export function SummarySection({
  foldKey,
  title,
  count,
  summary,
  action,
  defaultFolded = false,
  className,
  children,
  ...props
}: Omit<ComponentProps<"section">, "title"> & {
  /** Where the section's fold is kept (the same section in every session shares it). */
  foldKey: string;
  title: ReactNode;
  /** Shown after the title while the section is folded. */
  count?: number | undefined;
  /** Said instead of the count while folded ("2 working · 1 done"). */
  summary?: ReactNode;
  /** At the header's end: a `+` or an ⋯ menu. */
  action?: ReactNode;
  /** Folded until the user opens it (a plan whose steps are all done). */
  defaultFolded?: boolean;
}) {
  const bodyId = useId();
  const [kept, setKept] = useState(() => folds.get(foldKey));
  const folded = kept ?? defaultFolded;
  const toggle = () => {
    folds.set(foldKey, !folded);
    setKept(!folded);
  };
  return (
    <section
      data-slot="summary-section"
      data-folded={folded || undefined}
      className={cn(
        "after:bg-divider relative flex flex-col pb-2 after:absolute after:inset-x-5 after:bottom-0 after:h-px after:origin-bottom after:scale-y-50 last:pb-0 last:after:hidden",
        folded && "pb-0.5",
        className,
      )}
      {...props}
    >
      <header className="bg-popover sticky top-0 z-10 flex h-row-sm min-w-0 items-center gap-2 ps-5 pe-3.5">
        <button
          type="button"
          aria-expanded={!folded}
          aria-controls={bodyId}
          onClick={toggle}
          className="group/fold text-muted-foreground hover:text-foreground rounded-control -ms-1 inline-flex min-w-0 flex-1 items-center gap-1.5 py-0.5 ps-1 pe-1 text-start text-label transition-colors"
        >
          <span className="min-w-0 truncate">{title}</span>
          {folded && (summary || (count !== undefined && count > 0)) && (
            <span className="text-muted-foreground/70 min-w-0 shrink truncate tabular-nums">· {summary ?? count}</span>
          )}
          <ChevronDown
            aria-hidden
            className={cn(
              "size-icon-xs shrink-0 transition-[rotate,opacity] motion-reduce:transition-none",
              folded
                ? "-rotate-90"
                : "opacity-0 group-hover/fold:opacity-100 group-focus-visible/fold:opacity-100",
            )}
          />
        </button>
        {action && <div className="flex shrink-0 items-center gap-0.5">{action}</div>}
      </header>
      <div id={bodyId} hidden={folded} className="flex min-w-0 flex-col gap-0.5 px-5">
        {children}
      </div>
    </section>
  );
}

/** A row's look: 13px, a fixed slot for its icon, its meta at the end. */
const ROW =
  "relative flex min-h-row-sm w-full min-w-0 items-center gap-2 py-1 text-start text-label";

/**
 * An interactive row washes on hover and while what it opens is open, the wash a step wider
 * than the text it lines up with.
 */
const ROW_INTERACTIVE =
  "rounded-control before:rounded-control hover:before:bg-foreground/5 data-[state=open]:before:bg-foreground/5 active:before:bg-foreground/10 focus-visible:before:ring-ring cursor-pointer outline-none before:absolute before:inset-y-0 before:-inset-x-2 before:-z-10 before:transition-colors focus-visible:before:ring-1";

type RowContent = {
  /** In the icon slot; a 14px icon or a worker's glyph. */
  icon?: ReactNode;
  /** At the row's end: a state in words, +N −N, a count. */
  meta?: ReactNode;
  /** Dimmer: a done step, a link to more. */
  muted?: boolean;
  /** A second, smaller line under the label: a worker's state and time. */
  description?: ReactNode;
};

function RowParts({ icon, meta, description, children }: RowContent & { children: ReactNode }) {
  return (
    <>
      {icon !== undefined && (
        <span
          aria-hidden
          className="text-muted-foreground flex min-w-icon-md shrink-0 items-center justify-center [&>svg]:size-icon-md"
        >
          {icon}
        </span>
      )}
      {description ? (
        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="truncate">{children}</span>
          <span className="text-muted-foreground flex min-w-0 items-center gap-1.5 text-xs">{description}</span>
        </span>
      ) : (
        <span className="min-w-0 flex-1 truncate">{children}</span>
      )}
      {meta !== undefined && meta !== null && meta !== false && (
        <span className="text-muted-foreground/70 flex shrink-0 items-center gap-1.5 text-xs tabular-nums">
          {meta}
        </span>
      )}
    </>
  );
}

/** A row of a summary section that shows something. */
export function SummaryRow({
  icon,
  meta,
  muted,
  description,
  className,
  children,
  ...props
}: ComponentProps<"div"> & RowContent) {
  return (
    <div
      data-slot="summary-row"
      className={cn(ROW, "isolate", muted && "text-muted-foreground", className)}
      {...props}
    >
      <RowParts icon={icon} meta={meta} description={description}>
        {children}
      </RowParts>
    </div>
  );
}

/** A row of a summary section that does something when clicked. */
export function SummaryRowButton({
  icon,
  meta,
  muted,
  description,
  className,
  children,
  ...props
}: ComponentProps<"button"> & RowContent) {
  return (
    <button
      type="button"
      data-slot="summary-row"
      className={cn(ROW, ROW_INTERACTIVE, "isolate", muted && "text-muted-foreground", className)}
      {...props}
    >
      <RowParts icon={icon} meta={meta} description={description}>
        {children}
      </RowParts>
    </button>
  );
}

/** The wash and focus ring of an interactive summary row, for rows built by hand. */
export const summaryRowInteractive = ROW_INTERACTIVE;
