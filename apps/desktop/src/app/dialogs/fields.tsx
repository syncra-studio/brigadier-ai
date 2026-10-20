import { Check, FolderOpen } from "@openai/apps-sdk-ui/components/Icon";
import { Checkbox as CheckboxPrimitive } from "radix-ui";
import { useId, useState, type ReactNode } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { pickFolder } from "@/ipc/client";
import { cn } from "@/lib/utils";

export function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** A labelled form row. */
export function Field({
  label,
  hint,
  htmlFor,
  children,
}: {
  label: string;
  hint?: ReactNode;
  htmlFor?: string;
  children: ReactNode;
}) {
  return (
    <div className="grid gap-1.5">
      <label htmlFor={htmlFor} className="text-muted-foreground text-xs">
        {label}
      </label>
      {children}
      {hint && <p className="text-muted-foreground text-xs">{hint}</p>}
    </div>
  );
}

/** An error the daemon (or validation) reported, shown where it happened. */
export function ErrorLine({ error }: { error: string | null }) {
  if (!error) return null;
  return (
    <p role="alert" className="text-destructive text-xs whitespace-pre-wrap">
      {error}
    </p>
  );
}

/**
 * A folder path: the native folder picker, plus the typed path it fills in, so a folder can
 * also be pasted or scripted.
 */
export function FolderField({
  id,
  value,
  onChange,
  placeholder = "/path/to/repository",
  autoFocus,
}: {
  id?: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  autoFocus?: boolean;
}) {
  const [error, setError] = useState<string | null>(null);
  return (
    <div className="grid gap-1">
      <div className="flex gap-2">
        <Input
          id={id}
          value={value}
          autoFocus={autoFocus}
          spellCheck={false}
          autoComplete="off"
          placeholder={placeholder}
          className="font-mono text-xs"
          aria-label="Repository path"
          onChange={(event) => onChange(event.target.value)}
        />
        <Button
          type="button"
          variant="outline"
          onClick={() => {
            setError(null);
            pickFolder(value.trim() || undefined)
              .then((picked) => {
                if (picked) onChange(picked);
              })
              .catch((cause: unknown) => setError(errorText(cause)));
          }}
        >
          <FolderOpen />
          Choose…
        </Button>
      </div>
      <ErrorLine error={error} />
    </div>
  );
}

/** A checkbox with its label; `note` explains why it is disabled. */
export function CheckboxRow({
  label,
  note,
  checked,
  disabled,
  onCheckedChange,
}: {
  label: string;
  note?: ReactNode;
  checked: boolean;
  disabled?: boolean;
  onCheckedChange: (checked: boolean) => void;
}) {
  const id = useId();
  return (
    <div className={cn("flex items-start gap-2.5", disabled && "opacity-60")}>
      <CheckboxPrimitive.Root
        id={id}
        checked={checked}
        disabled={disabled}
        onCheckedChange={(state) => onCheckedChange(state === true)}
        className="border-input data-[state=checked]:bg-primary data-[state=checked]:border-primary data-[state=checked]:text-primary-foreground rounded-xs mt-0.5 flex size-icon-md shrink-0 items-center justify-center border disabled:cursor-not-allowed"
      >
        <CheckboxPrimitive.Indicator>
          <Check className="size-icon-xs" />
        </CheckboxPrimitive.Indicator>
      </CheckboxPrimitive.Root>
      <label htmlFor={id} className="grid gap-0.5">
        <span className="text-sm">{label}</span>
        {note && <span className="text-muted-foreground text-xs">{note}</span>}
      </label>
    </div>
  );
}
