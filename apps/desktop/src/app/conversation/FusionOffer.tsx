import { useState } from "react";

import { OfferBody, OfferCard, OfferError } from "@/app/conversation/OfferCard";
import { useAction } from "@/app/conversation/useAction";
import { Button } from "@/components/controls";
import { Check, PullRequestMerged as Merge } from "@/components/icons";
import { startOfferedFusion } from "@/state/actions";
import { useBoard } from "@/state/board";
import { FUSION_HINT, isFusionChat, useFusionReady } from "@/state/fusionSend";
import { useApp } from "@/state/store";

export function FusionOfferCard({ question, asked, ready = true, busy = false, error, onAsk }: {
  question: string; asked: boolean; ready?: boolean; busy?: boolean; error?: string | null; onAsk: () => void;
}) {
  return (
    <OfferCard label="Ask every installed CLI?" kind="fusionOffered" icon={asked ? <Check className="text-success" /> : <Merge />} title={asked ? "Asked" : "Ask every installed CLI?"} line="One answer from each CLI, merged into one reply" actions={asked ? undefined : <Button size="sm" disabled={busy || !ready} onClick={onAsk}>Ask</Button>}>
      <OfferBody title="Fusion" text={question} />
      {!asked && !ready && <p className="text-muted-foreground text-xs">{FUSION_HINT}</p>}
      {error && <OfferError>{error}</OfferError>}
    </OfferCard>
  );
}

export function FusionOffer({ id, question }: { id: string; question: string }) {
  const conversationId = useBoard((s) => s.board?.conversationId ?? null);
  const taken = useBoard((s) => s.board?.orchestratorSteps.some((step) => step.kind.type === "fusionOfferTaken" && step.kind.id === id) ?? false);
  const conversation = useApp((s) => conversationId ? s.conversations[conversationId] : undefined);
  const ready = useFusionReady();
  const [asked, setAsked] = useState(false);
  const action = useAction();
  if (!conversationId || !conversation || !isFusionChat(conversation)) return null;
  return <FusionOfferCard question={question} asked={taken || asked} ready={ready} busy={action.busy} error={action.error} onAsk={() => action.run(() => startOfferedFusion(conversationId, id).then(() => setAsked(true)))} />;
}
