import { useAction } from "@/app/conversation/useAction";
import { SettingsSelect } from "@/components/controls";
import { SettingsRow } from "@/components/shared";
import type { LinkTarget } from "@/ipc/generated";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

const TARGETS = [
  { value: "brigadier", label: "Brigadier's browser" },
  { value: "system", label: "Your default browser" },
] as const satisfies readonly { value: LinkTarget; label: string }[];

/** Where a kind of link opens: Brigadier's browser or the default one. */
export function LinkRow({
  setting,
  row,
}: {
  setting: "webLinksIn" | "localLinksIn";
  row: { label: string; description: string };
}) {
  const value = useApp((s) => s.settings[setting]);
  const save = useAction();
  return (
    <SettingsRow label={row.label} description={row.description} error={save.error}>
      <SettingsSelect<LinkTarget>
        label={row.label}
        value={value}
        options={TARGETS}
        onChange={(next) => save.run(() => setSetting(setting, next))}
      />
    </SettingsRow>
  );
}
