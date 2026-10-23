import { type ComponentProps, forwardRef, useId } from "react";

/**
 * A picker's button, named by its `label` and then the choice it shows: `shown` holds the ids
 * of the elements inside that show it, so a screen reader says "Model, Opus 5.5 High" (#564).
 * A button has no value of its own to announce. `aria-label` stays the label alone. Its id is
 * the one a menu's trigger gives it, when it is one, since the menu is named after it.
 */
export const PickerButton = forwardRef<
  HTMLButtonElement,
  Omit<ComponentProps<"button">, "aria-label" | "aria-labelledby"> & { label: string; shown: readonly (string | false | null | undefined)[] }
>(({ label, shown, id, ...props }, ref) => {
  const own = useId();
  const named = id ?? own;
  return (
    <button
      ref={ref}
      type="button"
      {...props}
      id={named}
      aria-label={label}
      aria-labelledby={[named, ...shown].filter(Boolean).join(" ")}
    />
  );
});
PickerButton.displayName = "PickerButton";
