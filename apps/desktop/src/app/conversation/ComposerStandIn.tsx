import { useEffect, useRef } from "react";

import { ComposerPrimitive } from "@/components/conversation";
import { PLAIN_FIELD } from "@/lib/plainTyping";

/**
 * The plain field that stands in for the composer while its editor loads: what's typed is what's
 * sent here too, with none of macOS's text substitutions (smart quotes and dashes, auto-correct,
 * capitals, the double-space period), as in the editor (#188).
 */
export function ComposerStandIn({ placeholder, autoFocus }: { placeholder?: string | undefined; autoFocus?: boolean | undefined }) {
  const field = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    const element = field.current;
    element?.addEventListener("beforeinput", keepTypedSpace, true);
    return () => element?.removeEventListener("beforeinput", keepTypedSpace, true);
  }, []);
  return (
    <ComposerPrimitive.Input
      ref={field}
      placeholder={placeholder}
      autoFocus={autoFocus}
      cancelOnEscape={false}
      rows={1}
      aria-label="Message input"
      {...PLAIN_FIELD}
      // A block, as the editor is: an inline field would sit on a line box a few pixels taller,
      // and the composer would drop as the editor takes its place (#469).
      className="aui-composer-input caret-primary placeholder:text-muted-foreground/60 min-h-composer max-h-composer-max block w-full resize-none bg-transparent px-2 py-2.5 text-base outline-none"
    />
  );
}

/**
 * macOS's "Add period with double-space" in a plain field: its input method puts ". " in place
 * of the space before the caret. The two spaces typed stay instead.
 */
export function keepTypedSpace(event: InputEvent) {
  if (event.data !== ". " || !/^insert(Replacement)?Text$/.test(event.inputType)) return;
  const field = event.currentTarget as HTMLTextAreaElement;
  const { selectionStart: start, selectionEnd: end, value } = field;
  // A textarea gives no target ranges: what it replaces is the space before the caret, or that
  // space selected.
  const selected = end - start === 1 && value[start] === " ";
  if (!selected && (start !== end || value[start - 1] !== " ")) return;
  event.preventDefault();
  event.stopImmediatePropagation();
  field.ownerDocument.execCommand("insertText", false, selected ? "  " : " ");
}
