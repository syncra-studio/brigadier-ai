import { type ComponentType, lazy, useState } from "react";

/**
 * A view whose code loads on demand, off the cold-start path, that can be loaded ahead of time
 * (`preload`: at idle, or when the pointer heads for what opens it). Once loaded it renders at
 * once: a lazy view suspends on its first render even when its code is already there, and React
 * then holds the fallback (nothing) for a moment before showing it. Shown before its code
 * arrived, it loads like any lazy view, inside a Suspense boundary.
 */
export function preloadable<P extends object>(
  load: () => Promise<ComponentType<P>>,
): { View: ComponentType<P>; preload: () => Promise<ComponentType<P>> } {
  let loaded: ComponentType<P> | null = null;
  let loading: Promise<ComponentType<P>> | null = null;
  const preload = () =>
    (loading ??= load().then(
      (component) => (loaded = component),
      (error: unknown) => {
        // A failed load is tried again the next time.
        loading = null;
        throw error;
      },
    ));
  const Lazy = lazy(() => preload().then((component) => ({ default: component })));
  function View(props: P) {
    // Chosen once per mount: the code arriving meanwhile must not remount what shows.
    const [Shown] = useState<ComponentType<P>>(() => loaded ?? Lazy);
    return <Shown {...props} />;
  }
  return { View, preload };
}

/** Runs `run` once the app has had `afterMs` to itself and then the main thread is idle. */
export function whenIdle(run: () => void, afterMs = 1000): () => void {
  let idle: number | null = null;
  const timer = window.setTimeout(() => {
    // WebKit has no idle callback: the delay alone stands in for it there.
    if (typeof window.requestIdleCallback === "function") idle = window.requestIdleCallback(run, { timeout: 2000 });
    else run();
  }, afterMs);
  return () => {
    window.clearTimeout(timer);
    if (idle !== null) window.cancelIdleCallback(idle);
  };
}
