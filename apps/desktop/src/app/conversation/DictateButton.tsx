import { shortcutLabel } from "@/app/side-panel/SidePanel";
import { TooltipIconButton } from "@/components/controls";
import { Mic, Spinner, Warning, X } from "@/components/icons";
import { cancelDictation, startDictation, useDictation } from "@/state/dictation";
import { useApp } from "@/state/store";

/**
 * The Dictate button, its own beside the send button: the microphone (or ⌃⇧D) starts
 * dictating wherever the caret is, with or without text around it. It spins while the speech
 * model downloads, the microphone opens (a click cancels either) and the text is worked out,
 * and offers a retry after a failure. It keeps the field's focus, so the caret stays put.
 */
export function DictateButton({ owner }: { owner: string }) {
  const { available, phase } = useDictation(owner);
  const mac = useApp((s) => s.info?.platform === "macos");
  if (!available) return null;
  const failed = phase.type === "failed" ? phase.message : null;
  const busy = phase.type === "downloading" || phase.type === "starting" || phase.type === "transcribing";
  const tooltip =
    phase.type === "downloading"
      ? "Cancel download"
      : phase.type === "starting"
        ? "Cancel dictation"
        : phase.type === "transcribing"
          ? "Transcribing…"
          : failed
            ? `Retry dictation: ${failed}`
            : "Dictate";
  return (
    <TooltipIconButton
      tooltip={tooltip}
      shortcut={busy ? undefined : shortcutLabel("⌃⇧D", mac)}
      aria-keyshortcuts="Control+Shift+D"
      side="top"
      type="button"
      size="icon-md"
      data-state={busy ? phase.type : failed ? "retry-dictation" : "dictate"}
      className="aui-composer-dictate rounded-capsule group/dictate"
      disabled={phase.type === "transcribing"}
      onMouseDown={(event) => event.preventDefault()}
      onClick={busy ? cancelDictation : () => void startDictation(owner)}
    >
      <span
        key={busy ? "busy" : failed ? "retry" : "dictate"}
        className="animate-in fade-in zoom-in-75 flex items-center duration-200 motion-reduce:animate-none"
      >
        {busy ? (
          <>
            <Spinner className="size-icon-sm animate-spin group-hover/dictate:hidden motion-reduce:animate-none" />
            <X className="size-icon-sm hidden group-hover/dictate:block" />
          </>
        ) : failed ? (
          <Warning className="size-icon-sm" />
        ) : (
          <Mic className="size-icon-sm" />
        )}
      </span>
    </TooltipIconButton>
  );
}
