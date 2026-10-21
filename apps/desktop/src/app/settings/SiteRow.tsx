import { SettingsSelect } from "@/components/controls";
import { Camera, Mic } from "@/components/icons";
import type { BrowserSite, SitePermission } from "@/ipc/generated";

const CHOICES = [
  { value: "ask", label: "Ask" },
  { value: "allow", label: "Allow" },
  { value: "block", label: "Block" },
] as const satisfies readonly { value: SitePermission; label: string }[];

/** One site in Site settings: its origin and what it may use, the camera and the microphone. */
export function SiteRow({
  site,
  onChoose,
}: {
  site: BrowserSite;
  onChoose: (device: "camera" | "microphone", choice: SitePermission) => void;
}) {
  return (
    <li className="flex min-h-14 flex-wrap items-center gap-x-4 gap-y-2 py-2">
      <span className="text-label min-w-0 flex-1 truncate" title={site.origin}>
        {site.origin.replace(/^https:\/\//, "")}
      </span>
      <span className="flex items-center gap-2">
        <Camera aria-hidden className="text-foreground/65 size-icon-sm" />
        <SettingsSelect
          label={`Camera for ${site.origin}`}
          value={site.camera}
          options={CHOICES}
          onChange={(choice) => onChoose("camera", choice)}
        />
      </span>
      <span className="flex items-center gap-2">
        <Mic aria-hidden className="text-foreground/65 size-icon-sm" />
        <SettingsSelect
          label={`Microphone for ${site.origin}`}
          value={site.microphone}
          options={CHOICES}
          onChange={(choice) => onChoose("microphone", choice)}
        />
      </span>
    </li>
  );
}
