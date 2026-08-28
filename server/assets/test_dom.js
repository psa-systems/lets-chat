// LC-832: shared DOM stub for the browser-asset node:test suites (not a browser
// asset, and not a suite itself - the `just test-js` glob only picks up
// *.test.js). voice.js and huddle_popout.js are IIFEs that reach for `document`
// at load, so their tests evaluate the source in a VM sandbox (the sw.test.js
// idiom) over the tiny element/selector implementation here. Only what those
// modules actually touch is implemented; anything else should fail loudly
// rather than pretend.
'use strict';

// Selector support: comma alternatives, an optional tag name, then any run of
// [attr], [attr="value"] and .class. Covers every selector the modules use,
// including the compound [data-lc-huddle-placeholder][data-room-id="N"].
function matches(el, sel) {
  return String(sel).split(',').some((alt) => {
    let part = alt.trim();
    if (!part) return false;
    const tag = part.match(/^[a-zA-Z][a-zA-Z0-9]*/);
    if (tag) {
      if (el.tagName !== tag[0].toUpperCase()) return false;
      part = part.slice(tag[0].length);
    }
    const re = /\[([^\]=]+)(?:="([^"]*)")?\]|\.([-\w]+)/g;
    let m;
    while ((m = re.exec(part)) !== null) {
      if (m[3] !== undefined) {
        if (!el.classList.contains(m[3])) return false;
      } else if (m[2] !== undefined) {
        if (el.getAttribute(m[1]) !== m[2]) return false;
      } else if (!el.hasAttribute(m[1])) return false;
    }
    return true;
  });
}

class El {
  constructor(tag) {
    this.tagName = String(tag).toUpperCase();
    this.attrs = new Map();
    this.childNodes = [];
    this.parentNode = null;
    this.textContent = '';
    this.paused = true;       // media elements start paused in this stub
    this.playCalls = 0;
    this.style = { removeProperty(p) { delete this[p]; } };
    const classes = new Set();
    this.classList = {
      add: (...c) => c.forEach((x) => classes.add(x)),
      remove: (...c) => c.forEach((x) => classes.delete(x)),
      contains: (c) => classes.has(c),
      toggle: (c, on) => (on ? classes.add(c) : classes.delete(c)),
      values: () => Array.from(classes),
    };
  }
  get className() { return this.classList.values().join(' '); }
  set className(v) {
    this.classList.remove(...this.classList.values());
    String(v).split(/\s+/).filter(Boolean).forEach((c) => this.classList.add(c));
  }
  getAttribute(k) { return this.attrs.has(k) ? this.attrs.get(k) : null; }
  setAttribute(k, v) { this.attrs.set(k, String(v)); }
  hasAttribute(k) { return this.attrs.has(k); }
  removeAttribute(k) { this.attrs.delete(k); }
  appendChild(c) {
    if (c.parentNode) c.parentNode.removeChild(c);
    c.parentNode = this;
    this.childNodes.push(c);
    return c;
  }
  insertBefore(c, ref) {
    if (c.parentNode) c.parentNode.removeChild(c);
    const i = ref ? this.childNodes.indexOf(ref) : -1;
    c.parentNode = this;
    if (i < 0) this.childNodes.push(c); else this.childNodes.splice(i, 0, c);
    return c;
  }
  removeChild(c) {
    const i = this.childNodes.indexOf(c);
    if (i >= 0) this.childNodes.splice(i, 1);
    c.parentNode = null;
    return c;
  }
  replaceChild(next, old) {
    const i = this.childNodes.indexOf(old);
    if (i < 0) return old;
    if (next.parentNode) next.parentNode.removeChild(next);
    this.childNodes[i] = next;
    next.parentNode = this;
    old.parentNode = null;
    return old;
  }
  replaceChildren() { this.childNodes.slice().forEach((c) => this.removeChild(c)); }
  remove() { if (this.parentNode) this.parentNode.removeChild(this); }
  contains(n) {
    for (let c = n; c; c = c.parentNode) if (c === this) return true;
    return false;
  }
  querySelectorAll(sel) {
    const out = [];
    const walk = (n) => n.childNodes.forEach((c) => { if (matches(c, sel)) out.push(c); walk(c); });
    walk(this);
    return out;
  }
  querySelector(sel) { return this.querySelectorAll(sel)[0] || null; }
  getBoundingClientRect() { return { left: 0, top: 0, width: 480, height: 320 }; }
  play() { this.playCalls++; this.paused = false; return Promise.resolve(); }
  addEventListener() {}
  removeEventListener() {}
}

module.exports = { El, matches };
