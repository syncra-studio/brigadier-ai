import type { ReactNode } from "react";

import { SettingsSection } from "@/app/settings/parts";
import { Badge, Button } from "@/components/controls";
import { ClaudeGlyph, IdGlyph, Info, Lock, ProviderGlyph } from "@/components/icons";
import { CopyButton } from "@/components/shared";
import { cn } from "@/lib/utils";
import type { IntegrationAgent, IntegrationSource } from "@/app/settings/integrations/types";

export const SESSION_NOTE = "A change takes effect the next time a session starts or resumes, not in one already mid-turn.";
export const OFF_NOTE = `Turning it off stops Brigadier giving it to any worker or the lead. ${SESSION_NOTE}`;
export const SOURCE_NAMES: Record<IntegrationSource, string> = {
  ClaudeUser: "Claude Code", ClaudeLocal: "Claude Code", ClaudeProjectFile: "Claude Code",
  CodexUser: "Codex", CodexProject: "Codex", Desktop: "Claude Desktop", CursorUser: "Cursor", CursorProject: "Cursor",
};
export function sourcePath(source: IntegrationSource, project?: string): string {
  switch (source) {
    case "ClaudeUser": return "~/.claude.json";
    case "ClaudeLocal": return `${project ?? ""}/.claude/settings.local.json`;
    case "ClaudeProjectFile": return `${project ?? ""}/.mcp.json`;
    case "CodexUser": return "~/.codex/config.toml";
    case "CodexProject": return `${project ?? ""}/.codex/config.toml`;
    case "Desktop": return "~/Library/Application Support/Claude/claude_desktop_config.json";
    case "CursorUser": return "~/.cursor/mcp.json";
    case "CursorProject": return `${project ?? ""}/.cursor/mcp.json`;
  }
}
export function AgentMarks({ agents }: { agents: IntegrationAgent[] }) {
  return <span className="flex shrink-0 items-center gap-0.75" aria-label={agents.map((a) => a === "claude" ? "Claude" : "Codex").join(", ")}>
    {agents.map((agent) => <ProviderGlyph key={agent} provider={agent} className="text-foreground/75 size-3" />)}
  </span>;
}
export function ClaudeAlways({ compact = false }: { compact?: boolean }) {
  return <span title="Claude: always on" className={cn("bg-foreground/5 text-foreground/65 rounded-capsule inline-flex h-5 shrink-0 items-center text-tiny", compact ? "gap-1 px-1.75" : "gap-1.25 px-2")}>
    <Lock className="size-2.75" />{compact ? <ClaudeGlyph className="size-2.75" /> : "Claude: always on"}
  </span>;
}
export function SkillMark({ id, large = false, dim = false }: { id: string; large?: boolean; dim?: boolean }) {
  return <span className={cn("bg-foreground/5 flex shrink-0 items-center justify-center", large ? "size-11 rounded-xl" : "size-9 rounded-toolbar-button")}>
    <IdGlyph id={id} className={cn(large ? "size-5.5" : "size-4.5", dim && "opacity-50")} />
  </span>;
}
export function IntegrationSection({ title, description, aside, children }: { title: string; description?: string | undefined; aside?: string | undefined; children: ReactNode }) {
  return <SettingsSection title={title} description={description} aside={aside && <span className="text-muted-foreground">{aside}</span>} setting={title} className="[&>div:first-child]:min-h-0">{children}</SettingsSection>;
}
export function InfoRow({ title, value, mono = true, titleMono = false, error = false, show, onShow }: {
  title: string; value: string; mono?: boolean; titleMono?: boolean; error?: boolean; show?: "file" | "folder"; onShow?: () => void;
}) {
  return <div data-setting={title} className="flex items-center gap-6 px-4 py-3">
    <div className="flex min-w-0 flex-1 flex-col gap-1">
      <span className={cn("text-label font-medium", titleMono && "font-mono text-xs", error && "text-destructive")}>{title}</span>
      <span className={cn("text-foreground/65 break-words text-xs leading-integration-value", mono && "font-mono")}>{value}</span>
    </div>
    {show ? <Button variant="soft" size="sm" onClick={onShow}>Show {show}</Button> : mono && <CopyButton text={value} size="xs" />}
  </div>;
}
export function WorkerRow({ agent, line, status, tone = "secondary", always = false, dim = false }: {
  agent: IntegrationAgent; line: string; status: string; tone?: "secondary" | "success" | "warning"; always?: boolean; dim?: boolean;
}) {
  return <div className="flex items-center gap-3 px-4 py-3">
    <span className={cn("bg-foreground/5 rounded-control flex size-7 shrink-0 items-center justify-center", dim && "opacity-50")}><ProviderGlyph provider={agent} className="text-foreground/85 size-3.5" /></span>
    <div className="flex min-w-0 flex-1 flex-col gap-0.75"><span className="text-label font-medium">{agent === "claude" ? "Claude" : "Codex"} workers</span><span className="text-foreground/65 text-xs leading-integration">{line}</span></div>
    {always ? <ClaudeAlways /> : <Badge variant={tone}>{status}</Badge>}
  </div>;
}
export function IntegrationNote({ children }: { children: ReactNode }) {
  return <div className="bg-foreground/5 text-foreground/65 rounded-control flex items-start gap-2 px-3 py-2.5 text-xs leading-integration"><Info className="size-3.5 shrink-0" /><div>{children}</div></div>;
}
