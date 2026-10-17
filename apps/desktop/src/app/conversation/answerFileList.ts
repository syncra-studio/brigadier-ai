import type { ArtifactRef, Task } from "@/ipc/generated";

import { fileNameOf } from "@/app/conversation/FileList";

/**
 * The files a finished request's answer hands over, offered with Save under it (BUILD-PLAN A2):
 * the stored outputs of its workers that the answer links (`outputs/notes.md`, or the path in a
 * worker's scratch folder), one per file name, the latest task's copy, in the order the tasks
 * started. The rest of the workers' outputs (screenshots, scripts, notes) stay on their task
 * cards (#218). A file the answer also links by its destination name or link label somewhere
 * else (the thread's corrected copy in its workspace, BUILD-PLAN A6) is the deliverable, so the
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
  const { linked, corrected } = linkedNames(answer, scratch);
  // `outputs/task-3/notes.md` names task-3's copy, whichever later task has one of that name: the
  // thread's corrected copy of task-3's file (#27).
  const pinned = [...linked].flatMap((stored) => {
    const found = /^task-(\d+)\/(?:.*\/)?([^/]+)$/.exec(stored);
    return found?.[1] && found[2] ? [{ number: Number(found[1]), name: found[2] }] : [];
  });
  for (const { number, name } of pinned) {
    const output = ordered.find((task) => task.number === number)?.outputs.find((each) => fileNameOf(each) === name);
    if (output) {
      byName.set(name, output);
      linked.add(name);
    }
  }
  return [...byName.entries()].filter(([name]) => linked.has(name) && !corrected.has(name)).map(([, output]) => output);
}

/**
 * What `answer` links: `linked`, the stored outputs' names (a link to `outputs/…` or into a
 * worker's outputs folder); `corrected`, the destination names and original-file labels of the
 * file links elsewhere, each a copy of its own.
 */
function linkedNames(answer: string, scratch: readonly string[]): { linked: Set<string>; corrected: Set<string> } {
  const linked = new Set<string>();
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
    if (path.startsWith("#")) continue;
    const stored = /^(?:\.\/)?outputs\/(.+)$/.exec(path)?.[1]
      ?? scratch.flatMap((root) => (path.startsWith(`${root}/outputs/`) ? [path.slice(root.length + 9)] : []))[0];
    if (stored !== undefined) {
      linked.add(stored);
      continue;
    }
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
  return { linked, corrected: new Set(names) };
}

/** ".md" of "rates.md", lower-cased; "" for a name without one. */
function extension(name: string): string {
  return /\.[^.]+$/.exec(name)?.[0].toLowerCase() ?? "";
}
