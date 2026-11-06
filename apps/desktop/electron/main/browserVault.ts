// A profile's saved passwords. One encrypted file in the profile's folder, so deleting a profile takes them with it and
// clearing its browsing data doesn't. Electron's safeStorage holds the key: on macOS a Keychain item named after the app
// ("Brigadier Safe Storage", identity.ts), DPAPI on Windows, the system's secret store on Linux. Nothing is kept in
// memory between calls. Only the app's own page reaches this (browserLogins.ts), no agent's call does, and nothing logs a value.
import fs from "node:fs";
import path from "node:path";

import { app, safeStorage } from "electron";

import { replaceFile } from "./durableStorage";
import { isDeleting } from "./profileStore";
import { profileFolderName } from "./profiles";

export type Login = { id: number; site: string; username: string; password: string; createdMs: number; usedMs: number };
type Vault = { nextId: number; logins: Login[]; never: string[] };

/** What section 5 hands over: an address (any page of the site), a username (may be empty) and a password. */
export type ImportedLogin = { url: string; username: string; password: string };
export type ImportedCounts = { added: number; updated: number; unchanged: number; skipped: number };

export const profileFile = (profileId: string, name: string): string =>
  path.join(app.getPath("userData"), "Partitions", profileFolderName(profileId), name);

const vaultFile = (profileId: string) => profileFile(profileId, "Brigadier Vault");

/** Why nothing can be stored here now; null when it can. Linux with no secret store refuses: a fixed key protects nothing. */
export function vaultRefusal(): string | null {
  if (!safeStorage.isEncryptionAvailable()) return "This computer's safe store isn't available, so passwords can't be saved.";
  if (process.platform === "linux" && safeStorage.getSelectedStorageBackend() === "basic_text") {
    return "No system keyring was found, so passwords can't be saved.";
  }
  return null;
}

function refuseIfBarred(profileId: string): void {
  if (isDeleting(profileId)) throw new Error("This profile is being deleted");
  const refusal = vaultRefusal();
  if (refusal) throw new Error(refusal);
}

/** The vault, or an empty one when there is no file. One that can't be opened throws, so a write can't replace it. */
export function readVault(profileId: string): Vault {
  refuseIfBarred(profileId);
  let sealed: Buffer;
  try {
    sealed = fs.readFileSync(vaultFile(profileId));
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return { nextId: 1, logins: [], never: [] };
    throw error;
  }
  return JSON.parse(safeStorage.decryptString(sealed)) as Vault;
}

export function writeVault(profileId: string, vault: Vault): void {
  refuseIfBarred(profileId);
  const file = vaultFile(profileId);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  replaceFile(file, safeStorage.encryptString(JSON.stringify(vault)));
}

/** The site a password belongs to: its origin, for http and https only. */
export function loginSite(url: string): string | null {
  try {
    const parsed = new URL(url);
    return parsed.protocol === "https:" || parsed.protocol === "http:" ? parsed.origin : null;
  } catch {
    return null;
  }
}

/** Saves one login: a known site and username takes the new password. */
export function putLogin(vault: Vault, site: string, username: string, password: string, now = Date.now()): "added" | "updated" | "unchanged" {
  const known = vault.logins.find((login) => login.site === site && login.username === username);
  if (!known) {
    vault.logins.push({ id: vault.nextId++, site, username, password, createdMs: now, usedMs: now });
    return "added";
  }
  if (known.password === password) return "unchanged";
  known.password = password;
  known.usedMs = now;
  return "updated";
}

/** Section 5's door: logins with no http(s) address or no password are counted and left. Nothing else is removed. */
export async function importLogins(profileId: string, logins: readonly ImportedLogin[]): Promise<ImportedCounts> {
  const vault = readVault(profileId);
  const counts: ImportedCounts = { added: 0, updated: 0, unchanged: 0, skipped: 0 };
  for (const login of logins) {
    const site = loginSite(login.url);
    if (!site || !login.password) {
      counts.skipped += 1;
      continue;
    }
    counts[putLogin(vault, site, login.username, login.password)] += 1;
  }
  if (counts.added + counts.updated > 0) writeVault(profileId, vault);
  return counts;
}
