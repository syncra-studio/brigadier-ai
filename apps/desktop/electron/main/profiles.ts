// A browser profile is a partition. A guest's partition can't change once it attaches, so a tab
// carries its profile id from the moment it's made.
const PREFIX = "persist:browser-profile-";
const STORAGE_NAME = "browser-profile-";

export function profilePartition(profileId: string): string {
  if (!/^[a-z0-9-]{1,64}$/.test(profileId)) throw new Error(`Bad profile id: ${profileId}`);
  return `${PREFIX}${profileId}`;
}

/** Whether a session's storage path belongs to a browser profile rather than the app's own pages. */
export function isProfilePartition(storagePath: string | null): boolean {
  return storagePath !== null && storagePath.includes(STORAGE_NAME);
}

/** The profile a session's storage path belongs to; null for the app's own pages. */
export function profileOfStorage(storagePath: string | null): string | null {
  return /[\\/]browser-profile-([a-z0-9-]{1,64})$/.exec(storagePath ?? "")?.[1] ?? null;
}

/** The profile's folder under the app's data; a bad id throws, so it can't name anything else. */
export function profileFolderName(profileId: string): string {
  return profilePartition(profileId).slice("persist:".length);
}

/** What main reads of the list: enough to refuse removing a profile the page hasn't marked, or a guarded one. */
export type SavedProfiles = { profiles: { id: string; deleting?: boolean }[]; defaultId: string };

/** Why a profile's data must not go; null when it may. The page marks it deleting and moves its tabs first. */
export function refusalToRemove(profileId: string, saved: SavedProfiles | null): string | null {
  if (!saved) return "The profile list couldn't be read";
  if (saved.defaultId === profileId) return "The default profile can't be deleted";
  const marked = saved.profiles.find((profile) => profile.id === profileId)?.deleting === true;
  if (!marked) return "The profile isn't marked for deleting";
  if (!saved.profiles.some((profile) => !profile.deleting)) return "The last profile can't be deleted";
  return null;
}

export type SitePermission = "ask" | "allow" | "block";
export type SiteDevice = "camera" | "microphone" | "location";
/** `location` is absent from what was saved before sites could ask for it. */
export type SiteChoice = { camera: SitePermission; microphone: SitePermission; location?: SitePermission; askedMs: number };
/** What a page asked for. */
export type SiteAsk = Record<SiteDevice, boolean>;
export type SiteAnswer = "grant" | "deny" | "ask";

/** A request goes by the site's saved choices for what it asks: a block refuses, all allowed grants. */
export function siteAnswer(site: SiteChoice | undefined, asked: SiteAsk): SiteAnswer {
  const devices = (["camera", "microphone", "location"] as const).filter((device) => asked[device]);
  if (devices.length === 0) return "deny";
  if (devices.some((device) => site?.[device] === "block")) return "deny";
  return devices.every((device) => site?.[device] === "allow") ? "grant" : "ask";
}
