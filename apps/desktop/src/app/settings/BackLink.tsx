import { ChevronLeft } from "@/components/icons";

/** A page inside another's link back to it: its name over the title. */
export function BackLink({ label, onBack }: { label: string; onBack: () => void }) {
  return (
    <button
      type="button"
      onClick={onBack}
      className="text-foreground/65 hover:text-foreground text-label -ms-1 flex w-fit items-center gap-0.5"
    >
      <ChevronLeft aria-hidden className="size-icon-sm" />
      {label}
    </button>
  );
}
