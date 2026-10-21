import { useAction } from "@/app/conversation/useAction";
import { GENERAL_ROWS } from "@/app/settings/generalRows";
import { HibernateRow } from "@/app/settings/HibernateRow";
import { OpenAtLoginRow } from "@/app/settings/OpenAtLoginRow";
import { SettingsSection, SettingsSwitch } from "@/app/settings/parts";
import { SettingsSelect } from "@/components/controls";
import { SettingsCard, SettingsRow } from "@/components/shared";
import {
  keepAwakeOptions,
  lidClosedHint,
  setKeepAwake,
  setKeepAwakeLidClosed,
  useKeepAwake,
} from "@/state/keepAwake";
import { useApp } from "@/state/store";

/** General › Power: keeping the computer awake, the lid, hibernating chats, open at login. */
export function PowerSection() {
  const keepAwake = useApp((s) => s.settings.keepAwake);
  const lidClosed = useApp((s) => s.settings.keepAwakeLidClosed);
  const status = useKeepAwake((s) => s.status);
  const settingUp = useKeepAwake((s) => s.settingUp);
  const keepAwakeAction = useAction();
  const lidAction = useAction();
  const options = keepAwakeOptions(status?.screenOn ?? false);
  return (
    <SettingsSection title="Power">
      <SettingsCard>
        <SettingsRow
          label={GENERAL_ROWS.keepAwake.label}
          description={
            status?.forRun
              ? "An overnight run keeps the computer awake, screen on, until it ends; then this applies again"
              : GENERAL_ROWS.keepAwake.description
          }
          error={keepAwakeAction.error}
        >
          <SettingsSelect
            label={GENERAL_ROWS.keepAwake.label}
            value={keepAwake}
            options={options}
            onChange={(value) => keepAwakeAction.run(() => setKeepAwake(value))}
          />
        </SettingsRow>
        {status?.lidClosed !== "unsupported" && (
          <SettingsRow
            label={GENERAL_ROWS.lidClosed.label}
            description={lidClosedHint(status, settingUp)}
            error={lidAction.error}
          >
            <SettingsSwitch
              label={GENERAL_ROWS.lidClosed.label}
              checked={lidClosed}
              disabled={settingUp}
              onCheckedChange={(on) => lidAction.run(() => setKeepAwakeLidClosed(on))}
            />
          </SettingsRow>
        )}
        <HibernateRow />
        <OpenAtLoginRow />
      </SettingsCard>
    </SettingsSection>
  );
}
