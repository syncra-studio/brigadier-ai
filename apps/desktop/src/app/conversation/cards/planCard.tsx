import { MarkdownBlock } from "@/components/conversation";
import { cn } from "@/lib/utils";

/**
 * The words a plan card asks with: the plan's own first heading when it names something
 * ("Add a.txt"), not a bare "Plan" (harness D7).
 */
export function planTitle(plan: string): string {
  const heading = /^#{1,6}\s+(.+?)\s*#*\s*$/m.exec(plan)?.[1]?.trim();
  return heading && !/^(the )?plan$/i.test(heading) ? `Build this plan? ${heading}` : "Build this plan?";
}

/** A plan the CLI proposed, as its message would read, in the card's scrolling box. */
export function PlanText({ plan, className }: { plan: string; className?: string }) {
  return (
    <div
      data-slot="plan-text"
      data-selectable
      className={cn(
        "bg-code-surface text-foreground/80 rounded-control max-h-60 overflow-auto px-3 py-2 text-sm wrap-anywhere [&_.aui-md>:first-child]:mt-0 [&_.aui-md>:last-child]:mb-0",
        className,
      )}
    >
      <MarkdownBlock text={plan} />
    </div>
  );
}
