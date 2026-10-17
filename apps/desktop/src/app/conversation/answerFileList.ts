import type { ArtifactRef, Task } from "@/ipc/generated";

import { fileNameOf } from "@/app/conversation/FileList";

/**
 * The files a finished request's workers left in their outputs folders, offered with Save under
 * its answer (BUILD-PLAN A2): one per file name, the latest task's copy, in the order the tasks
 * started. A file the answer links by its destination name or link label somewhere else
 * (the thread's corrected copy in its workspace, BUILD-PLAN A6) replaces it: the link is the
 * deliverable, so the
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
 * The destination names and original-file labels `answer` links outside the workers' scratch folders: a link to `outputs/…` or
 * into a scratch folder names the stored output itself, any other file link a copy of its own.
 */
function correctedNames(answer: string, scratch: readonly string[]): string[] {
  const names: string[] = [];
  for (const [, label, angled, plain] of answer.matchAll(/(?:\[([^\]\n]+))?\]\((?:<([^>\n]+)>|([^\s)]+))\)/g)) {
    const href = angled ?? plain;
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
    // A label naming the original also identifies a corrected copy saved under a new name, when
    // the copy is the same kind of file.
    if (label && name) {
      const original = label.replace(/^(`+|\*{1,2}|_{1,2})(.*?)\1$/, "$2").replace(/\\([!"#$%&'()*+,\-./:;<=>?@[\]\\^_`{|}~])/g, "$1")
        .replace(/(?::|#L)\d+(?:[-:]L?\d+)?$/, "").split("/").pop();
      if (original && extension(original) === extension(name)) names.push(original);
    }
  }
  return names;
}

/** ".md" of "rates.md", lower-cased; "" for a name without one. */
function extension(name: string): string {
  return /\.[^.]+$/.exec(name)?.[0].toLowerCase() ?? "";
}
