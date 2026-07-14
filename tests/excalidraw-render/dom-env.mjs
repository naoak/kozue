// Minimal browser environment for running Excalidraw's `exportToSvg` under Node.
//
// Importing this module for its side effects installs a jsdom `window`/`document`
// plus the handful of browser globals and CSS-Font-Loading / canvas stubs that
// Excalidraw's SVG exporter touches. Import it BEFORE the Excalidraw bundle so the
// globals exist when the bundle's top-level code runs.

import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  pretendToBeVisual: true,
});

globalThis.window = dom.window;
globalThis.document = dom.window.document;
try {
  Object.defineProperty(globalThis, "navigator", {
    value: dom.window.navigator,
    configurable: true,
  });
} catch {
  // Node already exposes a read-only `navigator`; jsdom's is not required.
}
globalThis.self = dom.window;
globalThis.top = dom.window;
globalThis.parent = dom.window;
globalThis.location = dom.window.location;
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
globalThis.devicePixelRatio = 1;
globalThis.requestAnimationFrame = (cb) => setTimeout(() => cb(Date.now()), 0);
globalThis.cancelAnimationFrame = (id) => clearTimeout(id);
if (!dom.window.matchMedia) {
  dom.window.matchMedia = () => ({
    matches: false,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
  });
}
globalThis.matchMedia = dom.window.matchMedia;
globalThis.XMLSerializer = dom.window.XMLSerializer;
globalThis.DOMParser = dom.window.DOMParser;

// jsdom has no 2-D canvas context; stub just enough for text measurement and the
// no-op drawing calls the exporter makes. Widths are approximate (full-width for
// CJK, ~0.55 em otherwise) — only used for layout, not asserted on.
dom.window.HTMLCanvasElement.prototype.getContext = function () {
  return {
    measureText: (t) => ({
      width:
        [...(t || "")].reduce(
          (a, c) => a + (c.charCodeAt(0) > 0x2e80 ? 1.0 : 0.55),
          0,
        ) * 16,
    }),
    fillText() {},
    save() {},
    restore() {},
    beginPath() {},
    moveTo() {},
    lineTo() {},
    stroke() {},
    fill() {},
    setLineDash() {},
    translate() {},
    scale() {},
    rotate() {},
    arc() {},
    closePath() {},
    rect() {},
    clip() {},
    fillRect() {},
    clearRect() {},
    set font(_v) {},
    get font() {
      return "16px sans";
    },
    canvas: { width: 0, height: 0 },
  };
};

// CSS Font Loading API stubs (jsdom ships neither `FontFace` nor `document.fonts`).
class FontFaceStub {
  constructor(family, _src, desc) {
    this.family = family;
    this.style = "normal";
    this.weight = "normal";
    Object.assign(this, desc || {});
  }
  async load() {
    return this;
  }
}
globalThis.FontFace = FontFaceStub;
const fontSet = {
  _s: new Set(),
  add(f) {
    this._s.add(f);
    return this;
  },
  delete() {
    return true;
  },
  has() {
    return false;
  },
  clear() {
    this._s.clear();
  },
  forEach(cb) {
    this._s.forEach(cb);
  },
  load: async () => [],
  ready: Promise.resolve(),
  addEventListener() {},
  removeEventListener() {},
  get size() {
    return this._s.size;
  },
  [Symbol.iterator]() {
    return this._s[Symbol.iterator]();
  },
};
try {
  Object.defineProperty(dom.window.document, "fonts", {
    value: fontSet,
    configurable: true,
  });
} catch {
  // Older jsdom may already define it; the stub is best-effort.
}
globalThis.fonts = fontSet;

// Expose any remaining DOM constructors (Element, Node, SVGElement, …).
for (const k of Object.getOwnPropertyNames(dom.window)) {
  if (k in globalThis) continue;
  try {
    Object.defineProperty(globalThis, k, {
      value: dom.window[k],
      configurable: true,
      writable: true,
    });
  } catch {
    // Read-only globals are fine to skip.
  }
}

export { dom };
