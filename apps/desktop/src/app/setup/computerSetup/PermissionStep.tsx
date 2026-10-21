import { useAction } from "@/app/conversation/useAction";
import { HelperCard } from "@/app/setup/computerSetup/HelperCard";
import { SetupPanel } from "@/app/setup/computerSetup/SetupPanel";
import { Button, ErrorLine } from "@/components/controls";
import type { ComputerPermission } from "@/ipc/generated";
import { openPrivacySettings } from "@/state/computerUse";

const PERMISSION_TEXT: Record<ComputerPermission, { title: string; why: string }> = {
  accessibility: {
    title: "Allow Accessibility",
    why: "Lets Brigadier read buttons and text in other apps, and press them for agents.",
  },
  screenRecording: {
    title: "Allow Screen Recording",
    why: "Lets Brigadier see app windows, so agents can check what happened.",
  },
};

export function PermissionStep({
  permission,
  busy,
  onCheck,
}: {
  permission: ComputerPermission;
  busy: boolean;
  onCheck: () => void;
}) {
  const open = useAction();
  const text = PERMISSION_TEXT[permission];
  return (
    <SetupPanel title={text.title}>
      <p className="text-foreground/80">{text.why}</p>
      <ol className="text-foreground/80 list-decimal space-y-1 pl-5">
        <li>Open System Settings.</li>
        <li>Drag Brigadier Computer Use into the list, or turn it on if it's already there.</li>
      </ol>
      {permission === "screenRecording" && (
        <p className="text-foreground/60">
          If macOS offers to quit and reopen Brigadier Computer Use, either answer is fine: Brigadier restarts it by
          itself and stays open.
        </p>
      )}
      <div className="flex gap-2">
        <Button onClick={() => open.run(() => openPrivacySettings(permission))}>Open System Settings</Button>
        <Button variant="outline" disabled={busy} onClick={onCheck}>
          {busy ? "Checking…" : "Check again"}
        </Button>
      </div>
      <HelperCard />
      <ErrorLine error={open.error} />
    </SetupPanel>
  );
}
