import { MarkdownBlock } from "@/components/conversation";
import { cn } from "@/lib/utils";

/** A plan's text in the plan's own type: a large title, section headings, compact lists. */
export function PlanText({ markdown }: { markdown: string }) {
  return (
    <div
      data-slot="plan-text"
      className={cn(
        "text-plan text-foreground [&_.aui-md-li]:leading-(--text-plan--line-height) [&_.aui-md-p]:leading-(--text-plan--line-height)",
        "[&_.aui-md-h1]:text-plan-title [&_.aui-md-h1]:mb-2",
        "[&_.aui-md-h2]:text-plan-heading [&_.aui-md-h2]:mt-4 [&_.aui-md-h2]:mb-1",
        "[&_.aui-md-h3]:text-plan [&_.aui-md-h3]:mt-3 [&_.aui-md-h3]:mb-1",
        "[&_.aui-md-p]:my-2 [&_.aui-md-ol]:my-1 [&_.aui-md-ul]:my-1 [&_.aui-md-ol>li]:mt-0 [&_.aui-md-ul>li]:mt-0",
      )}
    >
      <MarkdownBlock text={markdown} />
    </div>

  );
}
