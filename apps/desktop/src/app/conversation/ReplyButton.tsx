import { Reply } from "@/components/icons";

/** An approval's "Reply…" in the composer's place: brings the message field back. */
export function ReplyButton({ onClick }: { onClick: () => void }) {
  return (
    <button
      type="button"
      data-slot="approval-reply"
      className="text-foreground/55 hover:text-foreground hover:bg-foreground/8 text-code rounded-capsule h-control-sm inline-flex shrink-0 items-center gap-1.5 px-2 transition-colors"
      onClick={onClick}
    >
      <Reply className="size-icon-sm" />
      Reply…
    </button>
  );
}
