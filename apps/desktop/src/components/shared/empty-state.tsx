import type { ComponentType, ReactNode } from "react";

import { cn } from "@/lib/utils";

type EmptyStateProps = {
  /**
   * `line`: a muted line in a list (the sidebar, Settings search, history). `card`: the same line
   * as a card's only row, padded like a row. `centered`: icon, title and line in the middle.
   */
  variant?: "line" | "card" | "centered";
  icon?: ComponentType<{ className?: string; "aria-hidden"?: boolean }>;
  title: ReactNode;
  /** A line under the title (centered only); the title then reads as a heading. */
  detail?: ReactNode;
  /** `li` inside a list; such a line is not announced, a list item can't be a status. */
  as?: "p" | "div" | "li";
  className?: string;
  "data-setting"?: string;
};

const SHAPE = {
  line: "min-h-nav-row flex items-center gap-3 px-2",
  card: "flex items-center gap-3 px-4 py-3",
  centered: "flex flex-col items-center justify-center gap-1.5 py-8 text-center",
};

/** What a list, card or pane shows with nothing in it ("No sessions yet"). */
export function EmptyState({
  variant = "line",
  icon: Icon,
  title,
  detail,
  as: Tag = "p",
  className,
  ...props
}: EmptyStateProps) {
  return (
    <Tag
      data-slot="empty-state"
      role={Tag === "li" ? undefined : "status"}
      {...props}
      className={cn("text-foreground/65 text-sm", SHAPE[variant], className)}
    >
      {Icon && (
        <Icon aria-hidden className={cn("shrink-0", variant === "centered" ? "size-icon-lg" : "size-icon-md")} />
      )}
      {variant === "centered" && detail ? (
        <>
          <span className="text-foreground font-medium">{title}</span>
          <span className="text-label">{detail}</span>
        </>
      ) : (
        title
      )}
    </Tag>
  );
}
