import { useEffect, useState } from "react";

import { pullRequestKey } from "@/app/conversation/cliFacts";
import { request, type ResponseOf } from "@/ipc/client";
import { useThreadFact } from "@/state/cliThread";

export type PullRequestStatus = Pick<ResponseOf<"getPullRequest">, "pullRequest" | "authRequired">;
const EMPTY: PullRequestStatus = { pullRequest: null };

/** Mounted once per open session tab, including while its context card is hidden. */
export function usePullRequest(conversationId: string | null): PullRequestStatus {
  const pushed = useThreadFact(conversationId, pullRequestKey);
  const [found, setFound] = useState<{ id: string; status: PullRequestStatus } | null>(null);
  useEffect(() => {
    if (!conversationId) return;
    let current = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const refresh = async () => {
      let status: PullRequestStatus = EMPTY;
      try {
        status = await request({ method: "getPullRequest", conversationId });
      } catch {
        // Missing gh, a network error, or a non-GitHub checkout leaves the row out.
      }
      if (!current) return;
      setFound({ id: conversationId, status });
      if (status.pullRequest?.state !== "merged" && status.pullRequest?.state !== "closed") {
        timer = setTimeout(() => void refresh(), 60_000);
      }
    };
    void refresh();
    return () => {
      current = false;
      clearTimeout(timer);
    };
    // A finished push or gh pr command refreshes even a previously terminal PR.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [conversationId, pushed]);
  return found?.id === conversationId ? found.status : EMPTY;
}
