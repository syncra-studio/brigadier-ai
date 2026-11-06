/**
 * Projects' icons, asked of the daemon (which looks in each project's first repository) once per
 * project per run.
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

/** The project's icon as a `data:` URL, once known; `null` meanwhile and when it has none. */
export function useProjectIcon(projectId: string | null): string | null {
  const [, answered] = useState(0);
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
  return projectId === null ? null : (known.get(projectId) ?? null);
}
