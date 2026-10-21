import { size } from "@/app/settings/storage/model";

/** A size at a Storage row's end, in its fixed column. */
export function StorageSize({ bytes }: { bytes: number }) {
  return <span className="text-foreground w-18 shrink-0 text-right text-xs font-medium">{size(bytes)}</span>;
}
