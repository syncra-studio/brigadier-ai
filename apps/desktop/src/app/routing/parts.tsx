import { Check, ChevronDown, ChevronRight, ChevronUp, MinusCircle, Upgrade } from "@openai/apps-sdk-ui/components/Icon";
import type { ComponentProps, ReactNode, SVGProps } from "react";

import { selectTrigger, SettingsButton, settingsCard, SettingsSection } from "@/app/settings/parts";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import { Badge as ChipBadge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Segmented } from "@/components/ui/segmented";
import type { ProviderKind } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/*
 * The parts Settings › Routing is built from, laid out as the user's design has it and drawn with
 * every Settings page's parts: a section (a heading over a card), a card of rows split by
 * hairlines, a row's words, selects and choices, badges, the page's buttons and notes.
 */

/** A section: its title and line, an action at its end, then its card. */
export function Section({
  title,
  description,
  actions,
  children,
}: {
  title: string;
  description?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
}) {
  return (
    <SettingsSection title={title} description={description} actions={actions}>
      {children}
    </SettingsSection>
  );
}

/** A card of rows; each of its children but the last has a hairline under it. */
/** A divider line drawn inside the top or bottom edge, as the design's strokes are: it adds no height. */
export const HAIRLINE_TOP = "before:bg-divider relative before:absolute before:inset-x-0 before:top-0 before:h-px";
export const HAIRLINE_BOTTOM = "after:bg-divider relative after:absolute after:inset-x-0 after:bottom-0 after:h-px";

export function Card({ children, className, ...props }: ComponentProps<"div">) {
  return (
    <div
      className={cn(
        // Every Settings page's card; its rows keep their text 16 in, its hairlines inset as far.
        settingsCard,
        "px-4",
        "*:not-last:after:bg-divider *:not-last:relative *:not-last:after:absolute *:not-last:after:inset-x-0 *:not-last:after:bottom-0 *:not-last:after:h-px",
        className,
      )}
      {...props}
    >
      {children}
    </div>
  );
}

/** A row's title and line under it. `quiet`: a sub-row's title, not bold. */
export function RowText({
  title,
  description,
  quiet = false,
}: {
  title: ReactNode;
  description?: ReactNode;
  quiet?: boolean;
}) {
  return (
    <span className="flex min-w-0 flex-1 flex-col gap-1">
      <span className={cn("text-label truncate", !quiet && "font-medium")}>{title}</span>
      {description && <span className="text-foreground/65 truncate text-xs">{description}</span>}
    </span>
  );
}

/** A row that opens in place: right when closed, up when open. */
export function OpenChevron({ open }: { open: boolean }) {
  const Icon = open ? ChevronUp : ChevronRight;
  return <Icon aria-hidden className="text-muted-foreground size-icon-sm shrink-0" />;
}

/** "Never" in a rule: a circle struck through. */
export function BanIcon({ className }: { className?: string }) {
  return <MinusCircle aria-hidden className={cn("text-destructive size-icon-sm shrink-0 rotate-45", className)} />;
}

/** "Prefer" in a rule: an arrow up, in blue. */
export function PreferIcon({ className }: { className?: string }) {
  return <Upgrade aria-hidden className={cn("text-link size-icon-sm shrink-0", className)} />;
}

/** The handle a row is dragged by: two columns of three dots. */
/** A stroked 24px line icon, for the design's glyphs the app's icon set lacks. */
function LineIcon({ paths, className, ...rest }: SVGProps<SVGSVGElement> & { paths: string[] }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={cn("stroke-2", className)}
      {...rest}
    >
      {paths.map((path) => (
        <path key={path} d={path} />
      ))}
    </svg>
  );
}

/** A commit on a line: Write commit messages. */
export function CommitIcon(props: SVGProps<SVGSVGElement>) {
  return <LineIcon paths={["M15 12a3 3 0 1 1-6 0a3 3 0 1 1 6 0", "M3 12h6", "M15 12h6"]} {...props} />;
}

/** A numbered list: How a model is picked. */
export function ListOrderedIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <LineIcon paths={["M10 6h11", "M10 12h11", "M10 18h11", "M4 6h1v4", "M4 10h2", "M6 18H4c0-1 2-2 2-3s-1-1.5-2-1"]} {...props} />
  );
}

export function GripIcon({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden className={cn("size-3.5 shrink-0", className)}>
      {[5, 12, 19].map((y) => (
        <g key={y}>
          <circle cx="9" cy={y} r="1.6" />
          <circle cx="15" cy={y} r="1.6" />
        </g>
      ))}
    </svg>
  );
}

/** A model's agent mark, Codex's in the page's light grey. */
export function Glyph({ provider, className }: { provider: ProviderKind; className?: string }) {
  return (
    <ProviderGlyph
      provider={provider}
      className={cn("shrink-0", provider === "codex" && "text-foreground/85", className)}
    />
  );
}

/** A small rounded mark: green (Picked now) or a plain one (New). */
export function Badge({ tone, children }: { tone: "success" | "link"; children: ReactNode }) {
  return (
    <ChipBadge variant={tone === "success" ? "success" : "secondary"} className="shrink-0">
      {children}
    </ChipBadge>
  );
}

/** The look of a pill that opens a menu. */
export const pillTrigger = cn(selectTrigger, "inline-flex shrink-0 gap-1");

/** A pill that opens a menu: its value (or a quiet placeholder) and a chevron. */
export function PillTrigger({
  label,
  placeholder = false,
  children,
  className,
  ...props
}: { label: string; placeholder?: boolean; children: ReactNode } & Omit<ComponentProps<"button">, "children">) {
  return (
    <button type="button" aria-label={label} className={cn(pillTrigger, className)} {...props}>
      <span className={cn("min-w-0 truncate", placeholder && "text-muted-foreground")}>{children}</span>
      <ChevronDown aria-hidden className="text-muted-foreground size-icon-sm shrink-0" />
    </button>
  );
}

/** One of a few choices, as a pill with a menu. */
export function PillSelect<T extends string>({
  label,
  value,
  options,
  onChange,
  disabled,
  align = "start",
}: {
  label: string;
  value: T;
  options: readonly { value: T; label: string; shown?: string }[];
  onChange: (value: T) => void;
  disabled?: boolean;
  align?: "start" | "end";
}) {
  const chosen = options.find((option) => option.value === value);
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild disabled={disabled}>
        <PillTrigger label={label}>{chosen?.shown ?? chosen?.label}</PillTrigger>
      </DropdownMenuTrigger>
      <DropdownMenuContent align={align} className="max-h-96 overflow-y-auto">
        <DropdownMenuRadioGroup value={value} onValueChange={(next) => onChange(next as T)}>
          {options.map((option) => (
            <DropdownMenuRadioItem key={option.value} value={option.value} indicator={<Check className="size-icon-md" />}>
              {option.label}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** Two or three choices side by side; the chosen one is a filled pill. */
export function PillChoice<T extends string>(props: {
  label: string;
  value: T;
  options: readonly { value: T; label: string }[];
  onChange: (value: T) => void;
  disabled?: boolean;
}) {
  return <Segmented {...props} />;
}

/** A section's action: an outlined pill (Add rule). */
export function OutlineButton(props: ComponentProps<"button">) {
  return <SettingsButton {...props} />;
}

/** The button that confirms (Add rule in the form). */
export function LightButton({ className, ...props }: ComponentProps<"button">) {
  return <Button type="button" size="sm" className={className} {...props} />;
}

/** A quiet button with no fill (Cancel). */
export function GhostButton({ className, ...props }: ComponentProps<"button">) {
  return <Button type="button" variant="ghost" size="sm" className={className} {...props} />;
}

/** Blue words that act (Add model, See strengths, Refresh ratings). */
export function LinkButton({ className, ...props }: ComponentProps<"button">) {
  return (
    <button
      type="button"
      className={cn("text-link inline-flex items-center hover:underline disabled:opacity-50", className)}
      {...props}
    />
  );
}

/** A note inset in a card: what a change would do, or (warning) why it would do nothing. */
export function Note({ tone, icon, children }: { tone: "info" | "warning"; icon: ReactNode; children: ReactNode }) {
  return (
    <div
      role={tone === "warning" ? "alert" : "status"}
      className={cn(
        "rounded-control flex gap-2 px-3 py-2.5 [&>svg]:mt-0.5 [&>svg]:size-icon-sm [&>svg]:shrink-0",
        tone === "info" ? "bg-foreground/5 text-foreground/65" : "bg-warning/10 text-warning",
      )}
    >
      {icon}
      <p className="min-w-0 flex-1 text-xs">{children}</p>
    </div>
  );
}

/** A numbered step's number, in a ring. */
export function StepNumber({ children }: { children: ReactNode }) {
  return (
    <span className="border-divider text-muted-foreground text-2xs flex size-5.5 shrink-0 items-center justify-center rounded-full border font-mono">
      {children}
    </span>
  );
}

