import { Button } from "@/components/controls";

/** Start a raw session's Record switch: a recorded session is saved as a replay fixture. */
export function RecordToggle({ record, onRecordChange }: { record: boolean; onRecordChange: (record: boolean) => void }) {
  return (
    <Button
      type="button"
      size="xs"
      variant={record ? "secondary" : "ghost"}
      aria-pressed={record}
      onClick={() => onRecordChange(!record)}
    >
      {record ? "Recording" : "Record"}
    </Button>
  );
}
