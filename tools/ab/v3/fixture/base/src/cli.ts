import { readFileSync } from 'node:fs';
import { parseArgs } from 'node:util';
import { parseCart } from './cart.ts';
import { isValidDate } from './dates.ts';
import { getExporter } from './exporters/index.ts';
import { renderInvoiceText } from './invoice.ts';
import { formatMoney } from './money.ts';
import { renderReport, salesReport } from './report.ts';
import { createInvoiceFromCart, invoicesInRange } from './services.ts';
import { MemoryStore, readDataFile, writeDataFile } from './storage.ts';
import { isRegion, regions } from './tax.ts';

export interface Io {
  out(text: string): void;
  err(text: string): void;
}

const processIo: Io = {
  out: (text) => process.stdout.write(text),
  err: (text) => process.stderr.write(text),
};

export const USAGE = `usage: node src/cli.ts <command> --data <file> [options]

commands:
  products                              list the products in the data file
  invoice --cart <file> --region <code> --date <YYYY-MM-DD> [--terms <days>]
                                        create an invoice, save it, print it
  report [--from <date>] [--to <date>]  units and revenue per SKU
  export [--format <name>]              write all invoices (default format: json)
`;

class UsageError extends Error {}

function requireOption(values: Record<string, unknown>, name: string): string {
  const value = values[name];
  if (typeof value !== 'string' || value === '') {
    throw new UsageError(`missing --${name}`);
  }
  return value;
}

function requireDate(value: string, name: string): string {
  if (!isValidDate(value)) {
    throw new UsageError(`--${name} must be a date like 2024-01-31, got: ${value}`);
  }
  return value;
}

function commandProducts(dataPath: string, io: Io): void {
  const store = new MemoryStore(readDataFile(dataPath));
  for (const p of store.listProducts()) {
    io.out(`${p.sku.padEnd(10)}${p.name.padEnd(28)}${formatMoney(p.unitPrice).padStart(10)}  ${p.taxCategory}\n`);
  }
}

function commandInvoice(dataPath: string, values: Record<string, unknown>, io: Io): void {
  const cartPath = requireOption(values, 'cart');
  const region = requireOption(values, 'region');
  if (!isRegion(region)) {
    throw new UsageError(`unknown region: ${region} (known: ${regions().join(', ')})`);
  }
  const issueDate = requireDate(requireOption(values, 'date'), 'date');
  const termsText = typeof values.terms === 'string' ? values.terms : '30';
  if (!/^\d+$/.test(termsText)) {
    throw new UsageError(`--terms must be a whole number of days, got: ${termsText}`);
  }

  const store = new MemoryStore(readDataFile(dataPath));
  const cart = parseCart(JSON.parse(readFileSync(cartPath, 'utf8')));
  const invoice = createInvoiceFromCart(store, cart, { issueDate, termsDays: Number(termsText), region });
  writeDataFile(dataPath, store.toData());
  io.out(renderInvoiceText(invoice));
}

function commandReport(dataPath: string, values: Record<string, unknown>, io: Io): void {
  const from = typeof values.from === 'string' ? requireDate(values.from, 'from') : undefined;
  const to = typeof values.to === 'string' ? requireDate(values.to, 'to') : undefined;
  const store = new MemoryStore(readDataFile(dataPath));
  io.out(renderReport(salesReport(invoicesInRange(store, from, to))));
}

function commandExport(dataPath: string, values: Record<string, unknown>, io: Io): void {
  const format = typeof values.format === 'string' ? values.format : 'json';
  const exporter = getExporter(format);
  const store = new MemoryStore(readDataFile(dataPath));
  io.out(exporter(store.listInvoices()));
}

/** Runs the CLI and returns the process exit code. */
export function run(argv: string[], io: Io = processIo): number {
  let parsed;
  try {
    parsed = parseArgs({
      args: argv,
      allowPositionals: true,
      options: {
        data: { type: 'string' },
        cart: { type: 'string' },
        region: { type: 'string' },
        terms: { type: 'string' },
        date: { type: 'string' },
        from: { type: 'string' },
        to: { type: 'string' },
        format: { type: 'string' },
        help: { type: 'boolean', short: 'h' },
      },
    });
  } catch (error) {
    io.err(`error: ${(error as Error).message}\n${USAGE}`);
    return 2;
  }

  const { values, positionals } = parsed;
  const command = positionals[0];
  if (values.help || command === undefined || command === 'help') {
    io.out(USAGE);
    return 0;
  }

  try {
    if (positionals.length > 1) {
      throw new UsageError(`unexpected argument: ${positionals[1]}`);
    }
    const dataPath = requireOption(values, 'data');
    switch (command) {
      case 'products':
        commandProducts(dataPath, io);
        break;
      case 'invoice':
        commandInvoice(dataPath, values, io);
        break;
      case 'report':
        commandReport(dataPath, values, io);
        break;
      case 'export':
        commandExport(dataPath, values, io);
        break;
      default:
        throw new UsageError(`unknown command: ${command}`);
    }
    return 0;
  } catch (error) {
    if (error instanceof UsageError) {
      io.err(`error: ${error.message}\n${USAGE}`);
      return 2;
    }
    io.err(`error: ${(error as Error).message}\n`);
    return 1;
  }
}

if (import.meta.main) {
  process.exitCode = run(process.argv.slice(2));
}
