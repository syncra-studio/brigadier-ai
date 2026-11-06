// The browser tool's hand in a Browser tab's page (see `browser_agent.rs`): one call reads the
// page or acts on one of its controls, and returns what came of it. It runs fresh for each call,
// keeping nothing in the page but each control's ref, so a page can't stand in for it between
// calls. The test page (fixtures/agent-browser.html) runs this same file.
//
// Ops: { kind: "read", max } -> { url, title, text }
//      { kind: "click", target, declared, approved } -> { done } | { ask, url } | { error }
//      { kind: "type", target, text, submit, declared, approved } -> the same
// A password field, seen by its type, its autocomplete, its computed text security or its names, shows only as a line
// with no ref and nothing goes into it, yes or no: only the user fills those in. An action that pays, sends or signs in
// (declared by the agent), or that enters or submits a card or a one-time code or submits a password (seen here),
// returns `ask` and does nothing unless the user said yes to it on this page (`approved`: the address the yes was given at, which the app
// passes only while the tab is still on the page it was given on).
//
// With `trusted: true` (the Electron shell, in an isolated world) a click or typing isn't done here:
// it answers { point: { x, y } } for the shell to do with real input, or an error if something covers
// the control. { kind: "place", target, typing } answers where the control is now, without scrolling, and for typing
// refuses a field that lost the focus. With `isolated: true` it runs in a world of its own: refs are kept there, not
// in the page's DOM, where the page could see or forge them. { kind: "void" } forgets them.
// { kind: "sweep" } answers every password field's place and value, for the shell's own use: it never reaches an agent.
(op) => {
  const REF = "data-brigadier-ref";
  const NEXT = "data-brigadier-next-ref";
  const clean = (text) => (text || "").replace(/\s+/g, " ").trim();
  const cut = (text, most = 80) => (text.length > most ? `${text.slice(0, most - 1)}…` : text);
  const quoted = (text) => JSON.stringify(cut(clean(text)));

  const boxed = (element) => {
    if (element.checkVisibility) return element.checkVisibility({ visibilityProperty: true });
    const rect = element.getBoundingClientRect();
    return rect.width > 0 || rect.height > 0;
  };
  // A display: contents wrapper has no box of its own: what it holds shows or not by itself.
  const shown = (element) => boxed(element) || getComputedStyle(element).display === "contents";

  const INPUT_ROLES = {
    button: "button", submit: "button", reset: "button", image: "button",
    checkbox: "checkbox", radio: "radio", range: "slider", file: "button", color: "button",
  };
  const ROLES = new Set([
    "button", "link", "checkbox", "radio", "switch", "tab", "menuitem", "menuitemcheckbox",
    "menuitemradio", "option", "combobox", "textbox", "searchbox", "slider", "spinbutton",
    "treeitem",
  ]);

  /** The control's role, or null for anything that isn't one. */
  const roleOf = (element) => {
    const role = (element.getAttribute("role") || "").split(" ")[0];
    if (ROLES.has(role)) return role;
    switch (element.localName) {
      case "a": return element.hasAttribute("href") ? "link" : null;
      case "button": case "summary": return "button";
      case "select": return "combobox";
      case "textarea": return "textbox";
      case "iframe": return "frame";
      case "input": {
        if (element.type === "hidden") return null;
        return INPUT_ROLES[element.type] || "textbox";
      }
      default:
        return element.isContentEditable && !element.parentElement?.isContentEditable ? "textbox" : null;
    }
  };

  const labelled = (element) => {
    const ids = (element.getAttribute("aria-labelledby") || "").split(/\s+/).filter(Boolean);
    return ids.map((id) => element.getRootNode().getElementById?.(id)?.textContent || "").join(" ");
  };

  /** The control's name, as a screen reader would say it. */
  const nameOf = (element) => {
    const candidates = [
      element.getAttribute("aria-label"),
      labelled(element),
      element.labels?.[0]?.innerText,
      element.localName === "input" && ["button", "submit", "reset"].includes(element.type) ? element.value : "",
      element.localName === "iframe" ? element.title : "",
      element.localName === "textarea" || element.localName === "input" || element.isContentEditable ? "" : element.innerText,
      element.getAttribute("placeholder"),
      element.getAttribute("alt"),
      element.querySelector?.("img[alt]")?.getAttribute("alt"),
      element.getAttribute("title"),
    ];
    for (const candidate of candidates) {
      const name = clean(candidate);
      if (name) return name;
    }
    return "";
  };

  /** Fields that were password fields once, in this world: a "show password" toggle changes nothing. */
  const wasPassword = () => (window.__brigadierPasswordFields ??= new WeakSet());

  /** What kind of private thing a field takes: a password, a card or a one-time code. */
  const fieldKind = (element) => {
    if (!element?.getAttribute) return null;
    const editable = element.isContentEditable;
    const field = ["input", "textarea"].includes(element.localName);
    if (!field && !editable) return null;
    if (op.isolated && wasPassword().has(element)) return "credentials";
    const auto = (element.getAttribute("autocomplete") || "").toLowerCase();
    const hint = [element.name, element.id, element.getAttribute("aria-label"), element.placeholder, element.labels?.[0]?.innerText]
      .join(" ").toLowerCase();
    const password = () => {
      if (op.isolated) wasPassword().add(element);
      return "credentials";
    };
    if (element.type === "password" || /\b(current|new)-password\b/.test(auto)) return password();
    // A text field disguised as a password field.
    if ((getComputedStyle(element).webkitTextSecurity || "none") !== "none") return password();
    if (!field) return null;
    if (/\bcc-(number|csc|exp|exp-month|exp-year)\b/.test(auto) ||
        /card.?number|\bcc.?num|\bcvc\b|\bcvv\b|\bcsc\b|security code|expiry|expiration/.test(hint)) return "card";
    if (/\bone-time-code\b/.test(auto) ||
        /\botp\b|one.?time|verification code|2fa|two.?factor|auth(entication)? code/.test(hint)) return "oneTimeCode";
    if (["", "text", "search"].includes(element.getAttribute("type") ?? "") && /pass(word|wd|phrase|code)|pwd/.test(hint)) return password();
    return null;
  };

  const isPassword = (element) => fieldKind(element) === "credentials";
  const HANDS_OFF = "That is a password field, and only the user fills those in, so nothing was done. Ask them to sign in in this tab, and go on when they say so.";

  /** The private kind a filled field of `scope` holds (`also` counts as filled), if any. */
  const filledKind = (scope, also) => {
    for (const field of scope.querySelectorAll("input, textarea")) {
      const kind = fieldKind(field);
      if (kind && (field === also || field.value)) return kind;
    }
    return null;
  };

  /** What a control submits with: its form, else the nearest few levels around it. */
  const scopeOf = (element) => {
    const form = element.form || element.closest("form");
    if (form) return form;
    let around = element.parentElement;
    for (let level = 0; around && level < 4; level++, around = around.parentElement) {
      if (filledKind(around)) return around;
    }
    return element.parentElement || document;
  };

  /** Every element under `root`, open shadow roots included, in document order. */
  const each = function* (root) {
    for (const child of root.children) {
      yield child;
      if (child.shadowRoot) yield* each(child.shadowRoot);
      yield* each(child);
    }
  };

  const refs = () => (window.__brigadierRefs ??= { next: 1, byRef: new Map(), of: new WeakMap() });

  const find = (ref) => {
    if (op.isolated) return refs().byRef.get(ref)?.deref() ?? null;
    for (const element of each(document)) if (element.getAttribute(REF) === ref) return element;
    return null;
  };

  const refOf = (element) => {
    if (op.isolated) {
      const store = refs();
      let own = store.of.get(element);
      if (!own) {
        own = `e${store.next++}`;
        store.of.set(element, own);
        store.byRef.set(own, new WeakRef(element));
      }
      return own;
    }
    let ref = element.getAttribute(REF);
    if (ref) return ref;
    const root = document.documentElement;
    const next = Number(root.getAttribute(NEXT)) || 1;
    root.setAttribute(NEXT, String(next + 1));
    ref = `e${next}`;
    element.setAttribute(REF, ref);
    return ref;
  };

  const SKIP = new Set(["script", "style", "noscript", "template", "head", "meta", "link"]);
  const BLOCK = /^(address|article|aside|blockquote|br|dd|details|dialog|div|dl|dt|fieldset|figcaption|figure|footer|form|h[1-6]|header|hr|li|main|nav|ol|p|pre|section|summary|table|tbody|td|tfoot|th|thead|tr|ul)$/;

  const read = (most) => {
    const lines = [];
    let line = "";
    const flush = () => {
      const text = clean(line);
      if (text) lines.push(text);
      line = "";
    };
    const describe = (element, role) => {
      let text = `[${refOf(element)}] ${role} ${quoted(nameOf(element))}`;
      if (role === "textbox" || role === "searchbox" || role === "combobox" || role === "slider" || role === "spinbutton") {
        const value = element.isContentEditable ? element.innerText
          : element.localName === "select" ? element.selectedOptions?.[0]?.text : element.value;
        // A card number or code is never read out, only whether it is filled.
        if (fieldKind(element)) text += value ? " (filled)" : "";
        else if (value) text += ` value=${quoted(value)}`;
        if (element.localName === "select") {
          const options = [...element.options].slice(0, 12).map((option) => clean(option.text));
          text += ` options=${JSON.stringify(options)}`;
        }
      }
      if (role === "checkbox" || role === "radio" || role === "switch") {
        const on = element.checked ?? element.getAttribute("aria-checked") === "true";
        text += on ? " (checked)" : " (not checked)";
      }
      if (role === "link") {
        const href = element.getAttribute("href") || "";
        if (href && !href.startsWith("javascript:")) text += ` -> ${cut(element.href || href, 100)}`;
      }
      if (element.disabled || element.getAttribute("aria-disabled") === "true") text += " (disabled)";
      const kind = fieldKind(element);
      if (kind) text += ` (${kind === "card" ? "card" : "one-time code"} field)`;
      return text;
    };
    // A label's own text is its control's name, read with the control.
    // A display: contents wrapper passes `shown` with no box: its own text shows only while it's
    // visible (its elements check their own).
    const walk = (node, labelling = false) => {
      const quiet = labelling || (node.nodeType === Node.ELEMENT_NODE &&
        getComputedStyle(node).display === "contents" && getComputedStyle(node).visibility !== "visible");
      for (const child of node.childNodes) {
        if (child.nodeType === Node.TEXT_NODE) {
          if (!quiet) line += ` ${child.textContent}`;
          continue;
        }
        if (child.nodeType !== Node.ELEMENT_NODE) continue;
        const tag = child.localName;
        if (SKIP.has(tag) || !shown(child)) continue;
        const role = roleOf(child);
        if (role) {
          flush();
          // Nothing of a password field but that it is there: no ref to use, no name or value.
          lines.push(isPassword(child) ? "(A password field, hidden: only the user fills it in.)" : describe(child, role));
          continue;
        }
        const block = BLOCK.test(tag);
        if (block) flush();
        const heading = /^h[1-6]$/.test(tag) && !child.querySelector("a, button, input");
        if (heading) {
          lines.push(`${"#".repeat(Number(tag[1]))} ${clean(child.innerText)}`);
          continue;
        }
        if (tag === "img" && child.alt) line += ` [image: ${cut(clean(child.alt))}]`;
        walk(child, labelling || (tag === "label" && child.control !== null && child.contains(child.control)));
        if (child.shadowRoot) walk(child.shadowRoot);
        if (block) flush();
      }
    };
    if (document.body) walk(document.body);
    flush();
    let text = lines.join("\n");
    if (text.length > most) {
      const rest = text.length - most;
      text = `${text.slice(0, most)}\n(The page goes on: ${rest} more characters not shown.)`;
    }
    return { url: location.href, title: document.title, text };
  };

  /** What has the focus, through open shadow roots. */
  const deepFocused = () => {
    let focused = document.activeElement;
    while (focused?.shadowRoot?.activeElement) focused = focused.shadowRoot.activeElement;
    return focused;
  };

  const target = () => {
    const element = find(op.target);
    if (!element || !element.isConnected) {
      return { error: `There is no ${op.target} on the page now: read it again for its controls' refs.` };
    }
    return { element };
  };

  /** Asks the user first: why, and the control as the page names it. */
  const ask = (sensitive, element) => {
    // The yes was for this page, at this address.
    if (op.approved && op.approved === location.href) return null;
    return { ask: { sensitive, what: `${roleOf(element) || element.localName} ${quoted(nameOf(element))}` }, url: location.href };
  };

  const pointer = (element, type) => {
    const rect = element.getBoundingClientRect();
    const init = {
      bubbles: true, cancelable: true, composed: true, view: window, button: 0,
      clientX: rect.left + rect.width / 2, clientY: rect.top + rect.height / 2,
    };
    const Kind = type.startsWith("pointer") && window.PointerEvent ? PointerEvent : MouseEvent;
    return element.dispatchEvent(new Kind(type, { ...init, pointerType: "mouse", isPrimary: true }));
  };

  /** The element at a point of the page, through open shadow roots. */
  const deepest = (x, y) => {
    let hit = document.elementFromPoint(x, y);
    while (hit?.shadowRoot) {
      const inner = hit.shadowRoot.elementFromPoint(x, y);
      if (!inner || inner === hit) break;
      hit = inner;
    }
    return hit;
  };
  const within = (outer, inner) => {
    for (let at = inner; at; at = at.parentNode || at.host) if (at === outer) return true;
    return false;
  };

  /** Where real input reaches the control, or what is over it; null when it has no place to click. */
  const placeOf = (element) => {
    const rects = element.getClientRects();
    const rect = rects.length ? rects[0] : null;
    if (!rect || rect.width <= 0 || rect.height <= 0) return null;
    const x = rect.left + rect.width / 2;
    const y = rect.top + rect.height / 2;
    if (x < 0 || y < 0 || x >= innerWidth || y >= innerHeight) return null;
    const hit = deepest(x, y);
    const label = hit?.closest?.("label");
    if (hit && !within(element, hit) && !within(hit, element) && !label?.contains(element) && label?.control !== element) {
      const role = roleOf(hit);
      return { error: `${op.target} is covered by ${role || hit.localName} ${quoted(nameOf(hit) || hit.innerText || "")}: deal with that first.` };
    }
    return { point: { x, y } };
  };

  const click = () => {
    const found = target();
    if (found.error) return found;
    const { element } = found;
    if (isPassword(element)) return { error: HANDS_OFF };
    if (element.disabled) return { error: `${op.target} is disabled.` };
    const role = roleOf(element);
    const kind = role === "button" ? filledKind(scopeOf(element)) : null;
    const asked = (op.declared && ask(op.declared, element)) || (kind && ask(kind, element));
    if (asked) return asked;
    if (op.trusted) {
      element.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });
      // With no box it only takes the page's own click below.
      const place = placeOf(element);
      if (place) return place;
    }
    element.scrollIntoView({ block: "center", inline: "center" });
    pointer(element, "pointerdown");
    pointer(element, "mousedown");
    element.focus?.({ preventScroll: true });
    pointer(element, "pointerup");
    pointer(element, "mouseup");
    element.click();
    return { done: true };
  };

  const setValue = (element, value) => {
    const proto = element.localName === "textarea" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
    if (setter) setter.call(element, value);
    else element.value = value;
  };

  const key = (element, type) =>
    element.dispatchEvent(new KeyboardEvent(type, {
      key: "Enter", code: "Enter", keyCode: 13, which: 13, bubbles: true, cancelable: true, composed: true,
    }));

  const type = () => {
    const found = target();
    if (found.error) return found;
    const { element } = found;
    if (isPassword(element)) return { error: HANDS_OFF };
    const role = roleOf(element);
    if (!["textbox", "searchbox", "combobox", "spinbutton"].includes(role)) {
      return { error: `${op.target} isn't a field to type in (it is a ${role || element.localName}).` };
    }
    if (element.disabled || element.readOnly) return { error: `${op.target} can't be typed in now.` };
    // A card or code field asks as what it is, whatever the agent declared, so the card never
    // shows the secret typed into it.
    const own = fieldKind(element);
    const kind = own || (op.submit ? filledKind(scopeOf(element), element) : null);
    const asked = (own && ask(own, element)) || (op.declared && ask(op.declared, element)) || (kind && ask(kind, element));
    if (asked) return asked;
    element.scrollIntoView({ block: "center", inline: "center" });
    element.focus({ preventScroll: true });
    if (element.localName === "select") {
      const wanted = clean(op.text).toLowerCase();
      const option = [...element.options].find((each) => clean(each.text).toLowerCase() === wanted || each.value === op.text);
      if (!option) return { error: `${op.target} has no option ${quoted(op.text)}.` };
      element.value = option.value;
      element.dispatchEvent(new Event("input", { bubbles: true }));
      element.dispatchEvent(new Event("change", { bubbles: true }));
    } else if (op.trusted && op.text) {
      // Only where it is: the cursor goes there first, then { kind: "insert" }.
      const rect = element.getBoundingClientRect();
      return { point: { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 } };
    } else if (element.isContentEditable) {
      document.getSelection()?.selectAllChildren(element);
      document.execCommand("insertText", false, op.text);
    } else {
      setValue(element, op.text);
      element.dispatchEvent(new InputEvent("input", { bubbles: true, composed: true, inputType: "insertText", data: op.text }));
      element.dispatchEvent(new Event("change", { bubbles: true }));
    }
    if (op.submit) {
      const going = key(element, "keydown");
      key(element, "keypress");
      key(element, "keyup");
      const form = element.form;
      if (going && form && element.localName !== "textarea") form.requestSubmit();
    }
    return { done: true };
  };

  try {
    switch (op.kind) {
      case "read": return read(op.max || 20000);
      case "click": return click();
      case "type": return type();
      case "sweep": {
        const answer = { fields: [], values: [], width: innerWidth, frames: [] };
        const plain = (rect) => ({ x: rect.left, y: rect.top, width: rect.width, height: rect.height });
        for (const element of each(document)) {
          if (element.localName === "iframe") {
            let origin = location.origin;
            try {
              if (element.getAttribute("src")) origin = new URL(element.src).origin;
            } catch { /* The page's own, then. */ }
            answer.frames.push({ origin: origin === "null" ? location.origin : origin, rect: plain(element.getBoundingClientRect()) });
          }
          if (!isPassword(element)) continue;
          const value = element.isContentEditable ? element.innerText : element.value;
          if (value) answer.values.push(value);
          for (const rect of element.getClientRects()) answer.fields.push(plain(rect));
        }
        return answer;
      }
      case "void":
        window.__brigadierRefs = undefined;
        return { done: true };
      case "insert": {
        // The page's own editing, which needs no window focus: a debugger's insertText is routed by the window's.
        const found = target();
        if (found.error) return found;
        const { element } = found;
        // Looked at again right before: the page may have made it one, or moved the focus to one.
        const focused = deepFocused();
        if (isPassword(element) || isPassword(focused)) return { error: HANDS_OFF };
        if (element.isContentEditable) document.getSelection()?.selectAllChildren(element);
        else element.select?.();
        if (!document.execCommand("insertText", false, op.text)) {
          setValue(element, op.text);
          element.dispatchEvent(new InputEvent("input", { bubbles: true, composed: true, inputType: "insertText", data: op.text }));
          element.dispatchEvent(new Event("change", { bubbles: true }));
        }
        return { done: true };
      }
      case "place": {
        const found = target();
        if (found.error) return found;
        if (op.typing) {
          const focused = deepFocused();
          if (isPassword(found.element) || isPassword(focused)) return { error: HANDS_OFF };
          if (!focused || !within(found.element, focused)) return { error: `The page moved the focus away from ${op.target}, so nothing was typed. Read it again.` };
          return { done: true };
        }
        return placeOf(found.element) ?? { error: `${op.target} has no place to click now.` };
      }
      default: return { error: `unknown op ${op.kind}` };
    }
  } catch (error) {
    return { error: `The page refused it: ${error && error.message ? error.message : String(error)}` };
  }
}
