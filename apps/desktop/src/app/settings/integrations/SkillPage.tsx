import { SettingsPage, SettingsSwitch } from "@/app/settings/parts";
import { SettingsCard } from "@/components/shared";
import { InfoRow, IntegrationNote, IntegrationSection, OFF_NOTE, SESSION_NOTE, SkillMark, WorkerRow } from "./parts";
import type { IntegrationSkill } from "./types";

export function SkillPage({ skill, skills, enabled, busy, onEnabled, onBack, onShowPath }: {
  skill: IntegrationSkill; skills: IntegrationSkill[]; enabled: boolean; busy?: boolean; onEnabled: (enabled: boolean) => void; onBack: () => void; onShowPath: (path: string) => void;
}) {
  const always = !skill.canSwitchOff && skill.nativeAgents.includes("claude");
  const duplicate = skills.find((other) => other.name === skill.name && other.id !== skill.id);
  const locations: NonNullable<IntegrationSkill["locationDetails"]> = skill.locationDetails ?? skill.locations.map((path) => ({ path, agents: skill.nativeAgents }));
  return <SettingsPage title={skill.name} description={skill.description} mark={<SkillMark id={skill.id} large />} back={{ label: "Plugins", onBack }} className="max-w-plugins pt-0" sectionsClassName="gap-9 pt-4" actions={<label className="text-foreground/65 flex items-center gap-2.5 text-label">{always ? "Codex workers" : enabled ? "On" : "Off"}<SettingsSwitch label={always ? `${skill.name}: Codex workers` : skill.name} checked={enabled} disabled={!!busy || (!skill.canSwitchOff && !always)} onCheckedChange={onEnabled} /></label>}>
    {always && <IntegrationNote>Claude can't hide a skill in ~/.claude/skills for one session, so Claude workers always have this one. The switch decides whether Brigadier gives it to Codex workers.</IntegrationNote>}
    {duplicate && <IntegrationNote>Another skill is called {skill.name} too{duplicate.project ? `, in ${duplicate.project.split("/").pop()}` : ""}. They're two separate skills, each with its own switch.</IntegrationNote>}
    <IntegrationSection title="Folders" aside={locations.length > 1 ? `One skill, in ${locations.length} folders` : undefined}><SettingsCard>{locations.map((location) => <InfoRow key={location.path} title={location.path} titleMono value={`${location.linkedTo ? `Linked to ${location.linkedTo} · ` : location.path.includes("/.agents/") ? "Shared by several agents · " : ""}${location.agents.map((agent) => agent === "claude" ? "Claude" : "Codex").join(" and ")} reads this folder`} mono={false} show="folder" onShow={() => onShowPath(location.path)} />)}</SettingsCard></IntegrationSection>
    <IntegrationSection title="Workers" description={always ? undefined : "Every worker gets every skill. It costs a worker almost nothing until it uses it."}><SettingsCard>{(["claude", "codex"] as const).map((agent) => {
      const native = skill.nativeAgents.includes(agent);
      const locked = agent === "claude" && always;
      const name = agent === "claude" ? "Claude" : "Codex";
      const location = locations.find((folder) => folder.agents.includes(agent))?.path ?? locations[0]?.path ?? "";
      const parent = location.slice(0, location.lastIndexOf("/"));
      const line = locked ? `Find it in ${parent}, and Claude can't hide it for one session` : !enabled ? `Off: Brigadier doesn't give it to ${name} workers` : native ? `Find it in ${parent}` : `${name} doesn't read ${parent}, so Brigadier gives it to them`;
      return <WorkerRow key={agent} agent={agent} line={line} status={!enabled && !locked ? "Off" : native ? "Finds it" : "Given by Brigadier"} tone={enabled && native ? "success" : "secondary"} always={locked} dim={!enabled && !locked} />;
    })}</SettingsCard></IntegrationSection>
    <p className="text-foreground/50 text-xs leading-integration">{always ? SESSION_NOTE : OFF_NOTE}</p>
  </SettingsPage>;
}
