import type { ArtifactRef } from "@/ipc/generated";

/** Whether the in-app viewer can show it (text); everything else opens in its own app. */
export function isText(artifact: ArtifactRef): boolean {
  return artifact.mime.startsWith("text/") || artifact.mime === "application/json";
}

const EXTENSIONS: Record<string, string> = {
  "text/markdown": ".md",
  "text/x-diff": ".diff",
  "text/plain": ".txt",
  "application/json": ".json",
  "image/png": ".png",
  "image/jpeg": ".jpg",
};

/** The name a file is opened and saved under. */
export function fileNameOf(artifact: ArtifactRef): string {
  return artifact.fileName ?? `${artifact.id.slice(0, 12)}${EXTENSIONS[artifact.mime] ?? ""}`;
}
