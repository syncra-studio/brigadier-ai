/**
 * Which composer attachment an open view of pasted text `n` shows. Its own edit swaps the
 * attachment for a new one under the same number: that is still the text it shows. Any other
 * change (the text was sent or removed, and a new paste may have taken its number) is another
 * text: `generation` moves on and the view reads it afresh.
 */
export type PasteBinding = { id: string | undefined; editing: boolean; generation: number };

export const bound = (id: string | undefined): PasteBinding => ({ id, editing: false, generation: 0 });

/** The view puts its own edit in the attachment's place. */
export const editing = (binding: PasteBinding): PasteBinding => ({ ...binding, editing: true });

/** Number `n` now names attachment `id` (none: no attachment has it). */
export function rebound(binding: PasteBinding, id: string | undefined): PasteBinding {
  if (id === binding.id) return binding;
  if (binding.editing) return { id, editing: id === undefined, generation: binding.generation };
  return { id, editing: false, generation: binding.generation + 1 };
}
