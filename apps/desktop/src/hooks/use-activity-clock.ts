import { useEffect, useRef, useState, useSyncExternalStore } from "react";

let now = Date.now();
let timer: ReturnType<typeof setInterval> | undefined;
const listeners = new Set<() => void>();
const snapshot = () => now;
const idle = () => () => {};

function update() {
  now = Date.now();
  for (const listener of listeners) listener();
}

function visibilityChanged() {
  if (timer !== undefined) clearInterval(timer);
  timer = undefined;
  if (listeners.size && !document.hidden) {
    update();
    timer = setInterval(update, 1000);
  }
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  if (listeners.size === 1) {
    document.addEventListener("visibilitychange", visibilityChanged);
    visibilityChanged();
  }
  return () => {
    listeners.delete(listener);
    if (!listeners.size) {
      if (timer !== undefined) clearInterval(timer);
      timer = undefined;
      document.removeEventListener("visibilitychange", visibilityChanged);
    }
  };
}

/** One clock across visible active rows; offscreen rows and hidden tabs cost no ticks. */
export function useActivityClock(active: boolean) {
  const ref = useRef<HTMLDivElement>(null);
  const [initialNow] = useState(() => Date.now());
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    const element = ref.current;
    if (!active || !element) return;
    const observer = new IntersectionObserver(([entry]) => setVisible(entry?.isIntersecting ?? false));
    observer.observe(element);
    return () => observer.disconnect();
  }, [active]);
  const time = useSyncExternalStore(active && visible ? subscribe : idle, snapshot, snapshot);
  return { ref, now: Math.max(time, initialNow) };
}
