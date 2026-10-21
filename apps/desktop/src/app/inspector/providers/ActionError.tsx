export function ActionError({ error }: { error: string | null }) {
  if (!error) return null;
  return <p className="text-destructive text-xs">{error}</p>;
}
