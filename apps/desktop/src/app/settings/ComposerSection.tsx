import { useAction } from "@/app/conversation/useAction";
import { GENERAL_ROWS } from "@/app/settings/generalRows";
import { SettingsSection, SwitchSetting } from "@/app/settings/parts";
import { Segmented } from "@/components/controls";
import { SettingsCard, SettingsRow } from "@/components/shared";
import type { FollowUp } from "@/ipc/generated";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

const FOLLOW_UPS = [
  { value: "queue", label: "Queue" },
  { value: "steer", label: "Steer" },
] as const satisfies readonly { value: FollowUp; label: string }[];

/** General › Composer: what a follow-up does, plain text, the context ring. */
export function ComposerSection() {
  const mac = useApp((s) => s.info?.platform === "macos");
  const followUp = useApp((s) => s.settings.followUp);
  const followUpAction = useAction();
  // The key that sends the other way for one message.
  const other = mac ? "⌥Enter" : "Alt+Enter";
  return (
    <SettingsSection title="Composer">
      <SettingsCard>
        <SettingsRow
          label={GENERAL_ROWS.followUp.label}
          description={`${GENERAL_ROWS.followUp.description}; ${other} does the other for one message`}
          error={followUpAction.error}
        >
          <Segmented
            label={GENERAL_ROWS.followUp.label}
            value={followUp}
            options={FOLLOW_UPS}
            onChange={(value) => followUpAction.run(() => setSetting("followUp", value))}
          />
        </SettingsRow>
        <SwitchSetting setting="plainTextComposer" row={GENERAL_ROWS.plainText} />
        <SwitchSetting setting="showContextUsage" row={GENERAL_ROWS.contextUsage} />
      </SettingsCard>
    </SettingsSection>
  );
}
