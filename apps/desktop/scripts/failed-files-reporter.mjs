// A `node --test` reporter that writes the files with a failed test as a JSON list, so
// run-tests.mjs can run them once more under CI.
export default async function* failedFiles(source) {
  const files = new Set();
  for await (const event of source) {
    if (event.type === "test:fail" && event.data.file) files.add(event.data.file);
  }
  yield JSON.stringify([...files]);
}
