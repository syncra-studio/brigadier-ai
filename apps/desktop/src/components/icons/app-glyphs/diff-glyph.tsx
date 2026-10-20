/** The diff glyph: a square with a plus over a minus. */
export function DiffGlyph({ className }: { className?: string }) {
  return (
    <svg aria-hidden viewBox="0 0 20 20" fill="currentColor" fillRule="evenodd" className={className}>
      <path d="M6.5 3h7A3.5 3.5 0 0 1 17 6.5v7a3.5 3.5 0 0 1-3.5 3.5h-7A3.5 3.5 0 0 1 3 13.5v-7A3.5 3.5 0 0 1 6.5 3Zm0 1.5a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h7a2 2 0 0 0 2-2v-7a2 2 0 0 0-2-2ZM10 6a.75.75 0 0 1 .75.75V8H12a.75.75 0 0 1 0 1.5h-1.25v1.25a.75.75 0 0 1-1.5 0V9.5H8A.75.75 0 0 1 8 8h1.25V6.75A.75.75 0 0 1 10 6Zm-2 6.25h4a.75.75 0 0 1 0 1.5H8a.75.75 0 0 1 0-1.5Z" />
    </svg>
  );
}
