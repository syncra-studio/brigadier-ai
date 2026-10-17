/**
 * Brigadier lint plugin (oxlint JS plugin, ESLint v9 rule API).
 *
 * `brigadier/no-raw-design-values` keeps component code token-driven: every color and
 * size must come from styles/tokens.css, either through a Tailwind token utility
 * (`bg-muted`, `h-control-md`, `size-icon-sm`, `p-2`, `rounded-control`) or `var(--…)`.
 *
 * Rejected anywhere in string and template literals (className, cn()/cva() arguments,
 * variables holding class lists, style values):
 *   - hex colors (#fff, #0a0a0a, #0a0a0a80) and color functions (rgb(), hsl(), oklch(),
 *     oklab(), lab(), lch(), hwb(), color(), color-mix(), light-dark())
 *   - Tailwind arbitrary values or variants whose brackets hold a length, a percentage
 *     or a color (w-[2rem], text-[13px], p-[3px], max-w-[85%], bg-[#fff],
 *     bg-[rgb(1,2,3)], bg-[red], top-[calc(50%-2px)], [contain-intrinsic-size:auto_200px],
 *     max-[600px]:hidden)
 *   - Tailwind default palette classes (bg-red-500, text-white); the palette is removed
 *     from the theme, so these would silently render nothing
 *   - raw lengths written as CSS values ("12px", "1.5rem", "calc(100% - 2em)")
 * Rejected in JSX `style={{…}}` objects:
 *   - numeric literals for any property that is not unitless (padding: 12, width: 300)
 *   - named colors for color properties (color: "red")
 * Rejected on JSX attributes: numeric width/height/size (<Icon width={16} />), numeric
 * popover offsets and paddings (sideOffset={4}), and raw or named colors on color/fill/stroke.
 * The same size props are rejected as numeric parameter defaults (`sideOffset = 4`).
 *
 * Allowed: token utilities, var(--…) references (also as Tailwind shorthand `w-(--x)`),
 * `0`, keywords (auto, transparent, currentColor, inherit), percentages inside style
 * objects (pure layout, e.g. a progress fraction), timing values ([animation-delay:150ms]),
 * and template literals whose numbers come from expressions, e.g. a virtualizer's
 * `translateY(${start}px)`: the quasi "px)" carries no number of its own.
 *
 * `brigadier/thread-scroll-helper` keeps auto-follow right (thread-scroll.ts): code that scrolls
 * the thread's view on the user's behalf first says so with `jumpInThread(target)` (or the
 * scroller's own `release()`), or the scroller reads the move as the browser's and undoes it
 * while the view follows the bottom. A scroll (`scrollIntoView`, `scrollTo`, `scrollBy`, a
 * `scrollTop` set) counts as the thread's when it scrolls a `viewport`, or anywhere in a file
 * that works with the thread's view (it imports thread-scroll.ts or names the thread viewport's
 * slot); it passes once one of those calls came before it in its function or one around it.
 * A scroll there of something else (a list of its own) says why in a disable comment.
 */

const LENGTH_UNITS =
  "px|rem|em|ex|ch|lh|rlh|vh|vw|vmin|vmax|svh|lvh|dvh|svw|lvw|dvw|cqw|cqh|cqi|cqb|pt|pc|cm|mm|in|q";

// A number immediately followed by a length unit or a percent sign.
const RAW_LENGTH_IN_BRACKETS = new RegExp(`\\d*\\.?\\d+(?:${LENGTH_UNITS}|%)(?![a-z])`, "i");
// A standalone CSS length ("12px", "1.5rem", "-2em") as a value token.
const RAW_LENGTH_VALUE = new RegExp(
  `(?:^|[\\s(,/+*-])-?\\d*\\.?\\d+(?:${LENGTH_UNITS})(?=$|[\\s),/+*-])`,
  "i",
);
const HEX_COLOR = /(?:^|[^\w&#-])#(?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})(?![\w-])/i;
const COLOR_FUNCTION = /\b(?:rgba?|hsla?|hwb|oklch|oklab|lab|lch|color|color-mix|light-dark)\(/i;
const PALETTE_CLASS =
  /^(?:bg|text|border(?:-[trblxyse])?|ring|ring-offset|outline|fill|stroke|from|via|to|divide|placeholder|caret|accent|decoration|shadow|inset-shadow|drop-shadow)-(?:slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose|black|white)(?:-\d{2,3})?(?:\/\d+)?$/;

const NAMED_COLORS = new Set(
  (
    "aliceblue antiquewhite aqua aquamarine azure beige bisque black blanchedalmond blue " +
    "blueviolet brown burlywood cadetblue chartreuse chocolate coral cornflowerblue cornsilk " +
    "crimson cyan darkblue darkcyan darkgoldenrod darkgray darkgreen darkgrey darkkhaki " +
    "darkmagenta darkolivegreen darkorange darkorchid darkred darksalmon darkseagreen " +
    "darkslateblue darkslategray darkslategrey darkturquoise darkviolet deeppink deepskyblue " +
    "dimgray dimgrey dodgerblue firebrick floralwhite forestgreen fuchsia gainsboro ghostwhite " +
    "gold goldenrod gray green greenyellow grey honeydew hotpink indianred indigo ivory khaki " +
    "lavender lavenderblush lawngreen lemonchiffon lightblue lightcoral lightcyan " +
    "lightgoldenrodyellow lightgray lightgreen lightgrey lightpink lightsalmon lightseagreen " +
    "lightskyblue lightslategray lightslategrey lightsteelblue lightyellow lime limegreen linen " +
    "magenta maroon mediumaquamarine mediumblue mediumorchid mediumpurple mediumseagreen " +
    "mediumslateblue mediumspringgreen mediumturquoise mediumvioletred midnightblue mintcream " +
    "mistyrose moccasin navajowhite navy oldlace olive olivedrab orange orangered orchid " +
    "palegoldenrod palegreen paleturquoise palevioletred papayawhip peachpuff peru pink plum " +
    "powderblue purple rebeccapurple red rosybrown royalblue saddlebrown salmon sandybrown " +
    "seagreen seashell sienna silver skyblue slateblue slategray slategrey snow springgreen " +
    "steelblue tan teal thistle tomato turquoise violet wheat white whitesmoke yellow yellowgreen"
  ).split(" "),
);

// Style properties that legitimately take plain numbers.
const UNITLESS_STYLE_PROPERTIES = new Set([
  "animationIterationCount",
  "aspectRatio",
  "columnCount",
  "fillOpacity",
  "flex",
  "flexGrow",
  "flexShrink",
  "gridColumn",
  "gridColumnEnd",
  "gridColumnStart",
  "gridRow",
  "gridRowEnd",
  "gridRowStart",
  "opacity",
  "order",
  "stopOpacity",
  "strokeOpacity",
  "zIndex",
]);

const COLOR_STYLE_PROPERTY =
  /^(?:color|background|backgroundColor|border(?:Top|Right|Bottom|Left|Block|Inline)?(?:Start|End)?Color|borderColor|border|outline|outlineColor|fill|stroke|caretColor|accentColor|columnRuleColor|textDecorationColor|textEmphasisColor|boxShadow|textShadow|stopColor|floodColor|lightingColor|--.*)$/;

const SIZE_ATTRIBUTES = new Set([
  "width",
  "height",
  "size",
  "strokeWidth",
  // Radix popper geometry, in CSS pixels.
  "sideOffset",
  "alignOffset",
  "collisionPadding",
  "arrowPadding",
]);
const COLOR_ATTRIBUTES = new Set(["color", "fill", "stroke", "stopColor", "floodColor"]);

const SKIPPED_PARENTS = new Set([
  "ImportDeclaration",
  "ExportNamedDeclaration",
  "ExportAllDeclaration",
  "ImportExpression",
  "TSLiteralType",
  "TSExternalModuleReference",
  "TSImportType",
]);

/** Splits a class token into its bracketed groups, honoring nesting. */
function bracketGroups(token) {
  const groups = [];
  let depth = 0;
  let start = -1;
  for (let i = 0; i < token.length; i++) {
    const char = token[i];
    if (char === "[") {
      if (depth === 0) start = i + 1;
      depth++;
    } else if (char === "]" && depth > 0) {
      depth--;
      if (depth === 0) groups.push(token.slice(start, i));
    }
  }
  return groups;
}

function namedColorIn(content) {
  return content
    .toLowerCase()
    .split(/[\s_,:()=/'"]+/)
    .find((word) => NAMED_COLORS.has(word));
}

function isRawBracketValue(content) {
  if (content.startsWith("var(--") && !RAW_LENGTH_IN_BRACKETS.test(content)) return false;
  return (
    RAW_LENGTH_IN_BRACKETS.test(content) ||
    COLOR_FUNCTION.test(content) ||
    /#[0-9a-f]{3,8}\b/i.test(content) ||
    namedColorIn(content) !== undefined
  );
}

/** Returns human-readable problems found in one string. */
function problemsInString(text) {
  const problems = [];
  let rest = text;

  // Class-list checks first; a flagged token is not re-reported as a raw color below.
  for (const token of text.split(/\s+/)) {
    if (!token) continue;
    if (PALETTE_CLASS.test(token.split(":").at(-1) ?? "")) {
      problems.push(`default Tailwind palette class "${token}"; only Brigadier color tokens exist`);
      rest = rest.replace(token, " ");
    } else if (bracketGroups(token).some(isRawBracketValue)) {
      problems.push(`arbitrary Tailwind value "${token}"; use a token utility`);
      rest = rest.replace(token, " ");
    }
  }

  const hex = rest.match(HEX_COLOR);
  if (hex) problems.push(`raw color "${hex[0].slice(hex[0].indexOf("#"))}"; use a color token`);
  const fn = rest.match(COLOR_FUNCTION);
  if (fn) problems.push(`raw color function "${fn[0]}…)"; use a color token`);

  if (problems.length === 0) {
    const length = rest.match(RAW_LENGTH_VALUE);
    if (length) problems.push(`raw length "${length[0].replace(/^[\s(,/+*]/, "")}"; use a size or spacing token`);
  }
  return problems;
}

function propertyName(property) {
  if (property.computed) return undefined;
  if (property.key.type === "Identifier") return property.key.name;
  if (property.key.type === "Literal") return String(property.key.value);
  return undefined;
}

function unwrap(node) {
  let current = node;
  while (
    current &&
    (current.type === "TSAsExpression" ||
      current.type === "TSSatisfiesExpression" ||
      current.type === "TSNonNullExpression" ||
      current.type === "ParenthesizedExpression")
  ) {
    current = current.expression;
  }
  return current;
}

function numericLiteral(node) {
  const value = unwrap(node);
  if (!value) return undefined;
  if (value.type === "Literal" && typeof value.value === "number") return value.value;
  if (
    value.type === "UnaryExpression" &&
    value.operator === "-" &&
    value.argument.type === "Literal" &&
    typeof value.argument.value === "number"
  ) {
    return -value.argument.value;
  }
  return undefined;
}

function stringLiteral(node) {
  const value = unwrap(node);
  if (value?.type === "Literal" && typeof value.value === "string") return value.value;
  return undefined;
}

const noRawDesignValues = {
  meta: {
    type: "problem",
    docs: {
      description: "Disallow raw colors and sizes in component code; use design tokens.",
    },
    schema: [],
  },
  create(context) {
    const reportString = (node, text) => {
      for (const problem of problemsInString(text)) {
        context.report({ node, message: `Design token required: ${problem}.` });
      }
    };

    const checkStyleObject = (object) => {
      for (const property of object.properties) {
        if (property.type !== "Property") continue;
        const name = propertyName(property);
        if (name === undefined) continue;
        const number = numericLiteral(property.value);
        if (number !== undefined && number !== 0 && !UNITLESS_STYLE_PROPERTIES.has(name)) {
          context.report({
            node: property.value,
            message: `Design token required: numeric style value "${name}: ${number}"; use a token utility or var(--…).`,
          });
          continue;
        }
        const text = stringLiteral(property.value);
        if (text !== undefined && COLOR_STYLE_PROPERTY.test(name)) {
          const named = NAMED_COLORS.has(text.trim().toLowerCase());
          if (named) {
            context.report({
              node: property.value,
              message: `Design token required: named color "${name}: ${text}"; use a color token.`,
            });
          }
        }
      }
    };

    return {
      AssignmentPattern(node) {
        if (node.left.type !== "Identifier" || !SIZE_ATTRIBUTES.has(node.left.name)) return;
        const number = numericLiteral(node.right);
        if (number !== undefined && number !== 0) {
          context.report({
            node,
            message: `Design token required: numeric default ${node.left.name} = ${number}; derive it from a token (tokenPx).`,
          });
        }
      },
      Literal(node) {
        if (typeof node.value !== "string") return;
        if (node.parent && SKIPPED_PARENTS.has(node.parent.type)) return;
        if (node.parent?.type === "ExpressionStatement" && node.parent.directive) return;
        reportString(node, node.value);
      },
      TemplateElement(node) {
        const text = node.value.cooked ?? node.value.raw;
        if (text) reportString(node, text);
      },
      JSXAttribute(node) {
        if (node.name.type !== "JSXIdentifier") return;
        const name = node.name.name;
        const value =
          node.value?.type === "JSXExpressionContainer" ? node.value.expression : node.value;
        if (!value) return;

        if (name === "style") {
          const object = unwrap(value);
          if (object?.type === "ObjectExpression") checkStyleObject(object);
          return;
        }
        if (SIZE_ATTRIBUTES.has(name)) {
          const number = numericLiteral(value) ?? Number(stringLiteral(value) ?? Number.NaN);
          if (!Number.isNaN(number) && number !== 0) {
            context.report({
              node,
              message: `Design token required: numeric ${name}="${number}"; size with a token utility (size-icon-*, h-control-*) or tokenPx(…).`,
            });
          }
          return;
        }
        if (COLOR_ATTRIBUTES.has(name)) {
          const text = stringLiteral(value);
          if (text !== undefined && NAMED_COLORS.has(text.trim().toLowerCase())) {
            context.report({
              node,
              message: `Design token required: named color ${name}="${text}"; use currentColor with a text-* token.`,
            });
          }
        }
      },
    };
  },
};

const SCROLL_CALLS = new Set(["scrollIntoView", "scrollTo", "scrollBy"]);
const FUNCTIONS = ["FunctionDeclaration", "FunctionExpression", "ArrowFunctionExpression"];

/** The name a member chain starts from: `viewport` for `viewport.scrollTo`, `a` for `a.b?.c()`. */
function rootName(node) {
  let at = node;
  for (;;) {
    if (at.type === "ChainExpression") at = at.expression;
    else if (at.type === "MemberExpression") at = at.object;
    else if (at.type === "CallExpression") at = at.callee;
    else if (at.type === "TSNonNullExpression") at = at.expression;
    else break;
  }
  return at.type === "Identifier" ? at.name : null;
}

const memberName = (member) =>
  member?.type === "MemberExpression" && !member.computed && member.property.type === "Identifier"
    ? member.property.name
    : null;

const threadScrollHelper = {
  meta: {
    type: "problem",
    docs: {
      description: "Scrolling the thread's view on the user's behalf goes through jumpInThread() or the scroller's release() first.",
    },
    schema: [],
  },
  create(context) {
    const filename = context.filename ?? context.getFilename?.() ?? "";
    // The scroller itself moves the view as it decides.
    if (filename.endsWith("thread-scroll.ts")) return {};
    const text = (context.sourceCode ?? context.getSourceCode()).text;
    const threadFile = text.includes("assistant-ui/thread-scroll") || text.includes("aui_thread-viewport");
    // Whether each function around the current node has said it jumps, innermost last.
    const frames = [{ jumped: false }];
    const enter = () => frames.push({ jumped: false });
    const exit = () => frames.pop();
    const check = (node, scrolled) => {
      if (!threadFile && rootName(scrolled) !== "viewport") return;
      if (frames.some((frame) => frame.jumped)) return;
      context.report({
        node,
        message:
          "Scrolling the thread's view: call jumpInThread(target) (or the scroller's release()) first, so auto-follow reads the move as the user's, not the browser's.",
      });
    };
    const handlers = {
      CallExpression(node) {
        const callee = node.callee.type === "ChainExpression" ? node.callee.expression : node.callee;
        if (callee.type === "Identifier" && callee.name === "jumpInThread") frames.at(-1).jumped = true;
        const name = memberName(callee);
        if (name === "release" && node.arguments.length === 0) frames.at(-1).jumped = true;
        if (name && SCROLL_CALLS.has(name)) check(node, callee.object);
      },
      AssignmentExpression(node) {
        if (memberName(node.left) === "scrollTop") check(node, node.left.object);
      },
    };
    for (const type of FUNCTIONS) {
      handlers[type] = enter;
      handlers[`${type}:exit`] = exit;
    }
    return handlers;
  },
};

const plugin = {
  meta: { name: "brigadier" },
  rules: {
    "no-raw-design-values": noRawDesignValues,
    "thread-scroll-helper": threadScrollHelper,
  },
};

export default plugin;
