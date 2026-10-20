import { useId, type ComponentProps, type KeyboardEvent, type ReactNode } from "react";

import { useAction } from "@/app/conversation/useAction";
import { ErrorLine } from "@/app/dialogs/fields";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Check,
  ChevronDown,
  ChevronLeft,
  MagnifyingGlassSearch,
  XCircleFilled,
} from "@/components/icons";
import { ScrollFade } from "@/components/ui/scroll-fade";
import type { Choice } from "@/components/ui/segmented";
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
  mark,
  actions,
  back,
  wide = false,
  scrollable = true,
  children,
}: {
  /** Its name; a node when something sits beside it (an agent's plan). */
  title: ReactNode;
  description?: ReactNode;
  /** Beside the title and its line, before them (an agent's logo). */
  mark?: ReactNode;
  actions?: ReactNode;
  /** A page inside another: its name over the title goes back to it. */
  back?: { label: string; onBack: () => void };
  /** A wider column, for a data page (tables, charts, panes). */
  wide?: boolean;
  /** Disable page scrolling when the content owns its bounded scroll areas. */
  scrollable?: boolean;
  children: ReactNode;
}) {
  return (
    <div data-slot="settings-page" className="relative flex h-full flex-col">
      <div
        className={cn(
          "min-h-0 flex-1",
          scrollable ? "overflow-y-auto [scrollbar-gutter:stable]" : "flex flex-col overflow-hidden",
        )}
      >
        {/* The band at the page's top, under the titlebar and over the title, for dragging the
            window. It scrolls away with the page: lying over it, it would take the clicks on
            whatever scrolls under it. */}
        <div data-tauri-drag-region className="h-settings-band shrink-0" />
        <div
          className={cn(
            "mx-auto flex w-full flex-col px-5 pt-4",
            wide ? "max-w-settings-wide" : "max-w-settings",
            scrollable ? "pb-12" : "min-h-0 flex-1",
          )}
        >
          {back && (
            <button
              type="button"
              onClick={back.onBack}
              className="text-foreground/65 hover:text-foreground text-label -ms-1 flex w-fit items-center gap-0.5"
            >
              <ChevronLeft aria-hidden className="size-icon-sm" />
              {back.label}
            </button>
          )}
          <header className="flex shrink-0 items-start gap-4 py-3">
            <div className="flex min-w-0 flex-1 items-center gap-3.5">
              {mark}
              <div className="flex min-w-0 flex-1 flex-col gap-1">
                <h1 className="text-page-title font-medium">{title}</h1>
                {description && <p className="text-foreground/65 text-sm">{description}</p>}
              </div>
            </div>
            {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
          </header>
          <div className={cn("flex flex-col", scrollable ? "gap-10 pt-5" : "min-h-0 flex-1")}>
            {children}
          </div>
        </div>
      </div>
      {/* What scrolls fades out at the view's edges, as the conversation does. */}
      {scrollable && (
        <>
          <ScrollFade edge="top" />
          <ScrollFade edge="bottom" />
        </>
      )}
    </div>
  );
}

/** A heading over one or more cards (or any content), with optional actions at its end. */
export function SettingsSection({
  title,
  icon,
  description,
  aside,
  actions,
  setting,
  children,
  className,
}: {
  title?: string;
  /** Shown before the title (an archived group's folder). */
  icon?: ReactNode;
  description?: ReactNode;
  /** A grey note at the end ("Largest first", "Checked 3m ago"), before any actions. */
  aside?: ReactNode;
  actions?: ReactNode;
  /** The name Settings search finds the section by. */
  setting?: string;
  children: ReactNode;
  className?: string;
}) {
  const id = useId();
  return (
    <section
      aria-labelledby={title ? id : undefined}
      data-setting={setting}
      className={cn("flex flex-col", className)}
    >
      {(title || aside || actions) && (
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
          {aside && <div className="text-foreground/65 text-label flex items-center gap-1">{aside}</div>}
          {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
        </div>
      )}
      <div className="flex flex-col gap-1.5">{children}</div>
    </section>

  );
}

/**
 * One setting: its label and description on the start, its control on the end. `htmlFor` ties
 * the label to a control with that id; without it, give the control its own label. `error` is
 * what the last change of it failed with. `setting` is the name search and links find it by,
 * for a label that isn't plain text or changes as it is used.
 */
export function SettingsRow({
  label,
  setting,
  description,
  htmlFor,
  error,
  children,
  className,
}: {
  label: ReactNode;
  setting?: string;
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
      data-setting={setting ?? (typeof label === "string" ? label : undefined)}
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
  /** The accessible name, usually the row's label. */
  label: string;
  icon?: ReactNode;
  placeholder?: boolean;
  children: ReactNode;
} & Omit<ComponentProps<"button">, "children">) {
  return (
    <button
      type="button"
      aria-label={label}
      {...props}
      className={cn(selectTrigger, "inline-flex shrink-0", icon ? "gap-2" : "gap-1", className)}
    >
      {icon}
      <span className={cn("min-w-0 flex-1 truncate text-start", placeholder && "text-muted-foreground")}>
        {children}
      </span>
      <ChevronDown aria-hidden className="text-muted-foreground size-icon-sm shrink-0" />
    </button>
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
        <SettingsSelectTrigger id={id} label={label} icon={icon} className={className}>
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
              className={option.hint ? "h-auto py-1.5" : undefined}
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

/** A search field over a list; Esc empties it before it leaves Settings. */
export function SettingsSearch({
  label,
  value,
  onChange,
  onKeyDown,
  autoFocus,
  className,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  /** Keys after its own (Esc empties it). */
  onKeyDown?: (event: KeyboardEvent<HTMLInputElement>) => void;
  autoFocus?: boolean;
  className?: string;
}) {
  return (
    <label
      className={cn(
        "rounded-capsule border-foreground/16 bg-foreground/8 flex h-8 min-w-0 flex-1 items-center gap-2 border px-2.5",
        className,
      )}
    >
      <MagnifyingGlassSearch aria-hidden className="text-foreground/65 size-icon-md shrink-0" />
      <input
        type="search"
        value={value}
        placeholder={label}
        aria-label={label}
        spellCheck={false}
        autoComplete="off"
        className="placeholder:text-foreground/50 min-w-0 flex-1 bg-transparent text-sm outline-none [&::-webkit-search-cancel-button]:hidden"
        onChange={(event) => onChange(event.target.value)}
        autoFocus={autoFocus}
        onKeyDown={(event) => {
          if (event.key === "Escape" && value) {
            event.stopPropagation();
            onChange("");
            return;
          }
          onKeyDown?.(event);
        }}
      />
      {value && (
        <button
          type="button"
          aria-label="Clear search"
          className="text-foreground/50 hover:text-foreground flex shrink-0 items-center"
          onClick={() => onChange("")}
        >
          <XCircleFilled className="size-icon-sm" />
        </button>
      )}
    </label>
  );
}
