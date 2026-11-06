/**
 * Projects' icons, asked of the daemon (which looks in each project's first repository) once per
 * project per run. A project with none, still being asked, or whose file won't load has its
 * fallback.
 */
import { useEffect, useState } from "react";

import { request } from "@/ipc/client";

const known = new Map<string, string | null>();
const asked = new Map<string, Promise<void>>();

function ask(projectId: string): Promise<void> {
  let asking = asked.get(projectId);
  if (!asking) {
    asking = request({ method: "projectIcon", projectId }).then(
      (answer) => void known.set(projectId, answer.icon ?? null),
      // The next row to show asks again; meanwhile it has its fallback.
      () => void asked.delete(projectId),
    );
    asked.set(projectId, asking);
  }
  return asking;
}

/**
 * The project's icon as a `data:` URL, once known and until `broken` says it won't load; `null`
 * otherwise.
 */
export function useProjectIcon(projectId: string | null): { icon: string | null; broken: () => void } {
  const [, answered] = useState(0);
  const [failed, setFailed] = useState<string | null>(null);
  useEffect(() => {
    if (projectId === null || known.has(projectId)) return;
    let live = true;
    void ask(projectId).then(() => {
      if (live) answered((count) => count + 1);
    });
    return () => {
      live = false;
    };
  }, [projectId]);
  const found = projectId === null ? null : (known.get(projectId) ?? null);
  return { icon: found === failed ? null : found, broken: () => setFailed(found) };
}
