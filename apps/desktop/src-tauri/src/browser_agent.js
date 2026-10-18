// The browser tool's hand in a Browser tab's page (see `browser_agent.rs`): one call reads the
// page or acts on one of its controls, and returns what came of it. It runs fresh for each call,
// keeping nothing in the page but each control's ref, so a page can't stand in for it between
// calls. The test page (fixtures/agent-browser.html) runs this same file.
//
// Ops: { kind: "read", max } -> { url, title, text }
//      { kind: "click", target, declared, approved } -> { done } | { ask, url } | { error }
//      { kind: "type", target, text, submit, declared, approved } -> the same
// An action that pays, sends or signs in (declared by the agent), or that enters or submits a
// password, a card or a one-time code (seen here), returns `ask` and does nothing unless the
// user said yes to it on this page (`approved`: the address the yes was given at, which the app
// passes only while the tab is still on the page it was given on).
(op) => {
  const REF = "data-brigadier-ref";
  const NEXT = "data-brigadier-next-ref";
  const clean = (text) => (text || "").replace(/\s+/g, " ").trim();
  const cut = (text, most = 80) => (text.length > most ? `${text.slice(0, most - 1)}…` : text);
  const quoted = (text) => JSON.stringify(cut(clean(text)));

  const shown = (element) => {
    if (element.checkVisibility) return element.checkVisibility({ visibilityProperty: true });
    const rect = element.getBoundingClientRect();
    return rect.width > 0 || rect.height > 0;
  };

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

  /** What kind of private thing a field takes: a password, a card or a one-time code. */
  const fieldKind = (element) => {
    if (!element || !["input", "textarea"].includes(element.localName)) return null;
    const auto = (element.getAttribute("autocomplete") || "").toLowerCase();
    const hint = [element.name, element.id, element.getAttribute("aria-label"), element.placeholder, element.labels?.[0]?.innerText]
      .join(" ").toLowerCase();
    if (element.type === "password" || /\b(current|new)-password\b/.test(auto)) return "credentials";
    if (/\bcc-(number|csc|exp|exp-month|exp-year)\b/.test(auto) ||
        /card.?number|\bcc.?num|\bcvc\b|\bcvv\b|\bcsc\b|security code|expiry|expiration/.test(hint)) return "card";
    if (/\bone-time-code\b/.test(auto) ||
        /\botp\b|one.?time|verification code|2fa|two.?factor|auth(entication)? code/.test(hint)) return "oneTimeCode";
    return null;
  };

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

  const find = (ref) => {
    for (const element of each(document)) if (element.getAttribute(REF) === ref) return element;
    return null;
  };

  const refOf = (element) => {
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
        if (element.type === "password") text += value ? " (filled)" : "";
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
      if (kind) text += ` (${kind === "credentials" ? "password" : kind === "card" ? "card" : "one-time code"} field)`;
      return text;
    };
    // A label's own text is its control's name, read with the control.
    const walk = (node, labelling = false) => {
      for (const child of node.childNodes) {
        if (child.nodeType === Node.TEXT_NODE) {
          if (!labelling) line += ` ${child.textContent}`;
          continue;
        }
        if (child.nodeType !== Node.ELEMENT_NODE) continue;
        const tag = child.localName;
        if (SKIP.has(tag) || !shown(child)) continue;
        const role = roleOf(child);
        if (role) {
          flush();
          lines.push(describe(child, role));
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

  const click = () => {
    const found = target();
    if (found.error) return found;
    const { element } = found;
    if (element.disabled) return { error: `${op.target} is disabled.` };
    const role = roleOf(element);
    const kind = role === "button" ? filledKind(scopeOf(element)) : null;
    const asked = (op.declared && ask(op.declared, element)) || (kind && ask(kind, element));
    if (asked) return asked;
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
    const role = roleOf(element);
    if (!["textbox", "searchbox", "combobox", "spinbutton"].includes(role)) {
      return { error: `${op.target} isn't a field to type in (it is a ${role || element.localName}).` };
    }
    if (element.disabled || element.readOnly) return { error: `${op.target} can't be typed in now.` };
    const kind = fieldKind(element) || (op.submit ? filledKind(scopeOf(element), element) : null);
    const asked = (op.declared && ask(op.declared, element)) || (kind && ask(kind, element));
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
      default: return { error: `unknown op ${op.kind}` };
    }
  } catch (error) {
    return { error: `The page refused it: ${error && error.message ? error.message : String(error)}` };
  }
}
