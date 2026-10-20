import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { Timeline, type TimelineEvent } from "@/components/conversation/elements/timeline";

/** Merges in order, as the Reviews tab will list them: two landed, one still under review. */
const MERGES: TimelineEvent[] = [
  { id: "m1", when: "past", time: "02:14", title: "Merged 3 commits into main", detail: "Reviewed · 5 fixed · checks passed" },
  { id: "m2", when: "past", time: "03:40", title: "Merged 1 commit into main", detail: "Reviewed · nothing found · checks passed" },
  { id: "m3", when: "now", time: "04:05", title: "Reviewing 2 commits", detail: "Reading the change" },
  { id: "m4", when: "future", time: "", title: "Merge into main" },
];

/** No times: the dots line up with the titles alone. */
const UNTIMED: TimelineEvent[] = [
  { id: "u1", when: "past", time: "", title: "Read the platform code" },
  { id: "u2", when: "now", time: "", title: "Implement Windows paths" },
];

function Fixture() {
  useEffect(() => {
    const [timed, untimed] = [...document.querySelectorAll<HTMLElement>('[data-slot="timeline"]')];
    const items = [...timed!.querySelectorAll("li")];
    const seen = {
      titles: items.map((item) => item.querySelector("div > span")!.textContent),
      current: items.map((item) => item.getAttribute("aria-current")),
      times: items.map((item) => item.firstElementChild!.textContent),
      // A line joins each event to the next; the last has none.
      lines: items.map((item) => item.querySelector("[aria-hidden]")!.children.length),
      untimedColumns: untimed!.querySelector("li")!.children.length,
    };
    const result = document.createElement("pre");
    result.id = "timeline-result";
    result.textContent = JSON.stringify(seen);
    document.body.append(result);
    return () => result.remove();
  }, []);
  return (
    <div className="flex w-96 flex-col gap-6 p-4">
      <Timeline events={MERGES} />
      <Timeline events={UNTIMED} />
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
