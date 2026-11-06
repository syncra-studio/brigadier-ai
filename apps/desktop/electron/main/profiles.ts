// A browser profile is a partition. A guest's partition can't change once it attaches, so a tab
// carries its profile id from the moment it's made.
const PREFIX = "persist:browser-profile-";

export function profilePartition(profileId: string): string {
  if (!/^[a-z0-9-]{1,64}$/.test(profileId)) throw new Error(`Bad profile id: ${profileId}`);
  return `${PREFIX}${profileId}`;
}

/** Whether a session's storage path belongs to a browser profile rather than the app's own pages. */
export function isProfilePartition(storagePath: string | null): boolean {
  return storagePath !== null && storagePath.includes("browser-profile-");
}
