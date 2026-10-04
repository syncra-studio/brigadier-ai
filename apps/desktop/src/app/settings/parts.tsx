import { Check, ChevronDown, ChevronRight } from "@openai/apps-sdk-ui/components/Icon";
import { ToggleGroup as ToggleGroupPrimitive } from "radix-ui";
import { useId, type ComponentProps, type ReactNode } from "react";

import { useAction } from "@/app/conversation/useAction";
import { ErrorLine } from "@/app/dialogs/fields";
import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Switch } from "@/components/ui/switch";
import type { Settings } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

/*
 * The parts every Settings page is built from: a page (title, optional description and actions,
 * then its sections), a section (a heading over its cards), a card (rows split by inset
 * hairlines), a row (label and description on the start, the control on the end) and the
 * controls rows use (switch, segmented choice, select, button).
 */

/** A Settings page: its title, then its sections in a centred column. */
export function SettingsPage({
  title,
  description,
  actions,
  wide = false,
  scrollable = true,
  children,
}: {
  title: string;
  description?: ReactNode;
  actions?: ReactNode;
  /** A wider column, for a data page (tables, charts, panes). */
  wide?: boolean;
  /** Disable page scrolling when the content owns its bounded scroll areas. */
  scrollable?: boolean;
  children: ReactNode;
}) {
  return (
    <div data-slot="settings-page" className="flex h-full flex-col">
      {/* The page's toolbar strip, for dragging the window. */}
      <div data-tauri-drag-region className="h-page-toolbar shrink-0" />
      <div
        className={cn(
          "min-h-0 flex-1",
          scrollable ? "overflow-y-auto [scrollbar-gutter:stable]" : "flex flex-col overflow-hidden",
        )}
      >
        <div
          className={cn(
            "mx-auto flex w-full flex-col px-5 pt-4",
            wide ? "max-w-settings-wide" : "max-w-settings",
            scrollable ? "pb-12" : "min-h-0 flex-1",
          )}
        >
          <header className="flex shrink-0 items-start gap-4 py-3">
            <div className="flex min-w-0 flex-1 flex-col gap-1">
              <h1 className="text-page-title font-medium">{title}</h1>
              {description && <p className="text-foreground/65 text-sm">{description}</p>}
            </div>
            {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
          </header>
          <div className={cn("flex flex-col", scrollable ? "gap-10 pt-5" : "min-h-0 flex-1")}>
            {children}
          </div>
        </div>
      </div>
    </div>
  );
}

/** A heading over one or more cards (or any content), with optional actions at its end. */
export function SettingsSection({
  title,
  icon,
  description,
  actions,
  children,
  className,
}: {
  title?: string;
  /** Shown before the title (an archived group's folder). */
  icon?: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  const id = useId();
  return (
    <section aria-labelledby={title ? id : undefined} className={cn("flex flex-col", className)}>
      {(title || actions) && (
        <div className="flex min-h-11.5 items-center justify-between gap-4 pb-1.5">
          <div className="flex min-w-0 flex-1 flex-col gap-0.5">
            {title && (
              <h2 id={id} className="flex min-w-0 items-center gap-2 text-sm font-medium">
                {icon}
                <span className="truncate">{title}</span>
              </h2>
            )}
            {description && <p className="text-foreground/65 text-label">{description}</p>}
          </div>
          {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
        </div>
      )}
      <div className="flex flex-col gap-1.5">{children}</div>
    </section>
  );
}

/**
 * A page's power features, closed until asked for: "Advanced" and a line on what is inside,
 * then its sections.
 */
export function SettingsAdvanced({
  description,
  children,
}: {
  description: string;
  children: ReactNode;
}) {
  return (
    <Collapsible data-slot="settings-advanced">
      <CollapsibleTrigger className="focus-visible:ring-ring/50 group flex w-full items-start gap-2 rounded-xs text-start outline-none focus-visible:ring-2">
        <ChevronRight
          aria-hidden
          className="text-muted-foreground size-icon-sm mt-0.5 shrink-0 transition-transform group-data-[state=open]:rotate-90 motion-reduce:transition-none"
        />
        <span className="flex min-w-0 flex-col gap-0.5">
          <span className="text-sm font-medium">Advanced</span>
          <span className="text-foreground/65 text-label">{description}</span>
        </span>
      </CollapsibleTrigger>
      <CollapsibleContent className="flex flex-col gap-10 pt-6">{children}</CollapsibleContent>
    </Collapsible>
  );
}

/** A card of rows; every row but the last has a hairline under it, inset from the card's edges. */
export function SettingsCard({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div
      data-slot="settings-card"
      className={cn(
        "bg-card border-divider rounded-settings flex flex-col overflow-hidden border",
        "*:not-last:relative *:not-last:after:pointer-events-none *:not-last:after:absolute *:not-last:after:inset-x-4 *:not-last:after:bottom-0 *:not-last:after:h-px *:not-last:after:bg-divider",
        className,
      )}
    >
      {children}
    </div>
  );
}

/**
 * One setting: its label and description on the start, its control on the end. `htmlFor` ties
 * the label to a control with that id; without it, give the control its own label. `error` is
 * what the last change of it failed with.
 */
export function SettingsRow({
  label,
  description,
  htmlFor,
  error,
  children,
  className,
}: {
  label: ReactNode;
  description?: ReactNode;
  htmlFor?: string;
  error?: string | null;
  children?: ReactNode;
  className?: string;
}) {
  return (
    <div
      data-slot="settings-row"
      // Settings search scrolls to the row it found by this.
      data-setting={typeof label === "string" ? label : undefined}
      className={cn("@container flex items-center justify-between gap-6 px-4 py-3", className)}
    >
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        {htmlFor ? (
          <label htmlFor={htmlFor} className="text-label font-medium break-words">
            {label}
          </label>
        ) : (
          <div className="text-label font-medium break-words">{label}</div>
        )}
        {description && (
          <div className="text-foreground/65 text-xs break-words">{description}</div>
        )}
        <ErrorLine error={error ?? null} />
      </div>
      {children && (
        <div className="flex max-w-full min-w-settings-control shrink-0 items-center justify-end gap-2">
          {children}
        </div>
      )}
    </div>
  );
}

/** A setting that is on or off. */
export function SettingsSwitch({
  label,
  checked,
  onCheckedChange,
  disabled,
  id,
}: {
  /** The accessible name, usually the row's label. */
  label: string;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  disabled?: boolean;
  id?: string;
}) {
  return (
    <Switch
      id={id}
      aria-label={label}
      checked={checked}
      disabled={disabled}
      onCheckedChange={onCheckedChange}
    />
  );
}

type BooleanSetting = {
  [K in keyof Settings]: Settings[K] extends boolean ? K : never;
}[keyof Settings];

/** A row for a setting that is on or off, applied at once. */
export function SwitchSetting({
  setting,
  row,
}: {
  setting: BooleanSetting;
  row: { label: string; description: string };
}) {
  const checked = useApp((s) => s.settings[setting]);
  const save = useAction();
  return (
    <SettingsRow label={row.label} description={row.description} error={save.error}>
      <SettingsSwitch
        label={row.label}
        checked={checked}
        onCheckedChange={(on) => save.run(() => setSetting(setting, on))}
      />
    </SettingsRow>
  );
}

export type Choice<T extends string> = { value: T; label: ReactNode; hint?: ReactNode };

/**
 * One of a few choices as pills side by side; the chosen one is filled. With `fill` the pills
 * share the whole width, on a track.
 */
export function Segmented<T extends string>({
  label,
  value,
  options,
  onChange,
  disabled,
  fill = false,
}: {
  label: string;
  value: T;
  options: readonly Choice<T>[];
  onChange: (value: T) => void;
  disabled?: boolean;
  fill?: boolean;
}) {
  return (
    <ToggleGroupPrimitive.Root
      type="single"
      aria-label={label}
      value={value}
      disabled={disabled ?? false}
      // A single choice is always made: pressing the chosen pill again keeps it.
      onValueChange={(next) => next && onChange(next as T)}
      className={cn(
        "flex max-w-full min-w-0 items-center gap-0.5",
        fill && "bg-foreground/5 rounded-capsule w-full p-0.5",
      )}
    >
      {options.map((option) => (
        <ToggleGroupPrimitive.Item
          key={option.value}
          value={option.value}
          className={cn(
            "text-foreground/50 hover:text-foreground data-[state=on]:bg-foreground/5 data-[state=on]:text-foreground focus-visible:ring-ring/50 rounded-capsule text-label h-6 shrink-0 border border-transparent px-2 whitespace-nowrap transition-colors outline-none focus-visible:ring-2 disabled:opacity-50",
            fill && "data-[state=on]:bg-foreground/10 min-w-0 flex-auto truncate",
          )}
        >
          {option.label}
        </ToggleGroupPrimitive.Item>
      ))}
    </ToggleGroupPrimitive.Root>
  );
}

/** The look of a button that opens a menu of choices (a select, the model pickers). */
export const selectTrigger =
  "border-divider bg-foreground/3 hover:bg-foreground/6 data-[state=open]:bg-foreground/6 focus-visible:ring-ring/50 rounded-nav text-label h-7 max-w-full min-w-0 items-center border px-3 transition-colors outline-none focus-visible:ring-2 disabled:opacity-50";

/** One of several choices in a menu, shown as a button with the chosen one's label. */
export function SettingsSelect<T extends string>({
  label,
  value,
  options,
  onChange,
  disabled,
  id,
}: {
  label: string;
  value: T;
  options: readonly Choice<T>[];
  onChange: (value: T) => void;
  disabled?: boolean;
  id?: string;
}) {
  const chosen = options.find((option) => option.value === value);
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild disabled={disabled}>
        <button
          id={id}
          type="button"
          aria-label={label}
          className={cn(selectTrigger, "flex gap-1")}
        >
          <span className="flex min-w-0 flex-1 items-center gap-1.5 truncate">{chosen?.label}</span>
          <ChevronDown aria-hidden className="text-muted-foreground size-icon-sm shrink-0" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="max-w-sm">
        <DropdownMenuRadioGroup value={value} onValueChange={(next) => onChange(next as T)}>
          {options.map((option) => (
            <DropdownMenuRadioItem
              key={option.value}
              value={option.value}
              indicator={<Check className="size-icon-md" />}
              className={option.hint ? "h-auto py-1.5" : undefined}
            >
              <span className="flex min-w-0 flex-col">
                <span>{option.label}</span>
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

/** A row's action button; `destructive` for an action that deletes or removes. */
export function SettingsButton({
  destructive,
  className,
  ...props
}: ComponentProps<typeof Button> & { destructive?: boolean }) {
  return (
    <Button
      type="button"
      variant="outline"
      size="sm"
      className={cn(
        "rounded-nav bg-foreground/5 hover:bg-foreground/10 text-label font-normal",
        destructive && "text-destructive",
        className,
      )}
      {...props}
    />
  );
}
