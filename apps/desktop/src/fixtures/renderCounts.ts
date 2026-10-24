/**
 * Fixtures only: counts how often named components draw, through the hook React's developer
 * tools use. Import it before anything that imports React DOM, so React finds the hook as it
 * loads; it needs a development or profiling build (they time each component's work).
 *
 * At each commit it walks the tree: a component drew in that render when React began work on it
 * after the previous commit and its function ran (React's own "performed work" flag, which a
 * component that bailed out doesn't get).
 */

type Fiber = {
  type: unknown;
  flags: number;
  actualStartTime?: number;
  child: Fiber | null;
  sibling: Fiber | null;
};

/** React's flag for a fiber whose component function ran. */
const PERFORMED_WORK = 1;

const counted = new Set<string>();
const counts: Record<string, number> = {};
let lastCommit = 0;

function nameOf(type: unknown): string | null {
  if (typeof type === "function") return (type as { displayName?: string; name?: string }).displayName ?? type.name ?? null;
  if (type && typeof type === "object" && "type" in type) return nameOf((type as { type: unknown }).type);
  return null;
}

function walk(root: Fiber) {
  const stack: Fiber[] = [root];
  while (stack.length > 0) {
    const fiber = stack.pop()!;
    if ((fiber.flags & PERFORMED_WORK) !== 0 && (fiber.actualStartTime ?? -1) >= lastCommit) {
      const name = nameOf(fiber.type);
      if (name && counted.has(name)) counts[name] = (counts[name] ?? 0) + 1;
    }
    if (fiber.sibling) stack.push(fiber.sibling);
    if (fiber.child) stack.push(fiber.child);
  }
}

// oxlint-disable-next-line no-underscore-dangle -- React looks for its devtools hook by this exact name
(globalThis as unknown as { __REACT_DEVTOOLS_GLOBAL_HOOK__: object }).__REACT_DEVTOOLS_GLOBAL_HOOK__ = {
  renderers: new Map(),
  supportsFiber: true,
  isDisabled: false,
  inject: () => 1,
  checkDCE: () => {},
  onScheduleFiberRoot: () => {},
  onCommitFiberRoot: (_renderer: number, root: { current: Fiber }) => {
    if (counted.size > 0) walk(root.current);
    lastCommit = performance.now();
  },
  onPostCommitFiberRoot: () => {},
  onCommitFiberUnmount: () => {},
};

/** Starts counting these components' draws from now. */
export function countRenders(names: readonly string[]): void {
  for (const name of names) counted.add(name);
  for (const name of Object.keys(counts)) delete counts[name];
}

/** The draws counted since `countRenders`, by component, leaving out those that never drew. */
export function renderCounts(): Record<string, number> {
  return { ...counts };
}
