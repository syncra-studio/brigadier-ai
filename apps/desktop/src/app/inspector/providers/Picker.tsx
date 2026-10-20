import { Button } from "@/components/controls";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/overlays";
import { ChevronDown } from "@/components/icons";
import { cn } from "@/lib/utils";

export type PickerOption = { value: string; label: string; hint?: string };

/** A compact single-choice menu: the trigger shows the current choice. */
export function Picker({
  label,
  value,
  options,
  onChange,
  disabled,
  placeholder = "—",
  className,
}: {
  label: string;
  value: string;
  options: readonly PickerOption[];
  onChange: (value: string) => void;
  disabled?: boolean;
  /** Shown while no option matches the value. */
  placeholder?: string;
  className?: string;
}) {
  const current = options.find((option) => option.value === value);
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="outline"
          size="xs"
          aria-label={label}
          disabled={disabled || options.length === 0}
          className={cn("min-w-0 justify-between", className)}
        >
          <span className="truncate">{current?.label ?? placeholder}</span>
          <ChevronDown />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="max-w-sm">
        <DropdownMenuRadioGroup value={value} onValueChange={onChange}>
          {options.map((option) => (
            <DropdownMenuRadioItem
              key={option.value}
              value={option.value}
              className={cn("text-xs", option.hint && "h-auto py-1.5")}
            >
              <span className="flex min-w-0 flex-col">
                <span className="truncate">{option.label}</span>
                {option.hint && (
                  <span className="text-muted-foreground truncate">{option.hint}</span>
                )}
              </span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
