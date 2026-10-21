import { Output } from "@/app/conversation/activity/Output";
import { ToolBox } from "@/components/conversation";
import { Dialog, DialogContent, DialogTitle, DialogTrigger } from "@/components/overlays";

/** "Show raw tool call output": opens what a tool was given and gave back, as is. */
export function RawToolCallDialog({ name, input, output }: { name: string; input: string | null; output: string | null }) {
  const text = output?.trim() ?? "";
  return (
    <Dialog>
      <DialogTrigger asChild>
        <button type="button" data-slot="tool-raw" className="text-muted-foreground hover:text-foreground self-start text-xs transition-colors">
          Show raw tool call output
        </button>
      </DialogTrigger>
      <DialogContent aria-describedby={undefined} className="max-w-thread flex max-h-full flex-col gap-3">
        <DialogTitle>{name}</DialogTitle>
        <div className="flex min-h-0 flex-col gap-3 overflow-auto">
          {input && <ToolBox title="Input" className="mt-1"><Output text={input} /></ToolBox>}
          {text && <ToolBox title="Output" className="mt-1"><Output text={output ?? ""} /></ToolBox>}
        </div>
      </DialogContent>
    </Dialog>
  );
}
