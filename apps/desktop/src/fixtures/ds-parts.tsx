/**
 * The design system's merged small parts (cleanup §3, groups 6–10), one block per group, each
 * read back as the browser drew it into #parts-result for DsPartsRender.test.ts.
 */
import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { Picker } from "@/app/inspector/providers/Picker";
import { TooltipProvider } from "@/components/ui/tooltip";

const one = (selector: string) => document.querySelector(selector);

/** A dropdown trigger's words, whether it is off, and how it sits in its grid cell. */
function picker(label: string) {
  const trigger = one(`[aria-label="${label}"]`) as HTMLButtonElement | null;
  return (
    trigger && {
      text: trigger.textContent,
      disabled: trigger.disabled,
      justify: getComputedStyle(trigger).justifySelf,
    }
  );
}

function read() {
  return {
    picker: {
      chosen: picker("Session"),
      unpicked: picker("Window"),
      empty: picker("Task"),
    },
  };
}

const WINDOWS = [
  { value: "five_hour", label: "five_hour" },
  { value: "seven_day", label: "seven_day" },
];

function Fixture() {
  useEffect(() => {
    const result = document.createElement("pre");
    result.id = "parts-result";
    result.textContent = JSON.stringify(read());
    document.body.append(result);
    return () => result.remove();
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-card flex flex-col gap-6 p-6">
        <section data-group="picker" className="grid w-80 grid-cols-[auto_1fr] gap-2">
          <Picker label="Session" value="seven_day" options={WINDOWS} onChange={() => {}} />
          <Picker
            label="Window"
            value=""
            options={WINDOWS}
            onChange={() => {}}
            placeholder="Pick…"
            className="max-w-full justify-self-start"
          />
          <Picker label="Task" value="" options={[]} onChange={() => {}} placeholder="Pick…" />
        </section>
      </div>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
