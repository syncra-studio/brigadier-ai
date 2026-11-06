import { create } from "zustand";

import { invoke } from "@/ipc/native";

/*
 * Saved passwords of the built-in browser, by profile. A list holds sites and usernames only: a
 * password reaches the app only when the user asks to see that one and the system check passes,
 * and never travels with an offer to save. No agent tool reads any of this.
 */

export type SavedPassword = {
  id: number;
  /** The address it fills on, such as `https://github.com`. */
  site: string;
  username: string;
};

/** What asking to see a password came to. */
export type Reveal =
  | { kind: "shown"; password: string }
  /** The user cancelled the system's check, or it failed. */
  | { kind: "declined" }
  /** This Mac has no Touch ID to ask, or it can't be used now. */
  | { kind: "unavailable" };

/** The browser saw a sign-in in a tab the user is driving. The password stays in main. */
export type SaveOffer = {
  id: string;
  /** The Browser tab it came from: it shows only over that tab's page. */
  tab: string;
  profile: string;
  site: string;
  username: string;
  /** The site and username are saved already, with another password. */
  update: boolean;
};

export type OfferAnswer = "save" | "notNow" | "never";

export function savedPasswords(profile: string): Promise<SavedPassword[]> {
  return invoke<SavedPassword[]>("browser_passwords", { profile });
}

export function addPassword(profile: string, site: string, username: string, password: string): Promise<void> {
  return invoke("browser_password_add", { profile, site, username, password });
}

export function removePassword(profile: string, id: number): Promise<void> {
  return invoke("browser_password_remove", { profile, id });
}

/** Runs the system's check where the platform has one, then gives the one password. */
export function revealPassword(profile: string, id: number): Promise<Reveal> {
  return invoke<Reveal>("browser_password_reveal", { profile, id });
}

/** The sites this profile never offers to save on. */
export function neverSites(profile: string): Promise<string[]> {
  return invoke<string[]>("browser_password_never_sites", { profile });
}

export function allowSite(profile: string, site: string): Promise<void> {
  return invoke("browser_password_allow_site", { profile, site });
}

/** The offers waiting, newest last. */
export const useSaveOffers = create<{ offers: SaveOffer[] }>(() => ({ offers: [] }));

export function showSaveOffer(offer: SaveOffer): void {
  useSaveOffers.setState(({ offers }) => ({ offers: [...offers.filter((other) => other.id !== offer.id), offer] }));
}

export function dropSaveOffer(id: string): void {
  useSaveOffers.setState(({ offers }) => ({ offers: offers.filter((offer) => offer.id !== id) }));
}

/** Answers an offer. Whatever the answer, main then forgets the password it held for it. */
export function answerSaveOffer(offer: SaveOffer, answer: OfferAnswer): Promise<void> {
  dropSaveOffer(offer.id);
  return invoke("browser_password_offer_answer", { offer: offer.id, answer });
}

/** A site as the lists show it: its host without "www.". */
export function passwordSite(site: string): string {
  try {
    return new URL(site).host.replace(/^www\./, "");
  } catch {
    return site;
  }
}

/** The passwords whose site or username holds every word of `query`, by site then username. */
export function findPasswords(passwords: readonly SavedPassword[], query: string): SavedPassword[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  return passwords
    .filter((entry) => {
      const text = `${passwordSite(entry.site)} ${entry.username}`.toLowerCase();
      return words.every((word) => text.includes(word));
    })
    .toSorted(
      (a, b) => passwordSite(a.site).localeCompare(passwordSite(b.site)) || a.username.localeCompare(b.username),
    );
}

/** What the user typed as a site, as an address: only http and https can hold a password. */
export function siteAddress(typed: string): string | null {
  const text = typed.trim();
  if (!text) return null;
  try {
    const url = new URL(/^[a-z][a-z0-9+.-]*:/i.test(text) ? text : `https://${text}`);
    return (url.protocol === "https:" || url.protocol === "http:") && url.host ? url.origin : null;
  } catch {
    return null;
  }
}
