import { useAction } from "@/app/conversation/useAction";
import { DimmedRow } from "@/app/settings/DimmedRow";
import { GENERAL_ROWS } from "@/app/settings/generalRows";
import { SettingsSection, SettingsSwitch } from "@/app/settings/parts";
import { Button, SettingsSelect } from "@/components/controls";
import { SettingsCard, SettingsRow } from "@/components/shared";
import { openNotificationSettings } from "@/ipc/client";
import type { NotifyFinished } from "@/ipc/generated";
import { askNotifications, useNotificationPermission } from "@/state/notifications";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

const NOTIFY_FINISHED = [
  {
    value: "background",
    label: "In the background",
    hint: "Only while Brigadier isn't the app in front",
  },
  { value: "always", label: "Always", hint: "Also while you're in Brigadier" },
  { value: "never", label: "Never", hint: "No notification when work is done" },
] as const satisfies readonly { value: NotifyFinished; label: string; hint: string }[];

/** General › Notifications: whether they are on, when work is done, when something needs you. */
export function NotificationsSection() {
  const permission = useNotificationPermission(true);
  const notifyFinished = useApp((s) => s.settings.notifyFinished);
  const needsYou = useApp((s) => s.settings.notifyNeedsYou);
  const statusAction = useAction();
  const finishedAction = useAction();
  const needsYouAction = useAction();
  // The rows stay usable while notifications are off, dimmed: they apply once they're on.
  const off = permission === "off" || permission === "notAsked";
  return (
    <SettingsSection title="Notifications">
      <SettingsCard>
        {permission === "off" && (
          <SettingsRow
            label="Notifications are off"
            description="Turn them on in System Settings to use the options below"
            error={statusAction.error}
          >
            <Button variant="soft" size="sm" onClick={() => statusAction.run(() => openNotificationSettings())}>
              Open System Settings
            </Button>
          </SettingsRow>
        )}
        {permission === "notAsked" && (
          <SettingsRow
            label="Notifications aren't on yet"
            description="Allow them to hear when work is done or needs you"
            error={statusAction.error}
          >
            <Button variant="soft" size="sm" onClick={() => statusAction.run(() => askNotifications())}>
              Allow
            </Button>
          </SettingsRow>
        )}
        <DimmedRow
          dimmed={off}
          label={GENERAL_ROWS.notifyFinished.label}
          description={GENERAL_ROWS.notifyFinished.description}
          error={finishedAction.error}
        >
          <SettingsSelect
            label={GENERAL_ROWS.notifyFinished.label}
            value={notifyFinished}
            options={NOTIFY_FINISHED}
            onChange={(value) => finishedAction.run(() => setSetting("notifyFinished", value))}
          />
        </DimmedRow>
        <DimmedRow
          dimmed={off}
          label={GENERAL_ROWS.needsYou.label}
          description={GENERAL_ROWS.needsYou.description}
          error={needsYouAction.error}
        >
          <SettingsSwitch
            label={GENERAL_ROWS.needsYou.label}
            checked={needsYou}
            onCheckedChange={(on) => needsYouAction.run(() => setSetting("notifyNeedsYou", on))}
          />
        </DimmedRow>
      </SettingsCard>
    </SettingsSection>
  );
}
