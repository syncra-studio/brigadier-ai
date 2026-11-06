import { ConfirmDialog } from "@/components/overlays";
import { type BrowserProfile, clearProfile } from "@/state/browserProfiles";
import { toast } from "@/state/toasts";

/** Clears one profile's data; other profiles aren't touched. The last profile's only way to empty. */
export function ProfileClearDialog({
  profile,
  open,
  onOpenChange,
  onCleared,
}: {
  profile: BrowserProfile;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCleared?: () => void;
}) {
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={onOpenChange}
      title={`Clear ${profile.name}'s data?`}
      description="Its history, sign-ins, site data, cache and download history are removed, and its open tabs reload signed out. Other profiles aren't touched."
      confirm="Clear data"
      destructive
      onConfirm={() => {
        onOpenChange(false);
        clearProfile(profile.id).then(
          () => onCleared?.(),
          (cause: unknown) => toast(`Couldn't clear ${profile.name}: ${cause instanceof Error ? cause.message : String(cause)}`, { tone: "error" }),
        );
      }}
    />
  );
}
