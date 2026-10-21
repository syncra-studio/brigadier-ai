import type { FC } from "react";

import type { PeerRef } from "@/app/conversation/activity/peerWords";
import { openConversation } from "@/state/actions";

/** A session a row names: a button that opens it (at the message, once it has it). */
export const PeerLink: FC<{ peer: PeerRef }> = ({ peer }) => (
  <button
    type="button"
    data-slot="peer-link"
    title={`Open ${peer.title}`}
    onClick={(event) => {
      event.stopPropagation();
      openConversation(peer.session, peer.message ? { at: { message: peer.message } } : {});
    }}
    className="text-row-object hover:text-foreground focus-visible:text-foreground rounded-control align-bottom"
  >
    {peer.title}
  </button>
);
