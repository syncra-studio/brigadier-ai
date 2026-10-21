import { SessionRow } from "@/app/inspector/providers/SessionRow";
import { Section } from "@/app/inspector/providers/shared";
import { useApp } from "@/state/store";

export function SessionList() {
  const sessions = useApp((s) => s.providers.view?.sessions ?? []);
  return (
    <Section title="Raw sessions">
      {sessions.length === 0 ? (
        <p className="text-muted-foreground text-xs">
          None yet. Start one above{import.meta.env.DEV ? ", replay a fixture or simulate a usage limit" : " or replay a fixture"}.
        </p>
      ) : (
        <ul className="flex flex-col">
          {sessions.map((session) => (
            <SessionRow key={session.id} session={session} />
          ))}
        </ul>
      )}
    </Section>
  );
}
