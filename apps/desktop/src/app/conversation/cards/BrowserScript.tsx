/**
 * An eval card's body: where it runs and what it can do, then the script exactly as it will run. Plain text in a
 * block, with no code spans taken out of it, since what the user reads here is all they have to judge it by.
 */
export function BrowserScript({ details }: { details: string }) {
  const at = details.indexOf("\n\n");
  const [where, script] = at < 0 ? ["", details] : [details.slice(0, at), details.slice(at + 2)];
  return (
    <div data-slot="browser-script" className="mx-3 flex flex-col gap-2">
      {where && <p className="text-foreground text-sm font-normal wrap-anywhere">{where}</p>}
      <pre className="bg-muted/60 text-foreground max-h-72 overflow-auto rounded-md p-2 font-mono text-xs whitespace-pre-wrap wrap-anywhere">{script}</pre>
    </div>
  );
}
