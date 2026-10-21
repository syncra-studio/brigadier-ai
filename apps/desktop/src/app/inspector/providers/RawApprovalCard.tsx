import { Badge, Button } from "@/components/controls";
import type { ApprovalRequest } from "@/ipc/generated";
import { answerApproval } from "@/state/actions";

export function RawApprovalCard({
  sessionId,
  request,
  act,
}: {
  sessionId: string;
  request: ApprovalRequest;
  act: (action: () => Promise<void>) => void;
}) {
  const answer = (allow: boolean) =>
    act(() =>
      answerApproval(
        sessionId,
        request.id,
        allow ? { type: "allow" } : { type: "deny", message: "Denied by the user." },
      ),
    );
  return (
    <div className="border-warning/50 bg-warning/10 flex flex-col gap-1.5 rounded-lg border p-2.5 text-xs">
      <div className="flex items-center gap-2">
        <Badge variant="warning">{request.kind}</Badge>
        <span className="font-mono">{request.tool}</span>
        {request.escalation && <Badge variant="destructive">outside the sandbox</Badge>}
      </div>
      {request.command && <pre className="font-mono whitespace-pre-wrap">{request.command}</pre>}
      {request.paths.length > 0 && (
        <p className="font-mono">{request.paths.join("\n")}</p>
      )}
      {request.reason && <p className="text-muted-foreground">{request.reason}</p>}
      <div className="flex justify-end gap-2">
        <Button size="xs" variant="outline" onClick={() => answer(false)}>
          Deny
        </Button>
        <Button size="xs" onClick={() => answer(true)}>
          Allow
        </Button>
      </div>
    </div>
  );
}
