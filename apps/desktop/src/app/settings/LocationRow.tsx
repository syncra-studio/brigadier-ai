import { useAction } from "@/app/conversation/useAction";
import { Button } from "@/components/controls";
import { SettingsRow } from "@/components/shared";
import { pickFolder } from "@/ipc/client";
import { shortPath } from "@/lib/format";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

/** Downloads' folder: the system's Downloads folder until another is chosen. */
export function LocationRow({ row }: { row: { label: string } }) {
  const dir = useApp((s) => s.settings.browserDownloadsDir);
  const save = useAction();
  const change = () =>
    save.run(async () => {
      const picked = await pickFolder(dir || undefined);
      if (picked) await setSetting("browserDownloadsDir", picked);
    });
  return (
    <SettingsRow
      label={row.label}
      description={dir ? <span title={dir}>{shortPath(dir)}</span> : "Your Downloads folder"}
      error={save.error}
    >
      <div className="flex items-center gap-2">
        {dir && (
          <Button variant="soft" size="sm" onClick={() => save.run(() => setSetting("browserDownloadsDir", ""))}>
            Use Downloads
          </Button>
        )}
        <Button variant="soft" size="sm" onClick={change}>Change…</Button>
      </div>
    </SettingsRow>
  );
}
