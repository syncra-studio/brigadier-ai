import { useEffect, useState } from "react";

import { Button } from "@/components/controls";
import { Key } from "@/components/icons";
import { type OfferAnswer, passwordSite, type SaveOffer } from "@/state/browserPasswords";

/** A click meant for the page must not land on a card that just came up under it. */
const ARM_MS = 500;

/**
 * "Save this password?" over the page, after a sign-in in a tab the user drives; a tab an agent
 * drives never gets one. It shows the site and username: the password stays in main.
 */
export function BrowserPasswordsSavePrompt({
  offer,
  onAnswer,
}: {
  offer: SaveOffer;
  onAnswer: (answer: OfferAnswer) => void;
}) {
  const site = passwordSite(offer.site);
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    const timer = setTimeout(() => setArmed(true), ARM_MS);
    return () => clearTimeout(timer);
  }, []);
  return (
    <div
      data-slot="save-password-prompt"
      role="dialog"
      aria-label={offer.update ? `Update password for ${site}?` : `Save password for ${site}?`}
      className="bg-popover rounded-settings shadow-menu flex w-84 flex-col gap-3 border p-4"
    >
      <div className="flex items-start gap-2.5">
        <Key aria-hidden className="text-foreground/65 size-icon-md mt-0.5 shrink-0" />
        <div className="flex min-w-0 flex-col gap-1">
          <span className="text-sm font-medium">{offer.update ? "Update this password?" : "Save this password?"}</span>
          <span className="text-foreground/65 truncate text-xs" title={offer.username}>
            {offer.username ? `${offer.username} on ${site}` : site}
          </span>
        </div>
      </div>
      <div className="flex items-center justify-end gap-1.5">
        {!offer.update && (
          <Button size="sm" variant="ghost" className="me-auto" disabled={!armed} onClick={() => onAnswer("never")}>
            Never for this site
          </Button>
        )}
        <Button size="sm" variant="ghost" disabled={!armed} onClick={() => onAnswer("notNow")}>Not now</Button>
        <Button size="sm" disabled={!armed} onClick={() => onAnswer("save")}>{offer.update ? "Update" : "Save"}</Button>
      </div>
    </div>
  );
}
