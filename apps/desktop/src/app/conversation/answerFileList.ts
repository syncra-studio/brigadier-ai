import type { ArtifactRef, Task } from "@/ipc/generated";

import { fileNameOf } from "@/app/conversation/FileList";

/**
 * The files a finished request's workers left in their outputs folders, offered with Save under
 * its answer (BUILD-PLAN A2): one per file name, the latest task's copy, in the order the tasks
 * started.
 */
export function answerFiles(tasks: readonly Task[], requestIds: readonly string[]): ArtifactRef[] {
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
  return [...byName.values()];
}
