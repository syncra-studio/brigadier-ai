import notices from "../../../../../THIRD_PARTY_NOTICES.md?raw";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { MarkdownBlock } from "@/components/assistant-ui/thread";

/** The notices without their own title: the dialog has one. */
const NOTICES = notices.replace(/^#[^\n]*\n+/, "").trim();

/** Settings › General › About › Open source licenses: the code Brigadier adapts and its licenses. */
export function LicensesDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-9/10 max-w-lg flex-col">
        <DialogHeader>
          <DialogTitle>Open source licenses</DialogTitle>
          <DialogDescription>Open source code Brigadier adapts or uses, and its licenses</DialogDescription>
        </DialogHeader>
        <div className="text-label -mx-1 min-h-0 flex-1 overflow-y-auto px-1">
          <MarkdownBlock text={NOTICES} />
        </div>
      </DialogContent>
    </Dialog>
  );
}
