import { RawApprovalCard } from "@/app/inspector/providers/RawApprovalCard";
import type { ApprovalRequest } from "@/ipc/generated";

/** The raw session's approvals still waiting, over its composer. */
export function ApprovalsTray({
  sessionId,
  requests,
  act,
}: {
  sessionId: string;
  requests: readonly ApprovalRequest[];
  act: (action: () => Promise<void>) => void;
}) {
  return (
    <div className="flex max-h-60 shrink-0 flex-col gap-2 overflow-y-auto border-t p-2">
      {requests.map((request) => (
        <RawApprovalCard key={request.id} sessionId={sessionId} request={request} act={act} />
      ))}
    </div>
  );
}
