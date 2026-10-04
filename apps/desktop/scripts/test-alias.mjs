// Lets `node --test` load the app's pure modules as the bundler does: `@/x` is `src/x`, and an
// import without an extension finds its `.ts` file. Node strips the types itself; modules with
// JSX or browser-only imports stay out of the tests.
import { statSync } from "node:fs";
import { registerHooks } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";

const src = fileURLToPath(new URL("../src/", import.meta.url));

registerHooks({
  resolve(specifier, context, nextResolve) {
    // Leave dependency resolution (including CommonJS named exports) to Node.
    if (context.parentURL?.includes("/node_modules/")) return nextResolve(specifier, context);
    const base = specifier.startsWith("@/")
      ? `${src}${specifier.slice(2)}`
      : specifier.startsWith(".") && context.parentURL?.startsWith("file:")
        ? fileURLToPath(new URL(specifier, context.parentURL))
        : null;
    if (base === null) return nextResolve(specifier, context);
    const file = [base, `${base}.ts`, `${base}/index.ts`].find(
      (candidate) => statSync(candidate, { throwIfNoEntry: false })?.isFile(),
    );
    return nextResolve(file ? pathToFileURL(file).href : specifier, context);
  },
});
