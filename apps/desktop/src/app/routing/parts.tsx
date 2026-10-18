import { Check, ChevronDown, ChevronRight, ChevronUp, MinusCircle, Upgrade } from "@openai/apps-sdk-ui/components/Icon";
import { ToggleGroup as ToggleGroupPrimitive } from "radix-ui";
import { type ComponentProps, type ReactNode, type SVGProps, useId } from "react";

import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { ProviderKind } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/*
 * The parts Settings › Routing is drawn with, to the user's design: a section (a heading over a
 * card), a card of rows split by hairlines, a row's words, pill selects and choices, badges, the
 * page's buttons and notes. Its palette and type are the text-settings-* and *-settings-* tokens.
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
  const id = useId();
  return (
    <section aria-labelledby={id} className="flex flex-col gap-3.5">
      <div className="flex items-end gap-6">
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <h2 id={id} className="text-settings-heading text-settings-text font-medium">
            {title}
          </h2>
          {description && <p className="text-settings-small text-settings-text-secondary">{description}</p>}
        </div>
        {actions}
      </div>
      {children}
    </section>
  );
}

/** A card of rows; each of its children but the last has a hairline under it. */
/** A divider line drawn inside the top or bottom edge, as the design's strokes are: it adds no height. */
export const HAIRLINE_TOP = "before:bg-settings-divider relative before:absolute before:inset-x-0 before:top-0 before:h-px";
export const HAIRLINE_BOTTOM = "after:bg-settings-divider relative after:absolute after:inset-x-0 after:bottom-0 after:h-px";

export function Card({ children, className, ...props }: ComponentProps<"div">) {
  return (
    <div
      className={cn(
        // Drawn inside, as the design's strokes are: no border adds to a row's height.
        "bg-settings-surface inset-ring-settings-border flex flex-col rounded-xl px-5 inset-ring",
        "*:not-last:after:bg-settings-divider *:not-last:relative *:not-last:after:absolute *:not-last:after:inset-x-0 *:not-last:after:bottom-0 *:not-last:after:h-px",
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
      <span className={cn("text-settings-body text-settings-text truncate", !quiet && "font-medium")}>{title}</span>
      {description && <span className="text-settings-small text-settings-text-secondary truncate">{description}</span>}
    </span>
  );
}

/** A row that opens in place: right when closed, up when open. */
export function OpenChevron({ open }: { open: boolean }) {
  const Icon = open ? ChevronUp : ChevronRight;
  return <Icon aria-hidden className="text-settings-text-tertiary size-3.75 shrink-0" />;
}

/** "Never" in a rule: a circle struck through. */
export function BanIcon({ className }: { className?: string }) {
  return <MinusCircle aria-hidden className={cn("text-settings-danger size-3.75 shrink-0 rotate-45", className)} />;
}

/** "Prefer" in a rule: an arrow up, in blue. */
export function PreferIcon({ className }: { className?: string }) {
  return <Upgrade aria-hidden className={cn("text-settings-link size-3.75 shrink-0", className)} />;
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
      className={cn("shrink-0", provider === "codex" && "text-settings-glyph", className)}
    />
  );
}

/** A small rounded mark: green (Picked now) or blue (New). */
export function Badge({ tone, children }: { tone: "success" | "link"; children: ReactNode }) {
  return (
    <span
      className={cn(
        "text-settings-badge shrink-0 rounded-full px-2 py-0.5 font-medium whitespace-nowrap",
        tone === "success" ? "bg-settings-success-soft text-settings-success" : "bg-settings-link-soft text-settings-link",
      )}
    >
      {children}
    </span>
  );
}

/** The look of a pill that opens a menu. */
export const pillTrigger =
  "bg-settings-control border-settings-control-border text-settings-body text-settings-text inline-flex h-7.5 max-w-full min-w-0 shrink-0 items-center gap-1.5 rounded-full border ps-3.5 pe-3 disabled:opacity-50";

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
      <span className={cn("min-w-0 truncate", placeholder && "text-settings-text-tertiary")}>{children}</span>
      <ChevronDown aria-hidden className="size-3 shrink-0" />
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
export function PillChoice<T extends string>({
  label,
  value,
  options,
  onChange,
  disabled,
}: {
  label: string;
  value: T;
  options: readonly { value: T; label: string }[];
  onChange: (value: T) => void;
  disabled?: boolean;
}) {
  return (
    <ToggleGroupPrimitive.Root
      type="single"
      aria-label={label}
      value={value}
      disabled={disabled ?? false}
      onValueChange={(next) => next && onChange(next as T)}
      className="flex shrink-0 items-center gap-0.5"
    >
      {options.map((option) => (
        <ToggleGroupPrimitive.Item
          key={option.value}
          value={option.value}
          className="text-settings-small text-settings-text-secondary hover:text-settings-text data-[state=on]:bg-settings-chosen data-[state=on]:text-settings-text rounded-full px-3 py-1.25 whitespace-nowrap transition-colors disabled:opacity-50"
        >
          {option.label}
        </ToggleGroupPrimitive.Item>
      ))}
    </ToggleGroupPrimitive.Root>
  );
}

/** A section's action: an outlined pill (Add rule). */
export function OutlineButton({ className, ...props }: ComponentProps<"button">) {
  return (
    <button
      type="button"
      className={cn(
        "bg-settings-surface border-settings-border text-settings-small text-settings-text inline-flex h-7.5 shrink-0 items-center gap-1.5 rounded-full border px-3.5 font-medium disabled:opacity-40 [&_svg]:size-3.25",
        className,
      )}
      {...props}
    />
  );
}

/** The light pill that confirms (Add rule in the form). */
export function LightButton({ className, ...props }: ComponentProps<"button">) {
  return (
    <button
      type="button"
      className={cn(
        "bg-settings-text text-settings-on-light text-settings-small inline-flex h-7.5 shrink-0 items-center rounded-full px-3.5 font-medium disabled:opacity-35",
        className,
      )}
      {...props}
    />
  );
}

/** A quiet pill with no fill (Cancel). */
export function GhostButton({ className, ...props }: ComponentProps<"button">) {
  return (
    <button
      type="button"
      className={cn(
        "text-settings-small text-settings-text-secondary hover:text-settings-text inline-flex h-7.5 shrink-0 items-center rounded-full px-3 font-medium",
        className,
      )}
      {...props}
    />
  );
}

/** Blue words that act (Add model, See strengths, Refresh ratings). */
export function LinkButton({ className, ...props }: ComponentProps<"button">) {
  return (
    <button
      type="button"
      className={cn("text-settings-link inline-flex items-center hover:underline disabled:opacity-50", className)}
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
        "flex gap-2 rounded-lg px-3 py-2.5 inset-ring [&>svg]:mt-0.5 [&>svg]:size-3.5 [&>svg]:shrink-0",
        tone === "info"
          ? "bg-settings-inset inset-ring-settings-border text-settings-text-secondary [&>svg]:text-settings-text-secondary"
          : "bg-settings-warning-soft inset-ring-settings-warning-edge text-settings-warning-text [&>svg]:text-settings-warning",
      )}
    >
      {icon}
      <p className="text-settings-small min-w-0 flex-1 leading-[1.45]">{children}</p>
    </div>
  );
}

/** A numbered step's number, in a ring. */
export function StepNumber({ children }: { children: ReactNode }) {
  return (
    <span className="border-settings-ring text-settings-badge text-settings-text-secondary flex size-5.5 shrink-0 items-center justify-center rounded-full border font-mono">
      {children}
    </span>
  );
}

