import type { ComponentProps, FC } from "react";
import { defaultComponents } from "@/components/conversation/markdown-text";

type TableData = { title: string; columns: string[]; rows: string[][]; source: string };
const text = (value: unknown, max: number): value is string => typeof value === "string" && value.length <= max;

export function readTable(value: unknown): TableData | null {
  if (!value || typeof value !== "object") return null;
  const data = value as TableData;
  if (
    !text(data.title, 300) || !text(data.source, 1000) ||
    !Array.isArray(data.columns) || data.columns.length < 1 || data.columns.length > 12 ||
    !data.columns.every((column) => text(column, 200)) ||
    !Array.isArray(data.rows) || data.rows.length > 200 ||
    !data.rows.every((row) => Array.isArray(row) && row.length === data.columns.length && row.every((cell) => text(cell, 2000)))
  ) return null;
  return data;
}

const part = <K extends "table" | "tr" | "th" | "td">(name: K) =>
  (defaultComponents as Record<string, unknown>)[name] as FC<ComponentProps<K>>;

export function VisualTable({ value }: { value: unknown }) {
  const data = readTable(value);
  if (!data) return <p role="alert">This table has invalid rows or columns. Ask for a new table.</p>;
  const [Table, Tr, Th, Td] = [part("table"), part("tr"), part("th"), part("td")] as const;
  return (
    <section aria-label={data.title}>
      <p className="font-medium">{data.title}</p>
      <Table>
        <caption className="sr-only">{data.title}</caption>
        <thead><Tr>{data.columns.map((column, at) => <Th key={at} scope="col">{column}</Th>)}</Tr></thead>
        <tbody>{data.rows.map((row, at) => <Tr key={at}>{row.map((cell, col) => <Td key={col}>{cell}</Td>)}</Tr>)}</tbody>
      </Table>
      {data.rows.length === 0 && <p>No rows to display.</p>}
      <p className="text-xs text-muted-foreground">{data.source}</p>
    </section>
  );
}
