/** A phase's "done when" lines. */
export function DoneWhen({ lines }: { lines: readonly { id: string; text: string }[] }) {
  if (lines.length === 0) return null;
  return (
    <div>
      <p className="font-medium">Done when</p>
      <ul className="list-inside list-disc space-y-1">
        {lines.map((line) => (
          <li key={line.id}>{line.text}</li>
        ))}
      </ul>
    </div>
  );
}
