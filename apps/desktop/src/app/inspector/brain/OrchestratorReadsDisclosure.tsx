import { ChevronRight } from "@/components/icons";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/layout";
import { cn } from "@/lib/utils";

/** Under a Brain search's hits: the text the orchestrator would read, folded away. */
export function OrchestratorReadsDisclosure({
  open,
  onOpenChange,
  text,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  text: string;
}) {
  return (
    <Collapsible open={open} onOpenChange={onOpenChange} className="border-t">
      <CollapsibleTrigger className="text-muted-foreground hover:text-foreground flex w-full items-center gap-1 px-3 py-2 text-start">
        <ChevronRight className={cn("size-icon-xs transition-[rotate]", open && "rotate-90")} />
        What the orchestrator reads · {text.length.toLocaleString()} characters
      </CollapsibleTrigger>
      <CollapsibleContent>
        <pre className="bg-muted/50 mx-3 mb-3 overflow-x-auto rounded-control p-2 font-mono whitespace-pre-wrap">
          {text}
        </pre>
      </CollapsibleContent>
    </Collapsible>
  );
}
