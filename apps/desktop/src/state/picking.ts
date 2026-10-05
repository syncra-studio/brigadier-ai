import { create } from "zustand";

/** A list whose rows can be picked for a bulk action. */
export type PickList = "sidebar" | "archived";

type Picked = {
  list: PickList | null;
  /** In the order they were picked. */
  ids: string[];
  /** Where a Shift-click's range starts: the row clicked last. */
  anchor: string | null;
};

/**
 * Rows picked for a bulk action (Archive, Delete, Unarchive) with ⌘-click (Ctrl-click off
 * macOS) and Shift-click, in one list at a time.
 */
export const usePicked = create<Picked>(() => ({ list: null, ids: [], anchor: null }));

export function clearPicked(): void {
  if (usePicked.getState().ids.length > 0) usePicked.setState({ list: null, ids: [] });
}

/** Drops picked rows of `list` that no longer show in it (archived, deleted, filed away). */
export function prunePicked(list: PickList, shown: string[]): void {
  const state = usePicked.getState();
  if (state.list !== list) return;
  const ids = state.ids.filter((id) => shown.includes(id));
  if (ids.length !== state.ids.length) usePicked.setState({ ids });
}

/** The picked rows of `list`, in the order the list shows them. */
export function pickedIn(list: PickList, shown: string[]): string[] {
  const state = usePicked.getState();
  return state.list === list ? shown.filter((id) => state.ids.includes(id)) : [];
}

/**
 * A click on row `id` of `list`, whose rows show in `shown`: ⌘-click (Ctrl-click off macOS)
 * picks or unpicks it, Shift-click picks the rows from the last one clicked (else `start`) to
 * it. A plain click lets go of the picked rows and returns false: the row then does what it
 * does.
 */
export function pickClick(
  list: PickList,
  id: string,
  shown: string[],
  event: { metaKey: boolean; ctrlKey: boolean; shiftKey: boolean },
  mac: boolean,
  start: string | null = null,
): boolean {
  const state = usePicked.getState();
  const same = state.list === list;
  if (event.shiftKey) {
    const anchor = (same ? state.anchor : null) ?? start ?? id;
    const from = shown.indexOf(anchor);
    const to = shown.indexOf(id);
    const ids =
      from < 0 || to < 0 ? [id] : shown.slice(Math.min(from, to), Math.max(from, to) + 1);
    usePicked.setState({ list, ids, anchor: from < 0 ? id : anchor });
    return true;
  }
  if (mac ? event.metaKey : event.ctrlKey) {
    const ids = same ? state.ids : [];
    usePicked.setState({
      list,
      ids: ids.includes(id) ? ids.filter((entry) => entry !== id) : [...ids, id],
      anchor: id,
    });
    return true;
  }
  usePicked.setState({ list, ids: [], anchor: id });
  return false;
}

/** The conversations the Delete confirmation asks about; it is open while set. */
export const useDeleteAsk = create<{ ids: string[] | null }>(() => ({ ids: null }));

export function askDelete(ids: string[]): void {
  if (ids.length > 0) useDeleteAsk.setState({ ids });
}
