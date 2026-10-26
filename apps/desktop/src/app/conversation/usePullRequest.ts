import { useEffect, useState } from "react";

import { request, type ResponseOf } from "@/ipc/client";

export type PullRequestStatus = Pick<ResponseOf<"getPullRequest">, "pullRequest" | "authRequired">;
const EMPTY: PullRequestStatus = { pullRequest: null };

/** Mounted once per open session tab, including while its context card is hidden. */
export function usePullRequest(conversationId: string | null): PullRequestStatus {
  const [found, setFound] = useState<{ id: string; status: PullRequestStatus } | null>(null);
  useEffect(() => {
    if (!conversationId) return;
    let current = true;

    const refresh = async () => {
      let status: PullRequestStatus = EMPTY;
      try {
        status = await request({ method: "getPullRequest", conversationId });
      } catch {
        // Missing gh, a network error, or a non-GitHub checkout leaves the row out.
      }
      if (!current) return;
      setFound({ id: conversationId, status });

    };
    void refresh();
    return () => {
      current = false;

    };
    // Initial branch discovery only; the daemon owns subsequent canonical observations.
  }, [conversationId]);
  return found?.id === conversationId ? found.status : EMPTY;
}
