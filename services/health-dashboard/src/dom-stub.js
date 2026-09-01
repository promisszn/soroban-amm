/**
 * Minimal DOM stub used by dashboard.test.js so render functions (which call
 * document.getElementById/createElement) can be exercised under `node --test`
 * without pulling in a full jsdom dependency.
 */

class FakeClassList {
  constructor(el) { this.el = el; }
  add(name) { if (!this.el.className.split(/\s+/).includes(name)) this.el.className = `${this.el.className} ${name}`.trim(); }
}

class FakeElement {
  constructor(tag) {
    this.tagName = tag;
    this.children = [];
    this.attributes = {};
    this._textContent = "";
    this.className = "";
    this.hidden = false;
    this.style = {};
    this.dataset = {};
    this.listeners = {};
    this.value = "";
    this.title = "";
    this.classList = new FakeClassList(this);
  }
  set textContent(value) { this._textContent = value; this.children = []; }
  get textContent() { return this._textContent; }
  set innerHTML(value) { this._innerHTML = value; this.children = []; this._textContent = ""; }
  get innerHTML() { return this._innerHTML || ""; }
  appendChild(child) { this.children.push(child); return child; }
  prepend(child) { this.children.unshift(child); return child; }
  replaceChildren(...nodes) { this.children = nodes; this._innerHTML = ""; this._textContent = ""; }
  setAttribute(name, value) { this.attributes[name] = value; }
  getAttribute(name) { return this.attributes[name]; }
  addEventListener(type, handler) { (this.listeners[type] ||= []).push(handler); }
  querySelectorAll(selector) {
    if (selector !== ".alert-item") return [];
    return this.children.filter((c) => c.className?.includes("alert-item"));
  }
}

export class FakeDocument {
  constructor() { this.elements = new Map(); }
  /**
   * Register an element under `id`. Accepts either a FakeElement or a plain
   * object of properties to seed a fresh one with (e.g. `{ value: "tvl" }` for
   * an input), so tests can set up form fields without building elements by
   * hand.
   */
  registerElement(id, el) {
    const element = el instanceof FakeElement ? el : Object.assign(new FakeElement("div"), el || {});
    this.elements.set(id, element);
    return element;
  }
  getElementById(id) { return this.elements.get(id) || null; }
  createElement(tag) { return new FakeElement(tag); }
  querySelector() { return new FakeElement("div"); }
}

export function installFakeDocument(ids = []) {
  const doc = new FakeDocument();
  for (const id of ids) doc.registerElement(id, new FakeElement("div"));
  globalThis.document = doc;
  return doc;
}
