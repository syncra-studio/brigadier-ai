import type * as React from "react";
import { useRef } from "react";
import { DropdownMenu as DropdownMenuPrimitive } from "radix-ui";

import { Check, ChevronRight, Dot } from "@/components/icons";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";

function DropdownMenu({
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Root>) {
  return <DropdownMenuPrimitive.Root data-slot="dropdown-menu" {...props} />;
}

function DropdownMenuPortal({
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Portal>) {
  return (
    <DropdownMenuPrimitive.Portal data-slot="dropdown-menu-portal" {...props} />
  );
}

/**
 * Opens on a pointer press or a key, as Radix's trigger does, and on a click no pointer press
 * came before: an accessibility press (VoiceOver's, the computer-use helper's) is such a click,
 * and Radix alone would leave the menu shut (#564).
 */
function DropdownMenuTrigger({
  onPointerDown,
  onClick,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Trigger>) {
  const pointer = useRef(false);
  return (
    <DropdownMenuPrimitive.Trigger
      data-slot="dropdown-menu-trigger"
      onPointerDown={(event) => {
        pointer.current = true;
        onPointerDown?.(event);
      }}
      onClick={(event) => {
        onClick?.(event);
        const pressed = pointer.current;
        pointer.current = false;
        if (pressed || event.defaultPrevented) return;
        // Pressed as a pointer would: Radix opens (or shuts) it, and an open menu elsewhere shuts.
        event.currentTarget.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, cancelable: true, button: 0 }));
        pointer.current = false;
      }}
      {...props}
    />
  );
}

function DropdownMenuContent({
  className,
  // One spacing step from the trigger.
  sideOffset = tokenPx("--spacing"),
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Content>) {
  return (
    <DropdownMenuPrimitive.Portal>
      <DropdownMenuPrimitive.Content
        data-slot="dropdown-menu-content"
        sideOffset={sideOffset}
        className={cn(
          menuSurface,
          "max-h-(--radix-dropdown-menu-content-available-height) origin-(--radix-dropdown-menu-content-transform-origin) overflow-x-hidden overflow-y-auto duration-100",
          className,
        )}
        {...props}
      />
    </DropdownMenuPrimitive.Portal>
  );
}

function DropdownMenuGroup({
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Group>) {
  return (
    <DropdownMenuPrimitive.Group data-slot="dropdown-menu-group" {...props} />
  );
}

/** A menu's surface; the context menu shares it. */
export const menuSurface =
  "data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 bg-popover text-popover-foreground data-[state=closed]:animate-out data-[state=open]:animate-in ring-foreground/10 min-w-menu rounded-menu shadow-menu z-50 p-1 ring-1";

/** A menu row's destructive variant and icon colour; the context menu shares them. */
export const itemVariants =
  "data-[variant=destructive]:text-destructive data-[variant=destructive]:focus:bg-destructive/20 data-[variant=destructive]:focus:text-destructive [&_svg:not([class*='text-'])]:text-muted-foreground data-[variant=destructive]:*:[svg]:text-destructive! data-[inset]:ps-8";

/** A menu row; the context menu shares it. */
export const itemBase =
  "focus:bg-foreground/8 h-control-sm rounded-capsule relative flex cursor-default items-center gap-2 px-2 text-sm outline-hidden select-none data-[disabled]:pointer-events-none data-[disabled]:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-icon-md";

function DropdownMenuItem({
  className,
  inset,
  variant = "default",
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Item> & {
  inset?: boolean;
  variant?: "default" | "destructive";
}) {
  return (
    <DropdownMenuPrimitive.Item
      data-slot="dropdown-menu-item"
      data-inset={inset}
      data-variant={variant}
      className={cn(
        itemBase,
        itemVariants,
        className,
      )}
      {...props}
    />
  );
}

function DropdownMenuCheckboxItem({
  className,
  children,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.CheckboxItem>) {
  return (
    <DropdownMenuPrimitive.CheckboxItem
      data-slot="dropdown-menu-checkbox-item"
      className={cn(itemBase, "pe-8", className)}
      {...props}
    >
      <span
        className="pointer-events-none absolute end-2 flex items-center justify-center"
        data-slot="dropdown-menu-checkbox-item-indicator"
      >
        <DropdownMenuPrimitive.ItemIndicator>
          <Check className="size-icon-md" />
        </DropdownMenuPrimitive.ItemIndicator>
      </span>
      {children}
    </DropdownMenuPrimitive.CheckboxItem>
  );
}

function DropdownMenuRadioGroup({
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.RadioGroup>) {
  return (
    <DropdownMenuPrimitive.RadioGroup
      data-slot="dropdown-menu-radio-group"
      {...props}
    />
  );
}

function DropdownMenuRadioItem({
  className,
  children,
  indicator,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.RadioItem> & {
  /** What marks the chosen item (a dot by default). */
  indicator?: React.ReactNode;
}) {
  return (
    <DropdownMenuPrimitive.RadioItem
      data-slot="dropdown-menu-radio-item"
      className={cn(itemBase, "pe-8", className)}
      {...props}
    >
      <span
        className="pointer-events-none absolute end-2 flex items-center justify-center"
        data-slot="dropdown-menu-radio-item-indicator"
      >
        <DropdownMenuPrimitive.ItemIndicator>
          {indicator ?? <Dot className="size-icon-md" />}
        </DropdownMenuPrimitive.ItemIndicator>
      </span>
      {children}
    </DropdownMenuPrimitive.RadioItem>
  );
}

/**
 * A heading over a group of a menu's items: small mono capitals, or with `variant="heading"`
 * plain grey words ("Model", "Pinned"). It needs no menu around it (a popover's list uses it).
 */
function DropdownMenuLabel({
  className,
  inset,
  variant = "caps",
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Label> & {
  inset?: boolean;
  variant?: "caps" | "heading";
}) {
  return (
    <DropdownMenuPrimitive.Label
      data-slot="dropdown-menu-label"
      data-inset={inset}
      className={cn(
        "text-muted-foreground px-2 py-1.5 data-[inset]:ps-8",
        variant === "caps" ? "font-mono text-2xs font-medium tracking-wide uppercase" : "text-sm",
        className,
      )}
      {...props}
    />
  );
}

function DropdownMenuSeparator({
  className,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Separator>) {
  return (
    <DropdownMenuPrimitive.Separator
      data-slot="dropdown-menu-separator"
      className={cn("bg-foreground/10 -mx-1 my-1 h-px", className)}
      {...props}
    />
  );
}

function DropdownMenuShortcut({
  className,
  ...props
}: React.ComponentProps<"span">) {
  return (
    <span
      data-slot="dropdown-menu-shortcut"
      className={cn(
        "text-muted-foreground ms-auto font-mono text-2xs tracking-wider",
        className,
      )}
      {...props}
    />
  );
}

function DropdownMenuSub({
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Sub>) {
  return <DropdownMenuPrimitive.Sub data-slot="dropdown-menu-sub" {...props} />;
}

function DropdownMenuSubTrigger({
  className,
  inset,
  children,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.SubTrigger> & {
  inset?: boolean;
}) {
  return (
    <DropdownMenuPrimitive.SubTrigger
      data-slot="dropdown-menu-sub-trigger"
      data-inset={inset}
      className={cn(
        itemBase,
        "data-[state=open]:bg-foreground/8 [&_svg:not([class*='text-'])]:text-muted-foreground data-[inset]:ps-8",
        className,
      )}
      {...props}
    >
      {children}
      <ChevronRight className="size-icon-md ms-auto rtl:rotate-180" />
    </DropdownMenuPrimitive.SubTrigger>
  );
}

function DropdownMenuSubContent({
  className,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.SubContent>) {
  return (
    <DropdownMenuPrimitive.SubContent
      data-slot="dropdown-menu-sub-content"
      className={cn(
        menuSurface,
        "origin-(--radix-dropdown-menu-content-transform-origin) overflow-hidden",
        className,
      )}
      {...props}
    />
  );
}

/**
 * What a row of a menu or a picker list shows: a 16px icon (its box kept when it has none), its title, a grey detail
 * after it (or under it with `twoLine`), and a hint at the end (a shortcut, a date; `mono`
 * for code). Put it inside the row's own item, which keeps its keys and highlight.
 */
function MenuRow({
  icon,
  title,
  detail,
  hint,
  mono = false,
  twoLine = false,
}: {
  icon?: React.ReactNode;
  title: React.ReactNode;
  detail?: React.ReactNode;
  hint?: React.ReactNode;
  mono?: boolean;
  twoLine?: boolean;
}) {
  const end = hint !== undefined && hint !== null && hint !== false && (
    <span
      data-slot="menu-row-hint"
      className={cn("text-muted-foreground flex shrink-0 items-center gap-2", !twoLine && "ms-auto text-xs", mono && "font-mono")}
    >
      {hint}
    </span>
  );
  return (
    <>
      {icon !== undefined && (
        <span
          data-slot="menu-row-icon"
          className="text-muted-foreground flex size-icon-md shrink-0 items-center justify-center [&_svg]:size-icon-md"
        >
          {icon}
        </span>
      )}
      {twoLine ? (
        <span className="flex min-w-0 flex-1 flex-col py-1">
          <span className="flex min-w-0 gap-2">
            <span className="truncate">{title}</span>
            {end}
          </span>
          {detail && <span className="text-muted-foreground truncate text-xs">{detail}</span>}
        </span>
      ) : (
        <>
          <span className={detail ? "shrink-0" : "min-w-0 flex-1 truncate"}>{title}</span>
          {detail && <span className="text-muted-foreground min-w-0 flex-1 truncate">{detail}</span>}
          {end}
        </>
      )}
    </>
  );
}

export {
  MenuRow,
  DropdownMenu,
  DropdownMenuPortal,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuLabel,
  DropdownMenuItem,
  DropdownMenuCheckboxItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuSub,
  DropdownMenuSubTrigger,
  DropdownMenuSubContent,
};
