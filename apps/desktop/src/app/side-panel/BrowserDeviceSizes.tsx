import { useEffect, useRef, useState } from "react";

import { TooltipIconButton } from "@/components/controls";
import { Reload, X } from "@/components/icons";
import { browserGo } from "@/ipc/client";
import { useAgentPause } from "@/state/agentPause";
import {
  applyDevice,
  changeDev,
  clearDevice,
  CUSTOM_SIZE,
  DEVICE_SIZES,
  devOf,
  type DeviceSize,
  useBrowserDev,
} from "@/state/browserDev";

const FIELD =
  "bg-muted/60 ring-border h-control-sm w-16 rounded-md px-2 text-center text-sm tabular-nums ring-1 ring-inset outline-none";

/** The default size: a phone, what the tool is mostly opened for. */
const FIRST = DEVICE_SIZES[1]!;

/** Turns device sizes on or off for tab `id`. */
export function toggleDevice(id: string): void {
  const { device } = devOf(useBrowserDev.getState().tabs, id);
  changeDev(id, { device: device ? null : FIRST });
}

function SizeField({ label, value, onChange }: { label: string; value: number; onChange: (value: number) => void }) {
  const [typed, setTyped] = useState<string | null>(null);
  const commit = () => {
    const next = Number.parseInt(typed ?? "", 10);
    setTyped(null);
    if (Number.isFinite(next) && next >= 100 && next <= 4000 && next !== value) onChange(next);
  };
  return (
    <input
      aria-label={label}
      inputMode="numeric"
      className={FIELD}
      value={typed ?? value}
      onChange={(event) => setTyped(event.target.value)}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === "Enter") commit();
        if (event.key === "Escape") setTyped(null);
      }}
    />
  );
}

/** The strip under the toolbar while device sizes are on: the device, its width and height, turning it, and leaving. */
export function BrowserDeviceBar({ id }: { id: string }) {
  const device = useBrowserDev((s) => devOf(s.tabs, id).device);
  if (!device) return null;
  const change = (next: DeviceSize) => changeDev(id, { device: next });
  const known = DEVICE_SIZES.some((each) => each.name === device.name);
  return (
    <div data-slot="browser-device-bar" className="border-border flex h-10 shrink-0 items-center justify-center gap-2 border-b px-2 text-sm">
      <select
        aria-label="Device"
        className="bg-muted/60 ring-border h-control-sm max-w-44 rounded-md px-2 ring-1 ring-inset outline-none"
        value={known ? device.name : CUSTOM_SIZE}
        onChange={(event) => {
          const picked = DEVICE_SIZES.find((each) => each.name === event.target.value);
          if (picked) change(picked);
        }}
      >
        {DEVICE_SIZES.map((each) => <option key={each.name} value={each.name}>{each.name}</option>)}
        {!known && <option value={CUSTOM_SIZE}>{CUSTOM_SIZE}</option>}
      </select>
      <SizeField label="Width" value={device.width} onChange={(width) => change({ ...device, name: CUSTOM_SIZE, width })} />
      <span className="text-muted-foreground">×</span>
      <SizeField label="Height" value={device.height} onChange={(height) => change({ ...device, name: CUSTOM_SIZE, height })} />
      <TooltipIconButton
        tooltip="Turn the device"
        size="icon-md"
        className="shrink-0 rounded-full"
        onClick={() => change({ ...device, width: device.height, height: device.width })}
      >
        <Reload />
      </TooltipIconButton>
      <TooltipIconButton tooltip="Leave device sizes" size="icon-md" className="shrink-0 rounded-full" onClick={() => changeDev(id, { device: null })}>
        <X />
      </TooltipIconButton>
    </div>
  );
}

/**
 * Keeps the tab's page as `device` says, shown at `scale`. A changed user agent loads the page again, the one way a page
 * learns it. Continue on a driven tab sets it again too, as L128 restores emulation there.
 */
export function useDeviceSize(id: string, device: DeviceSize | null, scale: number, ready: boolean): void {
  const paused = useAgentPause((s) => id in s.paused);
  const told = useRef<string | null>(null);
  useEffect(() => {
    if (!ready || !device) return;
    const key = JSON.stringify([device, scale, paused]);
    if (key === told.current) return;
    told.current = key;
    void applyDevice(id, device, scale).then((changed) => (changed ? browserGo(id, "reload") : undefined)).catch(() => {});
  }, [id, device, scale, ready, paused]);
  useEffect(() => {
    if (!ready || device || told.current === null) return;
    told.current = null;
    void clearDevice(id).then((changed) => (changed ? browserGo(id, "reload") : undefined)).catch(() => {});
  }, [id, device, ready]);
}
