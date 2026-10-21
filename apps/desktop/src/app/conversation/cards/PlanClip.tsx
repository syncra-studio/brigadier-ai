import { useLayoutEffect, useRef, useState } from "react";

import { PlanText } from "@/app/conversation/cards/PlanText";

/** The start of a plan, clipped; the fade shows only over a plan that goes on past the clip. */
export function PlanClip({ markdown }: { markdown: string }) {
  const clip = useRef<HTMLDivElement>(null);
  const [clipped, setClipped] = useState(false);
  useLayoutEffect(() => {
    const element = clip.current;
    const text = element?.firstElementChild;
    if (!element || !text) return undefined;
    const measure = () => setClipped(element.scrollHeight > element.clientHeight + 1);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(text);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return (
    <div ref={clip} data-clipped={clipped || undefined} className="plan-clip data-clipped:plan-clip-fade">
      <PlanText markdown={markdown} />
    </div>
  );
}
