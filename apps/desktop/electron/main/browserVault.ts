// STUB until section 6's branch lands: same name and call, keeps nothing past the run. Its file replaces this one.
import fs from "node:fs";
import path from "node:path";

import { app } from "electron";

import { dev } from "./identity";

export type ImportedLogin = {
  originUrl: string;
  actionUrl: string;
  signonRealm: string;
  usernameElement: string;
  username: string;
  passwordElement: string;
  password: string;
  scheme: number;
  createdAtMs: number | null;
  lastUsedAtMs: number | null;
};

export function importLogins(profileId: string, logins: readonly ImportedLogin[]): number {
  // A dev run's scratch data is made up; the check reads this to see it arrived in the right profile.
  if (dev) fs.writeFileSync(path.join(app.getPath("userData"), `stub-logins-${profileId}.json`), JSON.stringify(logins));
  return logins.length;
}
