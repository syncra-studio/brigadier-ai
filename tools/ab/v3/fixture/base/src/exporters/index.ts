import type { Invoice } from '../invoice.ts';
import { exportJson } from './json.ts';

/** Turns a list of invoices into the full text of an export file. */
export type Exporter = (invoices: Invoice[]) => string;

export const exporters: Record<string, Exporter> = {
  json: exportJson,
};

export function exportFormats(): string[] {
  return Object.keys(exporters).sort();
}

export function getExporter(format: string): Exporter {
  const exporter = Object.hasOwn(exporters, format) ? exporters[format] : undefined;
  if (!exporter) {
    throw new Error(`unknown export format: ${format} (available: ${exportFormats().join(', ')})`);
  }
  return exporter;
}
