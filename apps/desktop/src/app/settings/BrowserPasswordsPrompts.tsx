import { type RefObject, useEffect, useState } from "react";
import { createPortal } from "react-dom";

import { BrowserPasswordsSavePrompt } from "@/app/settings/BrowserPasswordsSavePrompt";
import {
  answerSaveOffer,
  type OfferAnswer,
  type SaveOffer,
  useSaveOffers,
} from "@/state/browserPasswords";

function answer(offer: SaveOffer, choice: OfferAnswer): void {
  answerSaveOffer(offer, choice).catch((cause: unknown) => console.error("answering a password offer failed", cause));
}

/**
 * What a tab has to say about passwords, in the top end corner of `anchor`, its page's area. The
 * page lies over the app's content, so this goes at the end of the body and says where it is
 * (`data-above-page`); nothing shows while the tab doesn't.
 */
export function BrowserPasswordsPrompts({ tab, anchor }: { tab: string; anchor: RefObject<HTMLElement | null> }) {
  const offer = useSaveOffers((s) => s.offers.findLast((one) => one.tab === tab));
  const [corner, setCorner] = useState<{ end: number; top: number } | null>(null);
  const shown = offer !== undefined;

  useEffect(() => {
    if (!shown) return;
    let frame = 0;
    let placed = "";
    // The tab moves and resizes with the panels around it: followed every frame.
    const follow = () => {
      const element = anchor.current;
      const box = element && !element.closest("[inert]") ? element.getBoundingClientRect() : null;
      const rtl = document.dir === "rtl";
      const next = box && box.width > 0 && box.height > 0
        ? { end: (rtl ? box.left : window.innerWidth - box.right) + 12, top: box.top + 12 }
        : null;
      const key = JSON.stringify(next);
      if (key !== placed) {
        placed = key;
        setCorner(next);
      }
      frame = requestAnimationFrame(follow);
    };
    follow();
    return () => cancelAnimationFrame(frame);
  }, [anchor, shown]);

  if (!offer || !corner) return null;
  return createPortal(
    <div
      data-above-page
      data-slot="browser-password-prompts"
      className="fixed z-20 flex flex-col gap-2"
      style={{ insetInlineEnd: corner.end, top: corner.top }}
    >
      <BrowserPasswordsSavePrompt key={offer.id} offer={offer} onAnswer={(choice) => answer(offer, choice)} />
    </div>,
    document.body,
  );
}
