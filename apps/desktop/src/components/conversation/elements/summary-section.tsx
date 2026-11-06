import { type ComponentProps, type ReactNode, useId, useState } from "react";

import { ChevronDown } from "@/components/icons";
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
  collapsible = true,
  defaultFolded = false,
  unfoldOnIncrease = 0,
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
  /** A header without rows has no fold control or body. */
  collapsible?: boolean;
  /** Folded until the user opens it (a plan whose steps are all done). */
  defaultFolded?: boolean;
  /** Reveal new items locally, without changing the fold saved by hand. */
  unfoldOnIncrease?: number;
}) {
  const bodyId = useId();
  const [kept, setKept] = useState(() => folds.get(foldKey));
  const [previousCount, setPreviousCount] = useState(0);
  if (previousCount !== unfoldOnIncrease) {
    setPreviousCount(unfoldOnIncrease);
    if (unfoldOnIncrease > previousCount) setKept(false);
  }
  const folded = collapsible && (kept ?? defaultFolded);
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
        {collapsible ? <button
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
        </button> : <span className="text-muted-foreground min-w-0 flex-1 truncate text-label">{title}</span>}
        {action && <div className="flex shrink-0 items-center gap-0.5">{action}</div>}
      </header>
      {collapsible && <div id={bodyId} hidden={folded} className="flex min-w-0 flex-col gap-0.5 px-5">
        {children}
      </div>}
    </section>
  );
}

/** A row's look: one line, 28px, 13px at the prose weight, a fixed slot for its icon (which
 * carries its state), its meta at the end. */
const ROW =
  "relative flex h-row-sm w-full min-w-0 items-center gap-2 text-start text-label font-prose";

/**
 * An interactive row washes on hover and while what it opens is open, the wash a step wider
 * than the text it lines up with.
 */
const ROW_INTERACTIVE =
  "rounded-control before:rounded-control hover:before:bg-foreground/8 data-[state=open]:before:bg-foreground/8 active:before:bg-foreground/10 focus-visible:before:ring-ring cursor-pointer outline-none before:absolute before:inset-y-0 before:-inset-x-2 before:-z-10 before:transition-colors focus-visible:before:ring-1";

/**
 * What shows only on a row hovered or holding keyboard focus, over its end: a mouse click's focus
 * doesn't keep it showing. The row is a `group/row`.
 */
export const rowReveal =
  "pointer-events-none opacity-0 transition-opacity group-hover/row:pointer-events-auto group-hover/row:opacity-100 group-has-focus-visible/row:pointer-events-auto group-has-focus-visible/row:opacity-100";

type RowContent = {
  /** In the icon slot; a 14px icon (a spinner while it runs) or a worker's glyph. */
  icon?: ReactNode;
  /** At the row's end: +N −N, a count, a time. */
  meta?: ReactNode;
  /** Dimmer: done, a link to more. */
  muted?: boolean;
  /** The row's actions, over its meta while the row is hovered or focused: nothing moves. */
  action?: ReactNode;
};

function RowParts({ icon, meta, action, children }: RowContent & { children: ReactNode }) {
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
      <span className="min-w-0 flex-1 truncate">{children}</span>
      {meta !== undefined && meta !== null && meta !== false && (
        <span
          className={cn(
            "text-muted-foreground/70 flex shrink-0 items-center gap-1.5 text-xs tabular-nums",
            action && "group-hover/row:invisible group-has-focus-visible/row:invisible",
          )}
        >
          {meta}
        </span>
      )}
    </>
  );
}

/**
 * The action's place: over the row's end, on a backing in the row's own colour that fades in over
 * the words beneath, a step lighter on a row that `washed` on hover.
 */
function RowAction({ children, washed }: { children: ReactNode; washed?: boolean }) {
  return (
    <span
      data-slot="summary-row-action"
      className={cn(
        rowReveal,
        "row-backing absolute inset-y-0 end-0 flex items-center ps-4 has-[[role=alert]]:pointer-events-auto has-[[role=alert]]:opacity-100",
        washed && "row-backing-washed",
      )}
    >
      {children}
    </span>
  );
}

/** A row of a summary section that shows something. */
export function SummaryRow({
  icon,
  meta,
  muted,
  action,
  className,
  children,
  ...props
}: ComponentProps<"div"> & RowContent) {
  return (
    <div
      data-slot="summary-row"
      className={cn(ROW, "group/row isolate", muted && "text-muted-foreground", className)}
      {...props}
    >
      <RowParts icon={icon} meta={meta} action={action}>
        {children}
      </RowParts>
      {action && <RowAction>{action}</RowAction>}
    </div>
  );
}

/** A row of a summary section that does something when clicked; its action, if any, beside it. */
export function SummaryRowButton({
  icon,
  meta,
  muted,
  action,
  className,
  children,
  ...props
}: ComponentProps<"button"> & RowContent) {
  const button = (
    <button
      type="button"
      data-slot="summary-row"
      className={cn(
        ROW,
        ROW_INTERACTIVE,
        "isolate",
        // The pointer on its action is still on the row: the wash stays, the whole row wide.
        action ? "group-hover/row:before:bg-foreground/8 group-has-focus-visible/row:before:bg-foreground/8" : "group/row",
        muted && "text-muted-foreground",
        className,
      )}
      {...props}
    >
      <RowParts icon={icon} meta={meta} action={action}>
        {children}
      </RowParts>
    </button>
  );
  if (!action) return button;
  return (
    <div className="group/row relative isolate min-w-0">
      {button}
      <RowAction washed>{action}</RowAction>
    </div>
  );
}

/** The wash and focus ring of an interactive summary row, for rows built by hand. */
export const summaryRowInteractive = ROW_INTERACTIVE;

/** A summary row's look, for rows built by hand. */
export const summaryRow = ROW;
