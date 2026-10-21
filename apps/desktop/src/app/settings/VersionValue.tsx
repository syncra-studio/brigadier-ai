/** The version About shows: "Brigadier" and its number, once the app has said it. */
export function VersionValue({ version }: { version: string | undefined }) {
  return (
    <span className="text-muted-foreground text-label tabular-nums">
      {version ? `Brigadier ${version}` : "Brigadier"}
    </span>
  );
}
