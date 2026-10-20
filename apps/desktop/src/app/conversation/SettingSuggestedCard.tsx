import { isMade, makeChange, settingPage, settingWords } from "@/app/conversation/settingCard";
import { setSessionPermission } from "@/app/conversation/SetupPickers";
import { OfferCard, OfferError } from "@/app/conversation/OfferCard";
import { useAction } from "@/app/conversation/useAction";
import { useViewConversation } from "@/app/conversation/viewContext";
import { WorkerLine } from "@/app/conversation/WorkerChip";
import { Branch, Globe, ShieldExclamation } from "@/components/icons";
import { Button } from "@/components/controls/button";
import type { SettingChange } from "@/ipc/generated";
import { PERMISSION_LABELS } from "@/lib/setup";
import { openSettings } from "@/state/actions";
import { useApp } from "@/state/store";

/**
 * The thread's card with a settings change it suggests (its `suggest_setting`): the change, the
 * lead's reason, one button that makes it and a link to its Settings page. Nothing changes until
 * the button is pressed; once the setting is that way the button gives way to a line saying so.
 */
export function SettingSuggestedCard({ change, reason }: { change: SettingChange; reason: string }) {
  const conversation = useViewConversation();
  const settings = useApp((s) => s.settings);
  const action = useAction();
  const words = settingWords(change, PERMISSION_LABELS);
  const made = isMade(change, settings, conversation);
  const Icon = change.setting === "browser" ? Globe : change.setting === "mergeOnOwn" ? Branch : ShieldExclamation;
  return (
    <OfferCard
      label={words.title}
      kind="settingSuggested"
      icon={<Icon />}
      title={words.title}
      line={<WorkerLine text={reason} />}
      actions={
        <>
          {made ? (
            <p data-slot="setting-made" className="text-muted-foreground">
              {words.done}
            </p>
          ) : (
            <Button
              size="xs"
              className="rounded-capsule"
              disabled={action.busy}
              onClick={() => action.run(() => makeChange(change, conversation, setSessionPermission))}
            >
              {words.button}
            </Button>
          )}
          <button
            type="button"
            className="text-link text-xs hover:underline"
            onClick={() => openSettings(settingPage(change))}
          >
            Open Settings
          </button>
        </>
      }
    >
      {action.error && <OfferError>{action.error}</OfferError>}
    </OfferCard>
  );
}
