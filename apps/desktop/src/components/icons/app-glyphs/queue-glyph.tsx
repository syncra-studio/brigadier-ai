/** The queue glyph: lines of a list with an arrow into them. */
export function QueueGlyph({ className }: { className?: string }) {
  return (
    <svg aria-hidden viewBox="0 0 20 20" fill="currentColor" className={className}>
      <path d="M9.75 4.25h6.5a.75.75 0 0 1 0 1.5h-6.5a.75.75 0 0 1 0-1.5Zm0 5h6.5a.75.75 0 0 1 0 1.5h-6.5a.75.75 0 0 1 0-1.5Zm0 5h6.5a.75.75 0 0 1 0 1.5h-6.5a.75.75 0 0 1 0-1.5ZM3.5 6.6a.5.5 0 0 1 .8-.4l3.9 3.4a.5.5 0 0 1 0 .8l-3.9 3.4a.5.5 0 0 1-.8-.4Z" />
    </svg>
  );
}
