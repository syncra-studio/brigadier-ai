import { File, FileCode, FileDocument, FileImage } from "@/components/icons";
import type { ArtifactRef } from "@/ipc/generated";

import { isText } from "@/app/side-panel/artifactFiles";

export function FileGlyph({ artifact }: { artifact: ArtifactRef }) {
  const className = "text-foreground/50 size-icon-sm shrink-0";
  if (artifact.mime.startsWith("image/")) return <FileImage aria-hidden className={className} />;
  if (artifact.mime === "text/markdown" || artifact.mime === "text/plain") return <FileDocument aria-hidden className={className} />;
  if (isText(artifact)) return <FileCode aria-hidden className={className} />;
  return <File aria-hidden className={className} />;
}
