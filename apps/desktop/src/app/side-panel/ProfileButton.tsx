import { useState } from "react";

import { ProfileDialog } from "@/app/settings/ProfileDialog";
import { ProfileDisc } from "@/app/settings/ProfileMark";
import { Check } from "@/components/icons";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/overlays";
import { openSettings } from "@/state/actions";
import { profileKeyLabel, switchProfile, useDefaultProfile, useFrontDriver, useFrontProfile, useProfiles } from "@/state/browserProfiles";
import { openBrowserSettings } from "@/state/browserSettings";
import { useApp } from "@/state/store";

/**
 * The toolbar's profile button and its menu: which profile the tab in front is on, and the others
 * to switch it to. Every tab's toolbar shows the one in front; only that toolbar is seen.
 */
export function ProfileButton() {
  const mac = useApp((s) => s.info?.platform === "macos");
  const current = useFrontProfile();
  const fallback = useDefaultProfile();
  const profiles = useProfiles();
  const driver = useFrontDriver();
  const [creating, setCreating] = useState(false);
  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label={`Profile: ${current.name}`}
            data-slot="browser-profile-button"
            className="bg-muted/60 ring-border text-browser hover:bg-foreground/8 data-[state=open]:bg-foreground/8 h-control-md flex max-w-36 shrink-0 items-center gap-1.5 rounded-full ps-1.25 pe-3 ring-1 ring-inset @max-lg/browser-toolbar:pe-1.25"
          >
            <ProfileDisc profile={current} size="lg" />
            <span className="min-w-0 truncate @max-lg/browser-toolbar:hidden">{current.name}</span>
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent
          align="end"
          data-slot="browser-profiles"
          className="rounded-browser-menu w-browser-profiles [&_[role=menuitem]]:h-browser-menu-item [&_[role=menuitem]]:text-browser"
        >
          <div className="flex flex-col gap-0.5 px-2 pt-1.5 pb-2">
            {driver ? (
              <>
                <span className="text-browser font-medium">{driver} is working in {current.name}</span>
                <span className="text-muted-foreground text-xs">
                  It uses this profile's sign-ins and never sees its saved passwords. Picking another profile opens a new tab there, and {driver}'s next tabs follow.
                </span>
              </>
            ) : (
              <>
                <span className="text-browser font-medium">Profile for this tab</span>
                <span className="text-muted-foreground text-xs">
                  The tab reopens in the profile you pick. New tabs in this session follow it. Other tabs stay as they are.
                </span>
              </>
            )}
          </div>
          <DropdownMenuSeparator className="mx-2" />
          {profiles.map((profile, index) => (
            <DropdownMenuItem key={profile.id} onSelect={() => switchProfile(profile.id)}>
              <ProfileDisc profile={profile} />
              <span className="min-w-0 flex-1 truncate">{profile.name}</span>
              {profile.id === fallback.id && <span className="text-muted-foreground text-xs">Default</span>}
              {profile.id === current.id
                ? <Check className="size-icon-md" />
                : index < 9 && <DropdownMenuShortcut>{profileKeyLabel(index, mac)}</DropdownMenuShortcut>}
            </DropdownMenuItem>
          ))}
          <DropdownMenuSeparator className="mx-2" />
          <DropdownMenuItem onSelect={() => setCreating(true)}>New profile…</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => openSettings("browserImport")}>Import Browser Data…</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => openBrowserSettings()}>Manage profiles</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <ProfileDialog open={creating} onOpenChange={setCreating} onCreated={switchProfile} />
    </>
  );
}
