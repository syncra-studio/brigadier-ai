import { useEffect, useState } from "react";

import { ProfileClearDialog } from "@/app/settings/ProfileClearDialog";
import { ProfileDeleteDialog } from "@/app/settings/ProfileDeleteDialog";
import { ProfileDialog } from "@/app/settings/ProfileDialog";
import { ProfileDisc, ProfileSwatches } from "@/app/settings/ProfileMark";
import { SettingsSection } from "@/app/settings/parts";
import { Badge, Button, SettingsSelect } from "@/components/controls";
import { ChevronRight, DotsHorizontal, GripIcon, Plus } from "@/components/icons";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/overlays";
import { RowText, SettingsCard, SettingsRow } from "@/components/shared";
import { useDragReorder } from "@/hooks/use-drag-reorder";
import { cn } from "@/lib/utils";
import { openSettings, select } from "@/state/actions";
import { counted, importOrigins } from "@/state/browserImport";
import {
  type BrowserProfile,
  changeProfile,
  makeDefaultProfile,
  moveProfile,
  sessionsOn,
  useDefaultProfile,
  useProfilePlaces,
  useProfiles,
} from "@/state/browserProfiles";
import { useApp } from "@/state/store";

/** How a profile started and how many sessions are on it. */
export function profileLine(sessions: number, importedFrom?: string): string {
  return `${importedFrom ? `Imported from ${importedFrom}` : "Started empty"} · ${sessions === 0 ? "No sessions" : counted(sessions, "session", "sessions")}`;
}

export const openProfileDetail = (id: string) => select({ type: "settings", page: "browser", detail: { view: "profile", profile: id } });

/** Settings › Browser's Profiles: the list to reorder and open, and which one new sessions start on. */
export function ProfilesSection() {
  const profiles = useProfiles();
  const fallback = useDefaultProfile();
  const places = useProfilePlaces((s) => s.sessions);
  const conversations = useApp((s) => s.conversations);
  const [creating, setCreating] = useState(false);
  const [origins, setOrigins] = useState<Partial<Record<string, string>>>({});
  useEffect(() => {
    void importOrigins().then(setOrigins);
  }, []);
  const { listRef, shown, dragging, grip } = useDragReorder<BrowserProfile, HTMLDivElement>({
    items: profiles,
    idOf: (profile) => profile.id,
    rowSelector: '[data-slot="profile-row"]',
    onMove: moveProfile,
  });
  return (
    <SettingsSection
      title="Profiles"
      description="Each profile keeps its own sign-ins, history, bookmarks and passwords."
      actions={
        <>
          <Button variant="ghost" size="sm" onClick={() => openSettings("browserImport")}>Import…</Button>
          <Button variant="soft" size="sm" onClick={() => setCreating(true)}><Plus className="size-icon-sm" />New profile</Button>
        </>
      }
    >
      <div data-setting="Profiles" className="contents">
        <SettingsCard>
          <div ref={listRef} data-slot="profile-list" className="contents">
            {shown.map((profile, index) => (
              <ProfileRow
                key={profile.id}
                profile={profile}
                isDefault={profile.id === fallback.id}
                only={profiles.length === 1}
                sessions={sessionsOn(profile.id, places, conversations)}
                importedFrom={origins[profile.id]}
                dragging={dragging === profile.id}
                grip={grip(profile.id, index)}
              />
            ))}
          </div>
        </SettingsCard>
        <SettingsCard>
          <SettingsRow label="New sessions start on" description="You can switch any session from the browser toolbar">
            <SettingsSelect
              label="New sessions start on"
              value={fallback.id}
              onChange={makeDefaultProfile}
              options={profiles.map((profile) => ({ value: profile.id, label: profile.name }))}
            />
          </SettingsRow>
        </SettingsCard>
      </div>
      <ProfileDialog open={creating} onOpenChange={setCreating} />
    </SettingsSection>
  );
}

function ProfileRow({
  profile,
  isDefault,
  only,
  sessions,
  importedFrom,
  dragging,
  grip,
}: {
  profile: BrowserProfile;
  isDefault: boolean;
  only: boolean;
  sessions: number;
  importedFrom: string | undefined;
  dragging: boolean;
  grip: ReturnType<ReturnType<typeof useDragReorder>["grip"]>;
}) {
  const [renaming, setRenaming] = useState(false);
  const [clearing, setClearing] = useState(false);
  const [deleting, setDeleting] = useState(false);
  return (
    <div
      data-slot="profile-row"
      data-profile={profile.id}
      className={cn("bg-card not-last:border-divider flex items-center gap-3 px-4 py-3 not-last:border-b", dragging && "opacity-80")}
    >
      <button type="button" aria-label={`Move ${profile.name}`} {...grip} className="flex shrink-0 cursor-grab touch-none items-center">
        <GripIcon className="text-muted-foreground" />
      </button>
      <ProfileDisc profile={profile} size="lg" />
      <button type="button" className="flex min-w-0 flex-1 text-start" onClick={() => void openProfileDetail(profile.id)}>
        <RowText
          title={<span className="flex items-center gap-2">{profile.name}{isDefault && <Badge variant="secondary" className="font-normal">Default</Badge>}</span>}
          line={profileLine(sessions, importedFrom)}
        />
      </button>
      <div className="flex shrink-0 items-center gap-1">
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon-sm" aria-label={`${profile.name} options`} className="text-muted-foreground data-[state=open]:bg-foreground/8">
              <DotsHorizontal />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-60" data-slot="profile-row-menu">
            <DropdownMenuItem onSelect={() => setRenaming(true)}>Rename</DropdownMenuItem>
            <div role="group" aria-label="Colour" className="px-2 py-2">
              <ProfileSwatches value={profile.hue} onChange={(hue) => changeProfile(profile.id, { hue })} />
            </div>
            <DropdownMenuSeparator className="mx-2" />
            <DropdownMenuItem disabled={isDefault} onSelect={() => makeDefaultProfile(profile.id)}>Make default</DropdownMenuItem>
            <DropdownMenuItem onSelect={() => setClearing(true)}>Clear browsing data…</DropdownMenuItem>
            <DropdownMenuItem onSelect={() => void openProfileDetail(profile.id)}>Open details</DropdownMenuItem>
            <DropdownMenuSeparator className="mx-2" />
            <DropdownMenuItem variant="destructive" disabled={only || isDefault} onSelect={() => setDeleting(true)}>
              {only ? "Delete… (last profile)" : isDefault ? "Delete… (default profile)" : "Delete…"}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <ChevronRight aria-hidden className="text-muted-foreground size-icon-sm" />
      </div>
      <ProfileDialog open={renaming} onOpenChange={setRenaming} editing={profile} />
      <ProfileClearDialog profile={profile} open={clearing} onOpenChange={setClearing} />
      <ProfileDeleteDialog profile={profile} open={deleting} onOpenChange={setDeleting} />
    </div>
  );
}
