import { useState } from "react";

import { Button } from "@/components/controls";
import { PLAIN_FIELD } from "@/lib/plainTyping";

export function RawSessionComposer({
  disabled,
  turnActive,
  onSend,
  onInterrupt,
}: {
  disabled: boolean;
  turnActive: boolean;
  onSend: (text: string, steer: boolean) => void;
  onInterrupt: () => void;
}) {
  const [text, setText] = useState("");
  const send = (steer: boolean) => {
    const trimmed = text.trim();
    if (!trimmed) return;
    onSend(trimmed, steer);
    setText("");
  };
  return (
    <div className="flex shrink-0 flex-col gap-2 border-t p-2">
      <textarea
        {...PLAIN_FIELD}
        aria-label="Message"
        value={text}
        disabled={disabled}
        rows={3}
        placeholder={disabled ? "The session is not running." : "Message (⌘↩ to send, ⇧⌘↩ to steer)"}
        onChange={(event) => setText(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
            event.preventDefault();
            send(event.shiftKey);
          }
        }}
        className="bg-muted/60 focus-visible:bg-background rounded-control w-full resize-none border border-transparent p-2 text-xs disabled:opacity-50"
      />
      <div className="flex items-center gap-2">
        <span className="text-muted-foreground flex-1 text-xs">
          Steer delivers into the running turn, or starts one.
        </span>
        <Button size="xs" variant="outline" disabled={disabled || !turnActive} onClick={onInterrupt}>
          Interrupt
        </Button>
        <Button size="xs" variant="outline" disabled={disabled || !text.trim()} onClick={() => send(true)}>
          Steer
        </Button>
        <Button size="xs" disabled={disabled || !text.trim()} onClick={() => send(false)}>
          Send
        </Button>
      </div>
    </div>
  );
}
