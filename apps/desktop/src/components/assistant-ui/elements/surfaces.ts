/**
 * Shared surface classes for the assistant-ui elements (https://www.assistant-ui.com/elements),
 * rewritten onto Brigadier's tokens: dark only, density-driven sizes, no raw values.
 */

/** A raised card: task, approval, question and plan cards, the queue's running row. */
export const paper = "bg-card border border-border";

/** A recessed field inside a card: commands, queued rows, segmented controls. */
export const field = "bg-foreground/5";

export const fieldInteractive = "bg-foreground/5 transition-colors hover:bg-foreground/10";

/**
 * A row that opens and closes what it heads (a plan step, a `<summary>`): the whole row is the
 * target, washed while hovered and pressed, its edges a step outside the text it lines up with.
 */
export const disclosureRow =
  "rounded-control hover:bg-foreground/5 active:bg-foreground/10 -mx-1.5 cursor-pointer px-1.5 text-start transition-colors";

/** A quiet round icon button. */
export const ghostButton =
  "text-muted-foreground hover:bg-foreground/10 hover:text-foreground inline-flex shrink-0 items-center justify-center rounded-capsule transition-colors disabled:pointer-events-none disabled:opacity-50";

/** Small monospace meta text (ids, counts, models). */
export const mono = "font-mono text-2xs tracking-tight";

/** Something live (running, streaming). */
export const live = "text-success";

/** A floating menu anchored to the composer (mentions), on the round menu surface. */
export const floatingMenu =
  "bg-popover text-popover-foreground rounded-menu shadow-menu border p-1";

/**
 * The composer controls (the `+`, permission, model, the rail's project and branch): a
 * text-sm ghost at the height of a control, fully round, filled with a light wash while
 * hovered or open.
 */
export const composerPill =
  "h-control-sm rounded-capsule hover:bg-foreground/8 data-[state=open]:bg-foreground/8 inline-flex min-w-0 shrink-0 items-center gap-1.5 px-2 text-sm transition-colors disabled:pointer-events-none disabled:opacity-50 [&_svg]:size-icon-md [&_svg]:shrink-0";
