import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { ProgressLine } from "@/components/shared";

/** A download at a third, then each way a job can end. */
function Fixture() {
  useEffect(() => {
    const jobs = [...document.querySelectorAll<HTMLElement>('[data-slot="progress-line"]')];
    const seen = jobs.map((job) => {
      const bar = job.querySelector<HTMLElement>('[role="progressbar"]')!;
      return {
        state: job.dataset["state"],
        text: job.textContent,
        value: bar.getAttribute("aria-valuenow"),
        // The fill's share of the track, in whole percent.
        fill: Math.round((bar.firstElementChild!.getBoundingClientRect().width / bar.getBoundingClientRect().width) * 100),
        spinning: job.querySelector(".animate-spin, [class*='animate-spin']") !== null,
      };
    });
    const result = document.createElement("pre");
    result.id = "progress-line-result";
    result.textContent = JSON.stringify(seen);
    document.body.append(result);
    return () => result.remove();
  }, []);
  return (
    <div className="flex w-96 flex-col gap-6 p-4">
      <ProgressLine title="Downloading…" done={47} total={141} meta="47.0 MB of 141.0 MB" />
      <ProgressLine title="Windows support" done={6} total={6} meta="5h 12m" outcome={{ status: "success" }} />
      <ProgressLine
        title="Windows support"
        done={4}
        total={6}
        outcome={{ status: "partial", summary: "Four of six phases are verified." }}
      />
      <ProgressLine title="Windows support" done={2} total={6} outcome={{ status: "failed" }} />
      <ProgressLine title="Windows support" done={1} total={6} outcome={{ status: "cancelled" }} />
      <ProgressLine title="Nothing to do" done={0} total={0} />
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
