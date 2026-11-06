// The app's half of saved passwords on a page: only here is it known whether an agent drives a tab (isDriven). A tab it
// drives gets no fill and no offer; a tab the user has taken over counts as theirs, and fills on the takeover.
import { isDriven, useAgentPause } from "@/state/agentPause";
import { answerSaveOffer, dropSaveOffer, showSaveOffer, useSaveOffers } from "@/state/browserPasswords";

const failed = (cause: unknown) => console.error("saved passwords on a page failed", cause);

export function installGuestLogins(tabOf: (guest: number) => string | null): void {
  /** Tabs whose page has sign-in boxes it couldn't fill because an agent drove them, by guest. */
  const waiting = new Map<string, number>();
  const fill = (tab: string, guest: number) => {
    if (isDriven(tab)) {
      waiting.set(tab, guest);
      return;
    }
    waiting.delete(tab);
    window.brigadierShell.vault("fill", { guest }).catch(failed);
  };

  window.brigadierShell.onGuestLogin(({ from, kind, id, profile, site, username, update }) => {
    if (kind === "offer-gone") {
      if (id) dropSaveOffer(id);
      return;
    }
    const tab = tabOf(from);
    if (kind === "fields") {
      if (tab) fill(tab, from);
      return;
    }
    if (kind !== "offer" || !id) return;
    if (!tab || isDriven(tab)) {
      window.brigadierShell.vault("answer", { offer: id, answer: "notNow" }).catch(failed);
      return;
    }
    showSaveOffer({ id, tab, profile: profile ?? "", site: site ?? "", username: username ?? "", update: update === true });
  });

  useAgentPause.subscribe(() => {
    for (const [tab, guest] of waiting) if (!isDriven(tab)) fill(tab, guest);
    // Continue hands the tab back: an offer still up goes, and its password with it.
    for (const offer of useSaveOffers.getState().offers) {
      if (isDriven(offer.tab)) answerSaveOffer(offer, "notNow").catch(failed);
    }
  });
}
