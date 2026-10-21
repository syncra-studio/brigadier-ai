import { useAction } from "@/app/conversation/useAction";
import { SettingsSwitch } from "@/app/settings/parts";
import { Globe } from "@/components/icons";
import { SettingsRow } from "@/components/shared";
import { setSetting } from "@/state/settings";

/** The browser's own switch, on top of Settings › Browser: on, the rest of the page shows. */
export function BrowserSwitch({ on, row }: { on: boolean; row: { label: string; description: string } }) {
  const save = useAction();
  return (
    <SettingsRow
      label={
        <span className="flex items-center gap-3">
          <Globe aria-hidden className="size-icon-lg text-foreground/80 shrink-0" />
          {row.label}
        </span>
      }
      description={<span className="block ps-[calc(var(--spacing-icon-lg)+var(--spacing)*3)]">{row.description}</span>}
      error={save.error}
      className="py-4"
    >
      <SettingsSwitch
        label={row.label}
        checked={on}
        onCheckedChange={(next) => save.run(() => setSetting("browserEnabled", next))}
      />
    </SettingsRow>
  );
}
