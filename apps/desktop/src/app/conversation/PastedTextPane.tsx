import { pastedTextLabel } from "@/components/assistant-ui/elements/chipText";
import { PastedTextView } from "@/components/assistant-ui/elements/pasted-text";
import { FileDocument, X } from "@/components/icons";
import type { AttachmentRef } from "@/ipc/generated";

/**
 * Pasted text `n` over the thread of a Chat or a draft, which have no tabs (#427): a session's
 * tab of it as the only one, its title and a × that closes it, the Pages editor under it.
 */
export function PastedTextPane({
  n,
  paste,
  onClose,
}: {
  n: number;
  paste: AttachmentRef | null;
  onClose: () => void;
}) {
  const label = pastedTextLabel(n);
  return (
    <div role="region" aria-label={label} data-slot="pasted-text-pane" className="bg-background absolute inset-0 flex flex-col">
      <div className="flex shrink-0 items-center px-2 pt-1.5">
        <div className="bg-panel-tab shadow-session-tab text-foreground rounded-session-tab text-session-tab flex h-control-md w-session-tab-max max-w-full min-w-0 items-center gap-1 ps-2.5 pe-1.75 select-none">
          <FileDocument aria-hidden className="size-icon-sm shrink-0" />
          <span className="min-w-0 flex-1 truncate">{label}</span>
          <button
            type="button"
            aria-label={`Close ${label}`}
            onClick={onClose}
            className="hover:bg-toolbar-hover flex size-5 shrink-0 items-center justify-center rounded-full opacity-60 hover:opacity-100"
          >
            <X aria-hidden className="size-icon-xs" />
          </button>
        </div>
      </div>
      <PastedTextView n={n} paste={paste} />
    </div>
  );
}
