import { useKeptState } from "@/components/conversation";

/** Output this long shows its head and tail, with the lines between folded. */
const LONG_LINES = 40;
const SHOWN_LINES = 15;

/** A long output as its first and last lines, "… N lines …" between them, until opened in full. */
export function Output({ text }: { text: string }) {
  const [all, setAll] = useKeptState(`output:${text.length}`, false);
  const lines = text.split("\n");
  if (all || lines.length <= LONG_LINES) return <>{text}</>;
  const hidden = lines.length - 2 * SHOWN_LINES;
  return (
    <>
      {lines.slice(0, SHOWN_LINES).join("\n")}
      {"\n"}
      <button type="button" className="text-muted-foreground hover:text-foreground underline-offset-2 hover:underline" onClick={() => setAll(true)}>
        … {hidden} lines …
      </button>
      {"\n"}
      {lines.slice(-SHOWN_LINES).join("\n")}
    </>
  );
}
