import { type ComponentProps, type ReactNode, useId } from "react";

import { PickerButton } from "@/components/controls/picker-button";
import type { Choice } from "@/components/controls/segmented";
import { Check, ChevronDown } from "@/components/icons";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/overlays/dropdown-menu";
import { cn } from "@/lib/utils";

/** The look of a pill that opens a menu of choices (a select, the model pickers). */
export const selectTrigger =
  "border-divider bg-foreground/3 hover:bg-foreground/6 data-[state=open]:bg-foreground/6 rounded-capsule text-label h-7 max-w-full min-w-0 items-center border px-3 transition-colors disabled:opacity-50";

/**
 * A select's pill on its own, for a menu of other contents (put it in `DropdownMenuTrigger
 * asChild`): an optional icon, its value (grey while it is a `placeholder`), a chevron.
 */
export function SettingsSelectTrigger({
  label,
  icon,
  placeholder = false,
  className,
  children,
  ...props
}: {
  /** The accessible name, usually the row's label; the value it shows follows it. */
  label: string;
  icon?: ReactNode;
  placeholder?: boolean;
  children: ReactNode;
} & Omit<ComponentProps<"button">, "children" | "aria-label" | "aria-labelledby">) {
  const value = useId();
  return (
    <PickerButton
      label={label}
      shown={[value]}
      {...props}
      className={cn(selectTrigger, "inline-flex shrink-0", icon ? "gap-2" : "gap-1", className)}
    >
      {icon}
      <span id={value} className={cn("min-w-0 flex-1 truncate text-start", placeholder && "text-muted-foreground")}>
        {children}
      </span>
      <ChevronDown aria-hidden className="text-muted-foreground size-icon-sm shrink-0" />
    </PickerButton>
  );
}
/**
 * One of several choices in a menu, shown as a pill with the chosen one's label (or its
 * `shown` words). `className` sizes the pill; the menu is at least as wide.
 */
export function SettingsSelect<T extends string>({
  label,
  value,
  options,
  onChange,
  disabled,
  id,
  icon,
  align = "end",
  className,
}: {
  label: string;
  value: T;
  options: readonly (Choice<T> & { shown?: ReactNode })[];
  onChange: (value: T) => void;
  disabled?: boolean;
  id?: string;
  icon?: ReactNode;
  align?: "start" | "end";
  className?: string;
}) {
  const chosen = options.find((option) => option.value === value);
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild disabled={disabled}>
        <SettingsSelectTrigger {...(id && { id })} label={label} icon={icon} className={className}>
          {chosen?.shown ?? chosen?.label}
        </SettingsSelectTrigger>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align={align}
        className="max-h-96 max-w-sm min-w-(--radix-dropdown-menu-trigger-width) overflow-y-auto"
      >
        <DropdownMenuRadioGroup value={value} onValueChange={(next) => onChange(next as T)}>
          {options.map((option) => (
            <DropdownMenuRadioItem
              key={option.value}
              value={option.value}
              indicator={<Check className="size-icon-md" />}
              className={option.hint ? "h-auto py-2" : undefined}
            >
              <span className="flex min-w-0 flex-col">
                <span className="truncate">{option.label}</span>
                {option.hint && (
                  <span className="text-muted-foreground text-xs whitespace-normal">{option.hint}</span>
                )}
              </span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
