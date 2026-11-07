import { useEffect, useState } from "react";

import { errorText } from "@/app/shell/dialogs/fields";
import { Button, TooltipIconButton } from "@/components/controls";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/overlays";
import { DotsHorizontal, ExternalLink, Eye, EyeOff, Globe, Trash } from "@/components/icons";
import { passwordSite, revealPassword, type SavedPassword } from "@/state/browserPasswords";
import { openLink } from "@/state/links";

/** How long a shown password stays before it hides itself. */
const SHOWN_MS = 30_000;

/**
 * One saved password: its site and username, dots until the user asks to see it and the system's
 * check passes, and a menu. The password is held only while it shows.
 */
export function BrowserPasswordsRow({
  profile,
  entry,
  onRemove,
  onError,
}: {
  profile: string;
  entry: SavedPassword;
  onRemove: () => void;
  onError: (error: string | null) => void;
}) {
  const [shown, setShown] = useState<string | null>(null);
  const [asking, setAsking] = useState(false);
  const site = passwordSite(entry.site);

  useEffect(() => {
    if (shown === null) return;
    const timer = setTimeout(() => setShown(null), SHOWN_MS);
    return () => clearTimeout(timer);
  }, [shown]);

  const toggle = () => {
    if (shown !== null) return setShown(null);
    setAsking(true);
    onError(null);
    revealPassword(profile, entry.id)
      .then((reveal) => {
        if (reveal.kind === "shown") setShown(reveal.password);
        else if (reveal.kind === "unavailable") onError("This Mac can't ask for Touch ID now, so the password stays hidden.");
        else onError("Your computer didn't confirm it's you, so the password stays hidden.");
      })
      .catch((cause: unknown) => onError(errorText(cause)))
      .finally(() => setAsking(false));
  };

  return (
    <li data-slot="password-row" className="flex min-h-12 items-center gap-3 py-1.5">
      <Globe aria-hidden className="text-foreground/65 size-icon-md shrink-0" />
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="text-label truncate" title={entry.site}>{site}</span>
        <span className="text-foreground/65 truncate text-xs">{entry.username}</span>
      </span>
      <span
        data-slot="password-value"
        className="text-foreground/65 max-w-48 shrink-0 truncate text-xs tabular-nums select-text"
      >
        {shown ?? "••••••••"}
      </span>
      <TooltipIconButton
        tooltip={shown === null ? "Show password" : "Hide password"}
        size="icon-sm"
        disabled={asking}
        className="text-foreground/65 shrink-0"
        onClick={toggle}
      >
        {shown === null ? <Eye /> : <EyeOff />}
      </TooltipIconButton>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={`More for ${entry.username} on ${site}`}
            className="text-foreground/65 shrink-0"
          >
            <DotsHorizontal />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem onSelect={() => void openLink(entry.site)}>
            <ExternalLink />
            Open site
          </DropdownMenuItem>
          <DropdownMenuItem variant="destructive" onSelect={onRemove}>
            <Trash />
            Remove password
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </li>
  );
}
