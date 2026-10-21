import { useId, type ReactNode } from "react";

import { useAction } from "@/app/conversation/useAction";
import { BackLink } from "@/app/settings/BackLink";
import { ScrollFade } from "@/components/layout";
import { Switch } from "@/components/controls";
import { SettingsRow } from "@/components/shared";
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
          {back && <BackLink label={back.label} onBack={back.onBack} />}
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
