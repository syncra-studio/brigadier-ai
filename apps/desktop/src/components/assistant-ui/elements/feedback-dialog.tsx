import { ArrowUpRight, Check } from "@openai/apps-sdk-ui/components/Icon";
import type { ComponentProps, ReactNode } from "react";

import { field, paper } from "@/components/assistant-ui/elements/surfaces";
import { FeedbackGlyph } from "@/components/glyphs/feedback-glyph";
import { cn } from "@/lib/utils";

/**
 * Feedback as a short form: pick what it is, write it, send it. Adapted from assistant-ui's
 * Feedback dialog element (https://www.assistant-ui.com/elements/feedback-dialog): one choice
 * of kind instead of several reasons, the text the main field rather than an optional note,
 * the details that go with it shown under it, and Brigadier's tokens and icons. Controlled:
 * the caller owns the kind, the text, whether it was sent and what sending does.
 */
export function FeedbackDialog<Kind extends string>({
  heading,
  kinds,
  kind,
  note,
  placeholder,
  details,
  sent,
  sentText,
  submitLabel,
  error,
  onKindChange,
  onNoteChange,
  onSubmit,
  className,
  ...props
}: Omit<ComponentProps<"div">, "children" | "onSubmit"> & {
  /** The form's title (a dialog passes its DialogTitle). */
  heading: ReactNode;
  kinds: readonly { value: Kind; label: string }[];
  kind: Kind;
  note: string;
  placeholder: string;
  /** What goes with the text, shown under it. */
  details?: string | undefined;
  sent: boolean;
  /** Said once it is sent. */
  sentText: string;
  submitLabel: string;
  error?: ReactNode;
  onKindChange: (kind: Kind) => void;
  onNoteChange: (note: string) => void;
  onSubmit: () => void;
}) {
  return (
    <div
      data-slot="feedback-dialog"
      className={cn(
        paper,
        "rounded-question flex w-full",
        sent ? "items-center gap-2.5 text-sm" : "flex-col gap-4",
        className,
      )}
      {...props}
    >
      {/*
        Mounted whether or not it was sent: a live region announces only a change that comes
        after it is in the tree.
      */}
      <div role="status" className={sent ? "flex items-center gap-2.5" : "sr-only"}>
        {sent && (
          <>
            <Check className="text-success size-icon-md shrink-0" />
            {sentText}
          </>
        )}
      </div>

      {sent ? null : (
        <>
          <div className="flex items-center gap-2.5">
            <span className="bg-foreground/5 text-muted-foreground rounded-control flex size-icon-button-md shrink-0 items-center justify-center">
              <FeedbackGlyph className="size-icon-sm" />
            </span>
            {heading}
          </div>

          <div role="radiogroup" aria-label="Kind" className="flex flex-wrap gap-1.5">
            {kinds.map((choice) => {
              const active = choice.value === kind;
              return (
                <button
                  key={choice.value}
                  type="button"
                  role="radio"
                  aria-checked={active}
                  data-state={active ? "on" : "off"}
                  onClick={() => onKindChange(choice.value)}
                  className={cn(
                    "rounded-capsule h-control-sm px-3 text-sm transition-colors active:scale-95 motion-reduce:transition-none",
                    active ? "bg-foreground text-background" : cn(field, "text-muted-foreground hover:text-foreground"),
                  )}
                >
                  {choice.label}
                </button>
              );
            })}
          </div>

          <div className="grid gap-2">
            <textarea
              value={note}
              onChange={(event) => onNoteChange(event.target.value)}
              rows={5}
              spellCheck
              autoFocus
              placeholder={placeholder}
              aria-label={placeholder}
              className={cn(
                field,
                "text-foreground placeholder:text-muted-foreground focus-visible:ring-foreground/20 rounded-settings min-h-28 resize-none px-3 py-2.5 text-sm outline-none focus-visible:ring-1",
              )}
            />
            {details && <p className="text-foreground/50 text-xs">Adds {details}</p>}
          </div>

          {error}

          <button
            type="button"
            disabled={!note.trim()}
            onClick={onSubmit}
            className="bg-foreground text-background rounded-capsule h-control-md flex items-center gap-1.5 self-end px-4 text-sm font-medium transition-opacity hover:opacity-90 active:scale-95 disabled:pointer-events-none disabled:opacity-40 motion-reduce:transition-none"
          >
            {submitLabel}
            <ArrowUpRight className="size-icon-sm" />
          </button>
        </>
      )}
    </div>
  );
}
