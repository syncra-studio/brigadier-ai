// The launch smoke check (`--smoke`), hosted: the page measures, crates/shell-native judges, this prints the
// report, writes it to BRIGADIER_SMOKE_REPORT if set, and quits with its verdict.
import fs from "node:fs";

import { addCommands, RawJson } from "./commands";
import { coldStartMs } from "./core";
import { native } from "./native";
import { quit } from "./quit";
import { mainWindow } from "./windows";

const REPORT_ENV = "BRIGADIER_SMOKE_REPORT";
const WATCHDOG_MS = 120_000;

type SmokeFacts = { smoke: boolean; firstLaunch: boolean; budgetTolerance: number };

function write(report: string): void {
  const file = process.env[REPORT_ENV];
  if (!file) return;
  try {
    fs.writeFileSync(file, report);
  } catch (error) {
    console.error("could not write the smoke report:", error);
  }
}

export function installSmoke({ smoke, firstLaunch, budgetTolerance }: SmokeFacts): void {
  addCommands({
    smoke_finish: async ({ measurements }) => {
      if (!smoke) throw { code: "invalid", message: "not running the smoke check" };
      const asked = await native().request(JSON.stringify({ method: "getDiagnostics" }));
      if (asked.error !== undefined && asked.error !== null) throw JSON.parse(asked.error);
      const { diagnostics } = JSON.parse(asked.response ?? "{}") as { diagnostics: unknown };
      const json = native().evaluateSmoke(budgetTolerance, coldStartMs() ?? Number.MAX_VALUE, firstLaunch, JSON.stringify(measurements), JSON.stringify(diagnostics));
      const pretty = JSON.stringify(JSON.parse(json) as unknown, null, 2);
      console.log(pretty);
      write(pretty);
      const { passed } = JSON.parse(json) as { passed: boolean };
      quit(mainWindow, passed ? 0 : 1);
      return new RawJson(json);
    },
  });
  if (!smoke) return;
  setTimeout(() => {
    console.error(`smoke check timed out after ${WATCHDOG_MS} ms`);
    write('{"passed":false,"error":"timed out"}');
    quit(mainWindow, 2);
    // If even the quit hangs, leave anyway.
    setTimeout(() => process.exit(2), 10_000);
  }, WATCHDOG_MS);
}
