import {
  type ComponentProps,
  type KeyboardEvent,
  type ReactNode,
  type Ref,
  useImperativeHandle,
  useRef,
} from "react";

import { MagnifyingGlassSearch, XCircleFilled } from "@/components/icons";
import { cn } from "@/lib/utils";
import { PLAIN_FIELD } from "@/lib/plainTyping";

const LOOKS = {
  // The Settings look: a filled capsule with a hairline, taking the row's free width.
  pill: {
    box: "rounded-capsule border-foreground/16 bg-foreground/8 h-8 flex-1 gap-2 px-2.5",
    icon: "text-foreground/65",
    placeholder: "placeholder:text-foreground/50",
  },
  // In panels and popovers: an outlined box, ringed while typing.
  bordered: {
    box: "rounded-control border-border focus-within:border-ring h-control-md gap-1.5 px-2",
    icon: "text-muted-foreground",
    placeholder: "placeholder:text-muted-foreground",
  },
} as const;

/**
 * A search field: a magnifier, the input and a clear button. Esc empties it before anything
 * else sees the key, unless `clearOnEscape` is off (a picker's field, where Esc closes the
 * picker). Trailing `children` sit after the input; the field is then a plain box, not a label,
 * so their names stay their own.
 */
export function SettingsSearch({
  label,
  value,
  onChange,
  onKeyDown,
  variant = "pill",
  icon,
  placeholder = label,
  clearable = true,
  clearOnEscape = true,
  recording = false,
  display,
  className,
  children,
  ref,
  ...input
}: Omit<ComponentProps<"input">, "value" | "onChange" | "children"> & {
  /** The input's name; its placeholder too unless `placeholder` says otherwise. */
  label: string;
  value: string;
  onChange: (value: string) => void;
  /** Keys after its own (Esc empties it). */
  onKeyDown?: (event: KeyboardEvent<HTMLInputElement>) => void;
  variant?: keyof typeof LOOKS;
  /** In place of the magnifier; `null` for none. */
  icon?: ReactNode;
  /** A clear button while there is something to clear. */
  clearable?: boolean;
  clearOnEscape?: boolean;
  /** Ringed while it listens for keys (Shortcuts' search by keystrokes). */
  recording?: boolean;
  /** Shown in place of the input, e.g. the keys being searched for. */
  display?: ReactNode;
  children?: ReactNode;
  ref?: Ref<HTMLInputElement | null>;
}) {
  const field = useRef<HTMLInputElement>(null);
  useImperativeHandle(ref, () => field.current as HTMLInputElement, []);
  const Box = children ? "div" : "label";
  return (
    <Box
      data-slot="settings-search"
      className={cn(
        "flex min-w-0 items-center border",
        LOOKS[variant].box,
        recording && "border-ring",
        className,
      )}
    >
      {icon === undefined ? (
        <MagnifyingGlassSearch
          aria-hidden
          className={cn("size-icon-md shrink-0", LOOKS[variant].icon)}
        />
      ) : (
        icon
      )}
      {display ?? (
        <input
          ref={field}
          type={clearOnEscape ? "search" : "text"}
          value={value}
          placeholder={placeholder}
          aria-label={label}
          {...PLAIN_FIELD}
          className={cn(
            "min-w-0 flex-1 bg-transparent text-sm outline-none [&::-webkit-search-cancel-button]:hidden",
            LOOKS[variant].placeholder,
          )}
          onChange={(event) => onChange(event.target.value)}
          onKeyDown={(event) => {
            if (clearOnEscape && event.key === "Escape" && value) {
              event.stopPropagation();
              onChange("");
              return;
            }
            onKeyDown?.(event);
          }}
          {...input}
        />
      )}
      {clearable && (value || display) && !recording && (
        <button
          type="button"
          aria-label="Clear search"
          className="text-foreground/50 hover:text-foreground flex shrink-0 items-center"
          onClick={() => {
            onChange("");
            requestAnimationFrame(() => field.current?.focus());
          }}
        >
          <XCircleFilled className="size-icon-sm" />
        </button>
      )}
      {children}
    </Box>
  );
}
