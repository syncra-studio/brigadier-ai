/**
 * The app's shared dialog parts, drawn unseen, for a fixture to compare a dialog with: its
 * card, title (and the project dialogs' own), buttons, field, search field, key hint, badge, tooltip, choice pills and list-row colours, each
 * marked `data-canon`. `look` reads what an element looks like as drawn, so a dialog drawn with
 * the app's parts reads the same as the part it uses (#385).
 */
import { PROJECT_DIALOG_CARD, PROJECT_DIALOG_TITLE } from "@/app/dialogs/fields";
import { SettingsSearch } from "@/components/shared/settings-search";
import { Badge } from "@/components/controls/badge";
import { Button } from "@/components/controls/button";
import { Checkbox } from "@/components/controls/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/overlays/dialog";
import { Input } from "@/components/controls/input";
import { Kbd } from "@/components/controls/kbd";
import { Segmented } from "@/components/controls/segmented";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/overlays/tooltip";

/** The shared parts, unseen. Mount it only to measure: its dialog is one more open layer. */
export function Canon() {
  return (
    <TooltipProvider>
      <Dialog open modal={false}>
        <DialogContent
          data-canon="dialog"
          showCloseButton={false}
          className="invisible"
          onOpenAutoFocus={(event) => event.preventDefault()}
          onCloseAutoFocus={(event) => event.preventDefault()}
        >
          <DialogTitle data-canon="title">Canon</DialogTitle>
          <DialogDescription data-canon="line">Canon</DialogDescription>
        </DialogContent>
      </Dialog>
      <Dialog open modal={false}>
        <DialogContent
          data-canon="project-dialog"
          showCloseButton={false}
          className={`${PROJECT_DIALOG_CARD} invisible`}
          onOpenAutoFocus={(event) => event.preventDefault()}
          onCloseAutoFocus={(event) => event.preventDefault()}
        >
          <DialogTitle data-canon="project-title" className={PROJECT_DIALOG_TITLE}>
            Canon
          </DialogTitle>
        </DialogContent>
      </Dialog>
      <div className="invisible fixed start-0 top-0" aria-hidden>
        <Button data-canon="primary">Canon</Button>
        <Button data-canon="ghost" variant="ghost">
          Canon
        </Button>
        <Button data-canon="outline" variant="outline">
          Canon
        </Button>
        <Button data-canon="icon" variant="ghost" size="icon-sm">
          C
        </Button>
        <Input data-canon="input" placeholder="Canon" />
        <Checkbox data-canon="checked" checked />
        <div data-canon="search">
          <SettingsSearch label="Canon" value="" onChange={() => {}} />
        </div>
        <Kbd data-canon="kbd">↵</Kbd>
        <Badge data-canon="badge" variant="secondary">
          Canon
        </Badge>
        <div data-canon="chosen" className="bg-accent text-accent-foreground">
          Canon
        </div>
        <div data-canon="wash" className="bg-foreground/5">
          Canon
        </div>
        <div data-canon="divider" className="border-divider border-y" />
        <div data-canon="segmented">
          <Segmented
            fill
            label="Canon"
            value="a"
            options={[
              { value: "a", label: "A" },
              { value: "b", label: "B" },
            ]}
            onChange={() => {}}
          />
        </div>
        <Tooltip open>
          <TooltipTrigger>Canon</TooltipTrigger>
          <TooltipContent data-canon="tooltip">Canon</TooltipContent>
        </Tooltip>
      </div>
    </TooltipProvider>
  );
}

/** What each kind of part is drawn with. */
const PROPS = {
  surface: ["backgroundColor", "borderTopLeftRadius", "boxShadow", "borderTopWidth", "borderTopColor"],
  button: ["backgroundColor", "color", "borderTopLeftRadius", "borderTopWidth"],
  /** A button whose ink is its own (the project dialogs' close and back icons). */
  buttonShape: ["backgroundColor", "borderTopLeftRadius", "borderTopWidth"],
  text: ["color", "fontSize", "fontWeight", "fontFamily"],
  fill: ["backgroundColor"],
  ink: ["color"],
  /** A hairline above. */
  edgeTop: ["borderTopColor", "borderTopWidth"],
  /** A hairline below. */
  edgeBottom: ["borderBottomColor", "borderBottomWidth"],
} as const satisfies Record<string, readonly (keyof CSSStyleDeclaration)[]>;

type Kind = keyof typeof PROPS;

/** `element`'s look as drawn: the properties a `kind` of part is drawn with. */
export function look(element: Element | null | undefined, kind: Kind): Record<string, string> | null {
  if (!element) return null;
  const style = getComputedStyle(element);
  return Object.fromEntries(PROPS[kind].map((name) => [name, String(style[name])]));
}

/** The look of the canonical part marked `name` (a selector inside it with `inside`). */
export function canon(name: string, kind: Kind, inside = ""): Record<string, string> | null {
  return look(document.querySelector(`[data-canon="${name}"] ${inside}`.trim()), kind);
}

/** Each part, its look and the canonical one's, side by side: `[mine, canon]`. */
export type LookPairs = Record<string, [Record<string, string> | null, Record<string, string> | null]>;
