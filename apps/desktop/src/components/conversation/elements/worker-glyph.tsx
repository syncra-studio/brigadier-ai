import { IdGlyph } from "@/components/icons";
import { cn } from "@/lib/utils";

/**
 * A worker's own glyph and colour, the same wherever it appears; `working` adds its status dot,
 * `tone` recolours it for a state.
 */
export function WorkerGlyph({
  taskId,
  working,
  tone,
  className,
}: {
  taskId: string;
  working?: boolean | undefined;
  tone?: string | undefined;
  className?: string | undefined;
}) {
  return <IdGlyph id={taskId} dot={working} className={cn(className, tone)} />;
}
