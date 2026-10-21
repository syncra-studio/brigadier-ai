import { PILL_BUTTON } from "@/app/setup/onboarding/parts";
import { cn } from "@/lib/utils";

/** Select all, or Select none once every project is ticked. */
export function SelectAllToggle({
  allChosen,
  disabled,
  onToggle,
}: {
  allChosen: boolean;
  disabled: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onToggle}
      className={cn(
        PILL_BUTTON,
        "text-muted-foreground enabled:hover:text-foreground grid items-center px-2.5 font-medium transition-colors disabled:opacity-50",
      )}
    >
      {/* Both labels hold the room, so the button never moves as it switches. */}
      <span className="col-start-1 row-start-1">{allChosen ? "Select none" : "Select all"}</span>
      <span aria-hidden className="invisible col-start-1 row-start-1">
        Select none
      </span>
    </button>
  );
}
