// The Rust addon (crates/shell-native), loaded once. The bridge to the daemon lives there.
import { createRequire } from "node:module";
import path from "node:path";

export type Started = { platform: string; processStartMs: number; firstLaunch: boolean; dataDir: string };
export type OpeningDecision = { kind: "open" | "reveal" | "refuse"; detail?: string };

type Addon = {
  start(
    options: { identifier: string; dataDir?: string; sidecarDir?: string; daemonEnv: string[][] },
    onShell: (json: string) => void,
  ): Promise<Started>;
  request(json: string): Promise<{ response?: string; error?: string }>;
  attachUi(onEvent: (json: string) => void): void;
  attachSetupUi(onEvent: (json: string) => void): void;
  emit(json: string): void;
  openConversation(conversationId: string): void;
  shutdownDaemon(timeoutMs: number): Promise<boolean>;
  setInFront(inFront: boolean): void;
  setQuitting(quitting: boolean): void;
  decideOpening(target: string, withApp: boolean): OpeningDecision;
  evaluateSmoke(tolerance: number, coldStartMs: number, firstLaunch: boolean, measurements: string, diagnostics: string): string;
  systemName(): string;
  continuityReport(): Promise<string>;
  openSimulator(device?: string | null): Promise<void>;
  shiftHeld(): boolean;
  noticePermission(): Promise<string>;
  noticeAsk(): void;
  locationStatus(): "off" | "undetermined" | "denied" | "restricted" | "allowed" | "unknown";
};

let loaded: Addon | null = null;

export function native(): Addon {
  if (!loaded) {
    const file = process.env.BRIGADIER_ADDON ?? path.join(process.resourcesPath, "shell_native.node");
    loaded = createRequire(import.meta.url)(file) as Addon;
  }
  return loaded;
}
