import { ClearBrowsingDataDialog } from "@/app/settings/ClearBrowsingDataDialog";
import type { BrowserProfile } from "@/state/browserProfiles";

/** Clear browsing data, starting on one profile; other profiles aren't touched. */
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
  return <ClearBrowsingDataDialog profile={profile.id} open={open} onOpenChange={onOpenChange} siteData onCleared={onCleared} />;
}
