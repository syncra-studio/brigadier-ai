export function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** The project dialogs' card: its edge drawn inside it, and the menus' shadow, as drawn. */
export const PROJECT_DIALOG_CARD = "shadow-menu ring-inset";

/** The project dialogs' title: larger and bolder than the app's other dialogs, as drawn. */
export const PROJECT_DIALOG_TITLE = "font-sans text-lg leading-5.25 font-semibold";
