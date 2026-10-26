import { defineConfig } from "vitest/config";

// Keep Vitest cases separate from the existing node:test files.
export default defineConfig({
  test: { include: ["src/**/*.vitest.ts"], environment: "happy-dom" },
  oxc: { jsx: { runtime: "automatic" } },
});
