/** Another node named in a node's detail: its title opens it; one not loaded shows its id. */
export function NodeLink({
  id,
  titles,
  onSelect,
}: {
  id: string;
  titles: ReadonlyMap<string, string>;
  onSelect: (id: string) => void;
}) {
  return titles.has(id) ? (
    <button type="button" className="text-link hover:text-link/80 text-start" onClick={() => onSelect(id)}>
      {titles.get(id)}
    </button>
  ) : (
    <span className="font-mono">{id}</span>
  );
}
