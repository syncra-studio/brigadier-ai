import type { DiffStat, FileStat } from "@/ipc/generated";

/** Diff stats summed, file by file: null for none. */
export function mergeStats(stats: readonly DiffStat[]): DiffStat | null {
  if (stats.length === 0) return null;
  const files = new Map<string, FileStat>();
  for (const file of stats.flatMap((stat) => stat.files)) {
    const seen = files.get(file.path);
    files.set(
      file.path,
      seen
        ? {
            ...seen,
            insertions: seen.insertions + file.insertions,
            deletions: seen.deletions + file.deletions,
            binary: seen.binary || file.binary,
          }
        : file,
    );
  }
  return {
    files: [...files.values()],
    insertions: stats.reduce((sum, stat) => sum + stat.insertions, 0),
    deletions: stats.reduce((sum, stat) => sum + stat.deletions, 0),
  };
}
