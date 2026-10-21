import { cn } from "@/lib/utils";

/** An error the daemon (or validation) reported, shown where it happened. */
export function ErrorLine({ error, className }: { error: string | null; className?: string }) {
  if (!error) return null;
  return (
    <p role="alert" className={cn("text-destructive text-xs whitespace-pre-wrap", className)}>
      {error}
    </p>
  );
}
