import { isMade, makeChange, settingPage, settingWords } from "@/app/conversation/settingCard";
import { setSessionPermission } from "@/app/conversation/SetupPickers";
import { useAction } from "@/app/conversation/useAction";
import { useViewConversation } from "@/app/conversation/viewContext";
import { WorkerLine } from "@/app/conversation/WorkerChip";
import { ShieldExclamation } from "@/components/glyphs/permission-glyphs";
import { Button } from "@/components/ui/button";
import { Branch, Globe } from "@/components/ui/icon";
import type { SettingChange } from "@/ipc/generated";
import { PERMISSION_LABELS } from "@/lib/setup";
import { openSettings } from "@/state/actions";
import { useApp } from "@/state/store";

/**
 * The thread's card with a settings change it suggests (its `suggest_setting`): the change, the
 * lead's reason, one button that makes it and a link to its Settings page. Nothing changes until
 * the button is pressed; once the setting is that way the button gives way to a line saying so.
 * Full access has its own notice (`SandboxLimitNotice`), whose look this shares.
 */
export function SettingSuggestedCard({ change, reason }: { change: SettingChange; reason: string }) {
  const conversation = useViewConversation();
  const settings = useApp((s) => s.settings);
  const action = useAction();
  const words = settingWords(change, PERMISSION_LABELS);
  const made = isMade(change, settings, conversation);
  const Icon = change.setting === "browser" ? Globe : change.setting === "mergeOnOwn" ? Branch : ShieldExclamation;
  return (
    <section
      aria-label={words.title}
      data-slot="orchestrator-step"
      data-kind="settingSuggested"
      className="bg-rail border-foreground/10 rounded-surface my-1 flex flex-col gap-2 border px-4 py-3 text-sm"
    >
      <div className="flex min-w-0 items-start gap-3">
        <Icon className="size-icon-md mt-0.5 shrink-0" />
        <div className="min-w-0 flex-1">
          <p className="font-medium">{words.title}</p>
          <p className="text-muted-foreground wrap-break-word">
            <WorkerLine text={reason} />
          </p>
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 ps-8">
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
      </div>
      {action.error && (
        <p role="alert" className="text-destructive ps-8 text-xs">
          {action.error}
        </p>
      )}
    </section>
  );
}
