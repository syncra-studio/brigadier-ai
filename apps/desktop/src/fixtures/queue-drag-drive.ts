import { fixtureClick, fixtureInput } from "@/fixtures/input";

const rows = () => [...document.querySelectorAll<HTMLElement>("[data-queue-item-id]")];
const ids = () => rows().map((row) => row.dataset.queueItemId);
const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
async function until(check: () => boolean) {
  const end = performance.now() + 10_000;
  while (!check()) {
    if (performance.now() > end) throw new Error(`Queue did not settle: ${ids()}`);
    await new Promise((resolve) => setTimeout(resolve, 16));
  }
}

export async function driveQueue(demo: { calls: { method: string; itemId?: string; index?: number }[]; reorderWhileDragging: () => void }) {
  const seen: Record<string, unknown> = {};
  try {
    await until(() => rows().length === 5);
    // Let the queue's entrance animation finish before measuring pointer travel.
    await new Promise((resolve) => setTimeout(resolve, 300));
    const snapshots: (string | undefined)[][] = [];
    const snapshot = () => snapshots.push(ids());
    const observer = new MutationObserver(snapshot);
    observer.observe(document.querySelector("[data-slot=message-queue]")!, { childList: true });
    snapshot();
    const grip = rows()[0]!.querySelector<HTMLElement>("[aria-label='Drag to reorder']")!;
    const first = rows()[0]!.getBoundingClientRect();
    const handle = grip.getBoundingClientRect();
    const x = handle.x + handle.width / 2;
    const y = handle.y + handle.height / 2;
    await fixtureInput({ type: "pointer", x, y });
    await fixtureInput({ type: "pointerDown", x, y });
    await fixtureInput({ type: "pointer", x, y: y + 8 });
    await frame();
    seen.followsPointer = Math.abs(rows()[0]!.getBoundingClientRect().top - first.top - 8) < 1;
    const destination = rows()[3]!.getBoundingClientRect();
    const dropY = destination.y + destination.height / 2 + 2;
    await fixtureInput({ type: "pointer", x, y: dropY });
    await frame();
    seen.neighborSlides = Math.abs(Number.parseFloat(rows()[1]!.style.translate.split(" ").at(-1)!)) > 1;
    await fixtureInput({ type: "pointerUp", x, y: dropY });
    await until(() => ids().join() === "q2,q3,q4,q1,q5");
    await new Promise((resolve) => setTimeout(resolve, 350));
    seen.reordered = ids();
    seen.moves = demo.calls.filter((call) => call.method === "moveQueued");

    // Original regression: a new daemon/optimistic order arrives during the next drag.
    // The old hook filtered q2, then inserted items[0] (now q3), creating duplicate q3 keys.
    const next = rows()[0]!.querySelector<HTMLElement>("[aria-label='Drag to reorder']")!;
    const rect = next.getBoundingClientRect();
    const point = { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 };
    await fixtureInput({ type: "pointer", ...point });
    await fixtureInput({ type: "pointerDown", ...point });
    demo.reorderWhileDragging();
    await frame();
    seen.duringUpdate = ids();
    await fixtureInput({ type: "pointerUp", ...point });
    await frame();
    seen.afterUpdate = ids();
    seen.snapshots = snapshots;
    observer.disconnect();

    const beforeDelete = ids();
    const toDelete = rows().find((row) => row.dataset.queueItemId === "q3")!;
    await fixtureClick([...toDelete.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent === "Delete queued message")!);
    await until(() => rows().length === 4);
    seen.beforeDelete = beforeDelete;
    seen.afterDelete = ids();
    seen.deletes = demo.calls.filter((call) => call.method === "deleteQueued").map((call) => call.itemId);
  } catch (error) {
    seen.error = String(error);
    seen.calls = demo.calls;
  }
  const result = document.createElement("pre");
  result.id = "queue-drag-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}
