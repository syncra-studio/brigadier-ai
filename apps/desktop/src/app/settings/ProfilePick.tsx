import { useId } from "react";

import { Checkbox } from "@/components/controls";
import { counted } from "@/state/browserImport";

/** One profile in the import dialog's Choose step: a tick box, its name and its cookies. */
export function ProfilePick({
  name,
  cookies,
  checked,
  onCheckedChange,
}: {
  name: string;
  cookies: number | null;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
}) {
  const id = useId();
  return (
    <div
      data-slot="import-profile"
      className="not-last:border-divider flex items-start gap-3 px-4 py-3 not-last:border-b"
    >
      <Checkbox
        id={id}
        checked={checked}
        onCheckedChange={(state) => onCheckedChange(state === true)}
        className="mt-0.5"
      />
      <label htmlFor={id} className="grid min-w-0 flex-1 gap-0.5">
        <span className="text-label break-words">{name}</span>
        {cookies !== null && <span className="text-foreground/65 text-xs">{counted(cookies, "cookie", "cookies")}</span>}
      </label>
    </div>
  );
}
