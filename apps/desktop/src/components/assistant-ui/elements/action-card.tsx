import { ArrowRight, Pencil, X } from "@openai/apps-sdk-ui/components/Icon";
import {
  type ComponentProps,
  type CSSProperties,
  type ReactNode,
  useEffect,
  useRef,
  useState,
} from "react";

import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

/* Approval and question content on the composer rail; each section pads itself. */

/** Squircle corners where the engine draws them; each radius grows a quarter to match. */
const SQUIRCLE = "supports-[corner-shape:superellipse(1.5)]:[corner-shape:superellipse(1.5)]";

/**
 * The card itself; its content decides the keys (digits, arrows) through `onKeyDown`.
 * `container` names the container query its narrow layout keys off.
 */
export function ActionCard({
  className,
  container = "approval",
  onMouseDown,
  ...props
}: ComponentProps<"section"> & { container?: "approval" | "request" }) {
  return (
    // The mouse handler only keeps focus where it is; the card's keys are its buttons' own.
    // oxlint-disable-next-line jsx-a11y/no-static-element-interactions
    <section
      data-slot="action-card"
      // A click on the card's blank space keeps the user's current focus.
      onMouseDown={(event) => {
        event.stopPropagation();
        onMouseDown?.(event);
      }}
      className={cn(
        container === "approval" ? "@container/approval-card" : "@container/request-card",
        "text-foreground flex w-full flex-col outline-none",
        className,
      )}
      {...props}
    />
  );
}

/**
 * The approval's header: "▣ Terminal" small and dim, the question, a muted line under it.
 * Announced as a whole when it appears.
 */
export function ActionCardHeader({
  icon,
  kind,
  title,
  detail,
  children,
}: {
  icon: ReactNode;
  kind: ReactNode;
  title: ReactNode;
  detail?: ReactNode;
  /** Rows under the header (a warning badge). */
  children?: ReactNode;
}) {
  return (
    <div className="flex min-w-0 flex-col gap-2 px-4 pt-4 pb-3">
      <div role="alert" aria-atomic="true" className="flex min-w-0 flex-col gap-2">
        <p className="text-foreground/65 text-code flex items-center gap-2 leading-5 [&_svg]:size-4.5 [&_svg]:shrink-0">
          {icon}
          {kind}
        </p>
        <ActionCardTitle detail={detail}>{title}</ActionCardTitle>
      </div>
      {children}
    </div>
  );
}

/** The question the card asks, and a muted line under it. */
export function ActionCardTitle({ children, detail }: { children: ReactNode; detail?: ReactNode }) {
  return (
    <div className="flex min-w-0 flex-1 flex-col gap-0.5">
      <h2 className="min-w-0 text-sm leading-5 font-medium wrap-anywhere">{children}</h2>
      {detail && <p className="text-foreground/50 text-code">{detail}</p>}
    </div>
  );
}

/** A question card's header: the question, then × that puts it aside. */
export function ActionCardQuestion({
  children,
  detail,
  onDismiss,
}: {
  children: ReactNode;
  detail?: ReactNode;
  onDismiss: () => void;
}) {
  return (
    <div className="flex items-start justify-between gap-2 ps-4 pe-3 pt-4 pb-2">
      <div role="alert" aria-atomic="true" className="flex min-w-0 flex-1">
        <ActionCardTitle detail={detail}>{children}</ActionCardTitle>
      </div>
      <button
        type="button"
        aria-label="Dismiss"
        title="Dismiss"
        onClick={onDismiss}
        className="text-foreground/50 hover:bg-foreground/8 focus-visible:ring-ring rounded-capsule size-icon-button-sm [&_svg]:size-icon-sm -mt-0.5 flex shrink-0 items-center justify-center transition-colors outline-none focus-visible:ring-2"
      >
        <X />
      </button>
    </div>
  );
}

/** A button's look on the card: a capsule, the primary filled, the other outlined. */
export function actionButton(variant: "primary" | "outline", className?: string): string {
  return cn(
    "group/action h-control-sm rounded-capsule border-foreground/8 focus-visible:ring-ring text-code inline-flex shrink-0 cursor-default items-center gap-1 border px-2 leading-4.5 font-normal whitespace-nowrap transition-colors outline-none select-none focus-visible:ring-2 disabled:opacity-40",
    variant === "primary"
      ? "bg-foreground text-composer enabled:hover:bg-foreground/80 data-[state=open]:bg-foreground/80"
      : "bg-foreground/3 text-foreground enabled:hover:bg-foreground/8 data-[state=open]:bg-foreground/8",
    className,
  );
}

/** A key hint on a card's button ("⏎", "Esc"), hidden once the card is narrow. */
export function ActionKbd({ children, variant }: { children: ReactNode; variant: "primary" | "outline" }) {
  return (
    <kbd
      aria-hidden
      className={cn(
        "inline-flex h-4 min-w-4 items-center justify-center rounded-md bg-current/10 px-1.5 font-sans text-xs leading-4 text-current supports-[corner-shape:superellipse(1.5)]:rounded-lg @max-md/approval-card:hidden",
        SQUIRCLE,
        variant === "outline" && "group-hover/action:bg-current/15",
      )}
    >
      {children}
    </kbd>
  );
}

/**
 * The approval's buttons: the choices pushed to the end, [Deny Esc] [Allow once ⏎]. Below
 * 28rem they stack full width and drop their key hints.
 */
export function ActionCardActions({
  leading,
  children,
}: {
  /** Before the choices, at the start (an error). */
  leading?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="flex items-center gap-2 px-4 pt-2 pb-4 @max-md/approval-card:flex-col @max-md/approval-card:items-stretch">
      {leading}
      <div className="ms-auto flex min-w-0 items-center gap-2 @max-md/approval-card:ms-0 @max-md/approval-card:w-full @max-md/approval-card:flex-col @max-md/approval-card:items-stretch">
        {children}
      </div>
    </div>
  );
}

/** Exact text the card asks about (a command line, tool input), three lines until expanded. */
export function ActionCardCode({ children }: { children: ReactNode }) {
  const ref = useRef<HTMLSpanElement>(null);
  const [expanded, setExpanded] = useState(false);
  const [clamped, setClamped] = useState(false);
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const observer = new ResizeObserver(() =>
      setClamped(element.scrollHeight > element.clientHeight + 1),
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return (
    <div className="px-3">
      <div
        className={cn(
          "bg-background/25 text-foreground/50 flex max-h-80 w-full flex-col overflow-hidden rounded-md text-xs leading-4.5 supports-[corner-shape:superellipse(1.5)]:rounded-lg",
          SQUIRCLE,
        )}
      >
        <div className="min-h-0 overflow-y-auto p-2 font-mono font-medium">
          <span
            ref={ref}
            data-selectable
            className={cn("block break-words whitespace-pre-wrap", !expanded && "line-clamp-3")}
          >
            {children}
          </span>
        </div>
        {(clamped || expanded) && (
          <div className="flex shrink-0 justify-end p-1">
            <button
              type="button"
              onClick={() => setExpanded(!expanded)}
              className="text-foreground/50 hover:bg-foreground/8 focus-visible:ring-ring text-code rounded-lg px-2 py-0.5 font-sans leading-4.5 transition-colors outline-none focus-visible:ring-2"
            >
              {expanded ? "Collapse" : "Expand"}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

/** Files the card asks about: the folder dim, the name bright, a count at the end; scrolls. */
export function ActionFileList({ files }: { files: { path: string; trailing?: ReactNode }[] }) {
  return (
    <div className="px-4 py-2">
      <div className="border-border bg-background/50 flex flex-col overflow-hidden rounded-lg border">
        <div className="flex max-h-50 flex-col overflow-y-auto">
          {files.map(({ path, trailing }) => {
            const slash = path.lastIndexOf("/");
            return (
              <div key={path} className="flex min-w-0 items-center gap-2.5 p-1.5 text-sm font-medium">
                <span title={path} className="flex min-w-0 flex-1 items-center">
                  {slash > 0 && (
                    <span className="text-foreground/50 min-w-0 truncate">{path.slice(0, slash + 1)}</span>
                  )}
                  <span className="max-w-full shrink-0 truncate">{path.slice(slash + 1)}</span>
                </span>
                {trailing}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

/** The marker disc of a question row: its number, a dot once chosen, or ✎. */
function Marker({ chosen, children }: { chosen?: boolean | undefined; children: ReactNode }) {
  return (
    <span
      aria-hidden
      className={cn(
        "rounded-capsule size-icon-button-md flex shrink-0 items-center justify-center self-start border text-xs leading-none font-medium",
        chosen
          ? "border-foreground bg-primary text-primary-foreground"
          : "border-border bg-foreground/5 text-foreground/50",
      )}
    >
      {chosen ? <span className="size-1.5 rounded-full bg-current" /> : children}
    </span>
  );
}

/**
 * A numbered answer: its disc (a dot while it commits), label, "Recommended", a
 * description, → while highlighted or hovered.
 */
export function ActionOption({
  number,
  label,
  description,
  recommended,
  highlighted,
  chosen,
  className,
  ...props
}: Omit<ComponentProps<"button">, "children"> & {
  number: number;
  label: string;
  description?: ReactNode;
  recommended?: boolean | undefined;
  highlighted: boolean;
  /** Picked and about to go. */
  chosen?: boolean | undefined;
}) {
  return (
    <button
      type="button"
      data-slot="action-option"
      data-highlighted={highlighted || undefined}
      aria-label={label}
      className={cn(
        "group focus-visible:ring-ring text-code flex min-h-8 w-full items-center gap-2 rounded-xl px-2 py-1.5 text-start outline-none focus-visible:ring-2",
        highlighted ? "bg-foreground/5" : "hover:bg-foreground/5",
        className,
      )}
      {...props}
    >
      <Marker chosen={chosen}>{number}</Marker>
      <span className="flex min-w-0 flex-1 flex-wrap items-baseline gap-x-2 gap-y-0.5 @max-5xl/request-card:flex-col @max-5xl/request-card:items-stretch">
        <span className="inline-flex min-w-0 items-center gap-1.5">
          <span className="min-w-0 font-medium wrap-anywhere" title={label}>
            {label}
          </span>
          {recommended && (
            <Badge variant="secondary" className="shrink-0 px-1.5 py-0.5 text-xs">
              Recommended
            </Badge>
          )}
        </span>
        {description && <span className="text-foreground/50 min-w-0 wrap-anywhere">{description}</span>}
      </span>
      <ArrowRight
        aria-hidden
        className={cn(
          "text-foreground/50 size-icon-md ms-auto shrink-0 opacity-0 group-hover:opacity-100 group-focus-visible:opacity-100",
          highlighted && "opacity-100",
        )}
      />
    </button>
  );
}

/**
 * The last row: ✎ and "No, and tell Brigadier what to do differently", then its button
 * (Skip until something is typed). Lit while it holds the highlight.
 */
export function ActionFreeText({
  children,
  highlighted,
  marker = true,
  style,
  className,
  ...props
}: ComponentProps<"input"> & {
  children?: ReactNode;
  highlighted?: boolean;
  /** The ✎ disc, for the row under numbered answers; a bare field has none. */
  marker?: boolean;
}) {
  return (
    <div
      role="presentation"
      style={style}
      className={cn(
        "text-code flex min-h-8 w-full items-center gap-2 px-2 py-1.5",
        marker && "hover:bg-foreground/5 rounded-2xl transition-colors",
        highlighted && "bg-foreground/5",
        className,
      )}
    >
      {marker && (
        <Marker>
          <Pencil className="size-icon-sm" />
        </Marker>
      )}
      <input
        className="placeholder:text-foreground/50 min-w-0 flex-1 self-center bg-transparent outline-none"
        {...props}
      />
      {children && <div className="flex items-center gap-2 place-self-end">{children}</div>}
    </div>
  );
}

/**
 * When one question card follows another, its rows fade in 50ms apart, 300ms each; the
 * first card shows as is. Reduced motion shows them at once.
 */
export function staggered(on: boolean, index: number): { className?: string; style?: CSSProperties } {
  if (!on) return {};
  return {
    className: "animate-in fade-in fill-mode-backwards duration-300 ease-out motion-reduce:animate-none",
    style: { animationDelay: `${index * 50}ms` },
  };
}
