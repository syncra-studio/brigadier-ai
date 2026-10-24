import { type ComponentProps, createContext, type FC, type ReactNode, useContext, useState } from "react";

import { startupScreenGone } from "@/lib/splash";
import { cn } from "@/lib/utils";

/**
 * How long after the rail appears its items are simply there rather than opening: a
 * conversation's view is drawn anew on each switch, and its cards (and the ones its board
 * brings a moment later) belong to it, not to the moment.
 */
const SETTLE_MS = 400;

/** When the rail appeared (`performance.now()`); null outside one. */
const RailShown = createContext<number | null>(null);

/**
 * The composer rail: a strip attached to the top of the composer card, inset from its
 * sides and tucked under its top edge, so only the rail's top corners show. It carries the
 * utility bar on a new chat (project, run location, branch) and, in a thread, cards such as
 * the queue and /status. Hidden while it holds nothing.
 */
export const ComposerRail: FC<ComponentProps<"div">> = ({ className, ...props }) => {
  const [shown] = useState(() => performance.now());
  return (
    <RailShown value={shown}>
      <div
        data-slot="composer-rail"
        className={cn(
          "mx-rail-inset -mb-rail-tuck rounded-t-rail relative flex flex-col overflow-clip empty:hidden",
          className,
        )}
        {...props}
      />
    </RailShown>
  );
};

/**
 * One item of the rail. `controls` is the utility bar (a darker tab, no border); `card` is a
 * framed card (the queue, /status, the workers strip); `decision` is a card waiting for the
 * user, which at the rail's bottom shows its whole frame a small gap above the composer instead
 * of tucking under it. Items open to their height as they appear; those already there under the
 * startup screen are simply there, or the composer would rise as the app fades in (#469), and so
 * are those a conversation has as it is switched to, or its cards would close and open again.
 */
export const ComposerRailItem: FC<{
  variant?: "controls" | "card" | "decision";
  label?: string;
  className?: string;
  children: ReactNode;
}> = ({ variant = "card", label, className, children }) => {
  const railShown = useContext(RailShown);
  const [opens] = useState(
    () => startupScreenGone() && (railShown === null || performance.now() - railShown > SETTLE_MS),
  );
  return (
    <section
      data-slot="composer-rail-item"
      data-variant={variant}
      aria-label={label}
      className={cn(
        "grid grid-rows-1",
        opens && "animate-rail-open motion-reduce:animate-none",
        "first:rounded-t-rail",
        // The tuck under the card is padding, so the last item's content never hides behind it.
        variant !== "decision" && "last:pb-rail-tuck",
        variant === "controls"
          ? "bg-rail-controls"
          : "bg-rail/70 border-foreground/10 border-x border-t backdrop-blur-sm",
        variant === "decision" && "last:rounded-b-rail last:mb-rail-clear last:border-b",
        className,
      )}
    >
      <div className="min-h-0 overflow-hidden">{children}</div>
    </section>
  );
};
