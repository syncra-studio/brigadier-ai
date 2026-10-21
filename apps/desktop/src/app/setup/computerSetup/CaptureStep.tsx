import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { SetupPanel } from "@/app/setup/computerSetup/SetupPanel";
import { Button, ErrorLine } from "@/components/controls";
import { confirmComputerCapture } from "@/state/computerUse";

export function CaptureStep() {
  const capture = useAction();
  const [problem, setProblem] = useState<string | null>(null);
  return (
    <SetupPanel title="Check screen capture">
      <p className="text-foreground/80">Brigadier takes one test picture of a window to make sure everything works.</p>
      <p className="text-foreground/60">
        macOS may ask whether Brigadier Computer Use may “bypass the system private window picker”. That lets it look
        at app windows without asking you each time. Choose Allow.
      </p>
      <div>
        <Button
          disabled={capture.busy}
          onClick={() =>
            capture.run(async () => {
              setProblem(await confirmComputerCapture());
            })
          }
        >
          {capture.busy ? "Taking a picture…" : "Take a test picture"}
        </Button>
      </div>
      {problem && <p className="text-warning">The test picture didn't work: {problem}. Try again.</p>}
      <ErrorLine error={capture.error} />
    </SetupPanel>
  );
}
