import type { ArtifactRef, Task } from "@/ipc/generated";

import { fileNameOf } from "@/app/conversation/FileList";

/**
 * The files a finished request's workers left in their outputs folders, offered with Save under
 * its answer (BUILD-PLAN A2): one per file name, the latest task's copy, in the order the tasks
 * started. A file the answer links by the same name somewhere else (the thread's corrected copy
 * in its workspace, BUILD-PLAN A6) replaces it: the answer's link is the deliverable, so the
 * worker's original isn't offered as if it were.
 */
export function answerFiles(tasks: readonly Task[], requestIds: readonly string[], answer = ""): ArtifactRef[] {
  const byName = new Map<string, ArtifactRef>();
  const ordered = tasks
    .filter((task) => task.requestId != null && requestIds.includes(task.requestId))
    .toSorted((a, b) => a.position - b.position);
  for (const task of ordered) {
    for (const output of task.outputs) {
      const name = fileNameOf(output);
      // A later task's file of the same name replaces the earlier one, in its place.
      byName.delete(name);
      byName.set(name, output);
    }
  }
  const scratch = ordered.flatMap((task) => (task.workspace?.scratch ? [task.workspace.scratch.replace(/\/$/, "")] : []));
  const corrected = new Set(correctedNames(answer, scratch));
  return [...byName.entries()].filter(([name]) => !corrected.has(name)).map(([, output]) => output);
}

/**
 * The file names `answer` links outside the workers' scratch folders: a link to `outputs/…` or
 * into a scratch folder names the stored output itself, any other file link a copy of its own.
 */
function correctedNames(answer: string, scratch: readonly string[]): string[] {
  const names: string[] = [];
  for (const [, href] of answer.matchAll(/\]\(<?([^)\s>]+)>?\)/g)) {
    if (!href || /^[a-z][a-z0-9+.-]+:/i.test(href.replace(/^file:\/\//, "/"))) continue;
    let path = href.replace(/^file:\/\//, "");
    try {
      path = decodeURIComponent(path);
    } catch {
      // Keep it as written.
    }
    path = path.replace(/(?::|#L)\d+(?:[-:]L?\d+)?$/, "");
    if (path.startsWith("#") || /^(?:\.\/)?outputs\//.test(path)) continue;
    if (scratch.some((root) => path === root || path.startsWith(`${root}/`))) continue;
    const name = path.split("/").pop();
    if (name) names.push(name);
  }
  return names;
}
