import { useState } from "react";

import { ChevronRight, Spinner } from "@/components/icons";
import { Button } from "@/components/controls";
import { request } from "@/ipc/client";
import { cn } from "@/lib/utils";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** A stored text (the handoff note, the briefing), read when the user opens it. */
export function BlobText({ label, hash }: { label: string; hash: string }) {
  const [text, setText] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const toggle = () => {
    if (open) {
      setOpen(false);
      return;
    }
    setOpen(true);
    if (text !== null || loading) return;
    setLoading(true);
    setError(null);
    request({ method: "readBlobText", hash })
      .then((response) => setText(response.text))
      .catch((cause: unknown) => setError(errorText(cause)))
      .finally(() => setLoading(false));
  };
  return (
    <div className="flex flex-col gap-1">
      <Button size="xs" variant="ghost" className="self-start" aria-expanded={open} onClick={toggle}>
        <ChevronRight className={cn("transition-transform", open && "rotate-90")} />
        {label}
        {loading && <Spinner className="animate-spin" />}
      </Button>
      {open && error && (
        <p role="alert" className="text-destructive">
          {error}
        </p>
      )}
      {open && text !== null && (
        <pre className="bg-muted/50 rounded-control max-h-80 overflow-auto p-2 font-mono whitespace-pre-wrap wrap-break-word">
          {text}
        </pre>
      )}
    </div>
  );
}
