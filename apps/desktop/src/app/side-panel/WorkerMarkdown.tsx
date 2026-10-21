import { useWorkerText } from "@/app/conversation/WorkerChip";
import { MarkdownBlock } from "@/components/conversation";

/** A worker's words as markdown, its handles named. */
export function WorkerMarkdown({ text, streaming = false }: { text: string; streaming?: boolean }) {
  return <MarkdownBlock text={useWorkerText(text)} streaming={streaming} />;
}
